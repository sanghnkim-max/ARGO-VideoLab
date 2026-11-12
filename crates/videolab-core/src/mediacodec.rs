//! Hardware H.264 / HEVC decoding on Android through MediaCodec, behind FilmCraft's decoder seam.
//!
//! FilmCraft's desktop builds put the OS decoder in front of its own pure-Rust ones
//! (`filmcraft_codecs::register_video_decoder`): VideoToolbox on macOS, Media Foundation on
//! Windows, NVDEC / VA-API on Linux (filmcraft-platform). This is the same thing for Android:
//! [`factory`] builds a [`MediaCodecDecoder`] for `avcC` / `hvcC` streams and wraps it in
//! filmcraft-platform's [`HybridDecoder`], which replays into FilmCraft's software decoder if the
//! hardware fails mid-stream. Streams MediaCodec cannot open (profile, size, 4:2:2, 10-bit here)
//! are declined and keep the software decoder.
//!
//! MediaCodec decodes into its own output buffers (ByteBuffer mode, `COLOR_FormatYUV420Flexible`):
//! plain I420 or NV12 in memory the app can read. Not a surface: on Qualcomm the decoder fills an
//! `AImageReader` with its tiled, compressed UBWC layout (format `0x7FA30C06`) whatever format the
//! reader asks for, which the CPU cannot read. Each picture's planes (laid out as the format's
//! `image-data` says: I420 or NV12, any stride and slice height) are cropped and copied into the
//! planar `Yuv8` frame the software decoder would return, with the stream's own colour and pixel
//! aspect, so the compositor (GPU or CPU) cannot tell the two apart.
//!
//! Uses the `ndk` crate's safe API. The one `unsafe` is `Send` for the decoder (see there), the
//! same allowance FilmCraft makes for its platform FFI modules (docs/adr/0001-platform-ffi.md).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use filmcraft_codecs::hw::{NalCodec, NalStreamInfo};
use filmcraft_codecs::{CodecError, DecodedFrame, Result, VideoDecoder};
use filmcraft_frame::{Chroma, PixelData, VideoFrame};
use filmcraft_platform::HybridDecoder;
use ndk::media::media_codec::{DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection, OutputBuffer};
use ndk::media::media_format::MediaFormat;

const BUFFER_FLAG_END_OF_STREAM: u32 = 4;
const BUFFER_FLAG_CODEC_CONFIG: u32 = 2;
/// `MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible`.
const COLOR_FORMAT_YUV420_FLEXIBLE: i32 = 0x7F42_0888;
/// `COLOR_FormatYUV420SemiPlanar` (NV12) and `COLOR_QCOM_FormatYUV420SemiPlanar`: chroma
/// interleaved. Used only when the format has no `image-data`; then a flexible format is taken as
/// NV12 (what hardware decoders produce) and anything else as I420.
const SEMI_PLANAR: [i32; 2] = [21, 0x7FA3_0C00];
/// Offset added to pts so MediaCodec's unsigned microsecond timestamps carry negative pts too.
const PTS_OFFSET: i64 = 1 << 40;
/// Inputs in flight before decode() waits for a picture (bounds latency and memory).
const MAX_IN_FLIGHT: usize = 6;

/// `VIDEOLAB_MC_DEBUG=1`: trace codec events to stderr (the CLI's bench-decode).
fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VIDEOLAB_MC_DEBUG").is_some())
}

macro_rules! trace {
    ($($a:tt)*) => { if debug() { eprintln!("[mediacodec] {}", format!($($a)*)); } };
}

/// Set once a hardware session produced a picture on this device.
static WORKED: AtomicBool = AtomicBool::new(false);
/// Set when a session failed without ever producing a picture: this device's MediaCodec does not
/// work for us (a vendor quirk, a sandbox rule), so later streams go straight to software instead
/// of paying the failed attempt again.
static BROKEN: AtomicBool = AtomicBool::new(false);

/// Whether MediaCodec decoded on this device (`None`: not tried yet).
pub fn status() -> Option<bool> {
    if WORKED.load(Ordering::Relaxed) {
        Some(true)
    } else if BROKEN.load(Ordering::Relaxed) {
        Some(false)
    } else {
        None
    }
}

/// The factory registered with `filmcraft_codecs::register_video_decoder` on Android.
pub fn factory(entry: &filmcraft_isobmff::SampleEntry) -> Option<Result<Box<dyn VideoDecoder>>> {
    if !filmcraft_codecs::hw::hardware_decoding() || (BROKEN.load(Ordering::Relaxed) && !WORKED.load(Ordering::Relaxed)) {
        return None;
    }
    let info = NalStreamInfo::from_entry(entry)?.ok()?;
    match std::panic::catch_unwind(|| MediaCodecDecoder::new(info.clone())) {
        Ok(Ok(d)) => Some(Ok(Box::new(HybridDecoder::new(Box::new(d), entry.clone(), info)))),
        Ok(Err(why)) => {
            log::info!("MediaCodec declined {} video: {why}", entry.codec.name());
            filmcraft_codecs::hw::note_hw_declined();
            None
        }
        Err(_) => {
            filmcraft_codecs::hw::note_hw_declined();
            None
        }
    }
}

/// Register the MediaCodec factory in front of FilmCraft's own decoders (idempotent).
pub fn register() {
    // Logs (this module's warnings, FilmCraft's) to logcat under "videolab".
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("videolab"));
    filmcraft_codecs::register_video_decoder(factory);
    filmcraft_codecs::hw::set_hw_backend("MediaCodec");
}

/// Start this process's binder thread pool. MediaCodec's codecs run in the media service and
/// hand results back over binder: without threads to receive them a codec takes input and never
/// returns a picture or packet. App processes have the pool already (the Java runtime starts it);
/// a plain native process — the `videolab` CLI run from adb — must start it before using a codec.
/// `ABinderProcess_startThreadPool` is on every device but not in the NDK's link stubs, so it is
/// looked up at run time; when it is missing the CLI just keeps FilmCraft's software codecs.
pub fn start_binder_thread_pool() {
    // SAFETY: dlopen/dlsym with NUL-terminated names; the symbol, when found, is libbinder_ndk's
    // `void ABinderProcess_startThreadPool(void)`, callable from any thread (again: a no-op).
    #[allow(unsafe_code)]
    unsafe {
        let lib = libc::dlopen(c"libbinder_ndk.so".as_ptr(), libc::RTLD_NOW);
        if lib.is_null() {
            log::warn!("libbinder_ndk.so not found: hardware codecs will not answer this process");
            return;
        }
        let f = libc::dlsym(lib, c"ABinderProcess_startThreadPool".as_ptr());
        if f.is_null() {
            log::warn!("ABinderProcess_startThreadPool not found: hardware codecs will not answer this process");
            return;
        }
        let start: extern "C" fn() = std::mem::transmute(f);
        start();
    }
}

pub struct MediaCodecDecoder {
    codec: MediaCodec,
    info: NalStreamInfo,
    in_flight: usize,
    /// Where the planes are in an output buffer; from the codec's output format.
    layout: Option<Layout>,
    /// Pictures decoded but not yet returned, by pts (MediaCodec returns them in presentation order).
    ready: BTreeMap<i64, DecodedFrame>,
    name: String,
    /// Samples queued since start or the last flush (a flush of an idle codec is skipped).
    fed: bool,
    /// The parameter sets must be queued again (as a codec-config buffer) before the next
    /// sample: MediaCodec forgets the configured csd when flushed before its first output.
    need_config: bool,
}

/// An output buffer's picture layout: per plane (Y, Cb, Cr) the byte offset, the step between
/// samples and the step between rows, plus the visible rectangle.
#[derive(Clone, Copy, Debug)]
struct Layout {
    planes: [(usize, usize, usize); 3],
    /// Visible picture: x, y, width, height.
    crop: (usize, usize, usize, usize),
}

impl Layout {
    /// From the codec's output format: `image-data` (the platform's `MediaImage2`, which states
    /// every plane exactly) when present, else `stride` / `slice-height` / `color-format`.
    fn from_format(f: &MediaFormat, info: &NalStreamInfo) -> Option<Self> {
        let get = |k: &str| f.i32(k).and_then(|v| usize::try_from(v).ok());
        let width = get("width")?;
        let height = get("height")?;
        let planes = match f.buffer("image-data").and_then(media_image_planes) {
            Some(p) => p,
            None => {
                let s = get("stride").filter(|s| *s >= width).unwrap_or(width);
                let sh = get("slice-height").filter(|v| *v >= height).unwrap_or(height);
                if SEMI_PLANAR.contains(&f.i32("color-format").unwrap_or(21)) || f.i32("color-format") == Some(COLOR_FORMAT_YUV420_FLEXIBLE) {
                    [(0, 1, s), (s * sh, 2, s), (s * sh + 1, 2, s)]
                } else {
                    let (cs, csh) = (s.div_ceil(2), sh.div_ceil(2));
                    [(0, 1, s), (s * sh, 1, cs), (s * sh + cs * csh, 1, cs)]
                }
            }
        };
        let crop = match f.rect("crop") {
            Some((l, t, r, b)) if r >= l && b >= t => (l as usize, t as usize, (r - l + 1) as usize, (b - t + 1) as usize),
            _ => {
                let (x, y, w, h) = info.crop;
                (x as usize, y as usize, w as usize, h as usize)
            }
        };
        let (x, y) = (crop.0 & !1, crop.1 & !1);
        let w = crop.2.min(width.saturating_sub(x)) & !1;
        let h = crop.3.min(height.saturating_sub(y)) & !1;
        (w > 0 && h > 0).then_some(Self { planes, crop: (x, y, w, h) })
    }

    /// One picture as planar 8-bit 4:2:0 planes, cropped. `None` if `data` is too short.
    fn copy(&self, data: &[u8]) -> Option<[Vec<u8>; 3]> {
        let (x, y, w, h) = self.crop;
        let plane = |i: usize, x0: usize, y0: usize, w: usize, h: usize| -> Option<Vec<u8>> {
            let (base, step, stride) = self.planes[i];
            let mut out = vec![0u8; w * h];
            for r in 0..h {
                let at = base + (y0 + r) * stride + x0 * step;
                let dst = &mut out[r * w..(r + 1) * w];
                if step == 1 {
                    dst.copy_from_slice(data.get(at..at + w)?);
                } else {
                    let src = data.get(at..at + (w - 1) * step + 1)?;
                    for (k, d) in dst.iter_mut().enumerate() {
                        *d = src[k * step];
                    }
                }
            }
            Some(out)
        };
        let luma = plane(0, x, y, w, h)?;
        let (cx, cy, cw, ch) = (x / 2, y / 2, w / 2, h / 2);
        let ((ub, us, ur), (vb, vs, vr)) = (self.planes[1], self.planes[2]);
        if us == 2 && vs == 2 && ur == vr && vb == ub + 1 {
            // NV12 (what hardware decoders give): split each interleaved row in one pass.
            let mut u = vec![0u8; cw * ch];
            let mut v = vec![0u8; cw * ch];
            for r in 0..ch {
                let at = ub + (cy + r) * ur + cx * 2;
                let row = data.get(at..at + cw * 2)?;
                let (du, dv) = (&mut u[r * cw..(r + 1) * cw], &mut v[r * cw..(r + 1) * cw]);
                for ((pu, pv), pair) in du.iter_mut().zip(dv.iter_mut()).zip(row.as_chunks::<2>().0) {
                    *pu = pair[0];
                    *pv = pair[1];
                }
            }
            return Some([luma, u, v]);
        }
        Some([luma, plane(1, cx, cy, cw, ch)?, plane(2, cx, cy, cw, ch)?])
    }
}

/// The Y, Cb, Cr planes of a `MediaImage2` (8-bit YUV 4:2:0 only): offset, column and row
/// increments. Layout (native-endian 32-bit fields): type, numPlanes, width, height, bitDepth,
/// bitDepthAllocated, then per plane offset, colInc, rowInc, horizSubsampling, vertSubsampling.
fn media_image_planes(b: &[u8]) -> Option<[(usize, usize, usize); 3]> {
    let word = |i: usize| -> Option<u32> { Some(u32::from_ne_bytes(b.get(i * 4..i * 4 + 4)?.try_into().ok()?)) };
    const YUV: u32 = 1;
    if word(0)? != YUV || word(1)? < 3 || word(4)? != 8 || word(5)? != 8 {
        return None;
    }
    let plane = |p: usize| -> Option<(usize, usize, usize)> {
        let at = 6 + p * 5;
        let (offset, col, row, hs, vs) = (word(at)?, word(at + 1)?, word(at + 2)?, word(at + 3)?, word(at + 4)?);
        let want = if p == 0 { 1 } else { 2 };
        (hs == want && vs == want && col > 0 && row > 0).then_some((offset as usize, col as usize, row as usize))
    };
    Some([plane(0)?, plane(1)?, plane(2)?])
}

impl MediaCodecDecoder {
    pub fn new(info: NalStreamInfo) -> std::result::Result<Self, String> {
        if info.chroma_format_idc != 1 || info.bit_depth_luma != 8 || info.bit_depth_chroma != 8 {
            return Err("only 8-bit 4:2:0 is decoded in hardware here".into());
        }
        if info.interlaced {
            return Err("interlaced streams keep the software decoder".into());
        }
        let mime = match info.codec {
            NalCodec::H264 => "video/avc",
            NalCodec::Hevc => "video/hevc",
        };
        let codec = MediaCodec::from_decoder_type(mime).ok_or_else(|| format!("no {mime} decoder"))?;
        let name = codec.name().unwrap_or_else(|_| mime.to_string());
        let (w, h) = info.coded;
        let mut fmt = MediaFormat::new();
        fmt.set_str("mime", mime);
        fmt.set_i32("width", w as i32);
        fmt.set_i32("height", h as i32);
        // Release each picture as soon as it can be shown (no extra output buffering), and run at
        // playback speed rather than the codec's power-saving default.
        fmt.set_i32("low-latency", 1);
        fmt.set_i32("priority", 0);
        fmt.set_i32("operating-rate", 120);
        // Decoded pictures in ByteBuffers the app can read (see the module docs for why not a surface).
        fmt.set_i32("color-format", COLOR_FORMAT_YUV420_FLEXIBLE);
        // Parameter sets go in-band, as the first (codec-config) input buffer, not as csd-* in the
        // format: on some devices the format's csd travels through a shared-memory queue the codec
        // service may not open ("queueCSDInputBuffer failed w/ error 14" on the emulator).
        codec.configure(&fmt, None, MediaCodecDirection::Decoder).map_err(|e| format!("configure: {e:?}"))?;
        codec.start().map_err(|e| format!("start: {e:?}"))?;
        log::info!("started {name} {w}x{h} (buffer output)");
        Ok(Self { codec, info, in_flight: 0, layout: None, ready: BTreeMap::new(), name: format!("MediaCodec ({name})"), fed: false, need_config: true })
    }

    /// Length-prefixed NAL units → Annex-B with 4-byte start codes.
    fn to_annexb(&self, sample: &[u8]) -> Vec<u8> {
        let n = self.info.length_size;
        let mut out = Vec::with_capacity(sample.len() + 16);
        let mut i = 0;
        while i + n <= sample.len() {
            let len = sample[i..i + n].iter().fold(0usize, |a, b| (a << 8) | usize::from(*b));
            i += n;
            let end = (i + len).min(sample.len());
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&sample[i..end]);
            i = end;
        }
        out
    }

    fn queue(&mut self, data: &[u8], pts: i64, flags: u32) -> Result<()> {
        // Wait for an input buffer, collecting pictures meanwhile so the codec can make room.
        // A codec that has produced nothing and takes no input for ~0.5 s is not going to: fail
        // fast, so the hybrid decoder falls back to software before anyone notices.
        let tries = if WORKED.load(Ordering::Relaxed) { 200 } else { 30 };
        for _ in 0..tries {
            match self.codec.dequeue_input_buffer(Duration::from_millis(5)).map_err(err)? {
                DequeuedInputBufferResult::Buffer(mut buf) => {
                    let dst = buf.buffer_mut();
                    if dst.len() < data.len() {
                        return Err(CodecError::Decode(format!("sample of {} bytes exceeds MediaCodec's {}-byte input buffer", data.len(), dst.len())));
                    }
                    for (d, s) in dst.iter_mut().zip(data) {
                        d.write(*s);
                    }
                    let time = u64::try_from(pts.saturating_add(PTS_OFFSET)).unwrap_or(0);
                    if let Err(e) = self.codec.queue_input_buffer(buf, 0, data.len(), time, flags) {
                        if !WORKED.load(Ordering::Relaxed) {
                            BROKEN.store(true, Ordering::Relaxed);
                        }
                        return Err(err(e));
                    }
                    trace!("queued pts {pts} {} bytes flags {flags} in flight {}", data.len(), self.in_flight + 1);
                    if flags & BUFFER_FLAG_CODEC_CONFIG == 0 {
                        self.in_flight += 1;
                    }
                    return Ok(());
                }
                DequeuedInputBufferResult::TryAgainLater => {
                    trace!("no input buffer ({} in flight); waiting for output", self.in_flight);
                    self.drain(Duration::from_millis(10), false)?;
                }
            }
        }
        if !WORKED.load(Ordering::Relaxed) {
            BROKEN.store(true, Ordering::Relaxed);
            log::warn!("MediaCodec produced no picture; hardware decoding is off for this run");
        }
        Err(CodecError::Decode("MediaCodec gave no input buffer".into()))
    }

    /// Take decoded pictures: one wait of `wait`, then whatever is ready. With `until_eos`, keep
    /// going until the end-of-stream picture.
    fn drain(&mut self, wait: Duration, until_eos: bool) -> Result<bool> {
        let mut timeout = wait;
        let mut idle = 0;
        loop {
            match self.codec.dequeue_output_buffer(timeout).map_err(err)? {
                DequeuedOutputBufferInfoResult::Buffer(buf) => {
                    let info = *buf.info();
                    let pts = info.presentation_time_us() - PTS_OFFSET;
                    trace!("output pts {pts} size {} flags {}", info.size(), info.flags());
                    let eos = info.flags() & BUFFER_FLAG_END_OF_STREAM != 0;
                    self.in_flight = self.in_flight.saturating_sub(1);
                    let copied = if info.size() > 0 { Some(self.picture(&buf)) } else { None };
                    self.codec.release_output_buffer(buf, false).map_err(err)?;
                    match copied {
                        Some(Some(frame)) => {
                            WORKED.store(true, Ordering::Relaxed);
                            filmcraft_codecs::hw::note_hw_frames(1);
                            self.ready.insert(pts, DecodedFrame { pts, frame: VideoFrame { pts: filmcraft_time::Tick(pts), ..frame }, draft: false });
                        }
                        Some(None) => {
                            // A picture we cannot read: hand the stream to the software decoder now
                            // (the hybrid decoder replays it) rather than dropping frames.
                            if !WORKED.load(Ordering::Relaxed) {
                                BROKEN.store(true, Ordering::Relaxed);
                            }
                            return Err(CodecError::Decode("MediaCodec picture could not be read".into()));
                        }
                        None => {}
                    }
                    if eos {
                        return Ok(true);
                    }
                    timeout = Duration::ZERO;
                }
                DequeuedOutputBufferInfoResult::OutputFormatChanged => {
                    self.layout = Layout::from_format(&self.codec.output_format(), &self.info);
                    log::info!("{}: output layout {:?}", self.name, self.layout);
                }
                DequeuedOutputBufferInfoResult::OutputBuffersChanged => {}
                DequeuedOutputBufferInfoResult::TryAgainLater => {
                    if !until_eos {
                        return Ok(false);
                    }
                    idle += 1;
                    if idle > 200 {
                        return Err(CodecError::Decode("MediaCodec did not finish the stream".into()));
                    }
                    timeout = Duration::from_millis(10);
                }
            }
        }
    }

    /// One output buffer's picture as a planar 8-bit 4:2:0 frame. `None` (logged) when it cannot
    /// be read: no layout yet, a short buffer, or the ndk's `buffer()` panicking on a null one.
    fn picture(&self, buf: &OutputBuffer<'_>) -> Option<VideoFrame> {
        let Some(layout) = self.layout else {
            log::warn!("{}: picture before an output format", self.name);
            return None;
        };
        let info = *buf.info();
        let data = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| buf.buffer())).ok()?;
        let start = usize::try_from(info.offset()).ok()?;
        let len = usize::try_from(info.size()).ok()?;
        let Some([y, u, v]) = data.get(start..start.saturating_add(len)).and_then(|d| layout.copy(d)) else {
            log::warn!("{}: picture of {len} bytes does not fit {layout:?}", self.name);
            return None;
        };
        Some(VideoFrame {
            width: layout.crop.2 as u32,
            height: layout.crop.3 as u32,
            data: PixelData::Yuv8 { planes: [Arc::new(y), Arc::new(u), Arc::new(v)], chroma: Chroma::C420, alpha: None },
            color: self.info.color,
            par: self.info.par,
            pts: filmcraft_time::Tick(0),
        })
    }

    fn flush_codec(&mut self) {
        let _ = self.codec.flush();
        self.in_flight = 0;
        self.fed = false;
        self.need_config = true;
    }

    fn take_ready(&mut self) -> Vec<DecodedFrame> {
        std::mem::take(&mut self.ready).into_values().collect()
    }
}

fn annexb<'a>(nals: impl Iterator<Item = &'a Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    for n in nals {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(n);
    }
    out
}

fn err(e: ndk::media_error::MediaError) -> CodecError {
    CodecError::Decode(format!("MediaCodec: {e:?}"))
}

impl VideoDecoder for MediaCodecDecoder {
    fn decode(&mut self, sample: &[u8], pts: i64) -> Result<Vec<DecodedFrame>> {
        if self.need_config {
            let config = annexb(self.info.parameter_sets.iter());
            self.queue(&config, 0, BUFFER_FLAG_CODEC_CONFIG)?;
            self.need_config = false;
        }
        self.fed = true;
        let data = self.to_annexb(sample);
        self.queue(&data, pts, 0)?;
        // Keep the pipeline short: past a few samples in flight, wait for a picture.
        let wait = if self.in_flight > MAX_IN_FLIGHT { Duration::from_millis(40) } else { Duration::ZERO };
        self.drain(wait, false)?;
        Ok(self.take_ready())
    }

    fn flush(&mut self) -> Vec<DecodedFrame> {
        if !self.fed {
            return self.take_ready();
        }
        if self.in_flight > 0 && self.queue(&[], 0, BUFFER_FLAG_END_OF_STREAM).is_ok() {
            let _ = self.drain(Duration::from_millis(10), true);
        }
        // After end-of-stream the codec takes input again only after a flush.
        self.flush_codec();
        self.take_ready()
    }

    fn reset(&mut self) {
        trace!("reset");
        if self.fed {
            self.flush_codec();
        }
        self.ready.clear();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn is_random_access(&self, sample: &[u8]) -> Option<bool> {
        self.info.is_random_access(sample)
    }

    fn is_disposable(&self, sample: &[u8]) -> bool {
        self.info.is_disposable(sample)
    }
}

// SAFETY: an AMediaCodec has no thread affinity (the NDK documents it as usable from any thread;
// it only must not be used concurrently). `MediaCodecDecoder` reaches it only through `&mut self`
// (the `VideoDecoder` methods), so moving the decoder to another thread never shares it. ndk 0.9
// leaves `MediaCodec` !Send only because it holds a raw pointer.
#[allow(unsafe_code)]
unsafe impl Send for MediaCodecDecoder {}

impl Drop for MediaCodecDecoder {
    fn drop(&mut self) {
        let _ = self.codec.stop();
    }
}
