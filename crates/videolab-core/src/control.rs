//! FilmCraft's control channel, served by VideoLab over the app's own session.
//!
//! The desktop FilmCraft app listens on `127.0.0.1:<port>` for JSON lines (`filmcraft --control
//! PORT`, docs/control-protocol.md upstream); its MCP server's *bridge* mode
//! (`FilmcraftMcp::bridge`) and `filmcraft-cli --bridge` are clients of it. VideoLab speaks the
//! same protocol, so those two upstream programs drive the phone's live document **unmodified**:
//!
//! * in-process: the in-app agent's FilmCraft tools and the HTTP `/filmcraft/mcp` endpoint are
//!   `FilmcraftMcp::bridge("127.0.0.1:<port>")`;
//! * on the phone: the APK's bundled `filmcraft-cli` runs `--bridge 127.0.0.1:<port> exec …`;
//! * from a laptop: `adb forward tcp:<port> tcp:<port>`, then `filmcraft-cli --bridge …` there.
//!
//! Rules kept from the desktop server: loopback only; every line carries `token` (compared in
//! constant time; a wrong one closes the connection); one JSON object with a string `method` per
//! line, anything else gets one error reply and the connection closes; lines up to 4 MiB; at most
//! 16 connections; two minutes without a complete line closes the connection.
//!
//! Methods: `engine.execute` / `ui.menu.invoke`, `engine.commands`, and the `ui.*` methods that
//! make sense on a phone, answered by the app through [`UiHost`]: `ui.inspect`, `ui.screenshot`
//! (`panel: "Program"` renders the program frame in the core; no panel asks the app for a picture
//! of its window), `ui.playback`. Desktop-only methods (`ui.click`, `ui.elements`, docking…) are
//! refused with an error that says so.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::engine::Engine;

const MAX_LINE: usize = 4 << 20;
const MAX_CONNECTIONS: usize = 16;
const IDLE: Duration = Duration::from_secs(120);

/// What only the app (Kotlin) can answer. Called off the main thread.
pub trait UiHost: Send + Sync {
    /// UI state as JSON (screen, tool, playing, selection…).
    fn inspect(&self) -> Value;
    /// A PNG of the app window, or None when it cannot take one now.
    fn screenshot(&self) -> Option<Vec<u8>>;
    /// `play` / `stop` / `toggle`; returns `{playing, playhead}`.
    fn playback(&self, action: &str, speed: f64) -> Value;
}

/// The app is not attached (CLI, tests).
pub struct NoUi;
impl UiHost for NoUi {
    fn inspect(&self) -> Value {
        json!({"app": "videolab", "attached": false})
    }
    fn screenshot(&self) -> Option<Vec<u8>> {
        None
    }
    fn playback(&self, _: &str, _: f64) -> Value {
        json!({"playing": false})
    }
}

/// Called after every request that may have changed the session.
pub type AfterRequest = Arc<dyn Fn() + Send + Sync>;

pub struct ControlServer {
    port: u16,
    cancel: CancellationToken,
}

struct State {
    engine: Arc<Engine>,
    ui: Arc<std::sync::RwLock<Arc<dyn UiHost>>>,
    token: String,
    /// Where `ui.screenshot` may write: the workspace and the temp folder.
    write_roots: Vec<PathBuf>,
    after: AfterRequest,
    open: AtomicUsize,
}

impl ControlServer {
    /// Bind `127.0.0.1:port` (0 picks one) on the current runtime.
    pub async fn start(
        engine: Arc<Engine>,
        ui: Arc<std::sync::RwLock<Arc<dyn UiHost>>>,
        port: u16,
        token: String,
        write_roots: Vec<PathBuf>,
        after: AfterRequest,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let port = listener.local_addr()?.port();
        let cancel = CancellationToken::new();
        let state = Arc::new(State { engine, ui, token, write_roots, after, open: AtomicUsize::new(0) });
        let stop = cancel.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    a = listener.accept() => a,
                    () = stop.cancelled() => break,
                };
                let Ok((stream, peer)) = accepted else { continue };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let st = state.clone();
                let stop = stop.clone();
                tokio::spawn(async move {
                    if st.open.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                        let mut s = stream;
                        let _ = s.write_all(b"{\"ok\":false,\"error\":\"too many connections\"}\n").await;
                    } else {
                        tokio::select! {
                            () = serve(st.clone(), stream) => {},
                            () = stop.cancelled() => {},
                        }
                    }
                    st.open.fetch_sub(1, Ordering::AcqRel);
                });
            }
        });
        Ok(Self { port, cancel })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn serve(st: Arc<State>, stream: tokio::net::TcpStream) {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    loop {
        let mut buf = Vec::new();
        let line = tokio::time::timeout(IDLE, read_line(&mut reader, &mut buf)).await;
        let line = match line {
            Ok(Ok(Some(()))) => buf,
            _ => return,
        };
        let text = match String::from_utf8(line) {
            Ok(t) => t,
            Err(_) => return reply_close(&mut write, Value::Null, "the request is not UTF-8").await,
        };
        if text.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&text) {
            Ok(v @ Value::Object(_)) => v,
            _ => return reply_close(&mut write, Value::Null, "a request is one JSON object per line").await,
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = req.get("method").and_then(Value::as_str).map(str::to_string) else {
            return reply_close(&mut write, id, "missing `method`").await;
        };
        let presented = req.get("token").and_then(Value::as_str).unwrap_or("");
        if !lab_mcp::constant_time_eq(presented.as_bytes(), st.token.as_bytes()) {
            return reply_close(&mut write, id, "missing or wrong `token`").await;
        }
        let params = req.get("params").cloned().unwrap_or(json!({}));
        let s2 = st.clone();
        let result = tokio::task::spawn_blocking(move || handle(&s2, &method, params)).await.unwrap_or_else(|e| Err(format!("internal error: {e}")));
        let reply = match result {
            Ok(v) => json!({"id": id, "ok": true, "result": v}),
            Err(e) => json!({"id": id, "ok": false, "error": e}),
        };
        if write.write_all(format!("{reply}\n").as_bytes()).await.is_err() {
            return;
        }
    }
}

/// Read one line into `buf` (without the newline). `None` at EOF; an error past [`MAX_LINE`].
async fn read_line(r: &mut BufReader<tokio::net::tcp::OwnedReadHalf>, buf: &mut Vec<u8>) -> std::io::Result<Option<()>> {
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            return Ok(None);
        }
        let nl = avail.iter().position(|&b| b == b'\n');
        let take = nl.map_or(avail.len(), |i| i + 1);
        if buf.len() + take > MAX_LINE {
            return Err(std::io::Error::other("line too long"));
        }
        buf.extend_from_slice(&avail[..nl.unwrap_or(take)]);
        r.consume(take);
        if nl.is_some() {
            return Ok(Some(()));
        }
    }
}

async fn reply_close(w: &mut tokio::net::tcp::OwnedWriteHalf, id: Value, msg: &str) {
    let reply = json!({"id": id, "ok": false, "error": format!("{msg}; closing the connection")});
    let _ = w.write_all(format!("{reply}\n").as_bytes()).await;
}

fn handle(st: &State, method: &str, p: Value) -> Result<Value, String> {
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    let ui = st.ui.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let r = match method {
        "engine.execute" | "ui.menu.invoke" => {
            let id = s("command").or(s("id")).ok_or("missing `command`")?;
            st.engine.execute(id, p.get("params").cloned().unwrap_or(json!({})))
        }
        "engine.commands" | "ui.menu.list" => st.engine.execute("command.list", json!({})),
        "ui.inspect" => {
            let mut v = ui.inspect();
            if let Value::Object(m) = &mut v {
                let (wanted, active) = st.engine.gpu_status();
                m.insert("gpu".into(), json!({"wanted": wanted, "active": active}));
            }
            Ok(v)
        }
        "ui.playback" => Ok(ui.playback(s("action").unwrap_or("toggle"), p.get("speed").and_then(Value::as_f64).unwrap_or(1.0))),
        "ui.screenshot" => screenshot(st, ui.as_ref(), s("panel"), s("path")),
        m if m.starts_with("ui.") => Err(format!("`{m}` drives the desktop app's mouse and keyboard; VideoLab on Android answers ui.inspect, ui.screenshot and ui.playback — use engine commands (command_run) for everything else")),
        m => Err(format!("unknown method `{m}`")),
    };
    (st.after)();
    r
}

fn screenshot(st: &State, ui: &dyn UiHost, panel: Option<&str>, path: Option<&str>) -> Result<Value, String> {
    let png = match panel {
        Some(p) if p.eq_ignore_ascii_case("program") => {
            let t = st.engine.lock().playhead();
            let (img, _) = st.engine.render(t, 1920)?;
            lab_script::encode_png(img.width, img.height, lab_script::ColorType::Rgba, &img.rgba).map_err(|e| e.to_string())?
        }
        Some(p) => return Err(format!("panel `{p}`: VideoLab has the Program monitor only (or omit `panel` for the whole screen)")),
        None => ui.screenshot().ok_or("the app cannot take a screenshot now (is it in the foreground?)")?,
    };
    let Some(path) = path else {
        use base64::Engine as _;
        return Ok(json!({"pngBase64": base64::engine::general_purpose::STANDARD.encode(&png)}));
    };
    let target = PathBuf::from(path);
    let parent = target.parent().and_then(|d| d.canonicalize().ok()).ok_or_else(|| format!("{path}: its folder does not exist on the phone"))?;
    if !st.write_roots.iter().any(|r| parent.starts_with(r)) {
        return Err(format!("{path}: screenshots are written only inside the workspace or the app's temp folder"));
    }
    std::fs::write(&target, &png).map_err(|e| format!("{path}: {e}"))?;
    Ok(json!({"path": path, "bytes": png.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;

    async fn call(port: u16, lines: &[String]) -> Vec<Value> {
        let s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (r, mut w) = s.into_split();
        let mut r = tokio::io::BufReader::new(r).lines();
        let mut out = Vec::new();
        for l in lines {
            w.write_all(format!("{l}\n").as_bytes()).await.unwrap();
            match r.next_line().await.unwrap() {
                Some(line) => out.push(serde_json::from_str(&line).unwrap()),
                None => break,
            }
        }
        out
    }

    #[test]
    fn speaks_the_protocol_and_checks_the_token() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let services = Arc::new(crate::services::WorkspaceServices::new(dir.path().canonicalize().unwrap(), None));
            let engine = Arc::new(Engine::new(services));
            let ui: Arc<std::sync::RwLock<Arc<dyn UiHost>>> = Arc::new(std::sync::RwLock::new(Arc::new(NoUi)));
            let token = "t".repeat(64);
            let srv = ControlServer::start(engine, ui, 0, token.clone(), vec![], Arc::new(|| {})).await.unwrap();
            let ok = call(
                srv.port(),
                &[
                    json!({"id": 1, "token": token, "method": "engine.execute", "params": {"command": "file.newSequence", "params": {"name": "S"}}}).to_string(),
                    json!({"id": 2, "token": token, "method": "engine.commands"}).to_string(),
                    json!({"id": 3, "token": token, "method": "ui.click", "params": {"id": "x"}}).to_string(),
                ],
            )
            .await;
            assert_eq!(ok[0]["ok"], json!(true), "{}", ok[0]);
            assert!(ok[1]["result"].as_array().unwrap().len() > 100);
            assert_eq!(ok[2]["ok"], json!(false));
            let bad = call(srv.port(), &[json!({"id": 1, "token": "nope", "method": "engine.commands"}).to_string(), "{}".into()]).await;
            assert_eq!(bad.len(), 1, "a wrong token closes the connection");
            assert_eq!(bad[0]["ok"], json!(false));
            let junk = call(srv.port(), &["not json".into(), "{}".into()]).await;
            assert_eq!(junk.len(), 1);
            srv.stop();
        });
    }
}
