//! The part of the core every surface shares: registry + shared state, the swappable event sink,
//! `DocChanged` detection and running scripts with their cancel flags (adapted from ImageLab's
//! hub: the document's identity is FilmCraft's `(revision, playhead, active sequence)`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::Duration;

use lab_api::{ApiResult, Ctx, Event, EventSink, NoEvents, Registry, Shared, Source};
use lab_script::{Budget, ScriptOutcome, ScriptRuntime};
use serde_json::Value;

use crate::engine::Engine;

/// How often the watcher looks for edits that arrived without a source (the control channel's
/// external clients: `filmcraft-cli --bridge` over adb, a laptop MCP bridge).
const WATCH_INTERVAL: Duration = Duration::from_millis(200);

/// An [`EventSink`] whose target can be replaced at any time. A sink that panics loses that
/// event, nothing more.
pub(crate) struct ForwardingSink {
    inner: RwLock<Arc<dyn EventSink>>,
}

impl Default for ForwardingSink {
    fn default() -> Self {
        Self { inner: RwLock::new(Arc::new(NoEvents)) }
    }
}

impl ForwardingSink {
    pub(crate) fn set(&self, sink: Arc<dyn EventSink>) {
        *self.inner.write().unwrap_or_else(PoisonError::into_inner) = sink;
    }
}

impl EventSink for ForwardingSink {
    fn emit(&self, event: Event) {
        let sink = self.inner.read().unwrap_or_else(PoisonError::into_inner).clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.emit(event)));
    }
}

/// Decrements the in-flight counter when dropped.
pub(crate) struct Busy<'a>(&'a AtomicUsize);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// What the UI redraws for: the edit revision, the playhead and the open sequence.
type Mark = (u64, i64, Option<u64>);

pub(crate) struct Core {
    pub registry: Registry,
    pub shared: Shared,
    pub engine: Arc<Engine>,
    pub sink: Arc<ForwardingSink>,
    seen: Arc<Mutex<Option<Mark>>>,
    /// Calls in flight inside the core; the watcher stays quiet while any run, so an edit is
    /// attributed to the surface that made it.
    in_flight: AtomicUsize,
    running: Mutex<Vec<Arc<AtomicBool>>>,
}

fn mark(engine: &Engine) -> Mark {
    let g = engine.lock();
    (g.revision, g.playhead().0, g.state.active_sequence.map(|s| s.0))
}

fn report(seen: &Mutex<Option<Mark>>, shared: &Shared, engine: &Arc<Engine>, current: Mark, source: Source) {
    let mut s = seen.lock().unwrap_or_else(PoisonError::into_inner);
    if *s == Some(current) {
        return;
    }
    *s = Some(current);
    drop(s);
    // New heavy media (imported, or a project opened) gets a proxy in the background.
    engine.proxies.nudge(engine);
    shared.events.emit(Event::DocChanged { doc: current.2.unwrap_or(0) as usize, revision: current.0, source });
}

impl Core {
    pub(crate) fn new(mut registry: Registry, shared: Shared, engine: Arc<Engine>, sink: Arc<ForwardingSink>) -> Self {
        let seen = Arc::new(Mutex::new(Some(mark(&engine))));
        // Live preview: every editing call (each step of a script included) announces its change
        // as soon as it returns.
        let (s2, e2) = (seen.clone(), engine.clone());
        registry.set_after_mutation(Arc::new(move |ctx: &Ctx| report(&s2, &ctx.shared, &e2, mark(&e2), ctx.source)));
        Self { registry, shared, engine, sink, seen, in_flight: AtomicUsize::new(0), running: Mutex::new(Vec::new()) }
    }

    pub(crate) fn busy(&self) -> Busy<'_> {
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        Busy(&self.in_flight)
    }

    pub(crate) fn call(&self, id: &str, params: Value, source: Source) -> ApiResult<Value> {
        let _busy = self.busy();
        let ctx = Ctx::new(self.shared.clone(), source);
        let result = self.registry.call(&ctx, id, params);
        self.notice(source);
        result
    }

    pub(crate) fn run_script(&self, code: &str, read_only: bool, timeout: Duration, source: Source, cancel: Arc<AtomicBool>) -> ScriptOutcome {
        let _busy = self.busy();
        let mut ctx = Ctx::new(self.shared.clone(), source);
        ctx.cancel = cancel.clone();
        if read_only {
            ctx = ctx.read_only();
        }
        self.running.lock().unwrap_or_else(PoisonError::into_inner).push(cancel.clone());
        let outcome = ScriptRuntime::run(&self.registry, &ctx, code, Budget::with_wall(timeout));
        self.running.lock().unwrap_or_else(PoisonError::into_inner).retain(|c| !Arc::ptr_eq(c, &cancel));
        self.notice(source);
        outcome
    }

    pub(crate) fn cancel_scripts(&self) {
        for flag in self.running.lock().unwrap_or_else(PoisonError::into_inner).iter() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Emit `DocChanged` attributed to `source` if the document, playhead or sequence moved.
    pub(crate) fn notice(&self, source: Source) {
        report(&self.seen, &self.shared, &self.engine, mark(&self.engine), source);
    }

    /// [`Core::notice`] that gives up when the session is busy (the watcher).
    fn try_notice(&self, source: Source) {
        let current = match self.engine.session.try_lock() {
            Ok(g) => (g.revision, g.playhead().0, g.state.active_sequence.map(|s| s.0)),
            Err(_) => return,
        };
        report(&self.seen, &self.shared, &self.engine, current, source);
    }
}

/// Notices edits that reach the session from outside a core call (the control channel's
/// clients) and applies background exports' steps. Runs for the core's lifetime.
pub(crate) struct Watcher {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    pub(crate) fn start(core: Weak<Core>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("videolab-watch".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(WATCH_INTERVAL);
                    let Some(core) = core.upgrade() else { break };
                    if let Ok(mut g) = core.engine.session.try_lock()
                        && !g.stepped.is_empty()
                    {
                        g.pump_jobs(Duration::from_millis(20));
                    }
                    if core.in_flight.load(Ordering::Acquire) == 0 {
                        core.try_notice(Source::Mcp);
                    }
                }
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
