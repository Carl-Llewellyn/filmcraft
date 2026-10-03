//! Export: render a sequence range and encode it to a file.
//!
//! Frames are rendered in parallel batches (one frame per core, each frame itself row-parallel),
//! then encoded and muxed in order; audio is mixed per batch and interleaved. Progress and cancel
//! are shared atomics so the UI (Export mode, header progress) and MCP can observe/cancel jobs.
//!
//! Video encoders implement [`VideoEncoder`]; codec crates register theirs with
//! [`register_encoder`] (H.264, ProRes …). Built in: Motion-JPEG (MOV), PNG sequence, GIF, WAV.

use std::io::Write;

mod job;
pub use job::{Exporter, Step, stepped};
#[cfg(target_os = "linux")]
mod nvenc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use filmcraft_isobmff::SampleEntry;
use filmcraft_project::{ItemId, Project};
use filmcraft_render::{RenderOptions, SourceProvider};
use filmcraft_time::{FrameRate, Tick, TimeRange};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("no such sequence")]
    NoSequence,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("I/O: {0}")]
    Io(String),
    #[error("encode: {0}")]
    Encode(String),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, ExportError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    /// MPEG-4, H.264 video + AAC audio (needs the H.264/AAC encoders registered).
    H264,
    /// QuickTime, Apple ProRes 422 HQ + PCM (needs the ProRes encoder registered).
    ProRes,
    /// QuickTime, Avid DNxHR (HQ unless the settings pick another profile) + PCM.
    DnxHr,
    /// QuickTime, Motion-JPEG + 16-bit PCM.
    Mjpeg,
    PngSequence,
    Gif,
    Wav,
}

/// Preferred H.264 encoder backend. `Auto` uses hardware when available and otherwise software.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoEncoderPreference {
    #[default]
    Auto,
    Hardware,
    Software,
}

impl Format {
    pub fn from_name(s: &str) -> Option<Format> {
        Some(match s.to_ascii_lowercase().replace([' ', '-', '_', '.'], "").as_str() {
            "h264" | "mp4" | "avc" => Format::H264,
            "prores" | "mov" => Format::ProRes,
            "dnxhr" | "dnxhd" | "dnx" | "avid" | "vc3" => Format::DnxHr,
            "mjpeg" | "motionjpeg" | "jpeg" => Format::Mjpeg,
            "png" | "pngsequence" => Format::PngSequence,
            "gif" | "animatedgif" => Format::Gif,
            "wav" | "waveform" => Format::Wav,
            _ => return None,
        })
    }
    pub fn extension(self) -> &'static str {
        match self {
            Format::H264 => "mp4",
            Format::ProRes | Format::DnxHr | Format::Mjpeg => "mov",
            Format::PngSequence => "png",
            Format::Gif => "gif",
            Format::Wav => "wav",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::H264 => "H.264",
            Format::ProRes => "Apple ProRes",
            Format::DnxHr => "Avid DNxHR",
            Format::Mjpeg => "QuickTime (Motion JPEG)",
            Format::PngSequence => "PNG Sequence",
            Format::Gif => "Animated GIF",
            Format::Wav => "Waveform Audio",
        }
    }
    pub const ALL: [Format; 7] = [Format::H264, Format::ProRes, Format::DnxHr, Format::Mjpeg, Format::PngSequence, Format::Gif, Format::Wav];
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportSettings {
    pub format: Format,
    /// H.264 backend selection; ignored by formats with a dedicated encoder.
    #[serde(default)]
    pub video_encoder: VideoEncoderPreference,
    pub path: String,
    /// Timeline range (default: In/Out if set, else the whole sequence).
    pub range: Option<TimeRange>,
    /// Output scale (1.0 = sequence frame size).
    pub scale: f32,
    pub include_audio: bool,
    /// Quality 0–100 for lossy codecs.
    pub quality: u8,
    /// Target video bitrate (kbps) for bitrate-driven encoders.
    pub bitrate_kbps: u32,
    /// Burn the visible caption tracks into the picture (Export ▸ Captions ▸ Burn Captions Into
    /// Video).
    pub burn_captions: bool,
    /// This export is one part of a larger job (render previews): the caller owns
    /// `progress.total`, `finished` and the final status; this call only adds to `done`.
    #[serde(default)]
    pub part_of_batch: bool,
    /// ProRes flavour: `proxy`, `lt`, `standard` or `hq` (empty = HQ).
    #[serde(default)]
    pub prores_profile: String,
    /// DNxHR profile: `lb`, `sq`, `hq` or `hqx` (empty = HQ).
    #[serde(default)]
    pub dnx_profile: String,
    /// Encode display-referred SDR (Rec. 709, tone mapped) even when the sequence works in
    /// Rec. 2100 PQ/HLG. Otherwise H.264 and ProRes exports of an HDR sequence are encoded in the
    /// sequence's HDR space and signal it (VUI / `colr` / `mdcv` / `clli` / SEI).
    #[serde(default)]
    pub sdr: bool,
    /// Colour signalling chosen by [`export`] for the encoders (not set by callers).
    #[serde(skip)]
    pub signal: ColorSignal,
    /// Encode into memory and hand each finished file (path, bytes) to this sink instead of
    /// writing `path` (hosts without a filesystem: the web app offers the file as a download).
    #[serde(skip)]
    pub sink: Option<OutputSink>,
}

/// Receives in-memory export output: `(path, bytes)` per finished file.
#[derive(Clone)]
pub struct OutputSink(pub Arc<dyn Fn(&str, Vec<u8>) -> std::io::Result<()> + Send + Sync>);

impl std::fmt::Debug for OutputSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OutputSink")
    }
}

/// An export's output file: on disk, or in memory for an [`OutputSink`].
enum Out {
    File(std::io::BufWriter<std::fs::File>),
    Mem(std::io::Cursor<Vec<u8>>),
}

impl Out {
    fn create(settings: &ExportSettings) -> Result<Out> {
        if settings.sink.is_some() {
            return Ok(Out::Mem(std::io::Cursor::new(Vec::new())));
        }
        let f = std::fs::File::create(&settings.path).map_err(|e| ExportError::Io(e.to_string()))?;
        Ok(Out::File(std::io::BufWriter::new(f)))
    }

    /// Flush (and hand in-memory output to the sink); returns the file size.
    fn finish(self, settings: &ExportSettings) -> Result<u64> {
        match self {
            Out::File(mut w) => {
                w.flush().map_err(|e| ExportError::Io(e.to_string()))?;
                Ok(std::fs::metadata(&settings.path).map(|m| m.len()).unwrap_or(0))
            }
            Out::Mem(c) => write_output(settings, &settings.path, c.into_inner()),
        }
    }
}

/// Write one finished output file (to the sink when there is one); returns its size.
fn write_output(settings: &ExportSettings, path: &str, data: Vec<u8>) -> Result<u64> {
    let n = data.len() as u64;
    match &settings.sink {
        Some(sink) => (sink.0)(path, data),
        None => std::fs::write(path, &data),
    }
    .map_err(|e| ExportError::Io(e.to_string()))?;
    Ok(n)
}

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Out::File(w) => w.write(buf),
            Out::Mem(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Out::File(w) => w.flush(),
            Out::Mem(w) => w.flush(),
        }
    }
}

impl std::io::Seek for Out {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Out::File(w) => w.seek(pos),
            Out::Mem(w) => w.seek(pos),
        }
    }
}

/// Colour description of the encoded stream (ITU-T H.273 code points).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorSignal {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
}

impl Default for ColorSignal {
    fn default() -> Self {
        ColorSignal { primaries: 1, transfer: 1, matrix: 1 }
    }
}

impl ColorSignal {
    pub const PQ: ColorSignal = ColorSignal { primaries: 9, transfer: 16, matrix: 9 };
    pub const HLG: ColorSignal = ColorSignal { primaries: 9, transfer: 18, matrix: 9 };
    pub fn is_hdr(&self) -> bool {
        matches!(self.transfer, 16 | 18)
    }
    /// (Kr, Kb) of the matrix.
    pub fn kr_kb(&self) -> (f32, f32) {
        if self.matrix == 9 { (0.2627, 0.0593) } else { (0.2126, 0.0722) }
    }
    /// Sample-entry boxes: `colr` (nclx for MP4, nclc for MOV) plus `mdcv`/`clli` for PQ
    /// (mastering display BT.2020/D65, 1000/0.0001 cd/m²; MaxCLL/MaxFALL 0 = unknown).
    pub fn apply_to(&self, e: &mut SampleEntry, mov: bool) {
        if !self.is_hdr() {
            return;
        }
        if let Some(v) = e.video.as_mut() {
            let (p, t, m) = (self.primaries as u16, self.transfer as u16, self.matrix as u16);
            v.color = Some(if mov {
                filmcraft_isobmff::ColorInfo::Nclc { primaries: p, transfer: t, matrix: m }
            } else {
                filmcraft_isobmff::ColorInfo::Nclx { primaries: p, transfer: t, matrix: m, full_range: false }
            });
            if self.transfer == 16 {
                v.mastering_display = Some(filmcraft_isobmff::MasteringDisplay::bt2020(1000.0, 0.0001));
                v.content_light = Some((0, 0));
            }
        }
    }
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: Format::H264,
            video_encoder: VideoEncoderPreference::Auto,
            path: String::new(),
            range: None,
            scale: 1.0,
            include_audio: true,
            quality: 90,
            bitrate_kbps: 20_000,
            burn_captions: false,
            part_of_batch: false,
            prores_profile: String::new(),
            dnx_profile: String::new(),
            sdr: false,
            signal: ColorSignal::default(),
            sink: None,
        }
    }
}

/// Whether an NVIDIA NVENC H.264 device is currently discoverable on this machine.
pub fn hardware_encoder_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        nvenc::available()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Shared progress/cancel state of an export job.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    pub finished: AtomicBool,
    pub status: Mutex<String>,
    pub error: Mutex<Option<String>>,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed).max(1);
        self.done.load(Ordering::Relaxed) as f32 / t as f32
    }
    fn set_status(&self, s: impl Into<String>) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = s.into();
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub path: String,
    pub frames: u64,
    pub seconds: f64,
    pub bytes: u64,
    pub render_fps: f64,
}

/// A packet produced by a video encoder.
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub key: bool,
    /// Duration in encoder timescale units.
    pub duration: u32,
    /// pts − dts in encoder timescale units.
    pub composition_offset: i32,
}

/// Input picture for encoders: straight sRGB RGBA8 (encoders convert to their own YUV), or for
/// HDR exports the encoded R'G'B' (PQ/HLG, BT.2020; 3 floats per pixel, 0..1) in `hdr`.
pub struct EncoderFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
    pub hdr: Option<&'a [f32]>,
    pub index: u64,
}

pub trait VideoEncoder: Send {
    /// MP4/MOV sample entry (codec config) — may only be complete after the first frame.
    fn sample_entry(&self) -> SampleEntry;
    fn timescale(&self) -> u32;
    fn encode(&mut self, frame: &EncoderFrame) -> Result<Vec<EncodedPacket>>;
    fn flush(&mut self) -> Result<Vec<EncodedPacket>>;
    /// Human-readable backend label for export progress and diagnostics.
    fn name(&self) -> &'static str {
        "Video encoder"
    }
    /// Media start offset for an edit list (B-frame delay), in the encoder timescale.
    fn media_start(&self) -> Option<i64> {
        None
    }
}

/// Audio encoder (AAC) plugged in by codec crates; PCM is built in.
pub trait AudioEncoder: Send {
    fn sample_entry(&self) -> SampleEntry;
    fn encode(&mut self, planar: &[Vec<f32>]) -> Result<Vec<Vec<u8>>>;
    fn flush(&mut self) -> Result<Vec<Vec<u8>>>;
    /// Encoder delay (priming) in samples.
    fn priming(&self) -> u32;
    /// Samples per access unit (1024 for AAC).
    fn frame_size(&self) -> u32;
}

pub type EncoderFactory = fn(format: Format, width: u32, height: u32, rate: FrameRate, settings: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>>;
pub type AudioEncoderFactory = fn(format: Format, sample_rate: u32, channels: u32, settings: &ExportSettings) -> Option<Result<Box<dyn AudioEncoder>>>;

fn video_factories() -> &'static RwLock<Vec<EncoderFactory>> {
    static F: OnceLock<RwLock<Vec<EncoderFactory>>> = OnceLock::new();
    F.get_or_init(|| {
        let factories: Vec<EncoderFactory> = vec![h264_factory, prores_factory, dnx_factory, mjpeg_factory];
        #[cfg(target_os = "linux")]
        let factories = {
            let mut factories = factories;
            factories.insert(0, nvenc::factory);
            factories
        };
        RwLock::new(factories)
    })
}
fn audio_factories() -> &'static RwLock<Vec<AudioEncoderFactory>> {
    static F: OnceLock<RwLock<Vec<AudioEncoderFactory>>> = OnceLock::new();
    F.get_or_init(|| RwLock::new(vec![aac_factory]))
}

pub fn register_encoder(f: EncoderFactory) {
    video_factories().write().unwrap_or_else(|e| e.into_inner()).insert(0, f);
}

/// Create the first available encoder for a format using the registered backend preferences.
pub fn create_video_encoder(format: Format, width: u32, height: u32, rate: FrameRate, settings: &ExportSettings) -> Result<Box<dyn VideoEncoder>> {
    video_factories()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find_map(|factory| factory(format, width, height, rate, settings))
        .ok_or_else(|| ExportError::Unsupported(format!("{} encoder not available yet", format.label())))?
}

pub fn register_audio_encoder(f: AudioEncoderFactory) {
    audio_factories().write().unwrap_or_else(|e| e.into_inner()).insert(0, f);
}

/// Whether a format can currently be exported. Every [`Format`] has a built-in encoder; this stays
/// as the hook for formats whose encoders are registered at runtime.
pub fn available(format: Format) -> bool {
    Format::ALL.contains(&format)
}

struct MjpegEncoder {
    w: u16,
    h: u16,
    quality: u8,
    rate: FrameRate,
}

impl VideoEncoder for MjpegEncoder {
    fn sample_entry(&self) -> SampleEntry {
        SampleEntry::jpeg(self.w, self.h)
    }
    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }
    fn encode(&mut self, f: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        let rgb: Vec<u8> = f.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        let mut out = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, self.quality);
        enc.encode(&rgb, f.width, f.height, image::ExtendedColorType::Rgb8).map_err(|e| ExportError::Encode(e.to_string()))?;
        Ok(vec![EncodedPacket { data: out, key: true, duration: self.rate.den as u32, composition_offset: 0 }])
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        Ok(Vec::new())
    }
}

fn mjpeg_factory(format: Format, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
    (format == Format::Mjpeg).then(|| Ok(Box::new(MjpegEncoder { w: w as u16, h: h as u16, quality: s.quality.clamp(1, 100), rate }) as Box<dyn VideoEncoder>))
}

/// AAC-LC (our encoder), 320 kbps stereo by default.
struct AacEncoder {
    enc: filmcraft_aac::Encoder,
    rate: u32,
    channels: u32,
}

impl AudioEncoder for AacEncoder {
    fn sample_entry(&self) -> SampleEntry {
        SampleEntry::aac(self.enc.audio_specific_config(), self.channels, self.rate)
    }
    fn encode(&mut self, planar: &[Vec<f32>]) -> Result<Vec<Vec<u8>>> {
        let refs: Vec<&[f32]> = planar.iter().map(Vec::as_slice).collect();
        Ok(self.enc.encode(&refs))
    }
    fn flush(&mut self) -> Result<Vec<Vec<u8>>> {
        Ok(self.enc.flush())
    }
    fn priming(&self) -> u32 {
        self.enc.priming_samples()
    }
    fn frame_size(&self) -> u32 {
        1024
    }
}

fn aac_factory(_format: Format, sample_rate: u32, channels: u32, _s: &ExportSettings) -> Option<Result<Box<dyn AudioEncoder>>> {
    Some(
        filmcraft_aac::Encoder::new(filmcraft_aac::EncoderConfig::cbr(sample_rate, channels as usize, 320_000))
            .map(|enc| Box::new(AacEncoder { enc, rate: sample_rate, channels }) as Box<dyn AudioEncoder>)
            .map_err(|e| ExportError::Encode(e.to_string())),
    )
}

/// ProRes 422 encoder (HQ unless the settings pick another flavour): sRGB/709 RGBA8 → 10-bit limited-range BT.709 4:2:2.
struct ProResEncoder {
    enc: filmcraft_prores::Encoder,
    profile: filmcraft_prores::Profile,
    w: u32,
    h: u32,
    rate: FrameRate,
    signal: ColorSignal,
}

impl VideoEncoder for ProResEncoder {
    fn sample_entry(&self) -> SampleEntry {
        let mut e = SampleEntry::prores(filmcraft_isobmff::FourCc(self.profile.fourcc()), self.w as u16, self.h as u16);
        self.signal.apply_to(&mut e, true);
        e
    }
    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }
    fn encode(&mut self, f: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        let mut fr = filmcraft_prores::Frame::new(f.width, f.height, filmcraft_prores::ChromaFormat::Yuv422, 10, false);
        match f.hdr {
            Some(rgb) => {
                let (kr, kb) = self.signal.kr_kb();
                rgbf_to_yuv422_10(rgb, f.width as usize, f.height as usize, kr, kb, &mut fr.y, &mut fr.cb, &mut fr.cr)
            }
            None => rgba_to_yuv422_10(f.rgba, f.width as usize, f.height as usize, &mut fr.y, &mut fr.cb, &mut fr.cr),
        }
        let data = self.enc.encode(&fr).map_err(|e| ExportError::Encode(e.to_string()))?;
        Ok(vec![EncodedPacket { data, key: true, duration: self.rate.den as u32, composition_offset: 0 }])
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        Ok(Vec::new())
    }
}

/// BT.709 limited-range 10-bit 4:2:2 from straight RGBA8 (chroma averaged horizontally).
pub fn rgba_to_yuv422_10(rgba: &[u8], w: usize, h: usize, y: &mut [u16], cb: &mut [u16], cr: &mut [u16]) {
    let cw = w.div_ceil(2);
    debug_assert!(y.len() >= w * h && rgba.len() >= w * h * 4);
    y.par_chunks_mut(w).zip(cb.par_chunks_mut(cw).zip(cr.par_chunks_mut(cw))).enumerate().for_each(|(row, (yr, (cbr, crr)))| {
        let src = &rgba[row * w * 4..(row + 1) * w * 4];
        let mut us = vec![0f32; w];
        let mut vs = vec![0f32; w];
        for x in 0..w {
            let (r, g, b) = (src[x * 4] as f32 / 255.0, src[x * 4 + 1] as f32 / 255.0, src[x * 4 + 2] as f32 / 255.0);
            let yy = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            yr[x] = (64.0 + 876.0 * yy).round().clamp(4.0, 1019.0) as u16;
            us[x] = (b - yy) / 1.8556;
            vs[x] = (r - yy) / 1.5748;
        }
        for cx in 0..cw {
            let a = cx * 2;
            let b2 = (a + 1).min(w - 1);
            let u = (us[a] + us[b2]) * 0.5;
            let v = (vs[a] + vs[b2]) * 0.5;
            cbr[cx] = (512.0 + 896.0 * u).round().clamp(4.0, 1019.0) as u16;
            crr[cx] = (512.0 + 896.0 * v).round().clamp(4.0, 1019.0) as u16;
        }
    });
}

/// The ProRes profile named by [`ExportSettings::prores_profile`].
pub fn prores_profile(name: &str) -> filmcraft_prores::Profile {
    use filmcraft_prores::Profile;
    match name.to_ascii_lowercase().as_str() {
        "proxy" => Profile::Proxy,
        "lt" => Profile::Lt,
        "standard" | "422" => Profile::Standard,
        _ => Profile::Hq,
    }
}

/// Limited-range 10-bit 4:2:2 from encoded R'G'B' floats with matrix (Kr, Kb).
#[allow(clippy::too_many_arguments)]
pub fn rgbf_to_yuv422_10(rgb: &[f32], w: usize, h: usize, kr: f32, kb: f32, y: &mut [u16], cb: &mut [u16], cr: &mut [u16]) {
    let cw = w.div_ceil(2);
    debug_assert!(rgb.len() >= w * h * 3 && y.len() >= w * h);
    let kg = 1.0 - kr - kb;
    let (sb, sr) = (2.0 * (1.0 - kb), 2.0 * (1.0 - kr));
    y.par_chunks_mut(w).zip(cb.par_chunks_mut(cw).zip(cr.par_chunks_mut(cw))).enumerate().for_each(|(row, (yr, (cbr, crr)))| {
        let src = &rgb[row * w * 3..(row + 1) * w * 3];
        let mut us = vec![0f32; w];
        let mut vs = vec![0f32; w];
        for x in 0..w {
            let (r, g, b) = (src[x * 3], src[x * 3 + 1], src[x * 3 + 2]);
            let yy = kr * r + kg * g + kb * b;
            yr[x] = (64.0 + 876.0 * yy).round().clamp(4.0, 1019.0) as u16;
            us[x] = (b - yy) / sb;
            vs[x] = (r - yy) / sr;
        }
        for cx in 0..cw {
            let a = cx * 2;
            let b2 = (a + 1).min(w - 1);
            cbr[cx] = (512.0 + 896.0 * (us[a] + us[b2]) * 0.5).round().clamp(4.0, 1019.0) as u16;
            crr[cx] = (512.0 + 896.0 * (vs[a] + vs[b2]) * 0.5).round().clamp(4.0, 1019.0) as u16;
        }
    });
}

fn prores_factory(format: Format, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
    (format == Format::ProRes).then(|| {
        let profile = prores_profile(&s.prores_profile);
        let mut cfg = filmcraft_prores::EncoderConfig::new(profile, w, h);
        if s.signal.is_hdr() {
            cfg.color = filmcraft_prores::ColorInfo { primaries: s.signal.primaries, transfer: s.signal.transfer, matrix: s.signal.matrix };
        }
        Ok(Box::new(ProResEncoder { enc: filmcraft_prores::Encoder::with_config(cfg), profile, w, h, rate, signal: s.signal }) as Box<dyn VideoEncoder>)
    })
}

/// The DNxHR profile named by [`ExportSettings::dnx_profile`].
pub fn dnx_profile(name: &str) -> filmcraft_dnx::Profile {
    use filmcraft_dnx::Profile;
    match name.to_ascii_lowercase().as_str() {
        "lb" => Profile::Lb,
        "sq" => Profile::Sq,
        "hqx" => Profile::Hqx,
        _ => Profile::Hq,
    }
}

/// DNxHR encoder (RGBA8 or HDR floats → BT.709 / BT.2020 limited-range 4:2:2; 8-bit for
/// LB/SQ/HQ, 10-bit for HQX).
struct DnxEncoder {
    enc: filmcraft_dnx::Encoder,
    w: u32,
    h: u32,
    rate: FrameRate,
    signal: ColorSignal,
}

impl VideoEncoder for DnxEncoder {
    fn sample_entry(&self) -> SampleEntry {
        let mut e = SampleEntry::dnx(filmcraft_isobmff::FourCc(*b"AVdh"), self.w as u16, self.h as u16);
        self.signal.apply_to(&mut e, true);
        e
    }
    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }
    fn encode(&mut self, f: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        let mut fr = filmcraft_dnx::Frame::new(f.width, f.height, filmcraft_dnx::ChromaFormat::Yuv422, 10, false);
        match f.hdr {
            Some(rgb) => {
                let (kr, kb) = self.signal.kr_kb();
                rgbf_to_yuv422_10(rgb, f.width as usize, f.height as usize, kr, kb, &mut fr.y, &mut fr.cb, &mut fr.cr)
            }
            None => rgba_to_yuv422_10(f.rgba, f.width as usize, f.height as usize, &mut fr.y, &mut fr.cb, &mut fr.cr),
        }
        // the encoder rescales 10-bit input to its coded depth
        let data = self.enc.encode(&fr).map_err(|e| ExportError::Encode(e.to_string()))?;
        Ok(vec![EncodedPacket { data, key: true, duration: self.rate.den as u32, composition_offset: 0 }])
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        Ok(Vec::new())
    }
}

fn dnx_factory(format: Format, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
    (format == Format::DnxHr).then(|| {
        let mut cfg = filmcraft_dnx::EncoderConfig::new(dnx_profile(&s.dnx_profile), w, h);
        if s.signal.primaries == 9 {
            cfg.color_volume = filmcraft_dnx::ColorVolume::Bt2020Ncl;
        }
        let enc = filmcraft_dnx::Encoder::with_config(cfg).map_err(|e| ExportError::Encode(e.to_string()))?;
        Ok(Box::new(DnxEncoder { enc, w, h, rate, signal: s.signal }) as Box<dyn VideoEncoder>)
    })
}

/// H.264 High (our encoder): sRGB/709 RGBA8 → 8-bit limited-range BT.709 4:2:0, VBR at the
/// requested bitrate, length-prefixed samples with the `avcC` in the sample entry.
struct H264Encoder {
    enc: filmcraft_h264enc::Encoder,
    w: u32,
    h: u32,
    rate: FrameRate,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    signal: ColorSignal,
}

impl H264Encoder {
    fn packets(&self, ps: Vec<filmcraft_h264enc::Packet>) -> Vec<EncodedPacket> {
        ps.into_iter()
            .map(|p| EncodedPacket { data: p.data, key: p.keyframe, duration: self.rate.den as u32, composition_offset: (p.pts - p.dts) as i32 })
            .collect()
    }
}

impl VideoEncoder for H264Encoder {
    fn sample_entry(&self) -> SampleEntry {
        let cfg = filmcraft_isobmff::AvcConfig::parse(&self.enc.avcc()).unwrap_or_else(|_| {
            let (sps, pps) = self.enc.sps_pps();
            filmcraft_isobmff::AvcConfig::new(vec![sps], vec![pps], 4)
        });
        let mut e = SampleEntry::avc(cfg, self.w as u16, self.h as u16);
        self.signal.apply_to(&mut e, false);
        e
    }
    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }
    fn name(&self) -> &'static str {
        "Software H.264"
    }
    fn encode(&mut self, f: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        match f.hdr {
            Some(rgb) => {
                let (kr, kb) = self.signal.kr_kb();
                rgbf_to_yuv420_8(rgb, f.width as usize, f.height as usize, kr, kb, &mut self.y, &mut self.u, &mut self.v)
            }
            None => rgba_to_yuv420_8(f.rgba, f.width as usize, f.height as usize, &mut self.y, &mut self.u, &mut self.v),
        }
        let cw = (f.width as usize).div_ceil(2);
        let frame = filmcraft_h264enc::YuvFrame { y: &self.y, u: &self.u, v: &self.v, y_stride: f.width as usize, uv_stride: cw };
        let ps = self.enc.try_encode(&frame, f.index as i64 * self.rate.den).map_err(|e| ExportError::Encode(e.to_string()))?;
        Ok(self.packets(ps))
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        let ps = self.enc.flush();
        Ok(self.packets(ps))
    }
    fn media_start(&self) -> Option<i64> {
        // With B-frames the first DTS is one frame before the first PTS.
        (self.enc.delay() > 0).then_some(self.rate.den)
    }
}

/// BT.709 limited-range 8-bit 4:2:0 from straight RGBA8 (2×2 chroma average).
pub fn rgba_to_yuv420_8(rgba: &[u8], w: usize, h: usize, y: &mut Vec<u8>, u: &mut Vec<u8>, v: &mut Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    y.resize(w * h, 0);
    u.resize(cw * ch, 0);
    v.resize(cw * ch, 0);
    y.par_chunks_mut(w * 2).zip(u.par_chunks_mut(cw).zip(v.par_chunks_mut(cw))).enumerate().for_each(|(cy, (yr, (ur, vr)))| {
        let rows = yr.len() / w;
        let mut us = vec![0f32; cw];
        let mut vs = vec![0f32; cw];
        let mut cnt = vec![0f32; cw];
        for dy in 0..rows {
            let row = cy * 2 + dy;
            let src = &rgba[row * w * 4..(row + 1) * w * 4];
            for x in 0..w {
                let (r, g, b) = (src[x * 4] as f32 / 255.0, src[x * 4 + 1] as f32 / 255.0, src[x * 4 + 2] as f32 / 255.0);
                let yy = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                yr[dy * w + x] = (16.0 + 219.0 * yy).round().clamp(1.0, 254.0) as u8;
                us[x / 2] += (b - yy) / 1.8556;
                vs[x / 2] += (r - yy) / 1.5748;
                cnt[x / 2] += 1.0;
            }
        }
        for cx in 0..cw {
            ur[cx] = (128.0 + 224.0 * us[cx] / cnt[cx]).round().clamp(1.0, 254.0) as u8;
            vr[cx] = (128.0 + 224.0 * vs[cx] / cnt[cx]).round().clamp(1.0, 254.0) as u8;
        }
    });
}

/// Limited-range 8-bit 4:2:0 from encoded R'G'B' floats with matrix (Kr, Kb).
#[allow(clippy::too_many_arguments)]
pub fn rgbf_to_yuv420_8(rgb: &[f32], w: usize, h: usize, kr: f32, kb: f32, y: &mut Vec<u8>, u: &mut Vec<u8>, v: &mut Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let kg = 1.0 - kr - kb;
    let (sb, sr) = (2.0 * (1.0 - kb), 2.0 * (1.0 - kr));
    y.resize(w * h, 0);
    u.resize(cw * ch, 0);
    v.resize(cw * ch, 0);
    y.par_chunks_mut(w * 2).zip(u.par_chunks_mut(cw).zip(v.par_chunks_mut(cw))).enumerate().for_each(|(cy, (yr, (ur, vr)))| {
        let rows = yr.len() / w;
        let mut us = vec![0f32; cw];
        let mut vs = vec![0f32; cw];
        let mut cnt = vec![0f32; cw];
        for dy in 0..rows {
            let row = cy * 2 + dy;
            let src = &rgb[row * w * 3..(row + 1) * w * 3];
            for x in 0..w {
                let (r, g, b) = (src[x * 3], src[x * 3 + 1], src[x * 3 + 2]);
                let yy = kr * r + kg * g + kb * b;
                yr[dy * w + x] = (16.0 + 219.0 * yy).round().clamp(1.0, 254.0) as u8;
                us[x / 2] += (b - yy) / sb;
                vs[x / 2] += (r - yy) / sr;
                cnt[x / 2] += 1.0;
            }
        }
        for cx in 0..cw {
            ur[cx] = (128.0 + 224.0 * us[cx] / cnt[cx]).round().clamp(1.0, 254.0) as u8;
            vr[cx] = (128.0 + 224.0 * vs[cx] / cnt[cx]).round().clamp(1.0, 254.0) as u8;
        }
    });
}

fn h264_factory(format: Format, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
    if format != Format::H264 || s.video_encoder == VideoEncoderPreference::Hardware {
        return None;
    }
    let mut cfg = filmcraft_h264enc::EncoderConfig::new(w, h, rate.num as u32, rate.den as u32);
    cfg.format = filmcraft_h264enc::PacketFormat::LengthPrefixed;
    cfg.aud = false;
    cfg.keyint = (rate.num as f64 / rate.den as f64 * 2.0).round().max(1.0) as u32;
    let kbps = s.bitrate_kbps.max(100);
    cfg.rate = filmcraft_h264enc::RateControl::Vbr { target_kbps: kbps, max_kbps: kbps * 3 / 2 };
    if s.signal.is_hdr() {
        cfg.color = filmcraft_h264enc::ColorConfig { primaries: s.signal.primaries, transfer: s.signal.transfer, matrix: s.signal.matrix, full_range: false };
        if s.signal.transfer == 16 {
            let md = filmcraft_isobmff::MasteringDisplay::bt2020(1000.0, 0.0001).to_bytes();
            let mut b = [0u8; 24];
            b.copy_from_slice(&md);
            cfg.mastering_display = Some(b);
            cfg.content_light = Some((0, 0));
        }
    }
    Some(
        filmcraft_h264enc::Encoder::new(cfg)
            .map(|enc| Box::new(H264Encoder { enc, w, h, rate, y: Vec::new(), u: Vec::new(), v: Vec::new(), signal: s.signal }) as Box<dyn VideoEncoder>)
            .map_err(|e| ExportError::Encode(e.to_string())),
    )
}

/// The range to export (settings → In/Out → whole sequence).
pub fn export_range(project: &Project, seq: ItemId, settings: &ExportSettings) -> Result<TimeRange> {
    let q = project.sequence(seq).ok_or(ExportError::NoSequence)?;
    if let Some(r) = settings.range {
        return Ok(r);
    }
    let fd = q.settings.frame_rate.frame_duration();
    let a = q.mark_in.unwrap_or(Tick::ZERO);
    let b = q.mark_out.map(|o| o + fd).unwrap_or(q.duration());
    Ok(TimeRange::from_bounds(a, b.max(a + fd)))
}

/// Run an export (blocking; call from a worker thread).
pub fn export(project: &Arc<Project>, seq: ItemId, settings: &ExportSettings, sources: &dyn SourceProvider, progress: &Progress) -> Result<Report> {
    if stepped(settings.format) {
        let mut ex = Exporter::new(project.clone(), seq, settings, progress)?;
        loop {
            match ex.step(sources, progress)? {
                Step::Progress => {}
                Step::Done(r) => return Ok(r),
                // only asynchronous (web) sources defer; a blocking export cannot wait for them
                Step::Pending => return Err(ExportError::Io("media data is still loading; run the export as a stepped job".into())),
            }
        }
    }
    let t0 = web_time::Instant::now();
    let q = project.sequence(seq).ok_or(ExportError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let range = export_range(project, seq, settings)?;
    let f0 = rate.frame_at(range.start);
    let f1 = rate.frame_at(range.end() - Tick(1)) + 1;
    let nframes = (f1 - f0).max(0) as u64;
    let w = (((q.settings.width as f32 * settings.scale).round() as u32).max(2)) & !1;
    let h = (((q.settings.height as f32 * settings.scale).round() as u32).max(2)) & !1;
    if !settings.part_of_batch {
        progress.total.store(if settings.format == Format::Wav { 1 } else { nframes }, Ordering::Relaxed);
        progress.set_status(format!("Exporting {} frames ({})", nframes, settings.format.label()));
    }
    let opts = RenderOptions { scale: w as f32 / q.settings.width as f32, captions: settings.burn_captions, ..Default::default() };
    let render = |f: i64| -> Vec<u8> {
        let img = filmcraft_render::render_sequence(project, seq, rate.tick_of(f), opts, sources);
        let mut rgba = img.over_black_rgba8();
        if img.w as u32 != w || img.h as u32 != h {
            // even-size crop/pad
            let mut out = vec![0u8; (w * h * 4) as usize];
            for y in 0..(h as usize).min(img.h) {
                let n = (w as usize).min(img.w) * 4;
                out[y * w as usize * 4..y * w as usize * 4 + n].copy_from_slice(&rgba[y * img.w * 4..y * img.w * 4 + n]);
            }
            rgba = out;
        }
        rgba
    };
    let batch = rayon::current_num_threads().clamp(2, 16) as i64;
    let sr = q.settings.sample_rate;
    let bytes = match settings.format {
        Format::Wav => {
            let n = range.duration.to_units_floor(sr as i64) as usize;
            let buf = filmcraft_render::audio::mix_sequence(project, q, range.start.to_units_floor(sr as i64), n, sources);
            let data = filmcraft_media::wav::write_wav16(&buf.interleaved(), 2, sr);
            let n = write_output(settings, &settings.path, data)?;
            progress.done.store(1, Ordering::Relaxed);
            n
        }
        Format::PngSequence => {
            let base = settings.path.trim_end_matches(".png").to_string();
            let mut total = 0u64;
            let mut f = f0;
            while f < f1 {
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err(ExportError::Cancelled);
                }
                let end = (f + batch).min(f1);
                let written: Vec<Result<u64>> = (f..end)
                    .into_par_iter()
                    .map(|fi| {
                        let rgba = render(fi);
                        let path = format!("{base}_{:05}.png", fi - f0);
                        if settings.sink.is_some() {
                            let mut png = Vec::new();
                            image::ImageEncoder::write_image(image::codecs::png::PngEncoder::new(&mut png), &rgba, w, h, image::ExtendedColorType::Rgba8)
                                .map_err(|e| ExportError::Encode(e.to_string()))?;
                            return write_output(settings, &path, png);
                        }
                        image::save_buffer(&path, &rgba, w, h, image::ExtendedColorType::Rgba8).map_err(|e| ExportError::Io(e.to_string()))?;
                        Ok(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0))
                    })
                    .collect();
                for r in written {
                    total += r?;
                }
                progress.done.fetch_add((end - f) as u64, Ordering::Relaxed);
                f = end;
            }
            total
        }
        Format::Gif => {
            let mut out = Out::create(settings)?;
            let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut out, 10);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite).map_err(|e| ExportError::Encode(e.to_string()))?;
            let delay = image::Delay::from_numer_denom_ms((1000 * rate.den) as u32, rate.num as u32);
            let mut f = f0;
            while f < f1 {
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err(ExportError::Cancelled);
                }
                let end = (f + batch).min(f1);
                let frames: Vec<Vec<u8>> = (f..end).into_par_iter().map(render).collect();
                for rgba in frames {
                    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| ExportError::Encode("frame".into()))?;
                    enc.encode_frame(image::Frame::from_parts(img, 0, 0, delay)).map_err(|e| ExportError::Encode(e.to_string()))?;
                }
                progress.done.fetch_add((end - f) as u64, Ordering::Relaxed);
                f = end;
            }
            drop(enc);
            out.finish(settings)?
        }
        Format::H264 | Format::ProRes | Format::DnxHr | Format::Mjpeg => unreachable!("stepped export"),
    };
    let secs = t0.elapsed().as_secs_f64();
    if !settings.part_of_batch {
        progress.finished.store(true, Ordering::Relaxed);
        progress.set_status(format!("Done in {secs:.1}s"));
    }
    Ok(Report { path: settings.path.clone(), frames: nframes, seconds: secs, bytes, render_fps: nframes as f64 / secs.max(1e-6) })
}

#[cfg(test)]
mod tests;
