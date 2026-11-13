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

#[test]
fn copy_cut_paste() {
    let (_d, lab) = lab();
    demo(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap_or_else(|e| panic!("{f}: {e}"));
    let v1 = |lab: &Lab| lab.timeline()["tracks"].as_array().unwrap().iter().find(|t| t["kind"] == "video").unwrap()["clips"].as_array().unwrap().clone();
    assert_eq!(call("timeline.clipboard", json!({}))["clips"], 0);
    assert!(lab.call("timeline.paste", json!({}), Source::Agent).is_err(), "nothing to paste");
    let first = v1(&lab)[0].clone();
    let (clip, n, len) = (first["clip"].as_u64().unwrap(), v1(&lab).len(), first["end"].as_f64().unwrap() - first["start"].as_f64().unwrap());
    call("timeline.copy", json!({"clips": [clip]}));
    assert!(call("timeline.clipboard", json!({}))["clips"].as_u64().unwrap() >= 1);
    // paste (insert) at the end of the first clip: one more clip there, the rest pushed later
    let dur = lab.timeline()["duration"].as_f64().unwrap();
    let r = call("timeline.paste", json!({"seconds": first["end"]}));
    assert!(!r["pasted"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(v1(&lab).len(), n + 1);
    assert!((v1(&lab)[1]["start"].as_f64().unwrap() - first["end"].as_f64().unwrap()).abs() < 0.05);
    assert!((lab.timeline()["duration"].as_f64().unwrap() - dur - len).abs() < 0.1);
    // cut with ripple: gone from V1, and pasteable again
    let pasted = v1(&lab)[1]["clip"].as_u64().unwrap();
    call("timeline.cut", json!({"clips": [pasted], "ripple": true}));
    assert_eq!(v1(&lab).len(), n);
    assert!((lab.timeline()["duration"].as_f64().unwrap() - dur).abs() < 0.1, "ripple closed the gap");
    call("timeline.paste", json!({"seconds": 0.0}));
    assert_eq!(v1(&lab).len(), n + 1);
}

#[test]
fn stabilizer_analyses_in_the_background() {
    let (_d, lab) = lab();
    demo(&lab);
    let (clip, _, start) = demo_clips(&lab);
    let at = filmcraft_time::Tick::from_seconds_f64(start + 1.0);
    let plain = lab.engine.render(at, 320).unwrap().0.rgba;
    lab.call("compose.stabilize", json!({"clips": [clip]}), Source::Agent).unwrap();
    // Not analysed yet: the preview is drawn without it, at once, and the analysis is queued.
    let t0 = std::time::Instant::now();
    let first = lab.engine.render(at, 320).unwrap().0.rgba;
    assert_eq!(first, plain, "drawn without the stabilizer while it is analysed");
    assert!(t0.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(lab.engine.analyses.pending(), 1);
    // Playback holds the worker; then it finishes and the stabilizer shows.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while lab.engine.analyses.pending() > 0 {
        assert!(std::time::Instant::now() < deadline, "analysis did not finish");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let after = lab.engine.render(at, 320).unwrap().0.rgba;
    assert_ne!(after, plain, "the stabilizer applies once analysed");
}

#[test]
fn interchange_exports() {
    let (d, lab) = lab();
    demo(&lab);
    for (format, ext, head) in [("xml", "xml", "<xmeml"), ("fcpxml", "fcpxml", "<fcpxml"), ("edl", "edl", "TITLE"), ("otio", "otio", "OTIO_SCHEMA")] {
        let path = format!("exports/cut.{ext}");
        let r = lab.call("export.interchange", json!({"format": format, "path": path}), Source::Ui).unwrap_or_else(|e| panic!("{format}: {e}"));
        let text = std::fs::read_to_string(d.path().join(&path)).unwrap();
        assert!(text.contains(head), "{format}: {}", &text[..text.len().min(200)]);
        assert!(r["bytes"].as_u64().unwrap() > 100, "{r}");
    }
    assert!(lab.call("export.interchange", json!({"format": "mov", "path": "exports/x.mov"}), Source::Ui).is_err());
}

#[test]
#[ignore = "needs VIDEOLAB_HDR_CLIP: a 10-bit HDR phone clip"]
fn hdr_thumbnails_have_colour() {
    let Some(clip) = std::env::var_os("VIDEOLAB_HDR_CLIP") else { return };
    let (d, lab) = lab();
    std::fs::create_dir_all(d.path().join("media")).unwrap();
    std::fs::copy(&clip, d.path().join("media/hdr.mp4")).unwrap();
    let r = lab.call("media.import", json!({"paths": ["media/hdr.mp4"]}), Source::Ui).unwrap();
    let item = r["items"][0].as_u64().or_else(|| r[0].as_u64()).unwrap_or_else(|| panic!("{r}"));
    let thumbs = lab.engine.thumbnails(item, &[filmcraft_time::Tick::from_seconds_f64(1.0)], 360).unwrap();
    let px = &thumbs[0].rgba;
    let chroma: u64 = px.chunks(4).map(|p| (i32::from(p[0]) - i32::from(p[1])).unsigned_abs() as u64 + (i32::from(p[1]) - i32::from(p[2])).unsigned_abs() as u64).sum();
    let mean = chroma as f64 / (px.len() / 4) as f64;
    let png = lab_script::encode_png(thumbs[0].width, thumbs[0].height, lab_script::ColorType::Rgba, px).unwrap();
    std::fs::write(std::env::temp_dir().join("hdr-thumb.png"), png).unwrap();
    assert!(mean > 4.0, "thumbnail looks grey: mean |r-g|+|g-b| = {mean}");
}

#[test]
fn canvas_change_keeps_overlays() {
    let (_d, lab) = lab();
    demo(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap_or_else(|e| panic!("{f}: {e}"));
    let video = |lab: &Lab| lab.timeline()["tracks"].as_array().unwrap().iter().filter(|t| t["kind"] == "video").cloned().collect::<Vec<_>>();
    let clip = video(&lab)[0]["clips"][1]["clip"].as_u64().unwrap();
    call("timeline.move", json!({"clip": clip, "seconds": 1.0, "track": "new"}));
    call("clip.setTransform", json!({"clip": clip, "scale": 40, "seconds": 1.5}));
    call("project.setCanvas", json!({"ratio": "1:1"}));
    let scale = call("clip.transform", json!({"clip": clip, "seconds": 1.5}))["scale"].as_f64().unwrap();
    assert!((scale - 40.0).abs() < 0.01, "the overlay keeps its size: {scale}");
}

#[test]
fn stripped_project_is_reused_until_it_changes() {
    let (_d, lab) = lab();
    demo(&lab);
    let (clip, _, _) = demo_clips(&lab);
    lab.call("compose.stabilize", json!({"clips": [clip]}), Source::Agent).unwrap();
    // Hold the worker (as playback does) so the analysis stays pending across the renders below.
    lab.engine.analyses.note_playback();
    let snap = |lab: &Lab| {
        let g = lab.engine.lock();
        let seq = g.state.active_sequence.unwrap();
        lab.engine.analyses.strip_unready(g.project.clone(), seq, &g.media, &g.services)
    };
    let a = snap(&lab);
    lab.engine.analyses.note_playback();
    let b = snap(&lab);
    assert!(std::sync::Arc::ptr_eq(&a, &b), "the same stripped copy while nothing changed");
    assert!(!a.sequence(lab.engine.lock().state.active_sequence.unwrap()).unwrap().all_tracks().flat_map(|t| t.items.iter()).any(|i| i.effects.iter().any(|e| e.effect == "warp_stabilizer")), "stripped");
    // An edit: a new copy, with the edit in it.
    lab.call("timeline.volume", json!({"clips": [clip], "db": -3}), Source::Agent).unwrap();
    lab.engine.analyses.note_playback();
    let c = snap(&lab);
    assert!(!std::sync::Arc::ptr_eq(&b, &c), "an edit makes a new copy");
}

#[test]
fn transitions_show_and_change() {
    let (_d, lab) = lab();
    demo(&lab);
    let call = |f: &str, p: Value| lab.call(f, p, Source::Agent).unwrap_or_else(|e| panic!("{f}: {e}"));
    let v1 = |lab: &Lab| lab.timeline()["tracks"].as_array().unwrap().iter().find(|t| t["kind"] == "video").unwrap().clone();
    let clip = v1(&lab)["clips"][1]["clip"].as_u64().unwrap();
    for t in v1(&lab)["transitions"].as_array().unwrap().clone() {
        call("timeline.removeTransition", json!({"transition": t["id"]}));
    }
    assert!(v1(&lab)["transitions"].as_array().unwrap().is_empty());
    call("timeline.transition", json!({"clip": clip, "effect": "cross_dissolve", "edge": "in", "seconds": 0.5}));
    let t = v1(&lab)["transitions"][0].clone();
    assert_eq!(t["name"], "Cross Dissolve", "{t}");
    assert!((t["duration"].as_f64().unwrap() - 0.5).abs() < 0.05, "{t}");
    assert_eq!(t["to"], json!(clip), "{t}");
    call("timeline.setTransition", json!({"transition": t["id"], "seconds": 1.2}));
    let t2 = v1(&lab)["transitions"][0].clone();
    assert!((t2["duration"].as_f64().unwrap() - 1.2).abs() < 0.05, "{t2}");
    // Another type on the same edge replaces it.
    call("timeline.transition", json!({"clip": clip, "effect": "dip_to_black", "edge": "in", "seconds": 1.2}));
    let ts = v1(&lab)["transitions"].as_array().unwrap().clone();
    assert_eq!(ts.len(), 1, "replaced, not stacked: {ts:?}");
    assert_eq!(ts[0]["effect"], "dip_to_black");
    call("timeline.removeTransition", json!({"transition": ts[0]["id"]}));
    assert!(v1(&lab)["transitions"].as_array().unwrap().is_empty());
}

#[test]
fn mcp_resources_and_prompts() {
    let (_d, lab) = lab();
    demo(&lab);
    let list = lab.lab_mcp_request("resources/list", json!({})).unwrap();
    let uris: Vec<&str> = list["resources"].as_array().unwrap().iter().filter_map(|r| r["uri"].as_str()).collect();
    assert!(uris.contains(&"videolab://timeline") && uris.contains(&"videolab://frame"), "{list}");
    let timeline = lab.lab_mcp_request("resources/read", json!({"uri": "videolab://timeline"})).unwrap();
    let text = timeline["contents"][0]["text"].as_str().unwrap();
    assert!(serde_json::from_str::<Value>(text).unwrap()["tracks"].is_array(), "{text}");
    let frame = lab.lab_mcp_request("resources/read", json!({"uri": "videolab://frame"})).unwrap();
    assert_eq!(frame["contents"][0]["mimeType"], "image/png");
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(frame["contents"][0]["blob"].as_str().unwrap()).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    assert!(lab.lab_mcp_request("resources/read", json!({"uri": "videolab://nope"})).is_err());
    // the frame's handle is freed
    assert!(lab.shared().handles.ids().is_empty(), "{:?}", lab.shared().handles.ids());

    let prompts = lab.lab_mcp_request("prompts/list", json!({})).unwrap();
    let names: Vec<&str> = prompts["prompts"].as_array().unwrap().iter().filter_map(|p| p["name"].as_str()).collect();
    assert!(names.contains(&"make_reel") && names.contains(&"color_grade"), "{prompts}");
    let reel = lab.lab_mcp_request("prompts/get", json!({"name": "make_reel", "arguments": {"seconds": "15", "style": "calm"}})).unwrap();
    let message = reel["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(message.contains("15-second") && message.contains("calm") && message.contains("9:16"), "{message}");
    assert!(lab.lab_mcp_request("prompts/get", json!({"name": "color_grade"})).is_err(), "look is required");
}

/// ARGO mounts VideoLab's server as `videolab` and names each tool `videolab__<tool>`: a tool
/// name must not contain `__` or start/end with `_`, and the joined name fits 64 bytes.
#[test]
fn function_tool_names_fit_argos_rules() {
    let (_d, lab) = lab();
    let mut names = std::collections::HashSet::new();
    for f in lab.core.registry.functions() {
        let name = lab_mcp::function_tool_name(&f.area, &f.name);
        assert!(!name.contains("__") && !name.starts_with('_') && !name.ends_with('_'), "{name}");
        assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{name}");
        assert!(format!("videolab__{name}").len() <= 64, "{name}");
        assert!(names.insert(name.clone()), "two functions share {name}");
        assert!(!matches!(name.as_str(), "lab_read" | "lab_script" | "lab_help"), "{name}");
    }
    assert!(names.len() > 80, "{}", names.len());
}

#[test]
fn a_bare_project_name_saves_where_the_app_lists_projects() {
    use crate::areas::project_path;
    assert_eq!(project_path("ARGO Reel"), "projects/ARGO Reel.fcproj");
    assert_eq!(project_path("ARGO Reel.fcproj"), "projects/ARGO Reel.fcproj");
    assert_eq!(project_path("projects/trip.fcproj"), "projects/trip.fcproj");
    assert_eq!(project_path("exports/cut"), "exports/cut.fcproj");
}

#[test]
fn a_raw_export_runs_as_a_job_on_the_hardware_encoder() {
    let mut p = serde_json::json!({"path": "exports/a.mp4", "wait": true});
    crate::areas::export_as_job(&mut p);
    assert_eq!((p["wait"].clone(), p["hardwareEncoding"].clone()), (serde_json::json!(false), serde_json::json!("auto")));
    // The caller's own choice of encoder stays.
    let mut p = serde_json::json!({"path": "exports/a.mp4", "hardwareEncoding": "off"});
    crate::areas::export_as_job(&mut p);
    assert_eq!(p["hardwareEncoding"], "off");
}

#[test]
fn quick_renders_draw_optical_flow_by_frame_sampling() {
    use filmcraft_project::TimeInterpolation;
    let (_d, lab) = lab();
    demo(&lab);
    let (clip, _, _) = demo_clips(&lab);
    lab.call("clip.smoothSlowmo", json!({"clips": [clip]}), Source::Agent).unwrap();
    let interp = |lab: &Lab| {
        let g = lab.engine.lock();
        let seq = g.state.active_sequence.unwrap();
        let p = lab.engine.analyses.strip_unready(g.project.clone(), seq, &g.media, &g.services);
        p.sequence(seq).unwrap().all_tracks().flat_map(|t| t.items.iter()).find(|i| i.id.0 == clip).unwrap().time_interpolation
    };
    assert_eq!(interp(&lab), TimeInterpolation::OpticalFlow, "a paused frame keeps optical flow");
    assert_eq!(crate::analysis::with_quick(|| interp(&lab)), TimeInterpolation::FrameSampling, "playback and scrubbing sample");
    // Right after a quick render (still within playback's grace), a full render is optical flow:
    // the frame a scrub stops on.
    lab.engine.analyses.note_playback();
    assert_eq!(interp(&lab), TimeInterpolation::OpticalFlow, "a settled frame keeps optical flow");
}
