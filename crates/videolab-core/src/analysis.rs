//! Background analysis for effects that look at the whole clip before they can draw a frame:
//! Warp Stabilizer (tracks the camera through every frame of the clip) and Auto Reframe (finds
//! the subject in the frames around each one). FilmCraft runs that analysis inside the first
//! render that needs it and caches the result. Inside playback that does not work: the analysis
//! of a 20-second 4K clip takes seconds, playback cancels renders that fall behind, so it is
//! cancelled and restarted frame after frame, never finishes, and playback halts.
//!
//! So previews and playback draw a clip without those effects until its analysis is done
//! ([`Analyses::strip_unready`]), and one background worker does the analysis: it renders the
//! clip the way FilmCraft's own render would (one frame for the stabilizer, which analyses the
//! whole clip; every frame, small, for reframing), which fills FilmCraft's caches, then the
//! effect shows. The worker reads its own copies of the media files (never seeking the decoder
//! playback reads) and waits while playback runs, so it only uses the time between plays.
//! Exports are not affected: they render every effect themselves.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use filmcraft_frame::{AudioBuffer, VideoFrame};
use filmcraft_media::{FrameRequest, MediaInfo, MediaSource, SharedSource};
use filmcraft_project::{ItemId, Project, TimeInterpolation, TrackItem};

type SequenceId = ItemId;
use filmcraft_time::Tick;

/// Effects analysed in the background.
pub const ANALYSED: [&str; 2] = ["warp_stabilizer", "auto_reframe"];

/// Playback counts as running this long after its last frame.
const PLAYING_GRACE: Duration = Duration::from_millis(600);

/// A clip's analysis identity: what FilmCraft keys its caches on (the clip's media, place, length,
/// source in, speed, direction) plus the effect and its analysis settings.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    item: u64,
    start: i64,
    duration: i64,
    source_in: i64,
    speed: u64,
    reverse: bool,
    effect: &'static str,
    settings: String,
}

impl Key {
    fn of(it: &TrackItem, effect: &'static str) -> Self {
        let settings = it
            .effects
            .iter()
            .find(|e| e.effect == effect)
            .map(|e| ["method", "detailed", "preset"].iter().map(|p| format!("{:?}", e.params.get(*p).map(|x| &x.value))).collect::<Vec<_>>().join(","))
            .unwrap_or_default();
        Self { item: it.item.0, start: it.start.0, duration: it.duration.0, source_in: it.source_in.0, speed: it.speed.to_bits(), reverse: it.reverse, effect, settings }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Pending,
    Done,
    Failed,
}

struct Job {
    key: Key,
    project: Arc<Project>,
    seq: SequenceId,
    clip_start: Tick,
    clip_end: Tick,
    pool: Arc<filmcraft_engine::media_pool::MediaPool>,
    services: Arc<dyn filmcraft_engine::Services>,
}

#[derive(Default)]
pub struct Analyses {
    state: Mutex<HashMap<Key, State>>,
    /// Milliseconds since [`epoch`] of the last playback frame (0: none yet).
    last_playback: AtomicU64,
    worker: OnceLock<Mutex<Sender<Job>>>,
    /// The last stripped copy: the project it came from, what was stripped, and the copy. Renders
    /// reuse it while neither changes (playback renders 30+ frames a second; cloning the project
    /// for each would cost exactly while playback must stay smooth). Holding the source `Arc`
    /// also makes any edit copy the session's project (`Arc::make_mut`), so a changed project
    /// never matches by pointer.
    stripped: Mutex<Option<Stripped>>,
}

/// A stripped render snapshot and what it was made from.
struct Stripped {
    source: Arc<Project>,
    unready: Vec<(u64, &'static str)>,
    /// Whether optical-flow slow motion was drawn with frame sampling (playback).
    sampled: bool,
    copy: Arc<Project>,
}

thread_local! {
    static QUICK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` as a quick render (playback, scrubbing): see [`Analyses::strip_unready`].
pub fn with_quick<R>(f: impl FnOnce() -> R) -> R {
    let before = QUICK.with(|q| q.replace(true));
    let out = f();
    QUICK.with(|q| q.set(before));
    out
}

pub(crate) fn quick() -> bool {
    QUICK.with(std::cell::Cell::get)
}

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

impl Analyses {
    /// A playback frame is being rendered: the worker waits.
    pub fn note_playback(&self) {
        self.last_playback.store(epoch().elapsed().as_millis() as u64 + 1, Ordering::Relaxed);
    }

    pub fn playing(&self) -> bool {
        let last = self.last_playback.load(Ordering::Relaxed);
        last != 0 && epoch().elapsed().as_millis() as u64 + 1 - last < PLAYING_GRACE.as_millis() as u64
    }

    /// How many analyses are queued or running.
    pub fn pending(&self) -> usize {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).values().filter(|s| **s == State::Pending).count()
    }

    /// `project` (a render snapshot) without the analysed effects of clips whose analysis is not
    /// done, queueing their analysis; for a quick render ([`with_quick`]: playback, a scrub),
    /// also with optical-flow slow motion drawn by frame sampling. Unchanged (no copy) when
    /// neither applies.
    ///
    /// Optical flow estimates motion between two source frames for every frame it draws: on 4K
    /// phone footage that is hundreds of milliseconds a frame, so playback crawled at 2 to 8 fps.
    /// Playback and scrubbing draw such clips as Premiere does before they are rendered (frame
    /// sampling); the frame a scrub stops on, paused frames and exports keep optical flow.
    pub fn strip_unready(
        self: &Arc<Self>,
        project: Arc<Project>,
        seq: SequenceId,
        pool: &Arc<filmcraft_engine::media_pool::MediaPool>,
        services: &Arc<dyn filmcraft_engine::Services>,
    ) -> Arc<Project> {
        let Some(q) = project.sequence(seq) else { return project };
        let mut unready: Vec<(u64, &'static str)> = Vec::new();
        let mut jobs = Vec::new();
        {
            let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            for it in q.all_tracks().flat_map(|t| t.items.iter()) {
                for effect in ANALYSED {
                    if !it.effects.iter().any(|e| e.effect == effect && e.enabled) {
                        continue;
                    }
                    let key = Key::of(it, effect);
                    match st.get(&key) {
                        // Failed: draw it as FilmCraft would (analysing inline), so the preview
                        // matches the export rather than hiding the effect for the session.
                        Some(State::Done | State::Failed) => {}
                        Some(State::Pending) => unready.push((it.id.0, effect)),
                        None => {
                            st.insert(key.clone(), State::Pending);
                            unready.push((it.id.0, effect));
                            if st.len() > 512 {
                                st.retain(|_, s| *s == State::Pending);
                            }
                            jobs.push(Job { key, project: project.clone(), seq, clip_start: it.start, clip_end: it.end(), pool: pool.clone(), services: services.clone() });
                        }
                    }
                }
            }
        }
        // Queued with the state unlocked: a job that cannot be queued is marked failed.
        for job in jobs {
            self.queue(job);
        }
        let sampled = quick() && q.all_tracks().flat_map(|t| t.items.iter()).any(|it| it.time_interpolation == TimeInterpolation::OpticalFlow);
        if unready.is_empty() && !sampled {
            *self.stripped.lock().unwrap_or_else(PoisonError::into_inner) = None;
            return project;
        }
        if let Some(c) = self.stripped.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
            && Arc::ptr_eq(&c.source, &project)
            && c.unready == unready
            && c.sampled == sampled
        {
            return c.copy.clone();
        }
        let mut p = (*project).clone();
        if let Some(q) = p.sequence_mut(seq) {
            for t in q.all_tracks_mut() {
                for it in &mut t.items {
                    if sampled && it.time_interpolation == TimeInterpolation::OpticalFlow {
                        it.time_interpolation = TimeInterpolation::FrameSampling;
                    }
                    if unready.iter().any(|(c, _)| *c == it.id.0) {
                        it.effects.retain(|e| !unready.iter().any(|(c, eff)| *c == it.id.0 && e.effect == *eff));
                    }
                }
            }
        }
        let copy = Arc::new(p);
        *self.stripped.lock().unwrap_or_else(PoisonError::into_inner) = Some(Stripped { source: project, unready, sampled, copy: copy.clone() });
        copy
    }

    fn queue(self: &Arc<Self>, job: Job) {
        let tx = self.worker.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            let me = Arc::downgrade(self);
            let _ = std::thread::Builder::new().name("videolab-analysis".into()).spawn(move || {
                while let Ok(job) = rx.recv() {
                    let Some(me) = me.upgrade() else { break };
                    let key = job.key.clone();
                    let started = Instant::now();
                    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| me.run(&job))).unwrap_or(false);
                    log::info!("analysed {} for item {} in {:.1} s: {}", key.effect, key.item, started.elapsed().as_secs_f64(), if ok { "done" } else { "failed" });
                    me.state.lock().unwrap_or_else(PoisonError::into_inner).insert(key, if ok { State::Done } else { State::Failed });
                }
            });
            Mutex::new(tx)
        });
        // No worker (its thread could not start): the job cannot run, so it is not waited for.
        if let Err(std::sync::mpsc::SendError(job)) = tx.lock().unwrap_or_else(PoisonError::into_inner).send(job) {
            log::warn!("analysis worker unavailable: {} drawn as FilmCraft does", job.key.effect);
            self.state.lock().unwrap_or_else(PoisonError::into_inner).insert(job.key, State::Failed);
        }
    }

    /// Render the clip as FilmCraft would, small, so its analysis lands in FilmCraft's caches.
    fn run(self: &Arc<Self>, job: &Job) -> bool {
        let Some(q) = job.project.sequence(job.seq) else { return false };
        let long = q.settings.width.max(q.settings.height).max(1) as f32;
        let opts = filmcraft_render::RenderOptions { scale: (160.0 / long).min(1.0), captions: false, ..Default::default() };
        let inner = job.pool.full_res_provider(job.project.clone(), job.services.clone()).own_sources();
        let gate = self.clone();
        let provider = move |item: ItemId| -> Option<SharedSource> {
            use filmcraft_render::SourceProvider;
            inner.source(item).map(|s| Arc::new(Waiting { inner: s, gate: gate.clone() }) as SharedSource)
        };
        let frames: Vec<Tick> = match job.key.effect {
            // the stabilizer analyses the whole clip for its first frame
            "warp_stabilizer" => vec![Tick((job.clip_start.0 + job.clip_end.0) / 2)],
            // reframing looks at the frames around each one: every frame fills its cache
            _ => {
                let fd = q.settings.frame_rate.frame_duration().0.max(1);
                (job.clip_start.0..job.clip_end.0).step_by(fd as usize).map(Tick).collect()
            }
        };
        filmcraft_media::cancel::with_background(true, || {
            for t in frames {
                let _ = filmcraft_render::render_sequence(&job.project, job.seq, t, opts, &provider);
            }
        });
        true
    }
}

/// A media source that holds each frame request while playback runs.
struct Waiting {
    inner: SharedSource,
    gate: Arc<Analyses>,
}

impl Waiting {
    fn wait(&self) {
        while self.gate.playing() {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl MediaSource for Waiting {
    fn info(&self) -> &MediaInfo {
        self.inner.info()
    }
    fn video_frame(&self, req: FrameRequest) -> filmcraft_media::Result<Arc<VideoFrame>> {
        self.wait();
        self.inner.video_frame(req)
    }
    fn video_frame_unturned(&self, req: FrameRequest) -> filmcraft_media::Result<(Arc<VideoFrame>, u8)> {
        self.wait();
        self.inner.video_frame_unturned(req)
    }
    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> filmcraft_media::Result<AudioBuffer> {
        self.inner.audio(start, frames, sample_rate)
    }
    fn audio_stream(&self, stream: usize, start: i64, frames: usize, sample_rate: u32) -> filmcraft_media::Result<AudioBuffer> {
        self.inner.audio_stream(stream, start, frames, sample_rate)
    }
}
