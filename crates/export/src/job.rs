//! Step-wise export of the encoded-video formats (H.264 MP4, ProRes / DNxHR / Motion-JPEG MOV).
//!
//! [`Exporter::step`] renders, encodes and muxes one batch of frames (plus the audio up to its
//! end) per call, so a host without threads (the web app) can run an export a slice at a time
//! between UI frames; [`crate::export`] simply loops it on a worker thread.
//!
//! When a source's bytes are not available yet (asynchronous web reads mark
//! [`filmcraft_media::pending`]), the batch is rendered again on the next call: nothing is encoded
//! from frames or audio with missing media.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use filmcraft_frame::AudioBuffer;
use filmcraft_isobmff::{Brand, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};
use filmcraft_project::{ItemId, Project};
use filmcraft_render::{RenderOptions, SourceProvider};
use filmcraft_time::{FrameRate, Tick, TimeRange};
use rayon::prelude::*;

use crate::{
    AudioEncoder, ColorSignal, EncodedPacket, EncoderFrame, ExportError, ExportSettings, Format, Out, Progress, Report, Result, VideoEncoder, audio_factories,
    export_range,
};

/// What one [`Exporter::step`] did.
#[derive(Debug)]
pub enum Step {
    /// A batch was encoded; call again.
    Progress,
    /// Media bytes are still loading: nothing was encoded, call again later.
    Pending,
    /// The file is complete.
    Done(Report),
}

/// The immutable part of an export (rendering a frame needs only this).
struct Plan {
    project: Arc<Project>,
    seq: ItemId,
    rate: FrameRate,
    w: u32,
    h: u32,
    opts: RenderOptions,
    hdr_out: bool,
    out_tf: Option<filmcraft_color::OutputTransform>,
}

impl Plan {
    /// Straight sRGB RGBA8 at the (even) output size, or for HDR exports the encoded R'G'B'.
    fn frame(&self, f: i64, sources: &dyn SourceProvider) -> (Vec<u8>, Vec<f32>) {
        let (w, h) = (self.w, self.h);
        let img = filmcraft_render::render_sequence(&self.project, self.seq, self.rate.tick_of(f), self.opts, sources);
        if let Some(tf) = self.out_tf.as_ref().filter(|_| self.hdr_out) {
            // HDR: encoded R'G'B' floats over black, padded/cropped to the even output size
            let mut out = vec![0f32; (w * h * 3) as usize];
            let black = tf.encode([0.0; 3]);
            for (y, row) in out.chunks_exact_mut(w as usize * 3).enumerate() {
                for (x, o) in row.chunks_exact_mut(3).enumerate() {
                    let c = if x < img.w && y < img.h {
                        let i = (y * img.w + x) * 4;
                        tf.encode([img.px[i], img.px[i + 1], img.px[i + 2]])
                    } else {
                        black
                    };
                    o.copy_from_slice(&c);
                }
            }
            return (Vec::new(), out);
        }
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
        (rgba, Vec::new())
    }
}

/// An export of an encoded-video format, advanced one batch at a time.
pub struct Exporter {
    plan: Plan,
    settings: ExportSettings,
    range: TimeRange,
    f0: i64,
    f1: i64,
    /// Next frame to render.
    next: i64,
    batch: i64,
    sr: u32,
    brand: Brand,
    venc: Box<dyn VideoEncoder>,
    aenc: Option<Box<dyn AudioEncoder>>,
    /// Created after the first batch (encoders may finalise their codec config then).
    mux: Option<Mp4Writer<Out>>,
    vt: usize,
    at: Option<usize>,
    audio_cursor: i64,
    t0: web_time::Instant,
}

/// Whether [`Exporter`] handles a format.
pub fn stepped(format: Format) -> bool {
    matches!(format, Format::H264 | Format::ProRes | Format::DnxHr | Format::Mjpeg)
}

impl Exporter {
    /// Set up an export (encoder, output size, colour signalling); sets `progress.total`.
    pub fn new(project: Arc<Project>, seq: ItemId, settings: &ExportSettings, progress: &Progress) -> Result<Self> {
        if !stepped(settings.format) {
            return Err(ExportError::Unsupported(format!("{} is not a stepped export", settings.format.label())));
        }
        let q = project.sequence(seq).ok_or(ExportError::NoSequence)?;
        // HDR sequences export HDR (H.264 / ProRes) unless SDR is asked for
        let pipe = q.settings.color;
        let hdr_out = pipe.working.is_hdr() && !settings.sdr && matches!(settings.format, Format::H264 | Format::ProRes | Format::DnxHr);
        let mut settings = settings.clone();
        settings.signal = match (hdr_out, pipe.working) {
            (true, filmcraft_color::WorkingSpace::Rec2100Pq) => ColorSignal::PQ,
            (true, _) => ColorSignal::HLG,
            _ => ColorSignal::default(),
        };
        let out_tf = hdr_out.then(|| filmcraft_color::OutputTransform::new(&pipe, pipe.working.output_space()));
        let rate = q.settings.frame_rate;
        let range = export_range(&project, seq, &settings)?;
        let f0 = rate.frame_at(range.start);
        let f1 = rate.frame_at(range.end() - Tick(1)) + 1;
        let nframes = (f1 - f0).max(0) as u64;
        let w = (((q.settings.width as f32 * settings.scale).round() as u32).max(2)) & !1;
        let h = (((q.settings.height as f32 * settings.scale).round() as u32).max(2)) & !1;
        if !settings.part_of_batch {
            progress.total.store(nframes, Ordering::Relaxed);
            progress.set_status(format!("Exporting {} frames ({})", nframes, settings.format.label()));
        }
        let opts = RenderOptions { scale: w as f32 / q.settings.width as f32, captions: settings.burn_captions, working_output: hdr_out, ..Default::default() };
        let sr = q.settings.sample_rate;
        let venc = crate::create_video_encoder(settings.format, w, h, rate, &settings)?;
        if !settings.part_of_batch {
            progress.set_status(format!("Exporting {} frames ({}, {})", nframes, settings.format.label(), venc.name()));
        }
        let brand = if settings.format == Format::H264 { Brand::Mp4 } else { Brand::Mov };
        // audio: AAC for MP4 when available, PCM otherwise (MOV)
        let aenc: Option<Box<dyn AudioEncoder>> = if settings.include_audio && brand == Brand::Mp4 {
            audio_factories().read().unwrap_or_else(|e| e.into_inner()).iter().find_map(|f| f(settings.format, sr, 2, &settings)).transpose()?
        } else {
            None
        };
        Ok(Self {
            plan: Plan { project, seq, rate, w, h, opts, hdr_out, out_tf },
            range,
            f0,
            f1,
            next: f0,
            batch: rayon::current_num_threads().clamp(2, 16) as i64,
            sr,
            brand,
            venc,
            aenc,
            mux: None,
            vt: 0,
            at: None,
            audio_cursor: range.start.to_units_floor(sr as i64),
            t0: web_time::Instant::now(),
            settings,
        })
    }

    /// Frames in the export.
    pub fn frames(&self) -> u64 {
        (self.f1 - self.f0).max(0) as u64
    }

    /// Frames per [`Self::step`] (default: one per core); 1 keeps web UI frames short.
    pub fn set_batch(&mut self, n: i64) {
        self.batch = n.max(1);
    }

    /// Mix the sequence audio from the cursor up to `until` (None: no audio track or nothing to mix).
    fn mix_until(&self, until: Tick, sources: &dyn SourceProvider) -> Option<(AudioBuffer, i64)> {
        self.settings.include_audio.then_some(())?;
        let end = until.to_units_floor(self.sr as i64);
        if end <= self.audio_cursor {
            return None;
        }
        let q = self.plan.project.sequence(self.plan.seq)?;
        let n = (end - self.audio_cursor) as usize;
        Some((filmcraft_render::audio::mix_sequence(&self.plan.project, q, self.audio_cursor, n, sources), end))
    }

    fn write_audio(&mut self, mixed: Option<(AudioBuffer, i64)>) -> Result<()> {
        let (Some(at), Some((buf, end))) = (self.at, mixed) else { return Ok(()) };
        let n = (end - self.audio_cursor) as usize;
        self.audio_cursor = end;
        let mux = self.mux.as_mut().expect("mux created");
        match self.aenc.as_mut() {
            Some(a) => {
                for au in a.encode(&buf.channels)? {
                    mux.write_sample(at, WriteSample { data: &au, duration: a.frame_size(), composition_offset: 0, is_sync: true })
                        .map_err(|e| ExportError::Io(e.to_string()))?;
                }
            }
            None => {
                let mut pcm = Vec::with_capacity(n * 4);
                for s in buf.interleaved() {
                    pcm.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
                }
                mux.write_sample(at, WriteSample { data: &pcm, duration: n as u32, composition_offset: 0, is_sync: true })
                    .map_err(|e| ExportError::Io(e.to_string()))?;
            }
        }
        Ok(())
    }

    fn write_video(&mut self, packets: Vec<EncodedPacket>) -> Result<()> {
        let mux = self.mux.as_mut().expect("mux created");
        for p in packets {
            mux.write_sample(self.vt, WriteSample { data: &p.data, duration: p.duration, composition_offset: p.composition_offset, is_sync: p.key })
                .map_err(|e| ExportError::Io(e.to_string()))?;
        }
        Ok(())
    }

    /// Create the muxer and its tracks (after the first batch was encoded).
    fn open_mux(&mut self) -> Result<()> {
        let file = Out::create(&self.settings)?;
        let mut mux = Mp4Writer::new(file, WriterOptions::new(self.brand)).map_err(|e| ExportError::Io(e.to_string()))?;
        let mut vcfg = TrackConfig::new(self.venc.sample_entry(), self.venc.timescale());
        vcfg.media_start = self.venc.media_start();
        self.vt = mux.add_track(vcfg).map_err(|e| ExportError::Io(e.to_string()))?;
        self.at = if !self.settings.include_audio {
            None
        } else if let Some(a) = &self.aenc {
            let mut c = TrackConfig::new(a.sample_entry(), self.sr);
            c.media_start = Some(a.priming() as i64);
            Some(mux.add_track(c).map_err(|e| ExportError::Io(e.to_string()))?)
        } else if self.brand == Brand::Mov {
            let pcm = PcmConfig { bits: 16, float: false, big_endian: false, signed: true, channels: 2, sample_rate: self.sr as f64 };
            Some(mux.add_track(TrackConfig::new(SampleEntry::pcm(pcm), self.sr)).map_err(|e| ExportError::Io(e.to_string()))?)
        } else {
            None
        };
        self.mux = Some(mux);
        Ok(())
    }

    /// Render, encode and mux the next batch (or finish the file).
    pub fn step(&mut self, sources: &dyn SourceProvider, progress: &Progress) -> Result<Step> {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        let _ = filmcraft_media::pending::take();
        if self.next < self.f1 {
            let (f, end) = (self.next, (self.next + self.batch).min(self.f1));
            let plan = &self.plan;
            let frames: Vec<(Vec<u8>, Vec<f32>)> = (f..end).into_par_iter().map(|fi| plan.frame(fi, sources)).collect();
            let has_audio = self.mux.is_none() || self.at.is_some();
            let mixed = if has_audio { self.mix_until(self.plan.rate.tick_of(end), sources) } else { None };
            if filmcraft_media::pending::take() {
                return Ok(Step::Pending);
            }
            let mut packets = Vec::new();
            for (k, (rgba, hdr)) in frames.iter().enumerate() {
                let fr = EncoderFrame {
                    width: self.plan.w,
                    height: self.plan.h,
                    rgba,
                    hdr: self.plan.hdr_out.then_some(hdr.as_slice()),
                    index: (f - self.f0) as u64 + k as u64,
                };
                packets.extend(self.venc.encode(&fr)?);
            }
            if self.mux.is_none() {
                self.open_mux()?;
            }
            self.write_video(packets)?;
            self.write_audio(mixed)?;
            progress.done.fetch_add((end - f) as u64, Ordering::Relaxed);
            self.next = end;
            return Ok(Step::Progress);
        }
        let mixed = self.mix_until(self.range.end(), sources);
        if filmcraft_media::pending::take() {
            return Ok(Step::Pending);
        }
        if self.mux.is_none() {
            self.open_mux()?;
        }
        let tail = self.venc.flush()?;
        self.write_video(tail)?;
        self.write_audio(mixed)?;
        if let (Some(at), Some(a)) = (self.at, self.aenc.as_mut()) {
            let fs = a.frame_size();
            let mux = self.mux.as_mut().expect("mux created");
            for au in a.flush()? {
                mux.write_sample(at, WriteSample { data: &au, duration: fs, composition_offset: 0, is_sync: true })
                    .map_err(|e| ExportError::Io(e.to_string()))?;
            }
        }
        let w = self.mux.take().expect("mux created").finish().map_err(|e| ExportError::Io(e.to_string()))?;
        let bytes = w.finish(&self.settings)?;
        let secs = self.t0.elapsed().as_secs_f64();
        let nframes = self.frames();
        if !self.settings.part_of_batch {
            progress.finished.store(true, Ordering::Relaxed);
            progress.set_status(format!("Done in {secs:.1}s"));
        }
        Ok(Step::Done(Report { path: self.settings.path.clone(), frames: nframes, seconds: secs, bytes, render_fps: nframes as f64 / secs.max(1e-6) }))
    }
}
