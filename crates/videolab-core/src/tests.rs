use super::*;

fn lab() -> (tempfile::TempDir, Lab) {
    let dir = tempfile::tempdir().unwrap();
    let lab = Lab::new(dir.path(), &LabOptions::default()).unwrap();
    (dir, lab)
}

fn demo(lab: &Lab) {
    lab.call("doc.demo", json!({}), Source::Ui).unwrap();
}

#[test]
fn sanitize_and_unique_names() {
    assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
    assert_eq!(sanitize_file_name("C:\\clips\\a:b?.mp4"), "a_b_.mp4");
    assert_eq!(sanitize_file_name("..."), "media");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.mp4"), b"").unwrap();
    assert_eq!(unique_name(dir.path(), "a.mp4").unwrap(), "a (2).mp4");
}

#[test]
fn demo_timeline_preview_thumbnails_peaks_mix() {
    let (_d, lab) = lab();
    assert!(lab.timeline().is_null() || lab.timeline()["tracks"].as_array().is_some());
    demo(&lab);
    let t = lab.timeline();
    assert_eq!(t["name"], "Main Edit", "{t}");
    let tracks = t["tracks"].as_array().unwrap();
    assert!(tracks.iter().any(|x| x["kind"] == "video" && !x["clips"].as_array().unwrap().is_empty()));
    let (img, r) = lab.render(Some(2.0), 320).unwrap();
    assert_eq!(r, Renderer::Cpu);
    assert_eq!(img.width, 320);
    assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
    assert!(img.rgba.chunks(4).any(|p| p[0] > 10 || p[1] > 10 || p[2] > 10), "frame is black");
    // Item 5 is Ocean_Sunset.mp4 and item 11 the score (see doc.items).
    let items = lab.items();
    let video = items.as_array().unwrap().iter().find(|i| i["type"] == "Movie").unwrap()["item"].as_u64().unwrap();
    let audio = items.as_array().unwrap().iter().find(|i| i["type"] == "Audio").unwrap()["item"].as_u64().unwrap();
    let thumbs = lab.thumbnails(video, &[0.5, 1.5, 2.5], 64).unwrap();
    assert_eq!(thumbs.len(), 3);
    assert_eq!(thumbs[0].height, 64);
    let peaks = lab.peaks(audio, 0.0, 10.0, 50).unwrap();
    assert_eq!(peaks.len(), 50);
    assert!(peaks.iter().any(|(lo, hi)| hi - lo > 0.01), "score is silent? {peaks:?}");
    let (sr, mix) = lab.mix(1.0, 4800).unwrap();
    assert_eq!(sr, 48_000);
    assert_eq!(mix.len(), 9600);
}

#[test]
fn registry_edits_undo_and_events() {
    let (_d, lab) = lab();
    let events = Arc::new(lab_api::CollectEvents::default());
    lab.set_event_sink(events.clone());
    demo(&lab);
    let before = lab.timeline();
    let v1 = before["tracks"][0]["clips"].as_array().unwrap().len();
    lab.call("timeline.split", json!({"seconds": 1.0, "all": true}), Source::Agent).unwrap();
    let after = lab.timeline();
    assert_eq!(after["tracks"][0]["clips"].as_array().unwrap().len(), v1 + 1);
    lab.call("doc.undo", json!({}), Source::Agent).unwrap();
    assert_eq!(lab.timeline()["tracks"][0]["clips"].as_array().unwrap().len(), v1);
    let sources: Vec<Source> = events
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            lab_api::Event::DocChanged { source, .. } => Some(*source),
            _ => None,
        })
        .collect();
    assert!(sources.contains(&Source::Ui) && sources.contains(&Source::Agent), "{sources:?}");
    // Read-only realms refuse edits.
    let o = lab.run_script("return await lab.timeline.split({seconds: 2})", true, Duration::ZERO);
    assert_eq!(o.error.as_ref().map(|e| e.code.as_str()), Some("read_only"), "{:?}", o.error);
    let o = lab.run_script("const t = await lab.doc.timeline(); const p = await lab.doc.preview({seconds: 1, maxSide: 96}); return {n: t.tracks.length, p}", true, Duration::ZERO);
    assert_eq!(o.error, None);
    assert_eq!(o.images.len(), 1);
}

#[test]
fn lab_mcp_in_process() {
    let (_d, lab) = lab();
    demo(&lab);
    let tools = lab.lab_mcp_request("tools/list", json!({})).unwrap();
    let names: Vec<&str> = tools["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, ["lab_read", "lab_script", "lab_help"]);
    let r = lab
        .lab_mcp_request("tools/call", json!({"name": "lab_read", "arguments": {"code": "return await lab.doc.contactSheet({count: 2, maxSide: 64})", "description": "look"}}))
        .unwrap();
    let content = r["content"].as_array().unwrap();
    assert_eq!(content.len(), 3, "{r}");
    assert_eq!(content[1]["type"], "image");
}

#[test]
fn filmcraft_mcp_bridges_to_the_same_session() {
    let (_d, lab) = lab();
    let tools = lab.filmcraft_mcp_request("tools/list", json!({})).unwrap();
    let names: Vec<&str> = tools["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in ["command_list", "command_run", "doc_inspect", "render_frame", "ui_screenshot"] {
        assert!(names.contains(&expected), "{expected} missing from {names:?}");
    }
    // FilmCraft's own server opens the demo; the app's session sees it.
    let r = lab.filmcraft_mcp_request("tools/call", json!({"name": "command_run", "arguments": {"id": "file.openDemoProject"}})).unwrap();
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(lab.timeline()["name"], "Main Edit");
    // render_frame in bridge mode: seek, screenshot the Program monitor, seek back.
    let r = lab.filmcraft_mcp_request("tools/call", json!({"name": "render_frame", "arguments": {"seconds": 3.0, "max_side": 200}})).unwrap();
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(r["content"][0]["type"], "image", "{r}");
    // A desktop-only tool explains itself.
    let r = lab.filmcraft_mcp_request("tools/call", json!({"name": "ui_click", "arguments": {"id": "tools.Razor"}})).unwrap();
    assert_eq!(r["isError"], json!(true));
}

#[test]
fn http_endpoints() {
    let (_d, lab) = lab();
    let port = lab.start_http(0, "0123456789abcdef0123", false).unwrap();
    assert_eq!(lab.http_port(), Some(port));
    lab.stop_http();
    assert_eq!(lab.http_port(), None);
    assert!(lab.start_http(0, "short", true).is_err());
}

#[test]
fn import_bytes_lands_in_media() {
    let (dir, lab) = lab();
    // A tiny WAV: 0.1 s of silence, 8 kHz mono 16-bit.
    let n = 800u32;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + n * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&8000u32.to_le_bytes());
    wav.extend_from_slice(&16000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(n * 2).to_le_bytes());
    wav.extend(std::iter::repeat_n(0u8, (n * 2) as usize));
    let r = lab.import_bytes("../beep.wav", &wav).unwrap();
    assert_eq!(r["path"], "media/beep.wav", "{r}");
    assert!(dir.path().join("media/beep.wav").is_file());
    assert!(lab.items().as_array().unwrap().iter().any(|i| i["name"] == "beep.wav"), "{}", lab.items());
    let files = lab.call("media.files", json!({}), Source::Agent).unwrap();
    assert_eq!(files[0]["path"], "media/beep.wav");
}

#[test]
fn export_runs_in_the_background() {
    let (dir, lab) = lab();
    demo(&lab);
    lab.execute("sequence.setWorkArea", json!({"start": 0, "end": filmcraft_time::Tick::from_seconds_f64(1.0).0})).unwrap();
    let started = lab.call("export.start", json!({"path": "exports/clip.mp4", "range": "workArea", "scale": 0.25}), Source::Agent).unwrap();
    let job = started["job"].as_u64().unwrap_or_else(|| panic!("{started}"));
    let t0 = std::time::Instant::now();
    loop {
        let jobs = lab.call("export.jobs", json!({}), Source::Agent).unwrap();
        let j = jobs.as_array().unwrap().iter().find(|j| j["id"] == json!(job)).cloned().unwrap();
        if j["finished"] == json!(true) {
            assert!(j["result"].get("error").is_none(), "{j}");
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(120), "export did not finish: {j}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let len = std::fs::metadata(dir.path().join("exports/clip.mp4")).unwrap().len();
    assert!(len > 1000, "{len}");
}

#[test]
fn command_lines_split_like_a_shell() {
    assert_eq!(split_command_line("filmcraft-cli exec effects.apply '{\"effect\":\"Gaussian Blur\"}'").unwrap(), ["exec", "effects.apply", "{\"effect\":\"Gaussian Blur\"}"]);
    assert_eq!(split_command_line("commands  razor").unwrap(), ["commands", "razor"]);
    assert!(split_command_line("exec 'x").is_err());
}

#[test]
fn gpu_option_renders_either_way() {
    let dir = tempfile::tempdir().unwrap();
    let lab = Lab::new(dir.path(), &LabOptions { gpu: true, ..Default::default() }).unwrap();
    demo(&lab);
    let (img, r) = lab.render(Some(2.0), 480).unwrap();
    assert_eq!(img.width, 480);
    eprintln!("renderer: {r:?}, gpu status {:?}", lab.gpu_status());
}

/// The first video clip of the demo and the last sound clip (the score).
fn demo_clips(lab: &Lab) -> (u64, u64, f64) {
    let t = lab.timeline();
    let tracks = t["tracks"].as_array().unwrap();
    let v = &tracks.iter().find(|x| x["kind"] == "video").unwrap()["clips"][0];
    let score = tracks.iter().filter(|x| x["kind"] == "audio").flat_map(|x| x["clips"].as_array().unwrap().iter()).last().unwrap();
    (v["clip"].as_u64().unwrap(), score["clip"].as_u64().unwrap(), v["start"].as_f64().unwrap())
}

#[test]
fn filters_and_adjustments_stay_apart() {
    let (_d, lab) = lab();
    demo(&lab);
    let (clip, _, _) = demo_clips(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap();
    call("look.adjust", json!({"clip": clip, "values": {"temperature": 40, "saturation": 140}}));
    call("look.filter", json!({"clips": [clip], "name": "Golden Dusk"}));
    call("look.filter", json!({"clips": [clip], "name": "Neutral Mono"}));
    let s = call("look.state", json!({"clip": clip}));
    assert_eq!(s["filter"], true, "{s}");
    assert_eq!(s["adjust"]["temperature"], 40.0, "a filter keeps the adjustments: {s}");
    let lumetri = |lab: &Lab| lab.engine.execute("sequence.inspect", json!({})).unwrap()["video"][0]["items"][0]["effects"].as_array().unwrap().iter().filter(|e| e["effect"] == "lumetri").count();
    assert_eq!(lumetri(&lab), 2, "one adjust + one filter (the old filter replaced)");
    call("look.filter", json!({"clips": [clip], "name": null}));
    let s = call("look.state", json!({"clip": clip}));
    assert_eq!((s["filter"].clone(), s["adjust"]["saturation"].clone()), (json!(false), json!(140.0)), "{s}");
    assert!(lab.call("look.adjust", json!({"clip": clip, "values": {"loudness": 1}}), Source::Agent).is_err());
}

#[test]
fn effects_shelf_toggles() {
    let (_d, lab) = lab();
    demo(&lab);
    let (clip, _, _) = demo_clips(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap();
    call("look.effect", json!({"clips": [clip], "key": "glitch"}));
    call("look.effect", json!({"clips": [clip], "key": "blur"}));
    assert_eq!(call("look.state", json!({"clip": clip}))["effects"], json!(["glitch", "blur"]));
    call("look.effect", json!({"clips": [clip], "key": "glitch"}));
    assert_eq!(call("look.state", json!({"clip": clip}))["effects"], json!(["blur"]));
    // The blur's strength comes from the shelf, not FilmCraft's default of 0.
    let fx = lab.engine.execute("sequence.inspect", json!({})).unwrap()["video"][0]["items"][0]["effects"].clone();
    let blur = fx.as_array().unwrap().iter().find(|e| e["effect"] == "gaussian_blur").unwrap();
    assert_eq!(blur["params"]["blurriness"]["value"], "Float(40.0)");
    assert!(lab.call("look.effect", json!({"clips": [clip], "key": "nope"}), Source::Agent).is_err());
}

#[test]
fn keyframes_land_where_asked_and_toggle() {
    let (_d, lab) = lab();
    demo(&lab);
    let (clip, _, start) = demo_clips(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap();
    assert_eq!(call("clip.keyframe", json!({"clip": clip, "seconds": start + 1.0}))["added"], true);
    call("clip.setTransform", json!({"clip": clip, "seconds": start + 3.0, "scale": 150}));
    let times = |lab: &Lab| lab.timeline()["tracks"].as_array().unwrap().iter().flat_map(|t| t["clips"].as_array().unwrap().clone()).find(|c| c["clip"] == json!(clip)).unwrap()["keyframes"].clone();
    let k = times(&lab);
    let k: Vec<f64> = k.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(k.len(), 2, "{k:?}");
    assert!((k[0] - (start + 1.0)).abs() < 0.05 && (k[1] - (start + 3.0)).abs() < 0.05, "timeline seconds: {k:?}");
    let at = |s: f64| call("clip.transform", json!({"clip": clip, "seconds": s}))["scale"].as_f64().unwrap();
    assert!((at(start + 3.0) - 150.0).abs() < 0.5, "the keyframed value at its time: {}", at(start + 3.0));
    assert!(at(start + 2.0) > 100.5 && at(start + 2.0) < 149.5, "in between, interpolated: {}", at(start + 2.0));
    // Rotating an animated clip turns every keyframe, adding none.
    let n = times(&lab).as_array().unwrap().len();
    call("clip.rotate", json!({"clips": [clip]}));
    assert_eq!(times(&lab).as_array().unwrap().len(), n, "no new keyframe");
    assert!((call("clip.transform", json!({"clip": clip, "seconds": start + 1.0}))["rotation"].as_f64().unwrap() - 90.0).abs() < 0.01);
    assert_eq!(call("clip.keyframe", json!({"clip": clip, "seconds": start + 1.0}))["added"], false);
    assert_eq!(times(&lab).as_array().unwrap().len(), 1);
}

#[test]
fn clip_tools_and_sound() {
    let (_d, lab) = lab();
    demo(&lab);
    let (clip, score, start) = demo_clips(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap();
    assert_eq!(call("clip.mirror", json!({"clips": [clip]}))[0]["mirrored"], true);
    assert_eq!(call("clip.mirror", json!({"clips": [clip]}))[0]["mirrored"], false);
    assert_eq!(call("clip.rotate", json!({"clips": [clip]}))[0]["rotation"], 90.0);
    call("clip.fit", json!({"clips": [clip], "mode": "fill"}));
    call("clip.fit", json!({"clips": [clip]}));
    assert_eq!(call("clip.reverse", json!({"clips": [clip]}))[0]["reverse"], true);
    let before = lab.timeline()["duration"].as_f64().unwrap();
    call("clip.freeze", json!({"clip": clip, "seconds": start + 1.0, "duration": 2.0}));
    let after = lab.timeline()["duration"].as_f64().unwrap();
    assert!((after - before - 2.0).abs() < 0.1, "freeze frame adds 2 s: {before} -> {after}");
    let duck = call("audio.duck", json!({"music": [score]}));
    assert!(!duck["clips"][0]["keyframes"].as_array().unwrap().is_empty(), "{duck}");
    let n = call("audio.narrate", json!({"text": "Hello there", "seconds": 1.0}));
    assert!(n["path"].as_str().unwrap().contains("media/narration/"), "{n}");
    assert!(lab.call("audio.narrate", json!({"text": "  "}), Source::Agent).is_err());
}

#[test]
fn move_onto_a_new_track_and_back() {
    let (_d, lab) = lab();
    demo(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap();
    let video = |lab: &Lab| lab.timeline()["tracks"].as_array().unwrap().iter().filter(|t| t["kind"] == "video").cloned().collect::<Vec<_>>();
    let before = video(&lab).len();
    let clip = video(&lab)[0]["clips"][1]["clip"].as_u64().unwrap();
    call("timeline.move", json!({"clip": clip, "seconds": 1.0, "track": "new"}));
    let after = video(&lab);
    assert_eq!(after.len(), before + 1, "a new video track");
    let top = after.last().unwrap();
    assert_eq!(top["clips"][0]["clip"], json!(clip), "the clip is on it: {top}");
    assert!((top["clips"][0]["start"].as_f64().unwrap() - 1.0).abs() < 0.05);
    // Back onto V1 by the track's id.
    let v1 = after[0]["id"].clone();
    call("timeline.move", json!({"clip": clip, "seconds": 30.0, "track": v1}));
    assert!(video(&lab)[0]["clips"].as_array().unwrap().iter().any(|c| c["clip"] == json!(clip)));
    // Only in time (no track): it stays on its own track.
    call("timeline.move", json!({"clip": clip, "seconds": 40.0}));
    let c = video(&lab)[0]["clips"].as_array().unwrap().iter().find(|c| c["clip"] == json!(clip)).cloned().unwrap();
    assert!((c["start"].as_f64().unwrap() - 40.0).abs() < 0.05, "{c}");
}

#[test]
fn compose_tools_round_trip() {
    let (_d, lab) = lab();
    demo(&lab);
    let (clip, score, start) = demo_clips(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap_or_else(|e| panic!("{f}: {e}"));
    let st = |lab: &Lab| lab.call("compose.state", json!({"clip": clip}), Source::Agent).unwrap();
    let plain = lab.engine.render(filmcraft_time::Tick::from_seconds_f64(start + 1.0), 320).unwrap().0.rgba;
    // crop
    call("compose.crop", json!({"clip": clip, "left": 10, "bottom": 20}));
    assert_eq!(st(&lab)["crop"]["left"], 10.0, "{}", st(&lab));
    assert_ne!(lab.engine.render(filmcraft_time::Tick::from_seconds_f64(start + 1.0), 320).unwrap().0.rgba, plain, "crop changes the picture");
    call("compose.crop", json!({"clip": clip}));
    assert!(st(&lab)["crop"].is_null());
    // blend and opacity
    call("compose.blend", json!({"clips": [clip], "mode": "screen", "opacity": 70}));
    assert_eq!((st(&lab)["blend"].clone(), st(&lab)["opacity"].clone()), (json!("Screen"), json!(70.0)));
    assert!(lab.call("compose.blend", json!({"clips": [clip], "mode": "sparkle"}), Source::Agent).is_err());
    // chroma key
    call("compose.chromaKey", json!({"clip": clip, "strength": 70}));
    assert_eq!(st(&lab)["chromaKey"]["color"], "#00b140", "{}", st(&lab));
    call("compose.chromaKey", json!({"clip": clip, "color": "#0000ff"}));
    assert_eq!(st(&lab)["chromaKey"]["color"], "#0000ff");
    assert_eq!(st(&lab)["chromaKey"]["strength"], 70.0);
    call("compose.chromaKey", json!({"clip": clip, "on": false}));
    assert!(st(&lab)["chromaKey"].is_null());
    // mask: one at a time
    call("compose.mask", json!({"clip": clip, "shape": "circle", "feather": 0}));
    call("compose.mask", json!({"clip": clip, "shape": "rounded", "inverted": true}));
    let m = st(&lab)["mask"].clone();
    assert_eq!((m["shape"].clone(), m["inverted"].clone()), (json!("rounded"), json!(true)), "{m}");
    assert_eq!(lab.engine.execute("masks.list", json!({"clip": clip})).unwrap()["masks"].as_array().unwrap().len(), 1);
    call("compose.mask", json!({"clip": clip, "shape": "none"}));
    assert!(st(&lab)["mask"].is_null());
    // stabilize / reframe toggles
    call("compose.stabilize", json!({"clips": [clip]}));
    assert!(!st(&lab)["stabilize"].is_null());
    call("compose.stabilize", json!({"clips": [clip]}));
    assert!(st(&lab)["stabilize"].is_null());
    assert_eq!(call("compose.autoReframe", json!({"clips": [clip]}))[0]["on"], true);
    assert_eq!(st(&lab)["autoReframe"], true);
    // voice effects replace each other; enhance toggles two effects
    call("compose.voice", json!({"clips": [score], "key": "chipmunk"}));
    call("compose.voice", json!({"clips": [score], "key": "hall"}));
    let fx = |lab: &Lab| lab.engine.execute("sequence.inspect", json!({})).unwrap()["audio"].as_array().unwrap().iter().flat_map(|t| t["items"].as_array().unwrap().clone()).find(|c| c["clip"] == json!(score)).unwrap()["effects"].as_array().unwrap().iter().map(|e| e["effect"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert!(fx(&lab).contains(&"studio_reverb".into()) && !fx(&lab).contains(&"pitch_shifter".into()), "{:?}", fx(&lab));
    call("compose.voice", json!({"clips": [score], "key": "none"}));
    assert!(!fx(&lab).contains(&"studio_reverb".into()));
    assert_eq!(call("compose.enhanceVoice", json!({"clips": [score]}))[0]["on"], true);
    assert!(fx(&lab).contains(&"denoise".into()) && fx(&lab).contains(&"speech_enhance".into()), "{:?}", fx(&lab));
    call("compose.enhanceVoice", json!({"clips": [score]}));
    assert!(!fx(&lab).contains(&"denoise".into()) && !fx(&lab).contains(&"speech_enhance".into()));
}

#[test]
fn duplicate_detach_canvas_and_gaps() {
    let (_d, lab) = lab();
    demo(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap_or_else(|e| panic!("{f}: {e}"));
    let v1 = |lab: &Lab| lab.timeline()["tracks"].as_array().unwrap().iter().find(|t| t["kind"] == "video").unwrap()["clips"].as_array().unwrap().clone();
    let first = v1(&lab)[0].clone();
    let (clip, n, dur) = (first["clip"].as_u64().unwrap(), v1(&lab).len(), lab.timeline()["duration"].as_f64().unwrap());
    call("compose.duplicate", json!({"clip": clip}));
    let after = v1(&lab);
    assert_eq!(after.len(), n + 1, "one more clip on V1");
    let end = first["end"].as_f64().unwrap();
    let len = end - first["start"].as_f64().unwrap();
    assert!((after[1]["start"].as_f64().unwrap() - end).abs() < 0.05, "the copy right after: {}", after[1]);
    assert!((lab.timeline()["duration"].as_f64().unwrap() - dur - len).abs() < 0.1, "what follows moves later");
    // detach: linked picture + sound become independent
    let r = call("compose.detachAudio", json!({"clips": [clip]}));
    assert_eq!(r["detached"], json!([clip]), "{r}");
    assert_eq!(call("compose.detachAudio", json!({"clips": [clip]}))["detached"], json!([]));
    // canvas
    call("project.setCanvas", json!({"ratio": "1:1"}));
    let c = call("project.canvas", json!({}));
    assert_eq!((c["width"].clone(), c["height"].clone(), c["ratio"].clone()), (c["height"].clone(), c["width"].clone(), json!("1:1")), "{c}");
    call("project.setCanvas", json!({"ratio": "9:16"}));
    let c = call("project.canvas", json!({}));
    assert_eq!(c["ratio"], "9:16", "{c}");
    assert!(lab.call("project.setCanvas", json!({"ratio": "2:7"}), Source::Agent).is_err());
    // gaps: move a later clip away, then close
    let last = v1(&lab).last().unwrap().clone();
    let id = last["clip"].as_u64().unwrap();
    call("timeline.move", json!({"clip": id, "seconds": last["start"].as_f64().unwrap() + 5.0}));
    let gapped = lab.timeline()["duration"].as_f64().unwrap();
    assert!(call("project.closeGaps", json!({}))["moved"].as_u64().unwrap() >= 1);
    assert!(lab.timeline()["duration"].as_f64().unwrap() < gapped - 4.0, "the gap is gone");
}
