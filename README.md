# VideoLab

A standalone Android video editor on [FilmCraft](https://github.com/storytold/filmcraft)'s engine,
built to try FilmCraft's **MCP server and CLI** on a phone: an in-app Claude agent, MCP servers
that agents on your laptop drive over `adb`, and FilmCraft's own `filmcraft-cli`, all on the
document you see. Design: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Sibling of ImageLab
(PhotoCraft on Android); same skeleton.

| Path | What |
|---|---|
| `crates/videolab-core` | One FilmCraft session shared by UI, agent, MCP, CLI; FilmCraft's control channel; registry areas; MediaCodec decoder |
| `crates/videolab-ffi` | uniffi bindings for Kotlin (`libvideolab_ffi.so`) |
| `crates/videolab-cli` | `videolab`: `call` / `help` / `script` over the registry, `serve-http`, `bench-decode` |
| `crates/lab-api`, `lab-script`, `lab-mcp` | Engine-agnostic registry, JS runtime and MCP host, lifted from ImageLab |
| `android/` | The app: Kotlin + Jetpack Compose; agent on the Anthropic Java SDK |
| `skills/videolab` | Agent Skill for the VideoLab MCP servers |
| `tools/build-filmcraft-cli.sh` | Builds FilmCraft's unmodified `filmcraft-cli` for Android from the pinned rev |

FilmCraft is a pinned git dependency (root `Cargo.toml`), never vendored.

## Build

Prerequisites: Rust (stable ≥ 1.95) with `aarch64-linux-android`, `cargo install cargo-ndk`,
Android SDK (platform 37) + NDK 27, JDK 17. Android 13+ (minSdk 33), arm64.

```bash
cargo test -p videolab-core
```

```bash
cd android && ANDROID_NDK_HOME=$HOME/Library/Android/sdk/ndk/27.2.12479018 ./gradlew :app:installDebug
```

Gradle runs cargo-ndk (core + `videolab`), builds `filmcraft-cli`, and runs uniffi-bindgen
before compiling Kotlin.

## MCP from your laptop

In the app: **Settings → Developer → MCP server** → on (shows the token; "Allow edits" enables
`lab_script` and FilmCraft's endpoint). Then:

```bash
adb forward tcp:7700 tcp:7700
```

```bash
claude mcp add --transport http videolab http://localhost:7700/mcp --header "Authorization: Bearer <token>"
```

```bash
claude mcp add --transport http filmcraft http://localhost:7700/filmcraft/mcp --header "Authorization: Bearer <token>"
```

`/mcp` is VideoLab's server: `lab_read`, `lab_help`, and with allow edits `lab_script`
(JavaScript programs over the registry; see `skills/videolab/SKILL.md`). `/filmcraft/mcp` is
**FilmCraft's own MCP server**, unmodified, in bridge mode against the app's session. Or export
`VIDEOLAB_MCP_TOKEN=<token>` and use this repo's `.mcp.json`. Without a phone:

```bash
cargo run -p videolab-cli -- serve-http --root ./workspace --token "$(openssl rand -hex 24)" --allow-edits
```

## FilmCraft's CLI against the live app

The app serves FilmCraft's desktop control protocol on loopback, so `filmcraft-cli --bridge`
edits the document on screen. In-app: **Settings → Developer → FilmCraft CLI console**
(`commands razor`, `inspect sequence`, `exec timeline.razor seconds=2`). From a laptop, with the
port and token from Settings (or `adb shell run-as dev.argo.videolab cat files/VideoLab/.videolab/control.json`):

```bash
adb forward tcp:<port> tcp:<port>
```

```bash
FILMCRAFT_CONTROL_TOKEN=<token> filmcraft-cli --bridge 127.0.0.1:<port> inspect sequence
```

```bash
FILMCRAFT_CONTROL_TOKEN=<token> filmcraft-cli mcp --bridge 127.0.0.1:<port>
```

The last one is FilmCraft's stdio MCP server on your laptop, driving the phone.

## Registry CLI

```bash
cargo run -p videolab-cli -- help timeline
```

```bash
cargo run -p videolab-cli -- call doc.demo --root ./workspace
```

```bash
cargo run -p videolab-cli -- script cut.js --root ./workspace --out-dir out/
```

## App

* **Home:** new project from videos, 9:16 / 16:9 / 1:1, FilmCraft's demo project, saved projects.
* **Editor:** preview (wgpu/Vulkan or CPU, shown bottom-left), transport, a phone timeline with a
  fixed centre playhead (drag to scrub, pinch to zoom, tap to select, drag handles to trim),
  filmstrips and waveforms; tool bar: Media, Audio, Text, Templates, Captions (manual, **auto
  captions**, remove pauses / “um”s), Effects, Filters, Adjust, **Text to speech**, Split, Agent;
  with a clip selected: Split, Volume, Speed (with **smooth slow motion**), Fade, Transition,
  **Filters, Effects, Adjust, Transform** (keyframes: the ◇ by the transport), **Freeze,
  Reverse, Mirror, Rotate, Auto cut**; on sound: **Auto duck, Loudness**. Export
  480p/720p/1080p MP4 or GIF, then share. All of these are registry functions (`look`, `clip`,
  `audio`, `speech`), so agents can use them too.
* **Agent:** Settings → Agent → your Anthropic API key (sealed with an Android Keystore key).

Licence: Apache-2.0. FilmCraft is MIT OR Apache-2.0.
