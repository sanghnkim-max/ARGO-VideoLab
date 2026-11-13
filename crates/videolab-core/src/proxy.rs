//! Proxies for heavy phone footage, made and attached without anyone asking.
//!
//! A phone records 4K HEVC: decoding it costs milliseconds per frame and copies 12 MB out of the
//! hardware decoder for every one, and a jump to another point of the file (a cut, a scrub) means
//! decoding from the last keyframe. FilmCraft has Premiere's proxy workflow (`media.createProxies`:
//! a background job transcodes each clip, then the clip carries the proxy); this module drives it:
//! every video clip in the project whose picture is larger than [`PROXY_ABOVE`] on its long side
//! gets a ProRes 422 Proxy copy at a quarter of its size (every frame a keyframe, so any frame
//! decodes alone) under the workspace's hidden `.videolab/proxies`.
//!
//! Only quick renders read proxies (playback and scrubbing: `Engine::snapshot` with
//! `analysis::quick`); a paused frame, the frame a scrub stops on, exports, thumbnails and
//! background analysis read the original, so nothing the person keeps or looks at closely is
//! made from the smaller copy.
//!
//! The worker wakes on every reported document change (`hub::report`), so media imported by the
//! app, its agent or ARGO, and projects opened, are all covered. FilmCraft attaches a finished
//! proxy in its `proxies::poll`, which VideoLab calls here while a job runs.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use filmcraft_media::MediaKind;
use filmcraft_project::MediaRef;
use serde_json::json;

use crate::engine::Engine;

/// Video larger than this on its long side gets a proxy (1080p does not: it decodes fast).
pub const PROXY_ABOVE: u32 = 1920;

/// FilmCraft's preset: ProRes 422 Proxy at a quarter of the frame size.
const PRESET: &str = "prores_proxy_quarter";

/// Where proxies go, beneath the workspace (hidden from the media listing).
pub const PROXY_DIR: &str = ".videolab/proxies";

/// How long one round of proxy jobs is watched for before giving up on it.
const WATCH_LIMIT: Duration = Duration::from_secs(60 * 60);

pub struct Proxies {
    enabled: AtomicBool,
    worker: OnceLock<Mutex<Sender<()>>>,
    /// Media paths already asked for in this process: a clip whose proxy failed is not retried
    /// on every change.
    asked: Mutex<HashSet<String>>,
}

impl Default for Proxies {
    fn default() -> Self {
        Self { enabled: AtomicBool::new(true), worker: OnceLock::new(), asked: Mutex::new(HashSet::new()) }
    }
}

impl Proxies {
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// The document may have new heavy media: look (on the worker, never the caller's thread).
    pub fn nudge(&self, engine: &Arc<Engine>) {
        if !self.enabled() {
            return;
        }
        let tx = self.worker.get_or_init(|| {
            let (tx, rx) = channel::<()>();
            let weak = Arc::downgrade(engine);
            let _ = std::thread::Builder::new().name("videolab-proxies".into()).spawn(move || work(&weak, &rx));
            Mutex::new(tx)
        });
        let _ = tx.lock().unwrap_or_else(PoisonError::into_inner).send(());
    }
}

fn work(engine: &Weak<Engine>, rx: &Receiver<()>) {
    while rx.recv().is_ok() {
        // Many changes in a row (a script's steps) are one look.
        std::thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}
        let Some(engine) = engine.upgrade() else { return };
        if let Err(why) = round(&engine) {
            log::warn!("proxies: {why}");
        }
    }
}

/// Start proxies for the heavy clips that have none, then attach them as they finish.
fn round(engine: &Arc<Engine>) -> Result<(), String> {
    let wanted = heavy_without_proxy(engine);
    let fresh: Vec<(u64, String)> = {
        let mut asked = engine.proxies.asked.lock().unwrap_or_else(PoisonError::into_inner);
        wanted.into_iter().filter(|(_, path)| asked.insert(path.clone())).collect()
    };
    if fresh.is_empty() {
        return Ok(());
    }
    let dir = engine.resolve(PROXY_DIR).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let items: Vec<u64> = fresh.iter().map(|(i, _)| *i).collect();
    let started = Instant::now();
    let r = engine.execute("media.createProxies", json!({"items": items, "preset": PRESET, "destination": dir.to_string_lossy()}))?;
    log::info!("proxies: making {} for item(s) {items:?}", r["outputs"].as_array().map_or(0, Vec::len));
    // FilmCraft attaches finished proxies in its poll; nothing calls it in VideoLab but this.
    while started.elapsed() < WATCH_LIMIT {
        std::thread::sleep(Duration::from_secs(1));
        let mut g = engine.lock();
        filmcraft_engine::proxies::poll(&mut g);
        if g.media_jobs.is_empty() {
            break;
        }
    }
    let attached = items.iter().filter(|i| has_proxy(engine, **i)).count();
    log::info!("proxies: {attached} of {} attached in {:.1} s", items.len(), started.elapsed().as_secs_f64());
    Ok(())
}

/// Video media items whose picture is larger than [`PROXY_ABOVE`] and that carry no proxy, with
/// their media path.
fn heavy_without_proxy(engine: &Engine) -> Vec<(u64, String)> {
    let g = engine.lock();
    g.project
        .items
        .iter()
        .filter_map(|(id, it)| {
            let m = it.as_media()?;
            let MediaRef::File { path } = &m.media else { return None };
            let v = m.info.video.as_ref()?;
            let heavy = matches!(m.info.kind, MediaKind::Movie) && v.width.max(v.height) > PROXY_ABOVE;
            (heavy && m.proxy.is_none() && !m.offline).then(|| (id.0, path.clone()))
        })
        .collect()
}

fn has_proxy(engine: &Engine, item: u64) -> bool {
    engine.lock().project.item(filmcraft_project::ItemId(item)).and_then(|it| it.as_media()).is_some_and(|m| m.proxy.is_some())
}
