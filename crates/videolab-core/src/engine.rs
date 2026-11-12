//! The FilmCraft session VideoLab shares, and the pixel/sample paths a phone UI needs that are
//! not engine commands: Program frames as RGBA (CPU reference renderer, or FilmCraft's wgpu
//! compositor), clip filmstrip thumbnails, waveform peaks, and the program audio mix for playback.
//!
//! Renders take the session lock only long enough to snapshot the project (an `Arc`), so a
//! playback frame never waits for an agent's long command and vice versa.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use filmcraft_engine::Session;
use filmcraft_project::{ItemId, Project};
use filmcraft_time::Tick;

/// Longest side of a preview frame or thumbnail the UI or an agent may ask for.
pub const MAX_PREVIEW_SIDE: u32 = 4096;
/// Most waveform buckets one call returns.
pub const MAX_PEAK_BUCKETS: usize = 16_384;
/// Most audio frames one mix call returns (10 s at 48 kHz).
pub const MAX_MIX_FRAMES: usize = 480_000;

/// One rendered picture: RGBA8, opaque (over black), row-major.
#[derive(Debug, Clone)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Which renderer drew a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    Cpu,
    Gpu,
}

struct Snapshot {
    project: Arc<Project>,
    seq: ItemId,
    provider: filmcraft_engine::media_pool::PoolProvider,
    rate: filmcraft_time::FrameRate,
    width: u32,
    height: u32,
}

/// The app state every registry function reaches through `ctx.state::<Engine>()`.
pub struct Engine {
    pub session: Arc<Mutex<Session>>,
    /// The workspace confinement every engine file access goes through.
    pub services: Arc<crate::services::WorkspaceServices>,
    /// FilmCraft's off-screen wgpu compositor (Vulkan on Android), when it works on this device.
    gpu: Mutex<Option<filmcraft_gpu::ExportRenderer>>,
    gpu_wanted: AtomicBool,
    gpu_failed: AtomicBool,
    /// Filmstrip sources: each clip's own copy of its file (`MediaPool::own_source`, what FilmCraft
    /// gives background export jobs), so thumbnails seeking around never move the decoder the
    /// preview and playback read. Keyed by item, for the document epoch they were opened in
    /// (FilmCraft reuses item ids across projects).
    thumb_sources: Mutex<ThumbSources>,
    /// Stabilizer / reframe analysis, in the background (see `analysis`).
    pub analyses: Arc<crate::analysis::Analyses>,
}

#[derive(Default)]
struct ThumbSources {
    epoch: Option<u64>,
    sources: std::collections::HashMap<u64, filmcraft_media::SharedSource>,
}

/// Filmstrip sources kept open at once (each holds a decoder).
const MAX_THUMB_SOURCES: usize = 4;

impl Engine {
    /// A session over `services` (the workspace).
    pub fn new(services: Arc<crate::services::WorkspaceServices>) -> Self {
        let session = Session::new(services.clone());
        Self { session: Arc::new(Mutex::new(session)), services, gpu: Mutex::new(None), gpu_wanted: AtomicBool::new(false), gpu_failed: AtomicBool::new(false), thumb_sources: Mutex::default(), analyses: Arc::default() }
    }

    /// A workspace path (relative, or absolute inside it) as the absolute path the engine stores.
    pub fn resolve(&self, path: &str) -> std::io::Result<std::path::PathBuf> {
        self.services.resolve(path)
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run an engine command by id (the same call every surface makes).
    pub fn execute(&self, id: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let mut g = self.lock();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| g.execute(id, params))) {
            Ok(r) => r.map_err(|e| e.to_string()),
            Err(_) => Err(format!("internal error: {id} panicked")),
        }
    }

    // ------------------------------------------------------------------ GPU

    /// Turn the GPU preview renderer on (it is created on first use and probed with one frame).
    pub fn set_gpu(&self, on: bool) {
        self.gpu_wanted.store(on, Ordering::Relaxed);
        if !on {
            *self.gpu.lock().unwrap_or_else(PoisonError::into_inner) = None;
        }
    }

    /// `(wanted, active)`: active once a device exists and has not failed.
    pub fn gpu_status(&self) -> (bool, bool) {
        let wanted = self.gpu_wanted.load(Ordering::Relaxed);
        (wanted, wanted && !self.gpu_failed.load(Ordering::Relaxed))
    }

    // ------------------------------------------------------------------ frames

    fn snapshot(&self) -> Result<Snapshot, String> {
        let g = self.lock();
        let seq = g.state.active_sequence.ok_or("no sequence is open")?;
        let s = g.project.sequence(seq).ok_or("no sequence is open")?;
        let (width, height) = (s.settings.width, s.settings.height);
        let rate = g.sequence_rate();
        // Clips still being analysed draw without their stabilizer / reframe (see `analysis`).
        let project = self.analyses.strip_unready(g.project.clone(), seq, &g.media, &g.services);
        let provider = g.media.provider(project.clone(), g.services.clone());
        Ok(Snapshot { project, seq, provider, rate, width, height })
    }

    /// The active sequence's frame at `t` (snapped to its frame), longest side at most
    /// `max_side`, without moving the playhead.
    pub fn render(&self, t: Tick, max_side: u32) -> Result<(Rgba, Renderer), String> {
        if max_side == 0 || max_side > MAX_PREVIEW_SIDE {
            return Err(format!("max_side must be 1..={MAX_PREVIEW_SIDE}"));
        }
        let snap = self.snapshot()?;
        let scale = (max_side as f32 / snap.width.max(snap.height).max(1) as f32).min(1.0);
        let seq = snap.project.sequence(snap.seq).ok_or("no sequence is open")?;
        let (w, h) = filmcraft_render::output_size(seq, scale);
        let (Ok(w32), Ok(h32)) = (u32::try_from(w), u32::try_from(h)) else { return Err("frame too large".into()) };
        filmcraft_project::validate_frame_size(w32, h32)?;
        let t = snap.rate.snap(t.max(Tick::ZERO));
        let opts = filmcraft_render::RenderOptions { scale, captions: true, ..Default::default() };
        let started = std::time::Instant::now();
        let out = if let Some(img) = self.render_gpu(&snap, t, opts) {
            let composed = started.elapsed();
            (to_rgba(&img)?, Renderer::Gpu, composed)
        } else {
            let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filmcraft_render::render_sequence(&snap.project, snap.seq, t, opts, &snap.provider)))
                .map_err(|_| "rendering panicked".to_string())?;
            let composed = started.elapsed();
            (to_rgba(&img)?, Renderer::Cpu, composed)
        };
        RENDER_TIMES.note(started.elapsed(), out.2);
        Ok((out.0, out.1))
    }

    /// The GPU path: FilmCraft's export compositor rendering one frame. Any failure or panic
    /// turns the GPU off for the session (seen on emulators: wgpu's GL fallback panics), and the
    /// caller renders on the CPU reference path instead.
    fn render_gpu(&self, snap: &Snapshot, t: Tick, opts: filmcraft_render::RenderOptions) -> Option<filmcraft_render::Image> {
        if !self.gpu_wanted.load(Ordering::Relaxed) || self.gpu_failed.load(Ordering::Relaxed) {
            return None;
        }
        let mut slot = self.gpu.lock().unwrap_or_else(PoisonError::into_inner);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if slot.is_none() {
                *slot = filmcraft_gpu::ExportRenderer::new();
            }
            slot.as_mut().and_then(|r| r.render(&snap.project, snap.seq, t, opts, &snap.provider))
        }));
        match result {
            Ok(Some(img)) if img.w > 0 && img.h > 0 => Some(img),
            _ => {
                *slot = None;
                self.gpu_failed.store(true, Ordering::Relaxed);
                None
            }
        }
    }

    /// Frames of media item `item` at media times `times`, each scaled to `height` pixels high:
    /// the timeline's filmstrip and the media bin's posters. A time that cannot be decoded gives
    /// a black frame, so the strip keeps its length.
    pub fn thumbnails(&self, item: u64, times: &[Tick], height: u32) -> Result<Vec<Rgba>, String> {
        if height == 0 || height > 1024 {
            return Err("height must be 1..=1024".into());
        }
        if times.len() > 256 {
            return Err("at most 256 thumbnails per call".into());
        }
        let src = self.thumb_source(item)?;
        let info = src.info();
        if !info.has_video() {
            return Err(format!("item {item} has no picture"));
        }
        // Colour-managed as the Program monitor is: HDR (PQ / HLG) media tone-mapped into SDR
        // BT.709, so a phone's HDR clip does not come out washed white and colourless.
        let project = self.lock().project.clone();
        let mut out = Vec::with_capacity(times.len());
        for &t in times {
            let frame = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| src.video_frame(filmcraft_media::FrameRequest { time: t, scale: 0.25 })));
            let pic = match frame {
                Ok(Ok(f)) => frame_rgba(&project, ItemId(item), &f, height),
                _ => None,
            };
            out.push(pic.unwrap_or_else(|| black(height * 16 / 9, height)));
        }
        Ok(out)
    }

    /// The filmstrip's own source for `item` (see `thumb_sources`), or the shared one when the
    /// file cannot be opened twice (image sequences, offline media).
    fn thumb_source(&self, item: u64) -> Result<filmcraft_media::SharedSource, String> {
        let (project, epoch, media, services) = {
            let s = self.lock();
            (s.project.clone(), s.document_epoch, s.media.clone(), s.services.clone())
        };
        let mut cache = self.thumb_sources.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.epoch != Some(epoch) {
            *cache = ThumbSources { epoch: Some(epoch), sources: Default::default() };
        }
        if let Some(src) = cache.sources.get(&item) {
            return Ok(src.clone());
        }
        match media.own_source(&project, ItemId(item), &*services) {
            Some(src) => {
                if cache.sources.len() >= MAX_THUMB_SOURCES {
                    cache.sources.clear();
                }
                cache.sources.insert(item, src.clone());
                Ok(src)
            }
            None => {
                drop(cache);
                self.lock().source(ItemId(item)).ok_or_else(|| format!("item {item} has no media source"))
            }
        }
    }

    /// Waveform of media item `item`'s audio stream `stream` as `buckets` (min, max) pairs over
    /// media time `from`..`to`. Silence or no audio gives zeros.
    pub fn peaks(&self, item: u64, stream: usize, from: Tick, to: Tick, buckets: usize) -> Result<Vec<(f32, f32)>, String> {
        if buckets == 0 || buckets > MAX_PEAK_BUCKETS {
            return Err(format!("buckets must be 1..={MAX_PEAK_BUCKETS}"));
        }
        if to <= from {
            return Ok(vec![(0.0, 0.0); buckets]);
        }
        let src = self.lock().source(ItemId(item)).ok_or_else(|| format!("item {item} has no media source"))?;
        if !src.info().has_audio() {
            return Ok(vec![(0.0, 0.0); buckets]);
        }
        // Read at a low rate: peaks only need the envelope, and a phone reads less.
        let sr = 8_000u32;
        let a = from.to_units_floor(i64::from(sr)).max(0);
        let b = to.to_units_floor(i64::from(sr)).max(a + 1);
        let total = usize::try_from(b - a).unwrap_or(0).min(sr as usize * 60 * 60 * 4);
        let mut out = vec![(0f32, 0f32); buckets];
        let chunk = sr as usize * 10;
        let mut done = 0usize;
        while done < total {
            let n = chunk.min(total - done);
            let start = a + done as i64;
            let buf = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| src.audio_stream(stream, start, n, sr)));
            let Ok(Ok(buf)) = buf else { break };
            for (i, ch0) in buf.channels.first().map(|c| c.as_slice()).unwrap_or(&[]).iter().enumerate() {
                let k = ((done + i) * buckets / total.max(1)).min(buckets - 1);
                let v = buf.channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).fold(*ch0, |m, x| if x.abs() > m.abs() { x } else { m });
                let slot = &mut out[k];
                slot.0 = slot.0.min(v);
                slot.1 = slot.1.max(v);
            }
            done += n;
        }
        Ok(out)
    }

    /// The program mix of the active sequence: `frames` stereo frames from sample `start` at the
    /// sequence's sample rate, interleaved L R. Returns `(sample_rate, samples)`.
    pub fn mix(&self, start: i64, frames: usize) -> Result<(u32, Vec<f32>), String> {
        if frames == 0 || frames > MAX_MIX_FRAMES {
            return Err(format!("frames must be 1..={MAX_MIX_FRAMES}"));
        }
        let (project, seq, provider, previews, sr) = {
            let g = self.lock();
            let seq = g.state.active_sequence.ok_or("no sequence is open")?;
            let sr = g.project.sequence(seq).ok_or("no sequence is open")?.settings.sample_rate;
            let project = g.project.clone();
            let provider = g.media.provider(project.clone(), g.services.clone());
            (project, seq, provider, g.previews.clone(), sr)
        };
        let out = filmcraft_engine::previews::MixOutput { layout: filmcraft_audio_dsp::channels::Layout::Stereo, mixdown: Default::default() };
        let mix = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| previews.mix_layout(&project, seq, start, frames, &provider, out)))
            .map_err(|_| "mixing panicked".to_string())?;
        let l = mix.channels.first().cloned().unwrap_or_default();
        let r = mix.channels.get(1).cloned().unwrap_or_else(|| l.clone());
        let mut inter = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            inter.push(l.get(i).copied().unwrap_or(0.0));
            inter.push(r.get(i).copied().unwrap_or(0.0));
        }
        Ok((sr, inter))
    }
}

fn to_rgba(img: &filmcraft_render::Image) -> Result<Rgba, String> {
    let (Ok(width), Ok(height)) = (u32::try_from(img.w), u32::try_from(img.h)) else { return Err("bad frame size".into()) };
    if width == 0 || height == 0 {
        return Err("the renderer returned an empty frame".into());
    }
    Ok(Rgba { width, height, rgba: img.over_black_rgba8() })
}

fn black(width: u32, height: u32) -> Rgba {
    let mut rgba = vec![0u8; width.max(1) as usize * height.max(1) as usize * 4];
    for px in rgba.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    Rgba { width: width.max(1), height: height.max(1), rgba }
}

/// A decoded frame as opaque sRGB RGBA8, scaled (box decimation, then nearest) to `height`.
fn frame_rgba(project: &filmcraft_project::Project, item: ItemId, f: &filmcraft_frame::VideoFrame, height: u32) -> Option<Rgba> {
    if f.width == 0 || f.height == 0 {
        return None;
    }
    let n = (f.height / height.max(1)).clamp(1, 16) as usize;
    let pipe = filmcraft_color::ColorPipeline::REC709;
    let mut img = filmcraft_render::colorman::decode(project, item, f, n, &pipe);
    filmcraft_render::colorman::to_display(&mut img, &pipe);
    let (w, h) = (img.w, img.h);
    if w == 0 || h == 0 {
        return None;
    }
    let full = img.over_black_rgba8();
    // Display aspect: pixel aspect ratio applied to the width.
    let (pn, pd) = (f.par.0.max(1), f.par.1.max(1));
    let disp_w = (w as f64 * f64::from(pn) / f64::from(pd)) * (f64::from(height) / h as f64);
    let ow = (disp_w.round() as u32).clamp(1, 4096);
    let oh = height;
    let mut rgba = vec![0u8; ow as usize * oh as usize * 4];
    for y in 0..oh as usize {
        let sy = (y * h / oh as usize).min(h - 1);
        for x in 0..ow as usize {
            let sx = (x * w / ow as usize).min(w - 1);
            let s = (sy * w + sx) * 4;
            let d = (y * ow as usize + x) * 4;
            if let (Some(src), Some(dst)) = (full.get(s..s + 4), rgba.get_mut(d..d + 4)) {
                dst.copy_from_slice(src);
            }
        }
    }
    Some(Rgba { width: ow, height: oh, rgba })
}

/// Frame-render timings, logged every couple of seconds of activity (logcat on Android): how long
/// renders take and how much of it is decoding, from FilmCraft's own decoder counters.
static RENDER_TIMES: RenderTimes = RenderTimes { inner: Mutex::new(None) };

struct RenderTimes {
    inner: Mutex<Option<RenderWindow>>,
}

struct RenderWindow {
    since: std::time::Instant,
    renders: u32,
    total: Duration,
    composed: Duration,
    gop: filmcraft_codecs::gop::GopStats,
}

impl RenderTimes {
    fn note(&self, total: Duration, composed: Duration) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let w = g.get_or_insert_with(|| RenderWindow { since: std::time::Instant::now(), renders: 0, total: Duration::ZERO, composed: Duration::ZERO, gop: filmcraft_codecs::gop::gop_stats() });
        w.renders += 1;
        w.total += total;
        w.composed += composed;
        let span = w.since.elapsed();
        if span < Duration::from_secs(2) {
            return;
        }
        let now = filmcraft_codecs::gop::gop_stats();
        let n = f64::from(w.renders.max(1));
        log::info!(
            "renders: {:.1}/s, {:.1} ms each ({:.1} ms compose incl. decode, {:.1} ms readback); decoders: {} samples ({} seeks, {} hits, {} misses) {:.0} ms",
            n / span.as_secs_f64(),
            w.total.as_secs_f64() * 1000.0 / n,
            w.composed.as_secs_f64() * 1000.0 / n,
            (w.total - w.composed).as_secs_f64() * 1000.0 / n,
            now.decoded - w.gop.decoded,
            now.seeks - w.gop.seeks,
            now.hits - w.gop.hits,
            now.misses - w.gop.misses,
            (now.decode_ns - w.gop.decode_ns) as f64 / 1e6,
        );
        *g = None;
    }
}
