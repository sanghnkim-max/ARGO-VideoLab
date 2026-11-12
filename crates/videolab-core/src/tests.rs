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
