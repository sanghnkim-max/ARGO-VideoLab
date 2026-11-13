//! VideoLab's MCP resources and prompts, beside its three tools (`lab-mcp`): what a client may
//! read without writing a program (the timeline, the media, the projects, the frame under the
//! playhead, the function list) and task templates for the common jobs (a reel, captions, a
//! tighter cut, a look, an export, a description of the video).

use lab_api::{HandleId, Source};
use lab_mcp::{LabPrompt, LabResource, LabResourceBody};
use serde_json::{Map, Value, json};

use crate::hub::Core;

const FRAME_SIDE: u32 = 720;

fn res(uri: &str, name: &str, description: &str, mime: &str) -> LabResource {
    LabResource { uri: uri.into(), name: name.into(), description: description.into(), mime_type: mime.into() }
}

pub(crate) fn resources() -> Vec<LabResource> {
    vec![
        res("videolab://timeline", "Timeline", "The open video's tracks and clips in seconds, transitions, markers, playhead (JSON)", "application/json"),
        res("videolab://frame", "Frame at the playhead", "The open video's frame under the playhead, 720 px (PNG)", "image/png"),
        res("videolab://items", "Project items", "Media, sequences and graphics in the open project, with ids and durations (JSON)", "application/json"),
        res("videolab://media", "Media files", "Media files in VideoLab's media folder (JSON)", "application/json"),
        res("videolab://projects", "Saved projects", "Saved VideoLab projects (.fcproj), newest first (JSON)", "application/json"),
        res("videolab://functions", "Functions", "Every function a program can call, by area (text)", "text/plain"),
    ]
}

fn json_text(v: &Value) -> LabResourceBody {
    LabResourceBody::Text { mime_type: "application/json".into(), text: serde_json::to_string_pretty(v).unwrap_or_default() }
}

fn call(core: &Core, id: &str, params: Value) -> Result<Value, String> {
    core.call(id, params, Source::Mcp).map_err(|e| e.to_string())
}

pub(crate) fn read(core: &Core, uri: &str) -> Result<LabResourceBody, String> {
    match uri {
        "videolab://timeline" => call(core, "doc.timeline", json!({})).map(|v| json_text(&v)),
        "videolab://items" => call(core, "doc.items", json!({})).map(|v| json_text(&v)),
        "videolab://media" => call(core, "media.files", json!({})).map(|v| json_text(&v)),
        "videolab://functions" => Ok(LabResourceBody::Text { mime_type: "text/plain".into(), text: core.registry.areas_summary() }),
        "videolab://projects" => {
            let dir = core.engine.resolve(crate::PROJECTS_DIR).map_err(|e| e.to_string())?;
            let mut projects: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(&dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| e.path().extension().is_some_and(|x| x == "fcproj"))
                        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.file_name().to_string_lossy().into_owned()))
                        .collect()
                })
                .unwrap_or_default();
            projects.sort_by(|a, b| b.0.cmp(&a.0));
            let list: Vec<Value> = projects.into_iter().map(|(_, name)| json!({"path": format!("{}/{name}", crate::PROJECTS_DIR), "name": name.trim_end_matches(".fcproj")})).collect();
            Ok(json_text(&json!(list)))
        }
        "videolab://frame" => {
            let preview = call(core, "doc.preview", json!({"maxSide": FRAME_SIDE}))?;
            let handle = preview["image"].as_str().or_else(|| preview.as_str()).and_then(HandleId::parse).ok_or("no frame (is a video open?)")?;
            let png = lab_script::encode_handle_png(&core.shared.handles, handle, FRAME_SIDE).map_err(|e| e.to_string());
            core.shared.handles.free(handle);
            Ok(LabResourceBody::Blob { mime_type: "image/png".into(), bytes: png? })
        }
        other => Err(format!("no resource {other}; resources/list names them")),
    }
}

fn prompt_def(name: &str, description: &str, arguments: &[(&str, &str, bool)]) -> LabPrompt {
    LabPrompt {
        name: name.into(),
        description: description.into(),
        arguments: arguments.iter().map(|(n, d, r)| ((*n).into(), (*d).into(), *r)).collect(),
    }
}

pub(crate) fn prompts() -> Vec<LabPrompt> {
    vec![
        prompt_def("make_reel", "Make a short reel from the person's clips: pick the best moments, cut to length, music, captions", &[
            ("clips", "Which clips (default: the media already in the project, or the newest camera videos)", false),
            ("seconds", "Target length in seconds (default 30)", false),
            ("ratio", "Canvas: 9:16 (default), 1:1, 16:9", false),
            ("style", "Mood or pace, e.g. energetic, calm, cinematic", false),
        ]),
        prompt_def("auto_captions", "Add captions from the speech in the video (on-device speech recognition)", &[("language", "Spoken language, e.g. en, ko (default: detect)", false)]),
        prompt_def("tighten", "Tighten the edit: remove long pauses and filler words, close the gaps", &[]),
        prompt_def("color_grade", "Give the video a look: a filter plus adjustments, checked on frames before and after", &[("look", "The look, e.g. warm film, moody, bright and clean", true)]),
        prompt_def("export", "Export the video and report the file", &[
            ("format", "mp4 (default), gif, or an edit list: xml, fcpxml, edl, otio", false),
            ("resolution", "480, 720 or 1080 (default 1080)", false),
        ]),
        prompt_def("describe", "Look at the open video and describe it: clips, what happens, length, issues worth fixing", &[]),
    ]
}

fn arg<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

const WORKFLOW: &str = "Use VideoLab's tools: lab_read to look (await lab.doc.timeline(), lab.doc.contactSheet({count: 6, maxSide: 320}), lab.doc.preview), lab_script to edit, lab_help('<area>') for parameters. Look before and after editing, and keep each program short.";

pub(crate) fn prompt(name: &str, args: &Map<String, Value>) -> Result<String, String> {
    let text = match name {
        "make_reel" => {
            let seconds = arg(args, "seconds").unwrap_or("30");
            let ratio = arg(args, "ratio").unwrap_or("9:16");
            let clips = arg(args, "clips").map_or("the media already in the project (if none, import the newest camera videos with lab.media.import)".to_owned(), |c| c.to_owned());
            let style = arg(args, "style").map_or(String::new(), |s| format!(" The style: {s}."));
            format!("Make a {seconds}-second reel in VideoLab from {clips}. Set the canvas to {ratio} (lab.project.setCanvas). Look at the clips (contact sheets), keep the strongest moments, cut and order them to about {seconds} s, add a transition or two where the cut is abrupt, add music if there is any in the media, auto-duck it under speech, and add auto captions if people talk.{style} Then show me a contact sheet of the result and summarize what you did. {WORKFLOW}")
        }
        "auto_captions" => {
            let language = arg(args, "language").map_or(String::new(), |l| format!(" The spoken language is {l}."));
            format!("Add captions to the open VideoLab video from its speech (lab.speech.captions; check lab.speech.models first and say if a speech model needs downloading).{language} Then read back the captions and show a frame with one on screen. {WORKFLOW}")
        }
        "tighten" => format!("Tighten the open VideoLab video: remove long pauses and filler words (lab.speech.removePauses, lab.speech.removeFillers), then close gaps on the main track (lab.project.closeGaps). Report the length before and after. {WORKFLOW}"),
        "color_grade" => {
            let look = arg(args, "look").ok_or("`look` is required")?;
            format!("Give the open VideoLab video a \"{look}\" look: choose a filter (lab.look.filters) and adjust (lab.look.adjust) clip by clip so they match. Show a frame before and after. {WORKFLOW}")
        }
        "export" => {
            let format = arg(args, "format").unwrap_or("mp4");
            let resolution = arg(args, "resolution").unwrap_or("1080");
            if matches!(format, "xml" | "fcpxml" | "edl" | "otio" | "aaf") {
                format!("Export the open VideoLab video as a {format} edit list (lab.export.interchange to exports/) and report the path and anything the report says did not translate. {WORKFLOW}")
            } else {
                format!("Export the open VideoLab video as {format} at {resolution}p (lab.export.start with scale = {resolution} / the canvas's short side, to exports/), wait for the job with lab.export.jobs, and report the file's path, size and how long it took. {WORKFLOW}")
            }
        }
        "describe" => format!("Look at the open VideoLab video (timeline, then contact sheets) and describe it: the clips, what happens, the length, and anything worth fixing (shaky shots, long pauses, abrupt cuts, missing captions). Do not edit. {WORKFLOW}"),
        other => return Err(format!("no prompt {other}; prompts/list names them")),
    };
    Ok(text)
}
