//! # lab-mcp
//!
//! The MCP host every lab app shares (lifted from ImageLab's `imagelab-core::mcp`, with the app
//! specifics turned into parameters):
//!
//! * [`LabMcp`]: the app's own server, three tools over its function registry — `lab_read` (a
//!   read-only JavaScript program), `lab_script` (the same, allowed to change things; listed
//!   only with `allow_edits`) and `lab_help`. Images a program returns come back as MCP image
//!   content.
//! * [`InProcess`]: an MCP client over an in-memory pipe to any rmcp server, for the in-app agent.
//! * [`HttpServer`]: streamable HTTP on loopback with a bearer token and a Host / Origin /
//!   Sec-Fetch-Site fence, for agents off the phone (`adb forward tcp:7700 tcp:7700`, then
//!   `claude mcp add --transport http …`). Endpoints are mounted by the app ([`HttpRoutes`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use lab_api::{Registry, Source};
use lab_script::ScriptOutcome;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt as _};
use serde_json::{Value, json};
use tokio::io::DuplexStream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio_util::sync::CancellationToken;

/// The protocol version the in-process client asks for.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// A request (a long export, a large render) may take this long before the agent gets an error.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1900);
/// Largest response line the in-process client accepts.
const MAX_LINE_BYTES: usize = 64 << 20;
/// Largest HTTP request body (programs are at most 256 KiB).
const MAX_HTTP_BODY_BYTES: usize = 4 << 20;
/// Longest `timeoutMs` a tool call may ask for.
pub const MAX_TIMEOUT_MS: u64 = 30 * 60 * 1000;

#[derive(Debug, thiserror::Error)]
pub enum McpHostError {
    #[error("{0}")]
    BadRequest(String),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, McpHostError>;

// ------------------------------------------------------------------------------------------
// The app's own server

/// What [`LabMcp`] needs from the app: its registry and a way to run a program on it.
pub trait ScriptHost: Send + Sync + 'static {
    fn registry(&self) -> &Registry;
    /// Run `code` (`read_only`: a `lab_read` realm), attributed to `source`. `timeout` zero is
    /// the runtime's default. `cancel` is set when the MCP request is cancelled.
    fn run_script(&self, code: &str, read_only: bool, timeout: Duration, source: Source, cancel: Arc<AtomicBool>) -> ScriptOutcome;
}

/// How the app introduces itself and its tools.
#[derive(Debug, Clone)]
pub struct LabServerInfo {
    /// MCP server name (`videolab`).
    pub name: String,
    pub version: String,
    /// The server's `instructions`.
    pub instructions: String,
    /// What a read-only program can do, first paragraph of `lab_read`'s description.
    pub read_blurb: String,
    /// What an editing program can do, first paragraph of `lab_script`'s description.
    pub script_blurb: String,
}

const PROGRAM_RULES: &str = "The program is the body of an async JavaScript function: use `await`, and `return` the result (JSON). \
Call functions as `await lab.<area>.<fn>({...params})`; `lab.help('<area>')` returns that area's functions with descriptions and parameter schemas, `lab.help()` lists every area. \
Images are handles (\"img:N\", \"mask:N\"), never bytes; any object `{image: \"img:N\"}` in the returned value comes back to you as an image. \
Handles made during the program are freed when it ends unless the result mentions them or you call `lab.keep(handle)`. \
console.log is captured. A failed call throws LabCallError {code, message}; catch it to branch on `code`. No fetch, timers or file system.";

/// The app's MCP server. Cheap to clone (one per HTTP session).
#[derive(Clone)]
pub struct LabMcp {
    host: Arc<dyn ScriptHost>,
    info: Arc<LabServerInfo>,
    allow_edits: bool,
    source: Source,
}

impl LabMcp {
    pub fn new(host: Arc<dyn ScriptHost>, info: Arc<LabServerInfo>, allow_edits: bool, source: Source) -> Self {
        Self { host, info, allow_edits, source }
    }

    /// The tool list (`tools/list`'s `tools`), also what the in-app agent offers the model.
    pub fn tools(&self) -> Vec<Value> {
        let areas = self.host.registry().areas_summary();
        let program = json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "The program body (async JavaScript)."},
                "description": {"type": "string", "description": "One short line for the person: what this program does."},
                "timeoutMs": {"type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_MS, "description": "Wall-time budget; default 120000."}
            },
            "required": ["code", "description"],
            "additionalProperties": false
        });
        let mut tools = vec![json!({
            "name": "lab_read",
            "title": "Look (read-only program)",
            "description": format!("{} Any function that changes something throws LabCallError{{code:\"read_only\"}}.\n\n{PROGRAM_RULES}\n\nAreas:\n{areas}", self.info.read_blurb),
            "inputSchema": program,
            "annotations": {"title": "Look (read-only program)", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        })];
        if self.allow_edits {
            tools.push(json!({
                "name": "lab_script",
                "title": "Edit (program)",
                "description": format!("{}\n\n{PROGRAM_RULES}\n\nAreas:\n{areas}", self.info.script_blurb),
                "inputSchema": program,
                "annotations": {"title": "Edit (program)", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
            }));
        }
        tools.push(json!({
            "name": "lab_help",
            "title": "Lab help",
            "description": format!("The {} function registry: with no area, every area and its function names; with an area (\"timeline\") or a function (\"timeline.split\"), full descriptions, parameter schemas and effects.", self.info.name),
            "inputSchema": {"type": "object", "properties": {"area": {"type": "string"}}, "additionalProperties": false},
            "annotations": {"title": "Lab help", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }));
        tools
    }

    async fn run(&self, args: &serde_json::Map<String, Value>, read_only: bool, ct: CancellationToken) -> std::result::Result<CallToolResult, McpError> {
        let code = args.get("code").and_then(Value::as_str).ok_or_else(|| McpError::invalid_params("`code` (string) is required", None))?.to_string();
        let timeout_ms = match args.get("timeoutMs") {
            None | Some(Value::Null) => 0,
            Some(v) => v.as_u64().filter(|&ms| ms <= MAX_TIMEOUT_MS).ok_or_else(|| McpError::invalid_params(format!("`timeoutMs` must be 1..={MAX_TIMEOUT_MS}"), None))?,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let watch = tokio::spawn(async move {
            ct.cancelled().await;
            flag.store(true, Ordering::Relaxed);
        });
        let host = self.host.clone();
        let source = self.source;
        let outcome = tokio::task::spawn_blocking(move || host.run_script(&code, read_only, Duration::from_millis(timeout_ms), source, cancel)).await;
        watch.abort();
        let outcome = outcome.map_err(|e| McpError::internal_error(format!("the script task failed: {e}"), None))?;
        Ok(script_result(&outcome))
    }
}

/// A script outcome as MCP content: one text block (`{value, logs, error, images}` JSON), then
/// one image block per attached image, in `attached` order.
pub fn script_result(outcome: &ScriptOutcome) -> CallToolResult {
    let text = serde_json::to_string_pretty(&outcome.to_json()).unwrap_or_else(|_| "{}".into());
    let mut content = vec![ContentBlock::text(text)];
    for image in &outcome.images {
        content.push(ContentBlock::image(base64::engine::general_purpose::STANDARD.encode(&image.bytes), image.mime.clone()));
    }
    if outcome.error.is_some() { CallToolResult::error(content) } else { CallToolResult::success(content) }
}

impl ServerHandler for LabMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(self.info.name.clone(), self.info.version.clone()))
            .with_instructions(self.info.instructions.clone())
    }

    async fn list_tools(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> std::result::Result<ListToolsResult, McpError> {
        serde_json::from_value(json!({"tools": self.tools()})).map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> std::result::Result<CallToolResponse, McpError> {
        let args = request.arguments.clone().unwrap_or_default();
        let result = match request.name.as_ref() {
            "lab_read" => self.run(&args, true, context.ct.clone()).await?,
            "lab_script" if self.allow_edits => self.run(&args, false, context.ct.clone()).await?,
            "lab_script" => CallToolResult::error(vec![ContentBlock::text(
                "lab_script is off: this server is read-only. Use lab_read, or ask the person to turn on \"allow edits\" for the MCP server.",
            )]),
            "lab_help" => {
                let area = args.get("area").and_then(Value::as_str).filter(|a| !a.trim().is_empty());
                match self.host.registry().help(area) {
                    Ok(v) => CallToolResult::success(vec![ContentBlock::text(serde_json::to_string_pretty(&v).unwrap_or_default())]),
                    Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
                }
            }
            other => return Err(McpError::invalid_params(format!("unknown tool `{other}`"), None)),
        };
        Ok(result.into())
    }
}

// ------------------------------------------------------------------------------------------
// In-process client

struct Conn {
    writer: WriteHalf<DuplexStream>,
    reader: BufReader<ReadHalf<DuplexStream>>,
}

/// An MCP client connected to a server over an in-memory pipe. Requests are serialized.
pub struct InProcess {
    conn: tokio::sync::Mutex<Conn>,
    next_id: AtomicU64,
}

impl InProcess {
    /// Any rmcp server on the current runtime, handshake done. The server task ends when this
    /// client is dropped.
    pub async fn start<S: ServerHandler>(server: S, client_name: &str) -> Result<Self> {
        Self::start_with(
            move |read, write| async move {
                if let Ok(running) = server.serve((read, write)).await {
                    let _ = running.waiting().await;
                }
            },
            client_name,
        )
        .await
    }

    /// A server that serves itself over the given halves (e.g. FilmCraft's `serve_io`, which
    /// adds its own parse-error handling).
    pub async fn start_with<F, Fut>(serve: F, client_name: &str) -> Result<Self>
    where
        F: FnOnce(ReadHalf<DuplexStream>, WriteHalf<DuplexStream>) -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (server_read, server_write) = tokio::io::split(server);
        tokio::spawn(serve(server_read, server_write));
        let (client_read, client_write) = tokio::io::split(client);
        let this = Self { conn: tokio::sync::Mutex::new(Conn { writer: client_write, reader: BufReader::new(client_read) }), next_id: AtomicU64::new(1) };
        this.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": client_name, "version": env!("CARGO_PKG_VERSION")}
            }),
        )
        .await?;
        this.notify("notifications/initialized", json!({})).await?;
        Ok(this)
    }

    /// Send a request and wait for its result (`result` on success; an MCP `error` becomes
    /// [`McpHostError::BadRequest`]). Tool failures are results with `isError: true`, as in MCP.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut conn = self.conn.lock().await;
        write_line(&mut conn.writer, &message).await?;
        let wait = async {
            loop {
                let line = read_line(&mut conn.reader).await?;
                let Ok(reply) = serde_json::from_str::<Value>(&line) else { continue };
                // Notifications (progress, list_changed) carry no id; skip them.
                if reply.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = reply.get("error") {
                    let text = error.get("message").and_then(Value::as_str).unwrap_or("MCP error");
                    return Err(McpHostError::BadRequest(text.to_string()));
                }
                return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
            }
        };
        tokio::time::timeout(REQUEST_TIMEOUT, wait).await.map_err(|_| McpHostError::Other(format!("MCP {method} timed out")))?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let mut conn = self.conn.lock().await;
        write_line(&mut conn.writer, &json!({"jsonrpc": "2.0", "method": method, "params": params})).await
    }
}

async fn write_line(writer: &mut WriteHalf<DuplexStream>, message: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(message).map_err(|e| McpHostError::Other(e.to_string()))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_line(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Result<String> {
    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Err(McpHostError::Other("MCP server closed the connection".into()));
        }
        let newline = available.iter().position(|&b| b == b'\n');
        let take = newline.map_or(available.len(), |at| at + 1);
        if buf.len() + take > MAX_LINE_BYTES {
            return Err(McpHostError::Other(format!("MCP response exceeds {MAX_LINE_BYTES} bytes")));
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            return String::from_utf8(buf).map_err(|e| McpHostError::Other(e.to_string()));
        }
    }
}

// ------------------------------------------------------------------------------------------
// HTTP

/// The endpoints an [`HttpServer`] serves, each an rmcp server over streamable HTTP.
pub struct HttpRoutes {
    router: axum::Router,
    config: StreamableHttpServerConfig,
}

impl HttpRoutes {
    fn new(cancel: &CancellationToken) -> Self {
        let config = StreamableHttpServerConfig::default()
            .with_cancellation_token(cancel.child_token())
            .with_allowed_origins(["http://localhost:*", "http://127.0.0.1:*", "http://[::1]:*"])
            .with_max_request_body_bytes(MAX_HTTP_BODY_BYTES);
        Self { router: axum::Router::new(), config }
    }

    /// Serve `server` (cloned per MCP session) at `path`.
    pub fn mcp<S: ServerHandler + Clone>(mut self, path: &str, server: S) -> Self {
        let service = StreamableHttpService::new(move || Ok(server.clone()), Arc::new(LocalSessionManager::default()), self.config.clone());
        self.router = self.router.nest_service(path, service);
        self
    }

    /// Answer `path` with 403 and `message` (an endpoint that exists but is switched off).
    pub fn forbidden(mut self, path: &str, message: &'static str) -> Self {
        self.router = self.router.route(path, axum::routing::any(move || async move { (StatusCode::FORBIDDEN, message) }));
        self
    }
}

/// MCP over streamable HTTP on `127.0.0.1:<port>`.
///
/// Loopback only, and every request needs `Authorization: Bearer <token>`: on Android any app
/// on the phone can reach 127.0.0.1, so the token, not the address, is the access control.
/// Before the token, a fence (as ARGO's `HttpGuard`): `Host` must be a loopback name (blocks
/// DNS rebinding), a present `Origin` must be a loopback origin, and `Sec-Fetch-Site:
/// cross-site` is refused, so a web page in the phone's browser cannot drive the server.
pub struct HttpServer {
    port: u16,
    cancel: CancellationToken,
}

impl HttpServer {
    /// Bind and serve on the current runtime. `routes` mounts the endpoints.
    pub async fn start(port: u16, token: String, routes: impl FnOnce(HttpRoutes) -> HttpRoutes) -> Result<Self> {
        if token.len() < 16 {
            return Err(McpHostError::BadRequest("the MCP token must be at least 16 characters".into()));
        }
        let cancel = CancellationToken::new();
        let router = routes(HttpRoutes::new(&cancel)).router;
        let token = Arc::new(token);
        let router = router.layer(axum::middleware::from_fn(move |req: Request, next: Next| {
            let token = token.clone();
            async move { guard(&token, req, next).await }
        }));
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await?;
        let port = listener.local_addr()?.port();
        let shutdown = cancel.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).with_graceful_shutdown(shutdown.cancelled_owned()).await;
        });
        Ok(Self { port, cancel })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(self) {
        self.cancel.cancel();
    }
}

async fn guard(token: &str, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let value = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if let Err(reason) = fence(value(header::HOST.as_str()), value(header::ORIGIN.as_str()), value("sec-fetch-site")) {
        return (StatusCode::FORBIDDEN, reason).into_response();
    }
    let presented = value(header::AUTHORIZATION.as_str()).and_then(bearer_token).unwrap_or("");
    if constant_time_eq(presented.as_bytes(), token.as_bytes()) {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer")], "missing or wrong bearer token").into_response()
    }
}

/// Host / Origin / Sec-Fetch-Site checks.
fn fence(host: Option<&str>, origin: Option<&str>, sec_fetch_site: Option<&str>) -> std::result::Result<(), &'static str> {
    let Some(host) = host else { return Err("missing Host") };
    if !is_loopback_authority(host) {
        return Err("Host is not a loopback name");
    }
    if let Some(origin) = origin {
        let authority = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://"));
        match authority {
            Some(a) if !a.contains('/') && is_loopback_authority(a) => {}
            _ => return Err("cross-origin requests are refused"),
        }
    }
    if sec_fetch_site.is_some_and(|s| s.trim().eq_ignore_ascii_case("cross-site")) {
        return Err("cross-site requests are refused");
    }
    Ok(())
}

/// `localhost`, `127.0.0.1` or `[::1]`, with an optional port.
pub fn is_loopback_authority(authority: &str) -> bool {
    let a = authority.trim();
    let host = if let Some(rest) = a.strip_prefix('[') {
        match rest.split_once(']') {
            Some((h, port)) if port.is_empty() || port.strip_prefix(':').is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) => {
                return h == "::1";
            }
            _ => return false,
        }
    } else {
        match a.rsplit_once(':') {
            Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
            Some(_) => return false,
            None => a,
        }
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1"
}

fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.trim().split_once(char::is_whitespace)?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// Compare secrets without an early exit.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_api::{Ctx, Handles, NoEvents, Shared};
    use lab_script::{Budget, ScriptRuntime};

    #[test]
    fn token_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert_eq!(bearer_token("Bearer  tok "), Some("tok"));
        assert_eq!(bearer_token("Basic tok"), None);
    }

    #[test]
    fn fence_checks_host_origin_and_fetch_site() {
        assert!(fence(Some("localhost:7700"), None, None).is_ok());
        assert!(fence(Some("127.0.0.1"), Some("http://localhost:3000"), Some("same-origin")).is_ok());
        assert!(fence(Some("[::1]:7700"), None, Some("none")).is_ok());
        assert!(fence(None, None, None).is_err());
        assert!(fence(Some("evil.example:7700"), None, None).is_err());
        assert!(fence(Some("localhost.evil.example"), None, None).is_err());
        assert!(fence(Some("localhost:7700"), Some("https://evil.example"), None).is_err());
        assert!(fence(Some("localhost:7700"), Some("null"), None).is_err());
        assert!(fence(Some("localhost:7700"), None, Some("cross-site")).is_err());
        assert!(!is_loopback_authority("[::2]:1"));
        assert!(!is_loopback_authority("localhost:"));
    }

    struct TestHost {
        registry: Registry,
        shared: Shared,
    }

    impl ScriptHost for TestHost {
        fn registry(&self) -> &Registry {
            &self.registry
        }
        fn run_script(&self, code: &str, read_only: bool, timeout: Duration, source: Source, cancel: Arc<AtomicBool>) -> ScriptOutcome {
            let mut ctx = Ctx::new(self.shared.clone(), source);
            ctx.cancel = cancel;
            if read_only {
                ctx = ctx.read_only();
            }
            ScriptRuntime::run(&self.registry, &ctx, code, Budget::with_wall(timeout))
        }
    }

    fn server(allow_edits: bool) -> LabMcp {
        let mut registry = Registry::new();
        registry.area("math", "arithmetic");
        registry.function("math", "add", "add").handler(|_, p| Ok(json!(p["a"].as_i64().unwrap_or(0) + p["b"].as_i64().unwrap_or(0)))).register();
        registry.function("math", "store", "write").effects(lab_api::Effects::Revertible).handler(|_, _| Ok(json!("stored"))).register();
        let shared = Shared { root: std::env::temp_dir(), state: Arc::new(()), handles: Arc::new(Handles::default()), events: Arc::new(NoEvents) };
        let info = LabServerInfo {
            name: "testlab".into(),
            version: "0".into(),
            instructions: "test".into(),
            read_blurb: "Read.".into(),
            script_blurb: "Write.".into(),
        };
        LabMcp::new(Arc::new(TestHost { registry, shared }), Arc::new(info), allow_edits, Source::Mcp)
    }

    #[test]
    fn in_process_tools_and_read_only() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            let c = InProcess::start(server(false), "t").await.unwrap();
            let tools = c.request("tools/list", json!({})).await.unwrap();
            let names: Vec<&str> = tools["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
            assert_eq!(names, ["lab_read", "lab_help"]);
            let r = c.request("tools/call", json!({"name": "lab_read", "arguments": {"code": "return await lab.math.add({a:2,b:3})", "description": "x"}})).await.unwrap();
            assert!(r["content"][0]["text"].as_str().unwrap().contains('5'), "{r}");
            let r = c.request("tools/call", json!({"name": "lab_read", "arguments": {"code": "return await lab.math.store({})", "description": "x"}})).await.unwrap();
            assert_eq!(r["isError"], json!(true));
            let r = c.request("tools/call", json!({"name": "lab_script", "arguments": {"code": "return 1", "description": "x"}})).await.unwrap();
            assert_eq!(r["isError"], json!(true));
            let edit = InProcess::start(server(true), "t").await.unwrap();
            let r = edit.request("tools/call", json!({"name": "lab_script", "arguments": {"code": "return await lab.math.store({})", "description": "x"}})).await.unwrap();
            assert_ne!(r["isError"], json!(true), "{r}");
        });
    }

    async fn post(port: u16, path: &str, headers: &str) -> String {
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let req = format!(
            "POST {path} HTTP/1.1\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{init}",
            init.len()
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut head = [0u8; 12];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut head).await.unwrap();
        String::from_utf8_lossy(&head[9..12]).to_string()
    }

    #[test]
    fn http_requires_the_token_and_the_fence() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            let s = server(false);
            let http = HttpServer::start(0, "0123456789abcdef".into(), |r| r.mcp("/mcp", s).forbidden("/off/mcp", "off")).await.unwrap();
            let port = http.port();
            let ok = "Host: localhost\r\nAuthorization: Bearer 0123456789abcdef\r\n";
            assert_eq!(post(port, "/mcp", "Host: localhost\r\n").await, "401");
            assert_eq!(post(port, "/mcp", "Host: localhost\r\nAuthorization: Bearer wrong-token-wrong!\r\n").await, "401");
            assert_eq!(post(port, "/mcp", ok).await, "200");
            assert_eq!(post(port, "/mcp", "Host: evil.example\r\nAuthorization: Bearer 0123456789abcdef\r\n").await, "403");
            assert_eq!(post(port, "/mcp", &format!("{ok}Origin: https://evil.example\r\n")).await, "403");
            assert_eq!(post(port, "/mcp", &format!("{ok}Sec-Fetch-Site: cross-site\r\n")).await, "403");
            assert_eq!(post(port, "/off/mcp", ok).await, "403");
            http.stop();
            assert!(HttpServer::start(0, "short".into(), |r| r).await.is_err());
        });
    }
}
