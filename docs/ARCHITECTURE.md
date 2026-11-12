# VideoLab — architecture

**Goal:** a standalone Android video editor whose real purpose is to try FilmCraft's **MCP server
and CLI** on a phone: an in-app Claude agent, MCP servers that agents on a laptop drive over
`adb`, and FilmCraft's own `filmcraft-cli`, all on the document the person sees. The editing UI is
kept small (a phone timeline: split, trim, speed, volume, fade, transitions, titles, templates,
captions, export); everything else is one of FilmCraft's ~700 engine commands away.

Second app on ImageLab's skeleton (ImageLab = PhotoCraft on Android). Same rules: the engine is a
pinned git dependency, never vendored or forked; one shared session; one function registry for
every surface; never crash.

```
┌──────────────────────────── Android app (Kotlin, Jetpack Compose) ─────────────────────────────┐
│  Home · Editor (preview · transport · centre-playhead timeline · tool bar/sheets) · Settings   │
│  Player (AudioTrack clock + frames)   Agent (Claude tool loop, Anthropic Java SDK)             │
└───────────────┬─────────────────────────────────────────────────────────────────────────────────┘
   uniffi (JNA) │ libvideolab_ffi.so                      exec │ libfilmcraft_cli.so (FilmCraft's own)
┌───────────────▼──────────────────────── videolab-core (Rust) ──────────▼───────────────────────┐
│ one filmcraft_engine::Session (WorkspaceServices: confined to files/VideoLab)                    │
│   ▲ UI: commands, timeline, frames (wgpu or CPU), filmstrips, peaks, mix                         │
│   ▲ registry (lab-api) + JS runtime (lab-script) ─ LabMcp: lab_read/lab_script/lab_help         │
│   ▲ FilmCraft control channel (127.0.0.1:<port>, token) ◄── FilmcraftMcp::bridge (upstream)     │
│                                                       ◄── filmcraft-cli --bridge (upstream)     │
│ lab-mcp: in-process clients · HTTP :7700 (/mcp, /filmcraft/mcp) bearer + loopback fence          │
│ MediaCodec → filmcraft_codecs::register_video_decoder (HybridDecoder fallback to software)      │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
 FilmCraft git deps: engine (~700 commands) · automation (MCP) · gpu (wgpu → Vulkan) · codecs (pure Rust) · export
```

## 1. Reuse

| From | What | How |
|---|---|---|
| FilmCraft | engine, automation (MCP server), gpu, export, codecs, platform | Cargo git deps at one rev (root `Cargo.toml`; `Cargo.lock` seeded from FilmCraft's) |
| FilmCraft | `filmcraft-cli` | binary-only package: built unmodified from cargo's checkout of the same rev (`tools/build-filmcraft-cli.sh`), shipped as `libfilmcraft_cli.so` |
| ImageLab | registry, handles, events | `crates/lab-api` (engine-agnostic: the session is an opaque `Shared::state`) |
| ImageLab | JS script runtime | `crates/lab-script` (unchanged behaviour; PNG via the `png` crate) |
| ImageLab | MCP server/clients/HTTP host | `crates/lab-mcp` (app name, blurbs and endpoints are parameters) |
| ImageLab | FFI pattern, CLI shape, uniffi-bindgen, Gradle/cargo-ndk build, agent loop, sealed API key, settings, console | copied and adapted |

The `lab-*` crates link no engine, so ImageLab can move onto them (ImageLab's own architecture
doc planned this "when the second app lands").

## 2. The control channel: maximum reuse of FilmCraft's MCP and CLI

FilmCraft's MCP server has two modes: *headless* (owns a `Session`) and *bridge* (a client of the
desktop app's JSON-lines control channel). Headless would mean a second session, invisible to
the UI. Instead `videolab-core::control` **serves the control protocol over the app's session**
(`engine.execute`, `engine.commands`, plus `ui.inspect` / `ui.screenshot` / `ui.playback`
answered by the app). Then, unmodified:

* `FilmcraftMcp::bridge` is the in-app agent's FilmCraft toolset and the `/filmcraft/mcp` HTTP
  endpoint — FilmCraft's own tool schemas, strict arguments, long-export progress;
* `filmcraft-cli --bridge 127.0.0.1:<port>` edits the open document from the in-app console,
  the agent's `filmcraft_cli` tool, `adb shell`, or a laptop after `adb forward`.

Security: loopback, a random 64-hex token per process (FilmCraft's `FILMCRAFT_CONTROL_TOKEN`
convention; constant-time compare, wrong token closes the connection), 4 MiB lines, 16
connections. `ui.screenshot` writes only inside the workspace or temp folder. The port and token
are also in `files/VideoLab/.videolab/control.json` (app-private) for adb workflows.

## 3. Rendering, decoding, playback ("graph acceleration")

* **Frames:** renders snapshot the project (`Arc`) under the session lock and render outside it,
  so playback never waits for an agent's command. The GPU path is FilmCraft's wgpu compositor
  (`filmcraft_gpu::ExportRenderer`, Vulkan; wgpu backends are chosen by the host), probed with
  one frame first; any failure falls back to FilmCraft's CPU reference renderer for the session.
  Exports register the same GPU renderer (`filmcraft_export::register_frame_renderer`).
* **Decoding:** FilmCraft's decoders are pure Rust (4K HEVC: ~12 fps on a 2026 flagship's CPU).
  `videolab-core::mediacodec` puts Android's MediaCodec in front of them through FilmCraft's own
  seam (`register_video_decoder`), as FilmCraft's desktop builds do with VideoToolbox / Media
  Foundation / VA-API, wrapped in FilmCraft's `HybridDecoder` (transparent software fallback).
  Output is ByteBuffer mode (`COLOR_FormatYUV420Flexible`, planes as the format's `image-data`
  states them), not a surface: Qualcomm's decoder fills an `AImageReader` with its tiled UBWC
  layout whatever format is asked for, which the CPU cannot read. Parameter sets are sent
  in-band. A picture that cannot be read hands the stream to software at once; a session that
  produces nothing latches hardware decoding off for the run.
* **Playback:** `Player.kt` — the program mix (`Engine::mix`, FilmCraft's `previews.mix_layout`)
  streamed to an `AudioTrack`; the clock is its presentation timestamp (output latency
  included). Frames are on the sequence's grid: a render stage makes each one ahead of time at
  640 px (`render_playback`: FilmCraft's zero catch-up margin, as its own player passes) into
  four reused bitmaps, a present stage shows each when the clock reaches it; behind, the
  renderer skips ahead. The preview frame is its own flow and the timeline reads the playhead
  while drawing (vsync-driven during playback), so a frame redraws the monitor and the timeline
  canvas, not the editor. 4K60 HEVC in a 30 fps sequence: 30 new frames/s, ~20 ms each.
* **Timeline media:** filmstrip frames (`Engine::thumbnails`, power-of-two time buckets so zoom
  reuses the cache; read from each clip's own copy of its file, `MediaPool::own_source`, so
  thumbnails never seek the decoder playback reads — sharing it held playback at ~1 fps) and waveform peaks (`Engine::peaks`, 8 kHz envelope) cached in Kotlin.

Render timings and FilmCraft's decoder counters are logged to logcat (tag `videolab`) every
two seconds of rendering.

Next, in order of payoff: zero-copy (decoder output imported into wgpu instead of copying 4K
planes), a SurfaceView the compositor renders into directly, proxies for sources the hardware
declines (10-bit HDR).

## 4. Agents

* **In-app agent** (`agent/Agent.kt`): Opus 5.5, adaptive thinking, effort high, prompt caching,
  server-side fallbacks. Tools: VideoLab's `lab_*` (exactly `/mcp`'s list), FilmCraft's MCP tools
  prefixed `filmcraft_` (listed from the bridge; desktop mouse/keyboard tools filtered out), and
  `filmcraft_cli`. Images from any of them go back to Claude as image blocks.
* **External agents:** Settings → Developer → MCP server: `127.0.0.1:7700` with a bearer token;
  `/mcp` always (read-only unless "allow edits"), `/filmcraft/mcp` only with "allow edits".
* **Off-device:** `videolab serve-http --root DIR` runs the same core, MCP endpoints and control
  channel on a laptop.

## 5. Events

`DocChanged {doc, revision, source}` when the revision, playhead or open sequence moves — after
every core call, script and control request, plus a 200 ms watcher. The editor re-reads the
timeline and re-renders the frame under the playhead (debounced; not while playing).

## 6. Rules carried over

No `unwrap`/`expect`/`panic` in production Rust; `unsafe` denied workspace-wide with two
documented exceptions (`set_var` for the control token once per process; `Send` for the
MediaCodec decoder). Paths confined by `WorkspaceServices`. Picked file names sanitized. Secrets:
API key sealed with an Android Keystore key; MCP token random and rotatable.

## 7. Status (2026-11-12)

* Verified: 39 host tests (registry, scripts, MCP in-process + HTTP fence, FilmCraft's MCP over
  the bridge, frames/thumbnails/peaks/mix on the demo project, a real H.264 export, GPU render on
  Metal); APK builds; runs on a Galaxy (SM-S948N) with the wgpu/Vulkan preview; the unmodified
  `filmcraft-cli` drives the live app over the bridge (emulator).
* MediaCodec verified in the app on the Galaxy: a 4K60 HEVC phone clip (96 Mbit/s) plays at
  ~48 fps in the 640 px preview (renders 13–19 ms; software decoding gave ~1–12 fps). 10-bit
  clips still decode in software. Shell-context MediaCodec tests are invalid (SELinux denies
  the codec service the shell's shared memory); the emulator's codec rejects all input on API 37.
