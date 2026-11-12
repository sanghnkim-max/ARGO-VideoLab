//! VideoLab's function registry (docs/INTERFACES.md §2): what the in-app agent's programs, the
//! `/mcp` server, the `videolab` CLI and the app's buttons all call.
//!
//! Most functions are thin, seconds-based wrappers over FilmCraft engine commands (ticks, item
//! ids and track names stay available through `engine.run`), plus what commands do not give:
//! a compact timeline projection an agent can read cheaply, rendered frames and filmstrips as
//! image handles it can *look at*, and waveform peaks.

use serde::Deserialize;
use serde_json::{Value, json};

use lab_api::{ApiError, ApiResult, Ctx, Effects, ImageBuf, Module, Registry};
use filmcraft_time::Tick;

use crate::engine::{Engine, Rgba};

fn engine(ctx: &Ctx) -> ApiResult<&Engine> {
    ctx.state::<Engine>()
}

fn run(ctx: &Ctx, id: &str, params: Value) -> ApiResult<Value> {
    engine(ctx)?.execute(id, params).map_err(ApiError::engine)
}

fn ticks(seconds: f64) -> i64 {
    Tick::from_seconds_f64(seconds).0
}

fn secs(t: &Value) -> f64 {
    t.as_i64().map_or(0.0, |t| Tick(t).seconds())
}

fn put(ctx: &Ctx, img: Rgba) -> ApiResult<String> {
    Ok(ctx.shared.handles.put_image(ImageBuf::new(img.width, img.height, img.rgba)?)?.to_string())
}

/// The active sequence as a compact projection: seconds instead of ticks, one line per clip.
pub fn timeline_projection(seq: &Value) -> Value {
    let rate = &seq["settings"]["frame_rate"];
    let fps = rate["num"].as_f64().unwrap_or(24.0) / rate["den"].as_f64().unwrap_or(1.0).max(1.0);
    let track = |t: &Value, kind: &str| {
        let clips: Vec<Value> = t["items"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|c| {
                json!({
                    "clip": c["clip"], "item": c["item"], "name": c["name"],
                    "start": secs(&c["start"]), "end": secs(&c["end"]),
                    "sourceIn": secs(&c["sourceIn"]), "sourceOut": secs(&c["sourceOut"]),
                    "speed": c["speed"], "reverse": c["reverse"], "gainDb": c["gainDb"], "enabled": c["enabled"],
                    "link": c["link"],
                    "effects": c["effects"].as_array().map(|e| e.iter().filter_map(|x| x["effect"].as_str().map(str::to_string)).collect::<Vec<_>>()).unwrap_or_default(),
                })
            })
            .collect();
        let transitions: Vec<Value> = t["transitions"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|x| json!({"id": x["id"], "effect": x["effect"], "from": x["from"], "to": x["to"], "start": secs(&x["start"]), "duration": secs(&x["duration"])}))
            .collect();
        json!({"id": t["id"], "name": t["name"], "kind": kind, "muted": t["muted"], "locked": t["locked"], "clips": clips, "transitions": transitions})
    };
    let mut tracks: Vec<Value> = Vec::new();
    for t in seq["video"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        tracks.push(track(t, "video"));
    }
    for t in seq["audio"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        tracks.push(track(t, "audio"));
    }
    json!({
        "sequence": seq["id"], "name": seq["name"],
        "width": seq["settings"]["width"], "height": seq["settings"]["height"], "fps": (fps * 1000.0).round() / 1000.0,
        "duration": secs(&seq["duration"]), "playhead": secs(&seq["playhead"]),
        "selection": seq["selection"],
        "markers": seq["markers"].as_array().map(|m| m.iter().map(|x| json!({"id": x["id"], "name": x["name"], "at": secs(&x["start"]), "color": x["color"]})).collect::<Vec<_>>()).unwrap_or_default(),
        "tracks": tracks,
    })
}

/// Project items (media, sequences, graphics) flattened out of the bin tree.
pub fn items_projection(project: &Value) -> Value {
    fn walk(node: &Value, bin: &str, out: &mut Vec<Value>) {
        for c in node["children"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            if c.get("item").is_some() {
                out.push(json!({"item": c["item"], "name": c["name"], "type": c["type"], "duration": secs(&c["duration"]), "bin": bin}));
            } else {
                let name = c["name"].as_str().unwrap_or("");
                walk(c, name, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&project["root"], "", &mut out);
    Value::Array(out)
}

pub struct VideoLabModule;

impl Module for VideoLabModule {
    fn register(&self, r: &mut Registry) {
        register_engine(r);
        register_doc(r);
        register_media(r);
        register_timeline(r);
        register_text(r);
        register_export(r);
    }
}

// ------------------------------------------------------------------------------------ engine

fn register_engine(r: &mut Registry) {
    r.area("engine", "FilmCraft's ~700 engine commands by id (the same ones its menus, CLI and MCP run): commands, describe, run, batch");

    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct List {
        filter: Option<String>,
        enabled_only: bool,
    }
    r.function("engine", "commands", "List engine commands (id, label, params), filtered")
        .description("{filter?: text matched against id and label, enabledOnly?: bool}. Returns [{id, label, params, enabled}]. Params use ticks (254016000000 per second); most commands also take seconds/frame/timecode.")
        .params(json!({"type": "object", "properties": {"filter": {"type": "string"}, "enabledOnly": {"type": "boolean"}}}))
        .typed(|ctx, p: List| {
            let all = run(ctx, "command.list", json!({}))?;
            let f = p.filter.unwrap_or_default().to_ascii_lowercase();
            Ok(Value::Array(
                all.as_array()
                    .map(|a| a.as_slice())
                    .unwrap_or(&[])
                    .iter()
                    .filter(|c| {
                        let id = c["id"].as_str().unwrap_or("").to_ascii_lowercase();
                        let label = c["label"].as_str().unwrap_or("").to_ascii_lowercase();
                        (f.is_empty() || id.contains(&f) || label.contains(&f)) && (!p.enabled_only || c["enabled"] == json!(true))
                    })
                    .map(|c| json!({"id": c["id"], "label": c["label"], "params": c["params"], "enabled": c["enabled"]}))
                    .collect(),
            ))
        })
        .register();

    #[derive(Deserialize)]
    struct Id {
        id: String,
    }
    r.function("engine", "describe", "One engine command in full: menu, shortcut, params, enabled now")
        .params(json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}))
        .typed(|ctx, p: Id| {
            let all = run(ctx, "command.list", json!({}))?;
            all.as_array().and_then(|a| a.iter().find(|c| c["id"] == p.id.as_str()).cloned()).ok_or_else(|| ApiError::not_found(format!("no engine command `{}`", p.id)))
        })
        .register();

    #[derive(Deserialize)]
    struct Run {
        id: String,
        #[serde(default)]
        params: Option<Value>,
    }
    r.function("engine", "run", "Run one engine command by id with its params (undoable edits land in history)")
        .description("{id, params?}. E.g. {id: \"timeline.razor\", params: {time: <ticks>}} or {id: \"effects.apply\", params: {clips: [12], effect: \"Gaussian Blur\"}}. Read-only commands (sequence.inspect, jobs.list) work too, but only lab_script may call this.")
        .params(json!({"type": "object", "properties": {"id": {"type": "string"}, "params": {"type": "object"}}, "required": ["id"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Run| run(ctx, &p.id, p.params.unwrap_or_else(|| json!({}))))
        .register();

    #[derive(Deserialize)]
    struct Batch {
        steps: Vec<Run>,
        #[serde(default = "yes")]
        stop_on_error: bool,
    }
    fn yes() -> bool {
        true
    }
    r.function("engine", "batch", "Run several engine commands in order; each edit is its own undo step")
        .params(json!({"type": "object", "properties": {"steps": {"type": "array", "items": {"type": "object"}}, "stop_on_error": {"type": "boolean"}}, "required": ["steps"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Batch| {
            let mut results = Vec::new();
            for s in p.steps {
                match run(ctx, &s.id, s.params.unwrap_or_else(|| json!({}))) {
                    Ok(v) => results.push(json!({"ok": true, "result": v})),
                    Err(e) => {
                        results.push(json!({"ok": false, "error": e.message}));
                        if p.stop_on_error {
                            break;
                        }
                    }
                }
            }
            Ok(json!({"results": results}))
        })
        .register();
}

// ------------------------------------------------------------------------------------ doc

fn register_doc(r: &mut Registry) {
    r.area("doc", "the project and its active sequence: timeline (compact), inspect, preview frames to look at, contact sheet, undo/redo, open/save/new");

    r.function("doc", "timeline", "The active sequence, compact: tracks and clips in seconds, transitions, markers, playhead")
        .description("Returns {sequence, name, width, height, fps, duration, playhead, selection, markers, tracks: [{id, name, kind: video|audio, clips: [{clip, item, name, start, end, sourceIn, sourceOut, speed, gainDb, effects, link}], transitions}]}. Times in seconds. `clip` ids go to timeline.* and engine commands.")
        .handler(|ctx, _| Ok(timeline_projection(&run(ctx, "sequence.inspect", json!({}))?)))
        .register();

    r.function("doc", "items", "Project items (media, sequences, graphics) with ids, types and durations")
        .handler(|ctx, _| Ok(items_projection(&run(ctx, "project.inspect", json!({}))?)))
        .register();

    r.function("doc", "inspect", "FilmCraft's full project + sequence JSON (ticks; large)")
        .handler(|ctx, _| Ok(json!({"project": run(ctx, "project.inspect", json!({}))?, "sequence": run(ctx, "sequence.inspect", json!({})).ok()})))
        .register();

    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Preview {
        seconds: Option<f64>,
        max_side: Option<u32>,
    }
    r.function("doc", "preview", "Render the program frame at `seconds` (default: playhead) as an image handle")
        .description("{seconds?, maxSide?: default 768}. Returns {image: \"img:N\", seconds, width, height, renderer: cpu|gpu}. Return {image} from the program to see it. Does not move the playhead.")
        .params(json!({"type": "object", "properties": {"seconds": {"type": "number"}, "maxSide": {"type": "integer", "minimum": 16, "maximum": 4096}}}))
        .typed(|ctx, p: Preview| {
            let e = engine(ctx)?;
            let t = p.seconds.map_or_else(|| e.lock().playhead(), Tick::from_seconds_f64);
            let (img, renderer) = e.render(t, p.max_side.unwrap_or(768)).map_err(ApiError::engine)?;
            let (w, h) = (img.width, img.height);
            Ok(json!({"image": put(ctx, img)?, "seconds": t.seconds(), "width": w, "height": h, "renderer": if renderer == crate::engine::Renderer::Gpu { "gpu" } else { "cpu" }}))
        })
        .register();

    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct Sheet {
        count: Option<usize>,
        seconds: Option<Vec<f64>>,
        max_side: Option<u32>,
    }
    r.function("doc", "contactSheet", "Several program frames at once (evenly spaced or at given seconds), as image handles")
        .description("{count?: 6 (evenly over the sequence), seconds?: [..], maxSide?: 320}. Returns {frames: [{image, seconds}]}. Up to 24 frames.")
        .params(json!({"type": "object", "properties": {"count": {"type": "integer", "minimum": 1, "maximum": 24}, "seconds": {"type": "array", "items": {"type": "number"}}, "maxSide": {"type": "integer"}}}))
        .typed(|ctx, p: Sheet| {
            let e = engine(ctx)?;
            let times: Vec<f64> = match p.seconds {
                Some(s) => s.into_iter().take(24).collect(),
                None => {
                    let dur = secs(&run(ctx, "sequence.inspect", json!({}))?["duration"]);
                    let n = p.count.unwrap_or(6).clamp(1, 24);
                    (0..n).map(|i| dur * (i as f64 + 0.5) / n as f64).collect()
                }
            };
            let mut frames = Vec::new();
            for s in times {
                ctx.check_cancel()?;
                let (img, _) = e.render(Tick::from_seconds_f64(s), p.max_side.unwrap_or(320).clamp(16, 1024)).map_err(ApiError::engine)?;
                frames.push(json!({"image": put(ctx, img)?, "seconds": s}));
            }
            Ok(json!({"frames": frames}))
        })
        .register();

    r.function("doc", "undo", "Undo the last edit").effects(Effects::Revertible).handler(|ctx, _| run(ctx, "edit.undo", json!({}))).register();
    r.function("doc", "redo", "Redo").effects(Effects::Revertible).handler(|ctx, _| run(ctx, "edit.redo", json!({}))).register();

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Path {
        path: Option<String>,
    }
    r.function("doc", "save", "Save the project (.fcproj) — to `path` beneath the workspace, or where it was opened from")
        .params(json!({"type": "object", "properties": {"path": {"type": "string", "description": "e.g. projects/trip.fcproj"}}}))
        .effects(Effects::Destructive)
        .typed(|ctx, p: Path| match p.path {
            Some(path) => run(ctx, "file.saveAs", json!({"path": path})),
            None => run(ctx, "file.save", json!({})),
        })
        .register();

    #[derive(Deserialize)]
    struct Open {
        path: String,
    }
    r.function("doc", "open", "Open a .fcproj project from the workspace (unsaved changes to the current one are lost)")
        .params(json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}))
        .effects(Effects::Destructive)
        .typed(|ctx, p: Open| run(ctx, "file.open", json!({"path": p.path})))
        .register();

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct New {
        name: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        fps: Option<f64>,
    }
    r.function("doc", "new", "A new empty project with one sequence (default 1080x1920 vertical, 30 fps)")
        .params(json!({"type": "object", "properties": {"name": {"type": "string"}, "width": {"type": "integer"}, "height": {"type": "integer"}, "fps": {"type": "number"}}}))
        .effects(Effects::Destructive)
        .typed(|ctx, p: New| {
            let name = p.name.unwrap_or_else(|| "Untitled".into());
            run(ctx, "file.newProject", json!({"name": name}))?;
            run(ctx, "file.newSequence", json!({"name": name, "width": p.width.unwrap_or(1080), "height": p.height.unwrap_or(1920), "fps": p.fps.unwrap_or(30.0), "video": 3, "audio": 3}))
        })
        .register();

    r.function("doc", "demo", "Open FilmCraft's built-in demo project (generated footage, music, transitions, markers)")
        .effects(Effects::Destructive)
        .handler(|ctx, _| run(ctx, "file.openDemoProject", json!({})))
        .register();
}

// ------------------------------------------------------------------------------------ media

fn register_media(r: &mut Registry) {
    r.area("media", "media in the workspace and the project: files, import, thumbnails (filmstrips), waveform peaks");

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Dir {
        dir: Option<String>,
    }
    r.function("media", "files", "Media files in a workspace folder (default `media/`), not yet necessarily imported")
        .params(json!({"type": "object", "properties": {"dir": {"type": "string"}}}))
        .typed(|ctx, p: Dir| {
            let dir = p.dir.unwrap_or_else(|| crate::MEDIA_DIR.into());
            let path = ctx.state::<Engine>()?.resolve(&dir).map_err(|e| ApiError::bad_params(e.to_string()))?;
            let mut out = Vec::new();
            for e in std::fs::read_dir(&path).map_err(|e| ApiError::not_found(format!("{dir}: {e}")))?.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let meta = e.metadata().ok();
                if meta.as_ref().is_some_and(std::fs::Metadata::is_file) && filmcraft_media::is_importable(std::path::Path::new(&name)) {
                    out.push(json!({"path": format!("{}/{name}", dir.trim_end_matches('/')), "bytes": meta.map_or(0, |m| m.len())}));
                }
            }
            Ok(Value::Array(out))
        })
        .register();

    #[derive(Deserialize)]
    struct Import {
        paths: Vec<String>,
    }
    r.function("media", "import", "Import workspace files into the project (MP4/MOV/MKV/WebM, WAV/MP3/AAC/FLAC, PNG/JPEG…)")
        .description("{paths: [\"media/a.mp4\"]}. Returns FilmCraft's import result with the new item ids.")
        .params(json!({"type": "object", "properties": {"paths": {"type": "array", "items": {"type": "string"}}}, "required": ["paths"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Import| {
            let e = engine(ctx)?;
            let abs: Vec<String> = p.paths.iter().map(|x| e.resolve(x).map(|p| p.to_string_lossy().into_owned())).collect::<Result<_, _>>().map_err(|e| ApiError::bad_params(e.to_string()))?;
            run(ctx, "file.import", json!({"paths": abs}))
        })
        .register();

    #[derive(Deserialize)]
    struct Thumb {
        item: u64,
        #[serde(default)]
        seconds: Option<Vec<f64>>,
        #[serde(default)]
        height: Option<u32>,
    }
    r.function("media", "thumbnails", "Frames of a media item at media times (a filmstrip), as image handles")
        .description("{item, seconds?: [..] (default: 6 evenly spaced), height?: 180}. Returns {frames: [{image, seconds}]}.")
        .params(json!({"type": "object", "properties": {"item": {"type": "integer"}, "seconds": {"type": "array", "items": {"type": "number"}}, "height": {"type": "integer"}}, "required": ["item"]}))
        .typed(|ctx, p: Thumb| {
            let times = match p.seconds {
                Some(s) => s,
                None => {
                    let items = items_projection(&run(ctx, "project.inspect", json!({}))?);
                    let dur = items.as_array().and_then(|a| a.iter().find(|i| i["item"] == json!(p.item))).and_then(|i| i["duration"].as_f64()).unwrap_or(0.0);
                    (0..6).map(|i| dur * (i as f64 + 0.5) / 6.0).collect()
                }
            };
            let ticks: Vec<Tick> = times.iter().map(|s| Tick::from_seconds_f64(*s)).collect();
            let imgs = engine(ctx)?.thumbnails(p.item, &ticks, p.height.unwrap_or(180)).map_err(ApiError::engine)?;
            let mut frames = Vec::new();
            for (img, s) in imgs.into_iter().zip(times) {
                frames.push(json!({"image": put(ctx, img)?, "seconds": s}));
            }
            Ok(json!({"frames": frames}))
        })
        .register();

    #[derive(Deserialize)]
    struct Peaks {
        item: u64,
        #[serde(default)]
        stream: usize,
        #[serde(default)]
        from: f64,
        to: f64,
        #[serde(default)]
        buckets: Option<usize>,
    }
    r.function("media", "peaks", "Waveform of a media item's audio over media seconds from..to: [[min,max], …]")
        .description("{item, from?: 0, to, buckets?: 200, stream?: 0}. Loud parts are where |min|,|max| approach 1; silence is ~0 — use it to find beats, pauses and peaks.")
        .params(json!({"type": "object", "properties": {"item": {"type": "integer"}, "from": {"type": "number"}, "to": {"type": "number"}, "buckets": {"type": "integer"}, "stream": {"type": "integer"}}, "required": ["item", "to"]}))
        .typed(|ctx, p: Peaks| {
            let v = engine(ctx)?
                .peaks(p.item, p.stream, Tick::from_seconds_f64(p.from), Tick::from_seconds_f64(p.to), p.buckets.unwrap_or(200))
                .map_err(ApiError::engine)?;
            Ok(json!(v.iter().map(|(a, b)| [(a * 1000.0).round() / 1000.0, (b * 1000.0).round() / 1000.0]).collect::<Vec<_>>()))
        })
        .register();
}

// ------------------------------------------------------------------------------------ timeline

fn register_timeline(r: &mut Registry) {
    r.area("timeline", "edits in seconds on the active sequence: append, place, split, trim, move, delete, speed, volume, fade, transition, select, seek");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Append {
        item: Value,
        #[serde(default)]
        source_in: Option<f64>,
        #[serde(default)]
        source_out: Option<f64>,
    }
    r.function("timeline", "append", "Add a media item (or items) at the end of the main track (V1/A1)")
        .description("{item: id | [ids], sourceIn?, sourceOut?: media seconds}. Returns the placed clips.")
        .params(json!({"type": "object", "properties": {"item": {}, "sourceIn": {"type": "number"}, "sourceOut": {"type": "number"}}, "required": ["item"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Append| {
            let seq = run(ctx, "sequence.inspect", json!({}))?;
            let end = seq["video"][0]["items"].as_array().and_then(|a| a.iter().filter_map(|c| c["end"].as_i64()).max()).unwrap_or(0);
            let mut params = json!({"track": "V1", "audioTrack": "A1", "time": end});
            if p.item.is_array() {
                params["items"] = p.item;
            } else {
                params["item"] = p.item;
            }
            if let Some(s) = p.source_in {
                params["sourceIn"] = json!(ticks(s));
            }
            if let (Some(a), Some(b)) = (p.source_in, p.source_out) {
                params["duration"] = json!(ticks((b - a).max(0.0)));
            }
            run(ctx, "timeline.place", params)
        })
        .register();

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Place {
        item: u64,
        seconds: f64,
        #[serde(default)]
        track: Option<String>,
        #[serde(default)]
        source_in: Option<f64>,
        #[serde(default)]
        duration: Option<f64>,
        #[serde(default)]
        insert: bool,
    }
    r.function("timeline", "place", "Place a media item at a time on a track (overwrite, or insert to ripple)")
        .description("{item, seconds, track?: \"V1\"|\"V2\"|\"A2\"|\"new\" (default V1 + its sound on A1), sourceIn?, duration?: seconds, insert?: false}")
        .params(json!({"type": "object", "properties": {"item": {"type": "integer"}, "seconds": {"type": "number"}, "track": {"type": "string"}, "sourceIn": {"type": "number"}, "duration": {"type": "number"}, "insert": {"type": "boolean"}}, "required": ["item", "seconds"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Place| {
            let mut params = json!({"item": p.item, "seconds": p.seconds, "insert": p.insert});
            if let Some(t) = p.track {
                params["track"] = json!(t);
            }
            if let Some(s) = p.source_in {
                params["sourceIn"] = json!(ticks(s));
            }
            if let Some(d) = p.duration {
                params["duration"] = json!(ticks(d));
            }
            run(ctx, "timeline.place", params)
        })
        .register();

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Split {
        seconds: Option<f64>,
        clip: Option<u64>,
        all: bool,
    }
    r.function("timeline", "split", "Split clips at `seconds` (default: playhead) — one clip, the targeted tracks, or all tracks")
        .params(json!({"type": "object", "properties": {"seconds": {"type": "number"}, "clip": {"type": "integer"}, "all": {"type": "boolean"}}}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Split| {
            let t = match p.seconds {
                Some(s) => ticks(s),
                None => engine(ctx)?.lock().playhead().0,
            };
            if let Some(c) = p.clip {
                run(ctx, "timeline.razor", json!({"time": t, "clip": c}))
            } else if p.all {
                run(ctx, "sequence.addEditAllTracks", json!({"time": t}))
            } else {
                run(ctx, "sequence.addEdit", json!({"time": t}))
            }
        })
        .register();

    #[derive(Deserialize)]
    struct Trim {
        clip: u64,
        edge: String,
        delta: f64,
        #[serde(default)]
        ripple: bool,
    }
    r.function("timeline", "trim", "Move a clip's in or out edge by `delta` seconds (+ later, − earlier); ripple shifts what follows")
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}, "edge": {"enum": ["in", "out"]}, "delta": {"type": "number"}, "ripple": {"type": "boolean"}}, "required": ["clip", "edge", "delta"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Trim| run(ctx, "timeline.trim", json!({"clip": p.clip, "edge": p.edge, "mode": if p.ripple { "ripple" } else { "regular" }, "delta": ticks(p.delta)})))
        .register();

    #[derive(Deserialize)]
    struct Move {
        clip: u64,
        seconds: f64,
        #[serde(default)]
        track: Option<String>,
    }
    r.function("timeline", "move", "Move a clip (and its linked sound) to start at `seconds`, optionally onto another track")
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}, "seconds": {"type": "number"}, "track": {"type": "string"}}, "required": ["clip", "seconds"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Move| {
            let mut m = json!({"clip": p.clip, "time": {"seconds": p.seconds}});
            if let Some(t) = p.track {
                m["track"] = json!(t);
            }
            run(ctx, "timeline.move", json!({"moves": [m]}))
        })
        .register();

    #[derive(Deserialize)]
    struct Delete {
        clips: Vec<u64>,
        #[serde(default)]
        ripple: bool,
    }
    r.function("timeline", "delete", "Delete clips; ripple closes the gap")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "ripple": {"type": "boolean"}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Delete| run(ctx, "timeline.delete", json!({"clips": p.clips, "ripple": p.ripple})))
        .register();

    #[derive(Deserialize)]
    struct Speed {
        clips: Vec<u64>,
        percent: f64,
        #[serde(default)]
        reverse: bool,
        #[serde(default = "yes")]
        ripple: bool,
    }
    fn yes() -> bool {
        true
    }
    r.function("timeline", "speed", "Change clip speed (100 = normal, 200 = 2× faster, 50 = slow motion), optionally reversed")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "percent": {"type": "number"}, "reverse": {"type": "boolean"}, "ripple": {"type": "boolean"}}, "required": ["clips", "percent"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Speed| run(ctx, "clip.speedDuration", json!({"clips": p.clips, "speed": p.percent, "reverse": p.reverse, "ripple": p.ripple, "maintainPitch": true})))
        .register();

    #[derive(Deserialize)]
    struct Volume {
        clips: Vec<u64>,
        db: f64,
        #[serde(default)]
        relative: bool,
    }
    r.function("timeline", "volume", "Set (or adjust, relative) the audio gain of clips in dB; -96 mutes")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "db": {"type": "number"}, "relative": {"type": "boolean"}}, "required": ["clips", "db"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Volume| run(ctx, "clip.audioGain", json!({"clips": p.clips, "mode": if p.relative { "adjust" } else { "set" }, "db": p.db})))
        .register();

    #[derive(Deserialize)]
    struct Fade {
        clip: u64,
        #[serde(default = "both")]
        edge: String,
        #[serde(default = "half")]
        seconds: f64,
    }
    fn both() -> String {
        "both".into()
    }
    fn half() -> f64 {
        0.5
    }
    r.function("timeline", "fade", "Fade a clip in and/or out: dip to black for picture, constant-power for sound")
        .description("{clip, edge?: in|out|both, seconds?: 0.5}. Applies to the clip's own kind (a video clip gets a dip to black; an audio clip a constant-power fade).")
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}, "edge": {"enum": ["in", "out", "both"]}, "seconds": {"type": "number"}}, "required": ["clip"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Fade| {
            let seq = run(ctx, "sequence.inspect", json!({}))?;
            let fps = seq["settings"]["frame_rate"]["num"].as_f64().unwrap_or(30.0) / seq["settings"]["frame_rate"]["den"].as_f64().unwrap_or(1.0).max(1.0);
            let frames = (p.seconds * fps).round().max(1.0) as i64;
            let is_audio = seq["audio"].as_array().is_some_and(|ts| ts.iter().any(|t| t["items"].as_array().is_some_and(|cs| cs.iter().any(|c| c["clip"] == json!(p.clip)))));
            let (cmd, effect) = if is_audio { ("sequence.applyAudioTransition", "constant_power") } else { ("sequence.applyVideoTransition", "dip_to_black") };
            let edges: &[&str] = match p.edge.as_str() {
                "in" => &["in"],
                "out" => &["out"],
                _ => &["in", "out"],
            };
            let mut out = Vec::new();
            for e in edges {
                out.push(run(ctx, cmd, json!({"clip": p.clip, "effect": effect, "frames": frames, "edge": e, "align": if *e == "in" { "start" } else { "end" }}))?);
            }
            Ok(json!(out))
        })
        .register();

    #[derive(Deserialize)]
    struct Transition {
        clip: u64,
        #[serde(default)]
        effect: Option<String>,
        #[serde(default)]
        edge: Option<String>,
        #[serde(default)]
        seconds: Option<f64>,
    }
    r.function("timeline", "transition", "Put a video transition on a clip's edge (cross_dissolve, dip_to_black, push, wipe…; see engine.run effects.list)")
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}, "effect": {"type": "string"}, "edge": {"enum": ["in", "out"]}, "seconds": {"type": "number"}}, "required": ["clip"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Transition| {
            let mut params = json!({"clip": p.clip, "effect": p.effect.unwrap_or_else(|| "cross_dissolve".into()), "edge": p.edge.unwrap_or_else(|| "out".into())});
            if let Some(s) = p.seconds {
                let seq = run(ctx, "sequence.inspect", json!({}))?;
                let fps = seq["settings"]["frame_rate"]["num"].as_f64().unwrap_or(30.0) / seq["settings"]["frame_rate"]["den"].as_f64().unwrap_or(1.0).max(1.0);
                params["frames"] = json!((s * fps).round().max(1.0) as i64);
            }
            run(ctx, "sequence.applyVideoTransition", params)
        })
        .register();

    #[derive(Deserialize)]
    struct Select {
        clips: Vec<u64>,
    }
    r.function("timeline", "select", "Select clips (the person sees the selection; [] clears it)")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Select| run(ctx, "timeline.select", json!({"clips": p.clips})))
        .register();

    #[derive(Deserialize)]
    struct Seek {
        seconds: f64,
    }
    r.function("timeline", "seek", "Move the playhead to `seconds` (the person's preview follows)")
        .params(json!({"type": "object", "properties": {"seconds": {"type": "number"}}, "required": ["seconds"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Seek| run(ctx, "playhead.set", json!({"seconds": p.seconds.max(0.0)})))
        .register();
}

// ------------------------------------------------------------------------------------ text

fn register_text(r: &mut Registry) {
    r.area("text", "titles, captions and motion-graphics templates: add text, style it, captions, list/apply templates");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AddText {
        text: String,
        #[serde(default)]
        seconds: Option<f64>,
        #[serde(default)]
        duration: Option<f64>,
        #[serde(default)]
        style: Option<Value>,
    }
    r.function("text", "add", "Add a title clip above the picture at `seconds` (default playhead)")
        .description("{text, seconds?, duration?: 3, style?: {font, size, fill_color: \"#ffcc00\", stroke, stroke_width, background, shadow, align, position: [x,y]}}. Returns the new graphic clip.")
        .params(json!({"type": "object", "properties": {"text": {"type": "string"}, "seconds": {"type": "number"}, "duration": {"type": "number"}, "style": {"type": "object"}}, "required": ["text"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: AddText| {
            let mut params = json!({"text": p.text, "seconds": p.duration.unwrap_or(3.0), "newClip": true});
            if let Some(s) = p.seconds {
                params["time"] = json!(ticks(s));
            }
            let made = run(ctx, "graphics.newText", params)?;
            if let Some(style) = p.style {
                let clip = made.get("clip").cloned().unwrap_or(Value::Null);
                let mut sp = json!({"props": style});
                if !clip.is_null() {
                    sp["clip"] = clip;
                }
                run(ctx, "graphics.set", sp)?;
            }
            Ok(made)
        })
        .register();

    #[derive(Deserialize)]
    struct Caption {
        text: String,
        seconds: f64,
        #[serde(default)]
        duration: Option<f64>,
    }
    r.function("text", "caption", "Add a subtitle caption at `seconds` (creates the caption track if needed)")
        .params(json!({"type": "object", "properties": {"text": {"type": "string"}, "seconds": {"type": "number"}, "duration": {"type": "number"}}, "required": ["text", "seconds"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Caption| {
            let first = run(ctx, "captions.add", json!({"text": p.text, "seconds": p.seconds, "durationSeconds": p.duration.unwrap_or(3.0)}));
            match first {
                Ok(v) => Ok(v),
                Err(_) => {
                    run(ctx, "captions.newTrack", json!({"format": "Subtitle"}))?;
                    run(ctx, "captions.add", json!({"text": p.text, "seconds": p.seconds, "durationSeconds": p.duration.unwrap_or(3.0)}))
                }
            }
        })
        .register();

    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Templates {
        query: Option<String>,
        category: Option<String>,
    }
    r.function("text", "templates", "Motion-graphics templates (titles, lower thirds, social) by query/category")
        .params(json!({"type": "object", "properties": {"query": {"type": "string"}, "category": {"type": "string"}}}))
        .typed(|ctx, p: Templates| {
            let mut params = json!({});
            if let Some(q) = p.query {
                params["query"] = json!(q);
            }
            if let Some(c) = p.category {
                params["category"] = json!(c);
            }
            run(ctx, "graphics.template.list", params)
        })
        .register();

    #[derive(Deserialize)]
    struct Apply {
        template: Value,
        #[serde(default)]
        values: Option<Value>,
        #[serde(default)]
        seconds: Option<f64>,
    }
    r.function("text", "applyTemplate", "Put a template on the timeline at `seconds` (default playhead), with control values (texts, colours)")
        .params(json!({"type": "object", "properties": {"template": {}, "values": {"type": "object"}, "seconds": {"type": "number"}}, "required": ["template"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Apply| {
            let mut params = json!({"template": p.template});
            if let Some(v) = p.values {
                params["values"] = v;
            }
            if let Some(s) = p.seconds {
                params["time"] = json!(ticks(s));
            }
            run(ctx, "graphics.template.apply", params)
        })
        .register();
}

// ------------------------------------------------------------------------------------ export

fn register_export(r: &mut Registry) {
    r.area("export", "render the sequence to a file in the background: start, jobs (progress), cancel");

    #[derive(Deserialize)]
    struct Start {
        path: String,
        #[serde(default)]
        format: Option<String>,
        #[serde(default)]
        preset: Option<String>,
        #[serde(default)]
        range: Option<String>,
        #[serde(default)]
        scale: Option<f64>,
    }
    r.function("export", "start", "Export the active sequence to `path` (e.g. exports/cut.mp4); returns at once with a job id")
        .description("{path, format?: h264|hevc|prores|gif|png|wav|aac|mp3… (default from the extension), preset?: an export preset name, range?: entire|inOut|workArea, scale?: 0.5}. Poll export.jobs.")
        .params(json!({"type": "object", "properties": {"path": {"type": "string"}, "format": {"type": "string"}, "preset": {"type": "string"}, "range": {"type": "string"}, "scale": {"type": "number"}}, "required": ["path"]}))
        .effects(Effects::Destructive)
        .typed(|ctx, p: Start| {
            let abs = engine(ctx)?.resolve(&p.path).map_err(|e| ApiError::bad_params(e.to_string()))?;
            if let Some(dir) = abs.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let mut params = json!({"path": abs.to_string_lossy(), "wait": false, "range": p.range.unwrap_or_else(|| "entire".into())});
            if let Some(f) = p.format {
                params["format"] = json!(f);
            }
            if let Some(pr) = p.preset {
                params["preset"] = json!(pr);
            }
            if let Some(s) = p.scale {
                params["scale"] = json!(s);
            }
            run(ctx, "file.exportMedia", params)
        })
        .register();

    r.function("export", "jobs", "Background jobs (exports) with progress, ETA and results").handler(|ctx, _| run(ctx, "jobs.list", json!({}))).register();

    #[derive(Deserialize)]
    struct Cancel {
        job: u64,
    }
    r.function("export", "cancel", "Cancel a background job")
        .params(json!({"type": "object", "properties": {"job": {"type": "integer"}}, "required": ["job"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Cancel| run(ctx, "jobs.cancel", json!({"job": p.job})))
        .register();
}
