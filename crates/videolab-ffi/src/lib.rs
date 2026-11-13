//! # videolab-ffi
//!
//! The Android app's view of [`videolab_core::Lab`], through uniffi (Kotlin + JNA). JSON crosses
//! as strings: the engine's params and results are open-ended JSON already, and the agent hands
//! them straight to the model. Pixels and samples cross as byte/float arrays.
//!
//! Every method blocks; the app calls them off the main thread. A panic inside becomes a Kotlin
//! exception (uniffi catches it), never a crash of the app.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use videolab_core::lab_api::{ApiError, Event, EventSink, Source};
use videolab_core::{Lab, LabError, LabOptions, Renderer};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum VideoLabError {
    /// The request was refused (a bad path, bad JSON, an unknown command, bad params).
    #[error("{reason}")]
    BadRequest { reason: String },
    /// The engine or the file system failed.
    #[error("{reason}")]
    Failed { reason: String },
}

impl From<LabError> for VideoLabError {
    fn from(e: LabError) -> Self {
        match e {
            LabError::BadRequest(reason) => VideoLabError::BadRequest { reason },
            LabError::Api(api) => api.into(),
            other => VideoLabError::Failed { reason: other.to_string() },
        }
    }
}

/// Registry errors keep their stable code at the front of `reason` (`read_only: ...`).
impl From<ApiError> for VideoLabError {
    fn from(e: ApiError) -> Self {
        let reason = e.to_string();
        match e.code.as_str() {
            "bad_params" | "not_found" | "read_only" | "too_large" => VideoLabError::BadRequest { reason },
            _ => VideoLabError::Failed { reason },
        }
    }
}

type Result<T> = std::result::Result<T, VideoLabError>;

fn parse_source(source: &str) -> Result<Source> {
    match source.trim().to_ascii_lowercase().as_str() {
        "" | "ui" => Ok(Source::Ui),
        "agent" => Ok(Source::Agent),
        "mcp" => Ok(Source::Mcp),
        "cli" => Ok(Source::Cli),
        other => Err(VideoLabError::BadRequest { reason: format!("unknown source `{other}` (ui, agent, mcp, cli)") }),
    }
}

fn parse(json: &str) -> Result<Value> {
    if json.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str(json).map_err(|e| VideoLabError::BadRequest { reason: format!("invalid JSON: {e}") })
}

fn text(value: &Value) -> String {
    value.to_string()
}

/// Receives core events as JSON, e.g. `{"type":"docChanged","doc":21,"revision":7,"source":"agent"}`.
/// Called off the main thread.
#[uniffi::export(callback_interface)]
pub trait EventListener: Send + Sync {
    fn on_event(&self, event_json: String);
}

struct ListenerSink(Box<dyn EventListener>);

impl EventSink for ListenerSink {
    fn emit(&self, event: Event) {
        if let Ok(json) = serde_json::to_string(&event) {
            self.0.on_event(json);
        }
    }
}

/// What only the app can answer on FilmCraft's control channel (`ui.inspect`, `ui.screenshot`,
/// `ui.playback`): agents driving the app through FilmCraft's MCP bridge or CLI see the phone.
#[uniffi::export(callback_interface)]
pub trait UiCallbacks: Send + Sync {
    /// UI state as a JSON object.
    fn inspect_json(&self) -> String;
    /// PNG of the app window; empty when none can be taken now.
    fn screenshot_png(&self) -> Vec<u8>;
    /// `play`, `stop` or `toggle`; returns `{playing, playhead}` JSON.
    fn playback(&self, action: String, speed: f64) -> String;
}

struct UiBridge(Box<dyn UiCallbacks>);

impl videolab_core::UiHost for UiBridge {
    fn inspect(&self) -> Value {
        serde_json::from_str(&self.0.inspect_json()).unwrap_or_else(|_| json!({}))
    }
    fn screenshot(&self) -> Option<Vec<u8>> {
        Some(self.0.screenshot_png()).filter(|b| !b.is_empty())
    }
    fn playback(&self, action: &str, speed: f64) -> Value {
        serde_json::from_str(&self.0.playback(action.to_string(), speed)).unwrap_or_else(|_| json!({}))
    }
}

/// One image a script returned (`{image: "img:N"}` in its value).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScriptImage {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// What a `lab_script` / `lab_read` program produced. `value_json` is `null` when `error` is
/// set; `error` is `"<code>: <message>"`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ScriptResult {
    pub value_json: String,
    pub logs: Vec<String>,
    pub images: Vec<ScriptImage>,
    pub error: Option<String>,
}

/// One rendered picture: opaque RGBA8, row-major (copy into an ARGB_8888 bitmap).
#[derive(Debug, Clone, uniffi::Record)]
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// `gpu` or `cpu`; empty for thumbnails.
    pub renderer: String,
}

/// Program audio: interleaved stereo 16-bit little-endian PCM at `sample_rate` (bytes, so Kotlin
/// gets a `ByteArray` for `AudioTrack.write` rather than a boxed float list).
#[derive(Debug, Clone, uniffi::Record)]
pub struct AudioChunk {
    pub sample_rate: u32,
    pub pcm16: Vec<u8>,
}

/// What `filmcraft-cli` printed and returned.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CliResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// The control channel's address, for `filmcraft-cli --bridge` over `adb forward`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ControlInfo {
    pub port: u16,
    pub token: String,
}

fn frame(img: videolab_core::Rgba, renderer: &str) -> RgbaFrame {
    RgbaFrame { width: img.width, height: img.height, rgba: img.rgba, renderer: renderer.to_string() }
}

/// The core: one FilmCraft session over the app's workspace folder.
#[derive(uniffi::Object)]
pub struct VideoLab {
    lab: Lab,
}

#[uniffi::export]
impl VideoLab {
    /// Open the workspace at `root` (created if missing). `temp_dir`: the app's cache folder.
    /// `cli_path`: the bundled `libfilmcraft_cli.so` (empty: none). `gpu`: try FilmCraft's wgpu
    /// compositor (Vulkan) for previews and exports; it is probed first and stays off if it fails.
    #[uniffi::constructor]
    /// `read_roots`: folders media may be imported from in place (read-only), e.g. shared storage.
    pub fn new(root: String, temp_dir: String, cli_path: String, gpu: bool, read_roots: Vec<String>) -> Result<Arc<Self>> {
        let opts = LabOptions {
            gpu,
            temp_dir: Some(PathBuf::from(temp_dir)).filter(|p| !p.as_os_str().is_empty()),
            control_port: 0,
            cli_path: Some(PathBuf::from(cli_path)).filter(|p| !p.as_os_str().is_empty()),
            read_roots: read_roots.into_iter().filter(|r| !r.is_empty()).map(PathBuf::from).collect(),
        };
        Ok(Arc::new(Self { lab: Lab::new(Path::new(&root), &opts)? }))
    }

    pub fn workspace_root(&self) -> String {
        self.lab.root().display().to_string()
    }

    // ------------------------------------------------------------ registry and scripts

    /// `Registry::call("area.name", params)`; `source` is `ui`, `agent`, `mcp` or `cli`.
    pub fn call(&self, id: String, params_json: String, source: String) -> Result<String> {
        let source = parse_source(&source)?;
        Ok(text(&self.lab.call(&id, parse(&params_json)?, source)?))
    }

    pub fn help(&self, area: Option<String>) -> Result<String> {
        Ok(text(&self.lab.help(area.as_deref().filter(|a| !a.trim().is_empty()))?))
    }

    pub fn areas_summary(&self) -> String {
        self.lab.areas_summary()
    }

    /// Run a program as the in-app agent: `lab_script` (`read_only` false) or `lab_read`.
    /// `timeout_ms` 0 means the default (120 s). Never throws.
    pub fn run_script(&self, code: String, read_only: bool, timeout_ms: u64) -> ScriptResult {
        let o = self.lab.run_script(&code, read_only, Duration::from_millis(timeout_ms));
        ScriptResult {
            value_json: text(&o.value),
            logs: o.logs,
            images: o.images.into_iter().map(|i| ScriptImage { mime: i.mime, bytes: i.bytes }).collect(),
            error: o.error.map(|e| e.to_string()),
        }
    }

    pub fn cancel_scripts(&self) {
        self.lab.cancel_scripts();
    }

    pub fn set_event_listener(&self, listener: Box<dyn EventListener>) {
        self.lab.set_event_sink(Arc::new(ListenerSink(listener)));
    }

    pub fn set_ui_callbacks(&self, callbacks: Box<dyn UiCallbacks>) {
        self.lab.set_ui_host(Arc::new(UiBridge(callbacks)));
    }

    pub fn render_handle(&self, handle: String, max_side: u32) -> Result<Vec<u8>> {
        Ok(self.lab.render_handle(&handle, max_side)?)
    }

    // ------------------------------------------------------------ engine

    /// Run a FilmCraft engine command by id with JSON params (undoable edits land in history).
    pub fn execute(&self, id: String, params_json: String) -> Result<String> {
        Ok(text(&self.lab.execute(&id, parse(&params_json)?)?))
    }

    /// The active sequence, compact (seconds); `null` when none is open.
    pub fn timeline(&self) -> String {
        text(&self.lab.timeline())
    }

    /// Project items, flattened: `[{item, name, type, duration, bin}]`.
    pub fn items(&self) -> String {
        text(&self.lab.items())
    }

    /// Program frame at `seconds` (negative: the playhead), longest side at most `max_side`.
    pub fn render(&self, seconds: f64, max_side: u32) -> Result<RgbaFrame> {
        let (img, r) = self.lab.render((seconds >= 0.0).then_some(seconds), max_side)?;
        Ok(frame(img, if r == Renderer::Gpu { "gpu" } else { "cpu" }))
    }

    /// Program frame at `seconds` for playback: like [`Self::render`], but a decoder catching up
    /// may skip pictures only earlier (late) frames need, as FilmCraft's own player does.
    pub fn render_playback(&self, seconds: f64, max_side: u32) -> Result<RgbaFrame> {
        let (img, r) = self.lab.render_playback(seconds, max_side)?;
        Ok(frame(img, if r == Renderer::Gpu { "gpu" } else { "cpu" }))
    }

    /// Frames of media `item` at media `seconds`, `height` pixels high (the filmstrip).
    pub fn thumbnails(&self, item: u64, seconds: Vec<f64>, height: u32) -> Result<Vec<RgbaFrame>> {
        Ok(self.lab.thumbnails(item, &seconds, height)?.into_iter().map(|i| frame(i, "")).collect())
    }

    /// Waveform of media `item` over media seconds: `buckets` (min, max) pairs, flattened.
    pub fn peaks(&self, item: u64, from: f64, to: f64, buckets: u32) -> Result<Vec<f32>> {
        Ok(self.lab.peaks(item, from, to, buckets as usize)?.into_iter().flat_map(|(a, b)| [a, b]).collect())
    }

    /// Program audio from `start_seconds`: `frames` stereo frames, interleaved.
    pub fn mix(&self, start_seconds: f64, frames: u32) -> Result<AudioChunk> {
        let (sample_rate, samples) = self.lab.mix(start_seconds, frames as usize)?;
        let mut pcm16 = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            pcm16.extend_from_slice(&v.to_le_bytes());
        }
        Ok(AudioChunk { sample_rate, pcm16 })
    }

    /// A free name in `media/` for a picked file; the app copies the file there, then calls
    /// [`VideoLab::import_file`].
    pub fn media_target(&self, name: String) -> Result<String> {
        Ok(self.lab.media_target(&name)?)
    }

    /// Import a workspace file into the project.
    pub fn import_file(&self, path: String) -> Result<String> {
        Ok(text(&self.lab.import_file(&path)?))
    }

    /// `(wanted, active)` for the GPU renderer, as `[bool, bool]`.
    pub fn gpu_status(&self) -> Vec<bool> {
        let (w, a) = self.lab.gpu_status();
        vec![w, a]
    }

    /// Hardware decoding backend and counters, JSON.
    pub fn decode_stats(&self) -> String {
        text(&self.lab.decode_stats())
    }

    pub fn set_gpu(&self, on: bool) {
        self.lab.set_gpu(on);
    }

    // ------------------------------------------------------------ MCP

    /// One MCP request to FilmCraft's own MCP server (bridge mode over the control channel).
    pub fn filmcraft_mcp_request(&self, method: String, params_json: String) -> Result<String> {
        Ok(text(&self.lab.filmcraft_mcp_request(&method, parse(&params_json)?)?))
    }

    /// One MCP request to VideoLab's own server (`lab_read`, `lab_script`, `lab_help`).
    /// One JSON-RPC message from ARGO (the companion service): the response, or "" for a notification.
    pub fn companion_mcp(&self, message: String) -> Result<String> {
        Ok(self.lab.companion_mcp(&message)?)
    }

    pub fn lab_mcp_request(&self, method: String, params_json: String) -> Result<String> {
        Ok(text(&self.lab.lab_mcp_request(&method, parse(&params_json)?)?))
    }

    /// Serve MCP over HTTP on 127.0.0.1:`port`: VideoLab at `/mcp`, FilmCraft at
    /// `/filmcraft/mcp` (only with `allow_edits`). Returns the port.
    pub fn start_mcp_http(&self, port: u16, token: String, allow_edits: bool) -> Result<u16> {
        Ok(self.lab.start_http(port, &token, allow_edits)?)
    }

    pub fn stop_mcp_http(&self) {
        self.lab.stop_http();
    }

    pub fn mcp_http_port(&self) -> Option<u16> {
        self.lab.http_port()
    }

    pub fn control_info(&self) -> ControlInfo {
        let (port, token) = self.lab.control();
        ControlInfo { port, token }
    }

    // ------------------------------------------------------------ CLI

    /// Run a `filmcraft-cli` command line against the document on screen, e.g.
    /// `exec timeline.razor seconds=3` or `inspect sequence`.
    pub fn cli(&self, command_line: String, timeout_ms: u64) -> CliResult {
        match videolab_core::split_command_line(&command_line) {
            Ok(args) => self.cli_args(args, timeout_ms),
            Err(message) => CliResult { code: 2, stdout: String::new(), stderr: message },
        }
    }

    pub fn cli_args(&self, args: Vec<String>, timeout_ms: u64) -> CliResult {
        match self.lab.cli(&args, Duration::from_millis(timeout_ms)) {
            Ok(o) => CliResult { code: o.code, stdout: o.stdout, stderr: o.stderr },
            Err(e) => CliResult { code: 1, stdout: String::new(), stderr: e.to_string() },
        }
    }
}
