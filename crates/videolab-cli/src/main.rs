//! `videolab`: VideoLab's registry from the shell, and its servers off-device.
//!
//! * `videolab call area.name '{json}' --root DIR`: one registry call; prints the JSON result.
//! * `videolab help [area | area.fn]`: the registry's areas, or one area in full.
//! * `videolab script FILE.js --root DIR [--read-only] [--out-dir D] [--timeout-ms N]`: run a
//!   `lab_script` program (`-` reads stdin); prints `{value, logs, error, images}` and writes
//!   attached images to `D/image-<k>.png`.
//! * `videolab serve-http --root DIR [--port N] [--token T] [--allow-edits]`: VideoLab's core
//!   with the same servers the app runs: MCP on `/mcp` (and `/filmcraft/mcp`), and FilmCraft's
//!   control channel for `filmcraft-cli --bridge` (its port and token are printed).
//!
//! FilmCraft's own CLI is not linked in (it is a binary-only package): the app bundles the real
//! `filmcraft-cli`, built from the pinned rev by tools/build-filmcraft-cli.sh.
//!
//! Exit codes: 0 ok, 1 the call/program failed, 2 usage.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::time::Duration;

use videolab_core::lab_api::Source;
use videolab_core::{Lab, LabOptions};
use serde_json::Value;

/// Largest program file read.
const MAX_SCRIPT_FILE: u64 = 1 << 20;

fn main() {
    videolab_core::prepare_native_process();
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let rest = || args[1..].iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
    let code = match args.first().and_then(|a| a.to_str()) {
        Some("serve-http") => serve(&rest()),
        Some("call") => call(&rest()),
        Some("help") => help(&rest()),
        Some("script") => script(&rest()),
        Some("bench-decode") => bench(&rest()),
        Some("-h" | "--help") | None => usage(""),
        Some("--version") => {
            println!("videolab {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some(other) => usage(&format!("unknown subcommand `{other}` (FilmCraft's own commands: use filmcraft-cli, e.g. `filmcraft-cli --bridge 127.0.0.1:PORT exec …` against `videolab serve-http`)")),
    };
    std::process::exit(code);
}

/// Flags shared by the videolab subcommands, plus positional arguments in order.
#[derive(Default)]
struct Flags {
    root: Option<PathBuf>,
    port: Option<u16>,
    token: Option<String>,
    allow_edits: bool,
    read_only: bool,
    out_dir: Option<PathBuf>,
    timeout_ms: u64,
    gpu: bool,
    positional: Vec<String>,
}

fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--root" => f.root = Some(PathBuf::from(value("--root")?)),
            "--port" => f.port = Some(value("--port")?.parse().map_err(|_| "--port needs a number".to_string())?),
            "--token" => f.token = Some(value("--token")?),
            "--out-dir" => f.out_dir = Some(PathBuf::from(value("--out-dir")?)),
            "--timeout-ms" => f.timeout_ms = value("--timeout-ms")?.parse().map_err(|_| "--timeout-ms needs a number".to_string())?,
            "--allow-edits" => f.allow_edits = true,
            "--read-only" => f.read_only = true,
            "--gpu" => f.gpu = true,
            "-h" | "--help" => return Err(String::new()),
            other if other.starts_with("--") => return Err(format!("unknown flag {other}")),
            other => f.positional.push(other.to_string()),
        }
    }
    Ok(f)
}

fn open_lab(f: &Flags, gpu: bool) -> Result<Lab, i32> {
    let Some(root) = &f.root else { return Err(usage("--root DIR is required")) };
    Lab::new(root, &LabOptions { gpu, ..Default::default() }).map_err(|e| {
        eprintln!("videolab: {e}");
        1
    })
}

fn print_json(v: &Value) {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{text}");
    let _ = out.flush();
}

fn call(args: &[String]) -> i32 {
    let f = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => return usage(&e),
    };
    let (id, params) = match f.positional.as_slice() {
        [id] => (id.clone(), Value::Object(Default::default())),
        [id, json] => match serde_json::from_str(json) {
            Ok(v) => (id.clone(), v),
            Err(e) => return usage(&format!("params are not JSON: {e}")),
        },
        _ => return usage("call takes area.name and an optional JSON object"),
    };
    let lab = match open_lab(&f, f.gpu) {
        Ok(l) => l,
        Err(code) => return code,
    };
    match lab.call(&id, params, Source::Cli) {
        Ok(v) => {
            print_json(&v);
            0
        }
        Err(e) => {
            eprintln!("videolab: {e}");
            1
        }
    }
}

fn help(args: &[String]) -> i32 {
    let f = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => return usage(&e),
    };
    // The registry needs no workspace; --root is accepted for symmetry.
    let registry = videolab_core::build_registry();
    match registry.help(f.positional.first().map(String::as_str)) {
        Ok(v) => {
            print_json(&v);
            0
        }
        Err(e) => {
            eprintln!("videolab: {e}");
            1
        }
    }
}

fn script(args: &[String]) -> i32 {
    let f = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => return usage(&e),
    };
    let [file] = f.positional.as_slice() else { return usage("script takes one FILE.js (or - for stdin)") };
    let code = match read_program(file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("videolab: {e}");
            return 1;
        }
    };
    let lab = match open_lab(&f, f.gpu) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let outcome = lab.run_script_as(&code, f.read_only, Duration::from_millis(f.timeout_ms), Source::Cli);
    let mut report = outcome.to_json();
    if let Some(dir) = &f.out_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("videolab: cannot create {}: {e}", dir.display());
            return 1;
        }
        let mut written = Vec::new();
        for (k, image) in outcome.images.iter().enumerate() {
            let path = dir.join(format!("image-{k}.png"));
            match std::fs::write(&path, &image.bytes) {
                Ok(()) => written.push(Value::String(path.display().to_string())),
                Err(e) => eprintln!("videolab: cannot write {}: {e}", path.display()),
            }
        }
        if let Value::Object(map) = &mut report {
            map.insert("written".into(), Value::Array(written));
        }
    } else if !outcome.images.is_empty() {
        eprintln!("videolab: {} image(s) attached; pass --out-dir DIR to write them", outcome.images.len());
    }
    print_json(&report);
    if outcome.error.is_some() { 1 } else { 0 }
}

fn read_program(file: &str) -> Result<String, String> {
    let mut code = String::new();
    if file == "-" {
        std::io::stdin().take(MAX_SCRIPT_FILE + 1).read_to_string(&mut code).map_err(|e| format!("stdin: {e}"))?;
    } else {
        let f = std::fs::File::open(file).map_err(|e| format!("{file}: {e}"))?;
        f.take(MAX_SCRIPT_FILE + 1).read_to_string(&mut code).map_err(|e| format!("{file}: {e}"))?;
    }
    if code.len() as u64 > MAX_SCRIPT_FILE {
        return Err(format!("{file} is larger than {MAX_SCRIPT_FILE} bytes"));
    }
    Ok(code)
}

fn serve(args: &[String]) -> i32 {
    let f = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => return usage(&e),
    };
    if !f.positional.is_empty() {
        return usage(&format!("unexpected argument {}", f.positional[0]));
    }
    let Some(token) = f.token.clone().or_else(|| std::env::var("VIDEOLAB_MCP_TOKEN").ok()) else {
        return usage("--token (or VIDEOLAB_MCP_TOKEN) is required");
    };
    let lab = match open_lab(&f, true) {
        Ok(l) => l,
        Err(code) => return code,
    };
    match lab.start_http(f.port.unwrap_or(7700), &token, f.allow_edits) {
        Ok(bound) => {
            let mode = if f.allow_edits { "edits allowed" } else { "read-only" };
            println!("VideoLab MCP: http://127.0.0.1:{bound}/mcp ({mode}; workspace {})", lab.root().display());
            if f.allow_edits {
                println!("FilmCraft MCP: http://127.0.0.1:{bound}/filmcraft/mcp");
            }
            let (port, token) = lab.control();
            println!("FilmCraft control channel: 127.0.0.1:{port}");
            println!("  FILMCRAFT_CONTROL_TOKEN={token} filmcraft-cli --bridge 127.0.0.1:{port} inspect sequence");
            let _ = std::io::stdout().flush();
        }
        Err(e) => {
            eprintln!("videolab: {e}");
            return 1;
        }
    }
    // Serve until killed.
    loop {
        std::thread::park();
    }
}

/// `videolab bench-decode FILE [--frames N] [--sw]`: decode speed with the hardware decoder
/// (MediaCodec on Android) or FilmCraft's software decoder.
fn bench(args: &[String]) -> i32 {
    let mut file = None;
    let mut frames = 120i64;
    let mut hardware = true;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--frames" => frames = it.next().and_then(|v| v.parse().ok()).unwrap_or(120),
            "--sw" => hardware = false,
            other => file = Some(other.to_string()),
        }
    }
    let Some(file) = file else { return usage("bench-decode FILE [--frames N] [--sw]") };
    match videolab_core::bench_decode(&file, frames, hardware) {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(e) => {
            eprintln!("videolab: {e}");
            1
        }
    }
}

fn usage(problem: &str) -> i32 {
    if !problem.is_empty() {
        eprintln!("videolab: {problem}");
    }
    eprintln!("usage: videolab call area.name ['{{json}}'] --root DIR");
    eprintln!("       videolab help [area | area.fn]");
    eprintln!("       videolab script FILE.js|- --root DIR [--read-only] [--out-dir DIR] [--timeout-ms N]");
    eprintln!("       videolab serve-http --root DIR [--port 7700] [--token TOKEN | env VIDEOLAB_MCP_TOKEN] [--allow-edits]");
    eprintln!("       videolab bench-decode FILE [--frames N] [--sw]   (hardware vs software decode speed)");
    eprintln!("       (call/script take --gpu to use FilmCraft's wgpu compositor)");
    if problem.is_empty() { 0 } else { 2 }
}
