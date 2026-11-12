//! Hardware H.264 export on Android through MediaCodec, behind FilmCraft's encoder seam.
//!
//! FilmCraft's desktop builds put the OS encoder in front of its pure-Rust H.264 encoder
//! (`filmcraft_export::register_encoder`): VideoToolbox on macOS, NVENC on Windows and Linux. Each
//! takes an export only when it asks for hardware encoding (`ExportSettings::hardware_encoding` =
//! `Auto`) and the settings suit it, and declines (`None`: the software encoder takes it) otherwise.
//! This is the same for Android: the phone's video encoder block encodes many times faster than the
//! software encoder on its CPU cores, and leaves those cores to render the picture.
//!
//! Pictures go in as 8-bit 4:2:0 in the codec's own input buffers (NV12, or I420 where the codec
//! wants planar), converted from the export's RGBA with FilmCraft's BT.709 limited-range
//! conversion, the same codes its software encoder writes. Packets come out Annex B; parameter
//! sets go into the `avcC` record and the samples are length-prefixed as MP4 wants. No B-frames,
//! so packets arrive in presentation order and need no composition offsets.

use std::time::Duration;

use filmcraft_export::{BitrateMode, EncodedPacket, EncoderFrame, ExportError, ExportSettings, FieldOrder, Format, H264Pass, H264Profile, HardwareEncoding, Result, VideoEncoder};
use filmcraft_isobmff::{AvcConfig, SampleEntry};
use filmcraft_time::FrameRate;
use ndk::media::media_codec::{DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection};
use ndk::media::media_format::MediaFormat;

const BUFFER_FLAG_KEY_FRAME: u32 = 1;
const BUFFER_FLAG_CODEC_CONFIG: u32 = 2;
const BUFFER_FLAG_END_OF_STREAM: u32 = 4;
/// `COLOR_FormatYUV420SemiPlanar` (NV12) and `COLOR_FormatYUV420Planar` (I420).
const NV12: i32 = 21;
const I420: i32 = 19;

/// `VIDEOLAB_MC_DEBUG=1`: trace encoder events to stderr (as the decoder does).
fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VIDEOLAB_MC_DEBUG").is_some())
}

macro_rules! trace {
    ($($a:tt)*) => { if debug() { eprintln!("[mediacodec-enc] {}", format!($($a)*)); } };
}

pub fn register() {
    filmcraft_export::register_encoder(factory);
}

/// The export encoder factory (see the module docs).
pub fn factory(format: Format, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
    if s.hardware_encoding != HardwareEncoding::Auto || format != Format::H264 {
        return None;
    }
    let declined = |why: &str| {
        log::info!("hardware encoding declined: {why}");
        filmcraft_export::note_hw_encode_declined();
        None
    };
    if let Err(why) = suits(w, h, rate, s) {
        return declined(&why);
    }
    match MediaCodecEncoder::new(w, h, rate, s) {
        Ok(enc) => {
            filmcraft_export::note_hw_encode_session();
            Some(Ok(Box::new(enc)))
        }
        Err(why) => declined(&why),
    }
}

/// Why this export is the software encoder's (MXF, 10-bit, all-intra, two-pass, HDR, interlaced).
fn suits(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> std::result::Result<(), String> {
    if s.format.is_mxf() {
        return Err("MXF".into());
    }
    if s.signal.is_hdr() || s.h264_profile == H264Profile::High10 {
        return Err("10-bit / HDR".into());
    }
    if s.h264_intra_only {
        return Err("all-intra".into());
    }
    if !matches!(s.h264_pass, H264Pass::Single) || s.bitrate_mode == BitrateMode::Vbr2Pass {
        return Err("two-pass".into());
    }
    if s.field_order != FieldOrder::Progressive {
        return Err("interlaced output".into());
    }
    if s.pixel_aspect.is_some_and(|(n, d)| n != d) {
        return Err("non-square pixels".into());
    }
    if w == 0 || h == 0 || !w.is_multiple_of(2) || !h.is_multiple_of(2) || w > 8192 || h > 8192 {
        return Err(format!("{w}x{h}"));
    }
    if rate.num <= 0 || rate.den <= 0 || rate.num > i64::from(u32::MAX) || rate.den > i64::from(u32::MAX) {
        return Err("frame rate".into());
    }
    Ok(())
}

pub struct MediaCodecEncoder {
    codec: MediaCodec,
    w: u32,
    h: u32,
    rate: FrameRate,
    signal: filmcraft_export::ColorSignal,
    /// The input layout: NV12 or I420, row stride and rows per plane.
    planar: bool,
    stride: usize,
    slice: usize,
    sps: Vec<u8>,
    pps: Vec<u8>,
    /// Packets taken from the codec, not yet returned.
    out: Vec<EncodedPacket>,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    name: String,
}

impl MediaCodecEncoder {
    fn new(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> std::result::Result<Self, String> {
        let fps = rate.num as f64 / rate.den as f64;
        let keyint = s.keyframe_distance.filter(|k| *k > 0).map_or(2.0, |k| f64::from(k) / fps);
        // Constant quality has no hardware equivalent: the bitrate that quality lands near for
        // phone footage (0.1 bit per pixel per frame), at least the export's own.
        let kbps = match s.bitrate_mode {
            BitrateMode::Crf => ((f64::from(w) * f64::from(h) * fps * 0.1 / 1000.0) as u32).max(2_000),
            _ => s.bitrate_kbps.max(100),
        };
        let format = |color: i32| {
            let mut f = MediaFormat::new();
            f.set_str("mime", "video/avc");
            f.set_i32("width", w as i32);
            f.set_i32("height", h as i32);
            f.set_i32("color-format", color);
            f.set_i32("bitrate", kbps.saturating_mul(1000).min(i32::MAX as u32) as i32);
            // MediaCodecInfo.EncoderCapabilities: BITRATE_MODE_VBR 1, CBR 2
            f.set_i32("bitrate-mode", if s.bitrate_mode == BitrateMode::Cbr { 2 } else { 1 });
            f.set_f32("frame-rate", fps as f32);
            f.set_f32("i-frame-interval", keyint as f32);
            // AVCProfileBaseline 1, Main 2, High 8
            f.set_i32("profile", match s.h264_profile {
                H264Profile::Baseline => 1,
                H264Profile::Main => 2,
                _ => 8,
            });
            f.set_i32("max-bframes", 0);
            // BT.709, limited range, SDR video: what the conversion writes
            f.set_i32("color-standard", 1);
            f.set_i32("color-range", 2);
            f.set_i32("color-transfer", 3);
            // Not real time: an export. Real-time priority with a high operating rate makes the
            // codec reserve the media block's capacity, and the decoders reading the footage for
            // this very export then get no buffers (Qualcomm), dropping them to software.
            f.set_i32("priority", 1);
            f
        };
        let mut last = String::new();
        for color in [NV12, I420] {
            let Some(codec) = MediaCodec::from_encoder_type("video/avc") else {
                return Err("no H.264 encoder".into());
            };
            let name = codec.name().unwrap_or_else(|_| "video/avc".into());
            if let Err(e) = codec.configure(&format(color), None, MediaCodecDirection::Encoder) {
                last = format!("{name}: configure (colour {color}): {e:?}");
                continue;
            }
            if let Err(e) = codec.start() {
                last = format!("{name}: start: {e:?}");
                continue;
            }
            let input = codec.input_format();
            let stride = input.i32("stride").filter(|s| *s >= w as i32).map_or(w as usize, |s| s as usize);
            let slice = input.i32("slice-height").filter(|s| *s >= h as i32).map_or(h as usize, |s| s as usize);
            log::info!("hardware encoding with {name}: {w}x{h} {fps:.2} fps {kbps} kb/s, input {} stride {stride} slice {slice}", if color == I420 { "I420" } else { "NV12" });
            return Ok(Self {
                codec,
                w,
                h,
                rate,
                signal: s.signal,
                planar: color == I420,
                stride,
                slice,
                sps: Vec::new(),
                pps: Vec::new(),
                out: Vec::new(),
                y: Vec::new(),
                u: Vec::new(),
                v: Vec::new(),
                name,
            });
        }
        Err(last)
    }

    fn err(&self, what: &str, e: impl std::fmt::Debug) -> ExportError {
        ExportError::Encode(format!("MediaCodec ({}): {what}: {e:?} (turn hardware encoding off to use the software encoder)", self.name))
    }

    /// Copy the converted planes into an input buffer in the codec's layout.
    fn fill(&self, dst: &mut [std::mem::MaybeUninit<u8>]) -> Option<usize> {
        let (w, h) = (self.w as usize, self.h as usize);
        let (cw, ch) = (w / 2, h / 2);
        let (stride, slice) = (self.stride, self.slice);
        let luma = stride * slice;
        let size = if self.planar { luma + 2 * (stride / 2) * (slice / 2) } else { luma + stride * (slice / 2) };
        let size = size.min(dst.len());
        // the last rows of the last plane may be cut short (buffers sized to the visible picture)
        let need = if self.planar { luma + (stride / 2) * (slice / 2) + (stride / 2) * (ch - 1) + cw } else { luma + stride * (ch - 1) + w };
        if dst.len() < need {
            return None;
        }
        let put = |dst: &mut [std::mem::MaybeUninit<u8>], at: usize, src: &[u8]| {
            for (d, s) in dst[at..at + src.len()].iter_mut().zip(src) {
                d.write(*s);
            }
        };
        for (r, row) in self.y.chunks_exact(w).take(h).enumerate() {
            put(dst, r * stride, row);
        }
        if self.planar {
            let cs = stride / 2;
            let v_at = luma + cs * (slice / 2);
            for r in 0..ch {
                put(dst, luma + r * cs, &self.u[r * cw..(r + 1) * cw]);
                put(dst, v_at + r * cs, &self.v[r * cw..(r + 1) * cw]);
            }
        } else {
            let mut row = vec![0u8; w];
            for r in 0..ch {
                let (u, v) = (&self.u[r * cw..(r + 1) * cw], &self.v[r * cw..(r + 1) * cw]);
                for ((pair, a), b) in row.as_chunks_mut::<2>().0.iter_mut().zip(u).zip(v) {
                    *pair = [*a, *b];
                }
                put(dst, luma + r * stride, &row);
            }
        }
        Some(size)
    }

    fn queue(&mut self, index: Option<u64>) -> Result<()> {
        for _ in 0..400 {
            match self.codec.dequeue_input_buffer(Duration::from_millis(5)).map_err(|e| self.err("input", e))? {
                DequeuedInputBufferResult::Buffer(mut buf) => {
                    let Some(i) = index else {
                        return self.codec.queue_input_buffer(buf, 0, 0, 0, BUFFER_FLAG_END_OF_STREAM).map_err(|e| self.err("end of stream", e));
                    };
                    let size = self.fill(buf.buffer_mut()).ok_or_else(|| ExportError::Encode(format!("MediaCodec ({}): input buffer too small", self.name)))?;
                    let us = (i as f64 * self.rate.den as f64 * 1e6 / self.rate.num as f64).round() as u64;
                    trace!("queue frame {i} ({size} bytes)");
                    return self.codec.queue_input_buffer(buf, 0, size, us, 0).map_err(|e| self.err("queue", e));
                }
                DequeuedInputBufferResult::TryAgainLater => {
                    self.drain(Duration::from_millis(5), false)?;
                }
            }
        }
        Err(ExportError::Encode(format!("MediaCodec ({}) took no input for 2 s", self.name)))
    }

    /// Take packets: one wait of `wait`, then whatever is ready; with `until_eos`, until the end.
    fn drain(&mut self, wait: Duration, until_eos: bool) -> Result<()> {
        let mut timeout = wait;
        let mut idle = 0;
        loop {
            match self.codec.dequeue_output_buffer(timeout).map_err(|e| self.err("output", e))? {
                DequeuedOutputBufferInfoResult::Buffer(buf) => {
                    let info = *buf.info();
                    let (off, len) = (info.offset().max(0) as usize, info.size().max(0) as usize);
                    let data = buf.buffer().get(off..off + len).map(<[u8]>::to_vec).unwrap_or_default();
                    self.codec.release_output_buffer(buf, false).map_err(|e| self.err("release", e))?;
                    trace!("output {} bytes flags {} pts {}", len, info.flags(), info.presentation_time_us());
                    if info.flags() & BUFFER_FLAG_CODEC_CONFIG != 0 {
                        self.parameter_sets(&data);
                    } else if !data.is_empty() {
                        let sample = self.sample(&data);
                        if !sample.is_empty() {
                            filmcraft_export::note_hw_encode_frame();
                            self.out.push(EncodedPacket { data: sample, key: info.flags() & BUFFER_FLAG_KEY_FRAME != 0, duration: self.rate.den as u32, composition_offset: 0 });
                        }
                    }
                    if info.flags() & BUFFER_FLAG_END_OF_STREAM != 0 {
                        return Ok(());
                    }
                    timeout = Duration::ZERO;
                }
                DequeuedOutputBufferInfoResult::OutputFormatChanged | DequeuedOutputBufferInfoResult::OutputBuffersChanged => {}
                DequeuedOutputBufferInfoResult::TryAgainLater => {
                    if !until_eos {
                        return Ok(());
                    }
                    idle += 1;
                    if idle > 500 {
                        return Err(ExportError::Encode(format!("MediaCodec ({}) did not finish the stream", self.name)));
                    }
                    timeout = Duration::from_millis(10);
                }
            }
        }
    }

    fn parameter_sets(&mut self, annexb: &[u8]) {
        for nal in nals(annexb) {
            match nal.first().map(|b| b & 0x1f) {
                Some(7) => self.sps = nal.to_vec(),
                Some(8) => self.pps = nal.to_vec(),
                _ => {}
            }
        }
    }

    /// An Annex-B access unit → 4-byte length-prefixed NAL units, without parameter sets and access
    /// unit delimiters (in-band copies some encoders repeat before key frames; they live in `avcC`).
    fn sample(&mut self, annexb: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(annexb.len() + 16);
        for nal in nals(annexb) {
            match nal.first().map(|b| b & 0x1f) {
                Some(7) => self.sps = nal.to_vec(),
                Some(8) => self.pps = nal.to_vec(),
                Some(9) | None => {}
                Some(_) => {
                    out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    out.extend_from_slice(nal);
                }
            }
        }
        out
    }
}

/// The NAL units of an Annex-B stream (3- or 4-byte start codes).
fn nals(b: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(b.len(), |n| n - 3);
        // a 4-byte start code's leading zero, and trailing zeros, are not part of the unit
        while end > s && b[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            out.push(&b[s..end]);
        }
    }
    out
}

impl VideoEncoder for MediaCodecEncoder {
    fn sample_entry(&self) -> SampleEntry {
        let mut e = SampleEntry::avc(AvcConfig::new(vec![self.sps.clone()], vec![self.pps.clone()], 4), self.w as u16, self.h as u16);
        self.signal.apply_to(&mut e, false);
        e
    }

    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }

    fn encode(&mut self, f: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        if f.width != self.w || f.height != self.h {
            return Err(ExportError::Encode(format!("MediaCodec: a {}x{} picture for a {}x{} encoder", f.width, f.height, self.w, self.h)));
        }
        let (w, h) = (self.w as usize, self.h as usize);
        filmcraft_export::timed(filmcraft_export::Stage::Convert, || filmcraft_export::rgba_to_yuv420_8(f.rgba, w, h, &mut self.y, &mut self.u, &mut self.v));
        self.queue(Some(f.index))?;
        // The first picture brings the parameter sets the sample entry needs: wait for them.
        let wait = if self.sps.is_empty() { Duration::from_millis(20) } else { Duration::ZERO };
        for _ in 0..100 {
            self.drain(wait, false)?;
            if !self.sps.is_empty() {
                break;
            }
        }
        Ok(std::mem::take(&mut self.out))
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        self.queue(None)?;
        self.drain(Duration::from_millis(10), true)?;
        Ok(std::mem::take(&mut self.out))
    }
}

// SAFETY: the codec is used from one thread at a time (the export's encoder stage owns it); the
// ndk leaves `MediaCodec` !Send only because it holds a raw pointer. Same allowance as the decoder.
#[allow(unsafe_code)]
unsafe impl Send for MediaCodecEncoder {}

impl Drop for MediaCodecEncoder {
    fn drop(&mut self) {
        let _ = self.codec.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::nals;

    #[test]
    fn splits_annexb() {
        let b = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5];
        assert_eq!(nals(&b), vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4, 5][..]]);
    }
}
