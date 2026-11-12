//! The features phone editors are known for, as registry functions over FilmCraft's engine
//! commands: filters and adjustments (FilmCraft's colour grading), a curated effects shelf,
//! keyframed transform, freeze frame, reverse, smooth slow motion, mirror / rotate, auto cut
//! (scene detection), auto-ducking and loudness, text to speech, and auto captions / pause and
//! filler removal (FilmCraft's Whisper transcription).
//!
//! The app's buttons call these; so can the in-app agent, `/mcp` clients and the CLI.

use serde::Deserialize;
use serde_json::{Value, json};

use lab_api::{ApiError, ApiResult, Ctx, Effects, Registry};

use crate::areas::{engine, run, secs, ticks};

pub fn register(r: &mut Registry) {
    register_look(r);
    register_clip(r);
    register_audio(r);
    register_speech(r);
}

// ----------------------------------------------------------------------------------- helpers

/// A clip's entry in `sequence.inspect` (video or audio track), with its effects in order.
fn clip_json(ctx: &Ctx, clip: u64) -> ApiResult<Value> {
    let seq = run(ctx, "sequence.inspect", json!({}))?;
    for kind in ["video", "audio"] {
        for t in seq[kind].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            for c in t["items"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                if c["clip"] == json!(clip) {
                    return Ok(c.clone());
                }
            }
        }
    }
    Err(ApiError::not_found(format!("no clip {clip} in the open sequence")))
}

fn effect_ids(clip: &Value) -> Vec<String> {
    clip["effects"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().map(|e| e["effect"].as_str().unwrap_or("").to_string()).collect()
}

/// A parameter value as `sequence.inspect` prints it (`Float(12.0)`, `Bool(true)`) as JSON.
fn param_value(e: &Value, param: &str) -> Value {
    let raw = e["params"][param]["value"].as_str().unwrap_or("");
    if let Some(f) = raw.strip_prefix("Float(").and_then(|r| r.strip_suffix(')')).and_then(|v| v.parse::<f64>().ok()) {
        return json!(f);
    }
    if let Some(b) = raw.strip_prefix("Bool(").and_then(|r| r.strip_suffix(')')) {
        return json!(b == "true");
    }
    if let Some(v) = raw.strip_prefix("Vec2(Vec2 { x: ").and_then(|r| r.strip_suffix(" })")) {
        let xy: Vec<f64> = v.split(", y: ").filter_map(|n| n.parse().ok()).collect();
        if xy.len() == 2 {
            return json!(xy);
        }
    }
    Value::Null
}

/// Remove the effects at `indices` from `clip` (highest first, so the others keep their index).
fn remove_indices(ctx: &Ctx, clip: u64, mut indices: Vec<usize>) -> ApiResult<usize> {
    indices.sort_unstable_by(|a, b| b.cmp(a));
    for i in &indices {
        run(ctx, "effects.remove", json!({"clip": clip, "index": i}))?;
    }
    Ok(indices.len())
}

/// Colour-grading instances (`lumetri`) on a clip, split into the Adjust one (creative section
/// off: only basic correction) and the filters (presets turn the creative section on).
fn lumetri_split(clip: &Value) -> (Option<usize>, Vec<usize>) {
    let mut adjust = None;
    let mut filters = Vec::new();
    for (i, e) in clip["effects"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().enumerate() {
        if e["effect"] != "lumetri" {
            continue;
        }
        if param_value(e, "creative_on") == json!(false) && adjust.is_none() {
            adjust = Some(i);
        } else {
            filters.push(i);
        }
    }
    (adjust, filters)
}

fn playhead_or(ctx: &Ctx, seconds: Option<f64>) -> ApiResult<f64> {
    match seconds {
        Some(s) => Ok(s),
        None => Ok(secs(&run(ctx, "sequence.inspect", json!({}))?["playhead"])),
    }
}

/// Keyframe times (timeline seconds) of each clip's transform (Motion, Opacity) in the active
/// sequence, for the timeline's diamonds: `{clip id: [seconds…]}`. FilmCraft's commands report
/// counts only; the times come from the project itself.
pub fn transform_keyframes(engine: &crate::engine::Engine) -> serde_json::Map<String, Value> {
    let s = engine.lock();
    let mut out = serde_json::Map::new();
    let Some(seq) = s.state.active_sequence.and_then(|id| s.project.sequence(id)) else { return out };
    for t in &seq.video_tracks {
        for item in &t.items {
            // Keyframes sit at media time (what the clip shows), as FilmCraft's keyframe commands
            // report: on the timeline that is start + (media − source in) / speed.
            let speed = item.speed.abs().max(1e-6);
            let at = |media: filmcraft_time::Tick| item.start.0 + ((media.0 - item.source_in.0) as f64 / speed).round() as i64;
            let mut times: Vec<i64> = item
                .effects
                .iter()
                .filter(|e| e.effect == "motion" || e.effect == "opacity")
                .flat_map(|e| e.params.values())
                .flat_map(|p| p.keyframes.iter().map(|k| at(k.time)))
                .filter(|t| *t >= item.start.0 && *t <= item.end().0)
                .collect();
            if times.is_empty() {
                continue;
            }
            times.sort_unstable();
            times.dedup();
            out.insert(item.id.0.to_string(), json!(times.iter().map(|t| filmcraft_time::Tick(*t).seconds()).collect::<Vec<_>>()));
        }
    }
    out
}

/// Add each clip's transform keyframe times (`keyframes`: timeline seconds) to a timeline
/// projection ([`crate::areas::timeline_projection`]).
pub fn add_keyframes(engine: &crate::engine::Engine, timeline: &mut Value) {
    let times = transform_keyframes(engine);
    if times.is_empty() {
        return;
    }
    for t in timeline["tracks"].as_array_mut().map(Vec::as_mut_slice).unwrap_or(&mut []) {
        for c in t["clips"].as_array_mut().map(Vec::as_mut_slice).unwrap_or(&mut []) {
            if let Some(k) = c["clip"].as_u64().and_then(|id| times.get(&id.to_string())) {
                c["keyframes"] = k.clone();
            }
        }
    }
}

/// The (media time, degrees) keyframes of a clip's Motion rotation; empty when it is not animated.
fn rotation_keyframes(engine: &crate::engine::Engine, clip: u64) -> Vec<(i64, f64)> {
    let s = engine.lock();
    let Some(seq) = s.state.active_sequence.and_then(|id| s.project.sequence(id)) else { return Vec::new() };
    seq.video_tracks
        .iter()
        .flat_map(|t| t.items.iter())
        .find(|i| i.id.0 == clip)
        .and_then(|i| i.effects.iter().find(|e| e.effect == "motion"))
        .and_then(|e| e.params.get("rotation"))
        .map(|p| p.keyframes.iter().filter_map(|k| k.value.as_f64().map(|v| (k.time.0, v))).collect())
        .unwrap_or_default()
}

/// A picture clip's Motion / Opacity values at timeline second `t`, keyframes evaluated (FilmCraft's
/// `Param::value_at`, at the media time the clip shows then).
fn transform_at(engine: &crate::engine::Engine, clip: u64, t: f64) -> Option<Value> {
    let s = engine.lock();
    let seq = s.state.active_sequence.and_then(|id| s.project.sequence(id))?;
    let item = seq.video_tracks.iter().flat_map(|tr| tr.items.iter()).find(|i| i.id.0 == clip)?;
    let at = filmcraft_time::Tick::from_seconds_f64(t).clamp(item.start, item.end());
    let media = filmcraft_time::Tick(item.source_in.0 + ((at.0 - item.start.0) as f64 * item.speed.abs()).round() as i64);
    let param = |effect: &str, name: &str| item.effects.iter().find(|e| e.effect == effect).and_then(|e| e.params.get(name));
    let num = |effect: &str, name: &str, default: f64| param(effect, name).map_or(default, |p| p.f64_at(media));
    let position = param("motion", "position").map(|p| p.vec2_at(media)).filter(|v| v.x.is_finite() && v.y.is_finite()).map(|v| json!([v.x, v.y]));
    let animated = ["position", "scale", "rotation"].iter().any(|n| param("motion", n).is_some_and(|p| p.is_animated())) || param("opacity", "opacity").is_some_and(|p| p.is_animated());
    Some(json!({
        "position": position.unwrap_or(Value::Null), "scale": num("motion", "scale", 100.0),
        "rotation": num("motion", "rotation", 0.0), "opacity": num("opacity", "opacity", 100.0),
        "animated": animated, "seconds": t,
    }))
}

// -------------------------------------------------------------------------------------- look

/// The effects shelf: a name, FilmCraft's effect and the parameters that make it read at a
/// glance (several of FilmCraft's effects default to doing nothing, e.g. a blur of 0).
const EFFECTS: &[(&str, &str, &str, &str)] = &[
    ("blur", "Blur", "gaussian_blur", r#"{"blurriness": 40, "repeat_edge": true}"#),
    ("glitch", "Glitch", "rgb_split", r#"{"amount": 28}"#),
    ("shake", "Shake", "camera_shake", r#"{}"#),
    ("light_leaks", "Light leaks", "light_leaks", r#"{}"#),
    ("glow", "Glow", "wonder_glow", r#"{}"#),
    ("neon", "Neon edges", "edge_glow", r#"{}"#),
    ("dreamy", "Dreamy", "bokeh_blur", r#"{"amount": 12}"#),
    ("zoom_in", "Zoom in", "grow", r#"{}"#),
    ("zoom_out", "Zoom out", "shrink", r#"{}"#),
    ("spin", "Spin", "spin", r#"{}"#),
    ("wiggle", "Wiggle", "wiggle", r#"{}"#),
    ("vignette", "Vignette", "vignette", r#"{"amount": -60}"#),
    ("grain", "Film grain", "noise", r#"{"amount": 18, "color": false}"#),
    ("mosaic", "Mosaic", "mosaic", r#"{"horizontal": 48, "vertical": 85}"#),
    ("stop_motion", "Stop motion", "posterize_time", r#"{"rate": 8}"#),
    ("black_white", "B&W", "black_white", r#"{}"#),
    ("mirror", "Mirror", "mirror", r#"{}"#),
    ("split", "Split screen", "replicate", r#"{"count": 2}"#),
    ("flare", "Lens flare", "lens_flare", r#"{}"#),
    ("twirl", "Twirl", "twirl", r#"{}"#),
];

/// Adjust sliders → FilmCraft colour-grading parameters (`lumetri`) and their ranges.
const ADJUST: &[(&str, &str, f64, f64)] = &[
    ("exposure", "exposure", -5.0, 5.0),
    ("contrast", "contrast", -100.0, 100.0),
    ("saturation", "saturation", 0.0, 200.0),
    ("temperature", "temperature", -100.0, 100.0),
    ("tint", "tint", -100.0, 100.0),
    ("highlights", "highlights", -100.0, 100.0),
    ("shadows", "shadows", -100.0, 100.0),
    ("vibrance", "vibrance", -100.0, 100.0),
    ("sharpen", "sharpen", -100.0, 100.0),
    ("fade", "faded_film", 0.0, 100.0),
    ("vignette", "vignette_amount", -100.0, 100.0),
];

fn register_look(r: &mut Registry) {
    r.area("look", "filters (FilmCraft's colour-grading presets), adjustments (exposure, contrast, saturation, warmth…) and the effects shelf (blur, glitch, shake, glow, zoom…)");

    r.function("look", "filters", "The filter presets: name, folder, description")
        .effects(Effects::ReadOnly)
        .handler(|ctx, _| {
            let p = run(ctx, "lumetri.presets", json!({}))?;
            Ok(json!(p["presets"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().map(|x| json!({"name": x["name"], "folder": x["folder"], "description": x["description"]})).collect::<Vec<_>>()))
        })
        .register();

    #[derive(Deserialize)]
    struct Thumbs {
        #[serde(default)]
        width: Option<u32>,
    }
    r.function("look", "filterThumbnails", "Every filter on FilmCraft's sample frame, as one PNG grid (path, cell size, columns, gap, preset order)")
        .effects(Effects::ReadOnly)
        .typed(|ctx, p: Thumbs| {
            let path = engine(ctx)?.resolve(".videolab/filter-thumbnails.png").map_err(|err| ApiError::internal(err.to_string()))?;
            let columns = 6u32;
            let made = run(ctx, "lumetri.presetThumbnails", json!({"width": p.width.unwrap_or(160).clamp(48, 400), "columns": columns, "path": path}))?;
            let cell_w = made["cell"][0].as_u64().unwrap_or(160);
            let width = made["width"].as_u64().unwrap_or(0);
            let gap = width.saturating_sub(cell_w * u64::from(columns)) / u64::from(columns + 1);
            Ok(json!({"path": path, "cell": made["cell"], "columns": columns, "gap": gap, "presets": made["presets"]}))
        })
        .register();

    #[derive(Deserialize)]
    struct Filter {
        clips: Vec<u64>,
        #[serde(default)]
        name: Option<String>,
    }
    r.function("look", "filter", "Put a filter on clips, replacing their filter (name null or \"\": remove it). Adjustments are kept")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "name": {"type": ["string", "null"]}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Filter| {
            for &clip in &p.clips {
                let (_, filters) = lumetri_split(&clip_json(ctx, clip)?);
                remove_indices(ctx, clip, filters)?;
            }
            match p.name.filter(|n| !n.is_empty() && n != "none") {
                Some(name) => run(ctx, "lumetri.applyPreset", json!({"name": name, "clips": p.clips})),
                None => Ok(json!({"clips": p.clips.len(), "preset": null})),
            }
        })
        .register();

    #[derive(Deserialize)]
    struct ClipOnly {
        clip: u64,
    }
    r.function("look", "state", "A clip's look: whether it has a filter, its adjustment values, and its shelf effects")
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}}, "required": ["clip"]}))
        .effects(Effects::ReadOnly)
        .typed(|ctx, p: ClipOnly| {
            let c = clip_json(ctx, p.clip)?;
            let (adjust, filters) = lumetri_split(&c);
            let fx = &c["effects"];
            let mut values = serde_json::Map::new();
            for (key, param, _, _) in ADJUST {
                let v = adjust.map(|i| param_value(&fx[i], param)).filter(|v| !v.is_null());
                values.insert((*key).into(), v.unwrap_or_else(|| json!(if *param == "saturation" { 100.0 } else { 0.0 })));
            }
            let ids = effect_ids(&c);
            let shelf: Vec<&str> = ids.iter().filter_map(|i| EFFECTS.iter().find(|(_, _, e, _)| e == i)).map(|(k, _, _, _)| *k).collect();
            Ok(json!({"filter": !filters.is_empty(), "adjust": values, "effects": shelf}))
        })
        .register();

    #[derive(Deserialize)]
    struct Adjust {
        clip: u64,
        values: serde_json::Map<String, Value>,
    }
    let keys: Vec<&str> = ADJUST.iter().map(|a| a.0).collect();
    r.function("look", "adjust", "Adjust a clip's picture: exposure, contrast, saturation (100 = unchanged), temperature, tint, highlights, shadows, vibrance, sharpen, fade, vignette")
        .description(&format!("{{clip, values: {{{}}}}}. Values outside a slider's range are clamped. Kept apart from the filter.", keys.join(", ")))
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}, "values": {"type": "object"}}, "required": ["clip", "values"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Adjust| {
            let index = match lumetri_split(&clip_json(ctx, p.clip)?).0 {
                Some(i) => i,
                None => {
                    let made = run(ctx, "effects.apply", json!({"clips": [p.clip], "effect": "lumetri"}))?;
                    let i = made["applied"][0]["index"].as_u64().ok_or_else(|| ApiError::engine("colour grading was not added"))? as usize;
                    run(ctx, "effects.setParam", json!({"clip": p.clip, "effect": i, "param": "creative_on", "value": false}))?;
                    i
                }
            };
            let mut set = serde_json::Map::new();
            for (k, v) in &p.values {
                let Some((_, param, lo, hi)) = ADJUST.iter().find(|a| a.0 == k) else {
                    return Err(ApiError::bad_params(format!("unknown adjustment `{k}`; one of {}", ADJUST.iter().map(|a| a.0).collect::<Vec<_>>().join(", "))));
                };
                let v = v.as_f64().ok_or_else(|| ApiError::bad_params(format!("`{k}` must be a number")))?.clamp(*lo, *hi);
                run(ctx, "effects.setParam", json!({"clip": p.clip, "effect": index, "param": param, "value": v}))?;
                set.insert(k.clone(), json!(v));
            }
            Ok(json!({"clip": p.clip, "set": set}))
        })
        .register();

    r.function("look", "effects", "The effects shelf: key, name, FilmCraft effect")
        .effects(Effects::ReadOnly)
        .handler(|_, _| Ok(json!(EFFECTS.iter().map(|(k, n, e, _)| json!({"key": k, "name": n, "effect": e})).collect::<Vec<_>>())))
        .register();

    #[derive(Deserialize)]
    struct Effect {
        clips: Vec<u64>,
        key: String,
        #[serde(default)]
        on: Option<bool>,
    }
    r.function("look", "effect", "Turn a shelf effect on clips on or off (`on` omitted: toggle)")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "key": {"type": "string"}, "on": {"type": "boolean"}}, "required": ["clips", "key"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Effect| {
            let Some((_, _, effect, preset)) = EFFECTS.iter().find(|e| e.0 == p.key) else {
                return Err(ApiError::bad_params(format!("unknown effect `{}`; see look.effects", p.key)));
            };
            let preset: serde_json::Map<String, Value> = serde_json::from_str(preset).unwrap_or_default();
            let mut out = Vec::new();
            for &clip in &p.clips {
                let ids = effect_ids(&clip_json(ctx, clip)?);
                let at: Vec<usize> = ids.iter().enumerate().filter(|(_, i)| i == effect).map(|(n, _)| n).collect();
                let on = p.on.unwrap_or(at.is_empty());
                if !on {
                    remove_indices(ctx, clip, at)?;
                    out.push(json!({"clip": clip, "on": false}));
                    continue;
                }
                if at.is_empty() {
                    let made = run(ctx, "effects.apply", json!({"clips": [clip], "effect": effect}))?;
                    let index = made["applied"][0]["index"].clone();
                    for (param, value) in &preset {
                        run(ctx, "effects.setParam", json!({"clip": clip, "effect": index, "param": param, "value": value}))?;
                    }
                }
                out.push(json!({"clip": clip, "on": true}));
            }
            Ok(json!(out))
        })
        .register();
}

// -------------------------------------------------------------------------------------- clip

fn register_clip(r: &mut Registry) {
    r.area("clip", "clip tools: transform with keyframes, fit / fill the frame, freeze frame, reverse, smooth slow motion, mirror, rotate, auto cut");

    #[derive(Deserialize)]
    struct ClipAt {
        clip: u64,
        #[serde(default)]
        seconds: Option<f64>,
    }
    r.function("clip", "transform", "A clip's transform at `seconds` (default playhead): position [x, y] (sequence pixels), scale %, rotation °, opacity %, whether it is animated, and its keyframe times")
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}, "seconds": {"type": "number"}}, "required": ["clip"]}))
        .effects(Effects::ReadOnly)
        .typed(|ctx, p: ClipAt| {
            let t = playhead_or(ctx, p.seconds)?;
            let e = engine(ctx)?;
            let mut out = transform_at(e, p.clip, t).ok_or_else(|| ApiError::not_found(format!("no picture clip {} in the open sequence", p.clip)))?;
            out["keyframes"] = transform_keyframes(e).remove(&p.clip.to_string()).unwrap_or_else(|| json!([]));
            Ok(out)
        })
        .register();

    #[derive(Deserialize)]
    struct SetTransform {
        clip: u64,
        #[serde(default)]
        position: Option<[f64; 2]>,
        #[serde(default)]
        scale: Option<f64>,
        #[serde(default)]
        rotation: Option<f64>,
        #[serde(default)]
        opacity: Option<f64>,
        #[serde(default)]
        seconds: Option<f64>,
    }
    r.function("clip", "setTransform", "Move, scale, rotate or fade a clip's picture. On an animated clip this sets the keyframe at `seconds` (default playhead)")
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}, "position": {"type": "array", "items": {"type": "number"}}, "scale": {"type": "number"}, "rotation": {"type": "number"}, "opacity": {"type": "number"}, "seconds": {"type": "number"}}, "required": ["clip"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: SetTransform| {
            let t = playhead_or(ctx, p.seconds)?;
            let set = |effect: &str, param: &str, value: Value| run(ctx, "effects.setParam", json!({"clip": p.clip, "effect": effect, "param": param, "value": value, "time": ticks(t)}));
            if let Some(v) = p.position {
                set("motion", "position", json!(v))?;
            }
            if let Some(v) = p.scale {
                set("motion", "scale", json!(v.clamp(1.0, 1000.0)))?;
            }
            if let Some(v) = p.rotation {
                set("motion", "rotation", json!(v))?;
            }
            if let Some(v) = p.opacity {
                set("opacity", "opacity", json!(v.clamp(0.0, 100.0)))?;
            }
            Ok(json!({"clip": p.clip}))
        })
        .register();

    r.function("clip", "keyframe", "Add (or remove, when there is one) a transform keyframe at `seconds` (default playhead): position, scale, rotation and opacity together")
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}, "seconds": {"type": "number"}}, "required": ["clip"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: ClipAt| {
            let t = ticks(playhead_or(ctx, p.seconds)?);
            let params = [("motion", "position"), ("motion", "scale"), ("motion", "rotation"), ("opacity", "opacity")];
            // Each toggle flips one parameter; the first decides, the rest follow it (re-toggled
            // when they started in the other state).
            let toggle = |(effect, param): (&str, &str)| -> ApiResult<bool> {
                let r = run(ctx, "effects.addKeyframe", json!({"clip": p.clip, "effect": effect, "param": param, "time": t}))?;
                Ok(r["added"].as_bool().unwrap_or(false))
            };
            let added = toggle(params[0])?;
            for &pp in &params[1..] {
                if toggle(pp)? != added {
                    toggle(pp)?;
                }
            }
            Ok(json!({"clip": p.clip, "added": added}))
        })
        .register();

    #[derive(Deserialize)]
    struct Fit {
        clips: Vec<u64>,
        #[serde(default)]
        mode: Option<String>,
    }
    r.function("clip", "fit", "Size clips' picture to the frame: `fit` (all of it shows, default) or `fill` (covers the frame, cropping)")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "mode": {"enum": ["fit", "fill"]}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Fit| {
            let cmd = if p.mode.as_deref() == Some("fill") { "clip.fillFrame" } else { "clip.fitToFrame" };
            run(ctx, cmd, json!({"clips": p.clips}))
        })
        .register();

    #[derive(Deserialize)]
    struct Freeze {
        clip: u64,
        #[serde(default)]
        seconds: Option<f64>,
        #[serde(default)]
        duration: Option<f64>,
    }
    r.function("clip", "freeze", "Freeze frame: insert a still of the frame at `seconds` (default playhead) for `duration` seconds (default 2), pushing the rest later")
        .params(json!({"type": "object", "properties": {"clip": {"type": "integer"}, "seconds": {"type": "number"}, "duration": {"type": "number"}}, "required": ["clip"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Freeze| {
            let t = playhead_or(ctx, p.seconds)?;
            run(ctx, "clip.insertFrameHoldSegment", json!({"clip": p.clip, "time": ticks(t), "seconds": p.duration.unwrap_or(2.0).clamp(0.1, 60.0)}))
        })
        .register();

    #[derive(Deserialize)]
    struct Clips {
        clips: Vec<u64>,
    }
    r.function("clip", "reverse", "Play clips backwards (again: forwards), keeping their speed")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Clips| {
            let mut out = Vec::new();
            for &clip in &p.clips {
                let c = clip_json(ctx, clip)?;
                let speed = c["speed"].as_f64().unwrap_or(1.0).abs() * 100.0;
                let reverse = !c["reverse"].as_bool().unwrap_or(false);
                run(ctx, "clip.speedDuration", json!({"clips": [clip], "speed": speed, "reverse": reverse, "ripple": true, "maintainPitch": true}))?;
                out.push(json!({"clip": clip, "reverse": reverse}));
            }
            Ok(json!(out))
        })
        .register();

    #[derive(Deserialize)]
    struct Slowmo {
        clips: Vec<u64>,
        #[serde(default)]
        percent: Option<f64>,
    }
    r.function("clip", "smoothSlowmo", "Slow clips down (default 50%) with optical-flow frame interpolation, for smooth slow motion")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "percent": {"type": "number"}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Slowmo| {
            run(ctx, "clip.speedDuration", json!({"clips": p.clips, "speed": p.percent.unwrap_or(50.0).clamp(5.0, 100.0), "reverse": false, "ripple": true, "maintainPitch": true, "interpolation": "opticalFlow"}))
        })
        .register();

    r.function("clip", "mirror", "Flip clips left-right (again: back)")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Clips| {
            let mut out = Vec::new();
            for &clip in &p.clips {
                let at: Vec<usize> = effect_ids(&clip_json(ctx, clip)?).iter().enumerate().filter(|(_, e)| *e == "horizontal_flip").map(|(i, _)| i).collect();
                let on = at.is_empty();
                if on {
                    run(ctx, "effects.apply", json!({"clips": [clip], "effect": "horizontal_flip"}))?;
                } else {
                    remove_indices(ctx, clip, at)?;
                }
                out.push(json!({"clip": clip, "mirrored": on}));
            }
            Ok(json!(out))
        })
        .register();

    #[derive(Deserialize)]
    struct Rotate {
        clips: Vec<u64>,
        #[serde(default)]
        degrees: Option<f64>,
    }
    r.function("clip", "rotate", "Rotate clips' picture by `degrees` (default 90, clockwise)")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "degrees": {"type": "number"}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Rotate| {
            let mut out = Vec::new();
            let by = p.degrees.unwrap_or(90.0);
            for &clip in &p.clips {
                // An animated rotation turns as a whole: every keyframe moves by `by` (setting
                // the parameter would add a keyframe at the playhead instead).
                let keys = rotation_keyframes(engine(ctx)?, clip);
                if !keys.is_empty() {
                    for (media, value) in &keys {
                        run(ctx, "effects.setKeyframe", json!({"clip": clip, "effect": "motion", "param": "rotation", "mediaTime": media, "value": value + by}))?;
                    }
                    out.push(json!({"clip": clip, "rotatedKeyframes": keys.len()}));
                    continue;
                }
                let c = clip_json(ctx, clip)?;
                let motion = c["effects"].as_array().and_then(|fx| fx.iter().find(|e| e["effect"] == "motion")).cloned().unwrap_or(Value::Null);
                let now = param_value(&motion, "rotation").as_f64().unwrap_or(0.0);
                let to = (now + by).rem_euclid(360.0);
                run(ctx, "effects.setParam", json!({"clip": clip, "effect": "motion", "param": "rotation", "value": to}))?;
                out.push(json!({"clip": clip, "rotation": to}));
            }
            Ok(json!(out))
        })
        .register();

    #[derive(Deserialize)]
    struct AutoCut {
        clips: Vec<u64>,
        #[serde(default)]
        sensitivity: Option<f64>,
    }
    r.function("clip", "autoCut", "Auto cut: split clips at every scene change FilmCraft detects (sensitivity 0–100, default 50)")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "sensitivity": {"type": "number"}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: AutoCut| {
            run(ctx, "clip.sceneEditDetection", json!({"clips": p.clips, "sensitivity": p.sensitivity.unwrap_or(50.0).clamp(0.0, 100.0), "applyCuts": true, "wait": true}))
        })
        .register();
}

// ------------------------------------------------------------------------------------- audio

fn register_audio(r: &mut Registry) {
    r.area("audio", "sound tools: auto-duck music under speech, even out loudness, text to speech (narration)");

    #[derive(Deserialize)]
    struct Duck {
        music: Vec<u64>,
    }
    r.function("audio", "duck", "Auto-duck: lower these music clips wherever the other sound clips have speech (FilmCraft's Essential Sound ducking)")
        .params(json!({"type": "object", "properties": {"music": {"type": "array", "items": {"type": "integer"}}}, "required": ["music"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Duck| {
            // Ducking listens to dialogue clips: every other sound clip without a type is speech.
            let seq = run(ctx, "sequence.inspect", json!({}))?;
            let all: Vec<u64> = seq["audio"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().flat_map(|t| t["items"].as_array().cloned().unwrap_or_default()).filter_map(|c| c["clip"].as_u64()).collect();
            let others: Vec<u64> = all.into_iter().filter(|c| !p.music.contains(c)).collect();
            let typed = run(ctx, "essentialSound.inspect", json!({"clips": others}))?;
            let untyped: Vec<u64> = typed["clips"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().filter(|c| c["type"].is_null()).filter_map(|c| c["clip"].as_u64()).collect();
            if !untyped.is_empty() {
                run(ctx, "essentialSound.setType", json!({"clips": untyped, "type": "dialogue"}))?;
            }
            run(ctx, "essentialSound.setType", json!({"clips": p.music, "type": "music"}))?;
            run(ctx, "essentialSound.generateDucking", json!({"clips": p.music}))
        })
        .register();

    #[derive(Deserialize)]
    struct Loudness {
        clips: Vec<u64>,
        #[serde(default)]
        lufs: Option<f64>,
    }
    r.function("audio", "normalize", "Even out loudness: set clips' gain so each measures `lufs` (default -14, what phone apps target)")
        .params(json!({"type": "object", "properties": {"clips": {"type": "array", "items": {"type": "integer"}}, "lufs": {"type": "number"}}, "required": ["clips"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Loudness| run(ctx, "essentialSound.autoMatch", json!({"clips": p.clips, "target": p.lufs.unwrap_or(-14.0).clamp(-40.0, -5.0)})))
        .register();

    r.function("audio", "voices", "Narration voices installed on this device: id, name, gender")
        .effects(Effects::ReadOnly)
        .handler(|ctx, _| {
            let v = run(ctx, "tts.voices", json!({}))?;
            Ok(json!(v["voices"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().filter(|x| x["installed"] == true).map(|x| json!({"id": x["id"], "name": x["name"], "gender": x["gender"]})).collect::<Vec<_>>()))
        })
        .register();

    #[derive(Deserialize)]
    struct Narrate {
        text: String,
        #[serde(default)]
        voice: Option<String>,
        #[serde(default)]
        seconds: Option<f64>,
        #[serde(default)]
        pace: Option<f64>,
    }
    r.function("audio", "narrate", "Text to speech: read `text` aloud as a new sound clip at `seconds` (default playhead)")
        .params(json!({"type": "object", "properties": {"text": {"type": "string"}, "voice": {"type": "string"}, "seconds": {"type": "number"}, "pace": {"type": "number"}}, "required": ["text"]}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Narrate| {
            if p.text.trim().is_empty() {
                return Err(ApiError::bad_params("nothing to say"));
            }
            let t = playhead_or(ctx, p.seconds)?;
            // An absolute folder: FilmCraft resolves a relative one against the process's
            // working directory, which only matches the workspace while one core runs.
            let dir = engine(ctx)?.resolve("media/narration").map_err(|e| ApiError::internal(e.to_string()))?;
            std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
            let mut params = json!({"text": p.text, "time": ticks(t), "dir": dir, "pace": p.pace.unwrap_or(1.0).clamp(0.5, 2.0)});
            if let Some(v) = p.voice {
                params["voice"] = json!(v);
            }
            run(ctx, "tts.create", params)
        })
        .register();
}

// ------------------------------------------------------------------------------------ speech

fn register_speech(r: &mut Registry) {
    r.area("speech", "speech recognition (Whisper, on the device): auto captions, remove pauses, remove filler words; speech models");

    r.function("speech", "models", "Speech models: id, name, size, installed")
        .effects(Effects::ReadOnly)
        .handler(|ctx, _| {
            let m = run(ctx, "transcript.models", json!({}))?;
            Ok(json!(m["models"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().map(|x| json!({"id": x["id"], "name": x["name"], "size": x["size"], "installed": x["installed"], "description": x["description"]})).collect::<Vec<_>>()))
        })
        .register();

    #[derive(Deserialize)]
    struct Model {
        #[serde(default)]
        model: Option<String>,
    }
    r.function("speech", "modelFiles", "Where a speech model's files come from and go (url, size, sha256, path), for the app's downloader")
        .params(json!({"type": "object", "properties": {"model": {"type": "string"}}}))
        .effects(Effects::ReadOnly)
        .typed(|_, p: Model| {
            let id = p.model.unwrap_or_else(|| "whisper-tiny".into());
            let m = filmcraft_speech::models::find(&id).ok_or_else(|| ApiError::not_found(format!("no speech model `{id}`")))?;
            let dir = filmcraft_engine::transcript::models_dir().ok_or_else(|| ApiError::unavailable("FilmCraft has no data folder here"))?;
            let at = filmcraft_speech::models::model_dir(&dir, m);
            Ok(json!({
                "model": m.id, "installed": filmcraft_speech::models::installed(&dir, m),
                "files": m.files.iter().map(|f| json!({"name": f.name, "url": f.url, "size": f.size, "sha256": f.sha256, "path": at.join(f.name)})).collect::<Vec<_>>(),
            }))
        })
        .register();

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Captions {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        max_chars: Option<u32>,
        #[serde(default)]
        lines: Option<u32>,
    }
    r.function("speech", "captions", "Auto captions: transcribe the sequence's sound with Whisper and add a captions track")
        .description("{model?: installed model (whisper-tiny first), language?: \"auto\" | \"en\" | …, maxChars?: 32, lines?: 1}. Transcription runs on the device; a minute of speech takes seconds to tens of seconds.")
        .params(json!({"type": "object", "properties": {"model": {"type": "string"}, "language": {"type": "string"}, "maxChars": {"type": "integer"}, "lines": {"type": "integer"}}}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Captions| {
            transcribe(ctx, p.model, p.language)?;
            run(ctx, "transcript.createCaptions", json!({"maxChars": p.max_chars.unwrap_or(32).clamp(8, 80), "lines": p.lines.unwrap_or(1).clamp(1, 2)}))
        })
        .register();

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Pauses {
        #[serde(default)]
        min_seconds: Option<f64>,
        #[serde(default)]
        keep_seconds: Option<f64>,
    }
    r.function("speech", "removePauses", "Cut out silent pauses in speech longer than `minSeconds` (default 0.8), keeping `keepSeconds` (default 0.2)")
        .params(json!({"type": "object", "properties": {"minSeconds": {"type": "number"}, "keepSeconds": {"type": "number"}}}))
        .effects(Effects::Revertible)
        .typed(|ctx, p: Pauses| {
            transcribe(ctx, None, None)?;
            run(ctx, "transcript.removePauses", json!({"minSeconds": p.min_seconds.unwrap_or(0.8), "keepSeconds": p.keep_seconds.unwrap_or(0.2)}))
        })
        .register();

    r.function("speech", "removeFillers", "Cut out filler words (um, uh, …) from speech")
        .effects(Effects::Revertible)
        .handler(|ctx, _| {
            transcribe(ctx, None, None)?;
            run(ctx, "transcript.removeFillers", json!({}))
        })
        .register();
}

/// Transcribe the sequence's mix with an installed model (the smallest unless one is named).
fn transcribe(ctx: &Ctx, model: Option<String>, language: Option<String>) -> ApiResult<Value> {
    let models = run(ctx, "transcript.models", json!({}))?;
    let installed: Vec<String> = models["models"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().filter(|m| m["installed"] == true).filter_map(|m| m["id"].as_str().map(str::to_string)).collect();
    let model = match model {
        Some(m) if installed.contains(&m) => m,
        Some(m) => return Err(ApiError::unavailable(format!("speech model `{m}` is not downloaded (speech.modelFiles)"))),
        None => installed.first().cloned().ok_or_else(|| ApiError::unavailable("no speech model is downloaded yet (speech.modelFiles; the app downloads whisper-tiny, 150 MB)"))?,
    };
    let r = run(ctx, "sequence.transcribe", json!({"track": "mix", "language": language.unwrap_or_else(|| "auto".into()), "model": model}))?;
    // Whisper "hears" a few words in music or room tone ("Thank you.", "Thanks for watching!").
    // FilmCraft's sequence transcript lets the first transcribed track claim its time, so such a
    // transcript on a video's own sound would hide real speech on a voice-over track: drop them.
    let noise = not_speech(engine(ctx)?);
    if !noise.is_empty() {
        run(ctx, "transcript.delete", json!({"items": noise}))?;
    }
    let words = crate::areas::run(ctx, "transcript.inspect", json!({})).map(|i| i["words"].as_array().map_or(0, Vec::len)).unwrap_or(0);
    if words == 0 {
        return Err(ApiError::engine("no speech was recognized in the sequence's sound"));
    }
    Ok(json!({"transcribed": r, "droppedNonSpeech": noise}))
}

/// Phrases Whisper is known to produce from non-speech audio.
const HALLUCINATIONS: &[&str] = &["thank you", "thank you for watching", "thanks for watching", "thank you very much", "you", "bye", "subscribe", "please subscribe", "music", "applause", "so"];

/// Media items of the active sequence's sound whose transcript is not speech: far too few words
/// for the time they cover (speech runs ~150 words a minute; under 20 is noise), or only one of
/// Whisper's stock hallucinations.
fn not_speech(engine: &crate::engine::Engine) -> Vec<u64> {
    let s = engine.lock();
    let Some(seq) = s.state.active_sequence.and_then(|id| s.project.sequence(id)) else { return Vec::new() };
    let mut seconds: std::collections::BTreeMap<u64, f64> = Default::default();
    for it in seq.audio_tracks.iter().flat_map(|t| t.items.iter()) {
        *seconds.entry(it.item.0).or_default() += it.duration.seconds();
    }
    let mut out = Vec::new();
    for (item, tr) in &s.project.transcripts {
        let Some(&secs) = seconds.get(&item.0) else { continue };
        let n = tr.words.len();
        let text: String = tr.words.iter().map(|w| w.text.to_lowercase()).collect::<Vec<_>>().join(" ").chars().filter(|c| c.is_alphanumeric() || *c == ' ').collect();
        let per_minute = n as f64 / (secs / 60.0).max(1e-3);
        if n == 0 || (secs >= 4.0 && per_minute < 20.0) || (n <= 6 && HALLUCINATIONS.contains(&text.trim())) {
            out.push(item.0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_noise_is_not_speech() {
        let dir = tempfile::tempdir().unwrap();
        let lab = crate::Lab::new(dir.path(), &crate::LabOptions::default()).unwrap();
        lab.call("doc.demo", json!({}), lab_api::Source::Ui).unwrap();
        let e = &lab.engine;
        let t = lab.timeline();
        let audio: Vec<u64> = t["tracks"].as_array().unwrap().iter().filter(|x| x["kind"] == "audio").flat_map(|x| x["clips"].as_array().unwrap().iter().filter_map(|c| c["item"].as_u64()).collect::<Vec<_>>()).collect();
        let (noise, speech) = (audio[0], audio[audio.len() - 1]);
        let words = |text: &str, every: f64| -> Value {
            json!(text.split(' ').enumerate().map(|(i, w)| json!({"text": w, "start": ticks(i as f64 * every), "end": ticks(i as f64 * every + every * 0.8)})).collect::<Vec<_>>())
        };
        let set = |item: u64, w: Value| e.execute("transcript.set", json!({"item": item, "transcript": {"language": "en", "speakers": [{"name": "A"}], "words": w}})).unwrap();
        set(noise, words("Thank you for watching!", 1.0));
        // ~150 words a minute over the score's length: real speech.
        let long: Vec<&str> = std::iter::repeat_n("word", 60).collect();
        set(speech, words(&long.join(" "), 0.4));
        let dropped = not_speech(e);
        assert!(dropped.contains(&noise), "stock hallucination dropped: {dropped:?}");
        assert!(!dropped.contains(&speech), "speech kept: {dropped:?}");
    }
}
