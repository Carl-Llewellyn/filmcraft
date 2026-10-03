//! Linux NVIDIA NVENC H.264 backend. Frames are rendered by the normal FilmCraft pipeline on
//! the host; only encoding is sent to CUDA/NVENC, so this does not change the UI's GPU.

use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use filmcraft_isobmff::{AvcConfig, SampleEntry};
use filmcraft_time::FrameRate;
use shiguredo_nvcodec::{
    BufferFormat, CodecConfig, EncodeOptions, Encoder, EncoderConfig, FnEncodeHandler, H264EncoderConfig, PictureType, Preset, RateControlMode, TuningInfo,
};

use crate::{
    ColorSignal, EncodedPacket, EncoderFrame, ExportError, ExportSettings, Format, Result, VideoEncoder, VideoEncoderPreference, rgba_to_yuv420_8,
    rgbf_to_yuv420_8,
};

type CallbackResult = std::result::Result<shiguredo_nvcodec::EncodedFrame<u64>, shiguredo_nvcodec::Error>;

pub(super) fn factory(format: Format, w: u32, h: u32, rate: FrameRate, settings: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
    if format != Format::H264 || settings.video_encoder == VideoEncoderPreference::Software {
        return None;
    }

    let Some(device_id) = device_id() else {
        return if settings.video_encoder == VideoEncoderPreference::Hardware {
            Some(Err(ExportError::Unsupported("NVIDIA NVENC is not available".into())))
        } else {
            None
        };
    };
    if settings.signal.is_hdr() {
        return if settings.video_encoder == VideoEncoderPreference::Hardware {
            Some(Err(ExportError::Unsupported("NVIDIA NVENC HDR export is not supported yet; choose Software".into())))
        } else {
            None
        };
    }
    if !w.is_multiple_of(2) || !h.is_multiple_of(2) {
        return Some(Err(ExportError::Unsupported("NVIDIA NVENC requires even frame dimensions".into())));
    }

    let bitrate = settings.bitrate_kbps.max(100).saturating_mul(1000);
    let gop = ((rate.num as f64 / rate.den as f64) * 2.0).round().max(1.0) as u32;
    let config = EncoderConfig {
        codec: CodecConfig::H264(H264EncoderConfig { profile: None, idr_period: Some(gop) }),
        width: w,
        height: h,
        max_encode_width: None,
        max_encode_height: None,
        framerate_num: rate.num as u32,
        framerate_den: rate.den as u32,
        average_bitrate: Some(bitrate),
        preset: Preset::P4,
        tuning_info: TuningInfo::HIGH_QUALITY,
        rate_control_mode: RateControlMode::Vbr,
        gop_length: Some(gop),
        // P-only GOP avoids reordering; MP4 composition offsets remain zero.
        frame_interval_p: 1,
        buffer_format: BufferFormat::Nv12,
        device_id,
    };
    let (tx, rx) = mpsc::channel::<CallbackResult>();
    let encoder = Encoder::new(
        config,
        FnEncodeHandler::new(move |result| {
            let _ = tx.send(result);
        }),
    );
    let encoder = match encoder {
        Ok(encoder) => encoder,
        Err(e) if settings.video_encoder == VideoEncoderPreference::Hardware => return Some(Err(ExportError::Encode(e.to_string()))),
        Err(_) => return None,
    };
    Some(Ok(Box::new(NvencEncoder {
        encoder,
        rx,
        w,
        h,
        rate,
        signal: settings.signal,
        y: Vec::new(),
        u: Vec::new(),
        v: Vec::new(),
        nv12: Vec::new(),
        sps: Vec::new(),
        pps: Vec::new(),
        started: false,
    }) as Box<dyn VideoEncoder>))
}

fn device_id() -> Option<i32> {
    static DEVICE: OnceLock<Option<i32>> = OnceLock::new();
    *DEVICE.get_or_init(|| {
        let count = shiguredo_nvcodec::device_count().ok()?;
        (0..count).find(|&id| shiguredo_nvcodec::device_name(id).is_ok_and(|name| name.to_ascii_lowercase().contains("nvidia")))
    })
}

pub(super) fn available() -> bool {
    device_id().is_some()
}

struct NvencEncoder<H: shiguredo_nvcodec::EncodeHandler<UserData = u64, Error = shiguredo_nvcodec::Error>> {
    encoder: Encoder<H>,
    rx: Receiver<CallbackResult>,
    w: u32,
    h: u32,
    rate: FrameRate,
    signal: ColorSignal,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    nv12: Vec<u8>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    started: bool,
}

impl<H: shiguredo_nvcodec::EncodeHandler<UserData = u64, Error = shiguredo_nvcodec::Error>> NvencEncoder<H> {
    fn collect(&mut self, frame: shiguredo_nvcodec::EncodedFrame<u64>) -> Result<EncodedPacket> {
        let mut data = Vec::new();
        let mut key = matches!(frame.picture_type(), PictureType::Idr | PictureType::I);
        for nal in filmcraft_bitstream::annexb_nals(frame.data()) {
            let Some(&header) = nal.first() else { continue };
            match header & 0x1f {
                7 => self.sps = nal.to_vec(),
                8 => self.pps = nal.to_vec(),
                5 => key = true,
                _ => {}
            }
            // Parameter sets are stored in avcC, not repeated in length-prefixed MP4 samples.
            if matches!(header & 0x1f, 7..=9) {
                continue;
            }
            data.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            data.extend_from_slice(nal);
        }
        if data.is_empty() {
            return Err(ExportError::Encode("NVENC returned an empty H.264 access unit".into()));
        }
        Ok(EncodedPacket { data, key, duration: self.rate.den as u32, composition_offset: 0 })
    }

    fn receive_one(&mut self) -> Result<EncodedPacket> {
        let frame =
            self.rx.recv().map_err(|e| ExportError::Encode(format!("NVENC callback channel closed: {e}")))?.map_err(|e| ExportError::Encode(e.to_string()))?;
        self.collect(frame)
    }

    fn pack_nv12(&mut self) {
        self.nv12.clear();
        self.nv12.extend_from_slice(&self.y);
        for (&u, &v) in self.u.iter().zip(&self.v) {
            self.nv12.extend_from_slice(&[u, v]);
        }
    }
}

impl<H: shiguredo_nvcodec::EncodeHandler<UserData = u64, Error = shiguredo_nvcodec::Error>> VideoEncoder for NvencEncoder<H> {
    fn sample_entry(&self) -> SampleEntry {
        let mut entry = SampleEntry::avc(AvcConfig::new(vec![self.sps.clone()], vec![self.pps.clone()], 4), self.w as u16, self.h as u16);
        self.signal.apply_to(&mut entry, false);
        entry
    }

    fn timescale(&self) -> u32 {
        self.rate.num as u32
    }

    fn encode(&mut self, frame: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        if let Some(rgb) = frame.hdr {
            let (kr, kb) = self.signal.kr_kb();
            rgbf_to_yuv420_8(rgb, frame.width as usize, frame.height as usize, kr, kb, &mut self.y, &mut self.u, &mut self.v);
        } else {
            rgba_to_yuv420_8(frame.rgba, frame.width as usize, frame.height as usize, &mut self.y, &mut self.u, &mut self.v);
        }
        self.pack_nv12();
        self.encoder
            .encode(&self.nv12, &EncodeOptions { force_intra: !self.started, force_idr: !self.started, output_spspps: !self.started }, frame.index)
            .map_err(|e| ExportError::Encode(e.to_string()))?;
        self.started = true;
        // This adapter deliberately synchronizes one input to one output. It makes export ordering
        // and sample-description setup deterministic while the renderer remains batched/parallel.
        Ok(vec![self.receive_one()?])
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        self.encoder.flush().map_err(|e| ExportError::Encode(e.to_string()))?;
        let mut packets = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(Ok(frame)) => packets.push(self.collect(frame)?),
                Ok(Err(e)) => return Err(ExportError::Encode(e.to_string())),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        Ok(packets)
    }

    fn name(&self) -> &'static str {
        "NVIDIA NVENC"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn nvenc_encodes_a_real_frame_when_a_device_is_available() {
        let settings = ExportSettings {
            format: Format::H264,
            video_encoder: VideoEncoderPreference::Hardware,
            path: String::new(),
            range: None,
            scale: 1.0,
            include_audio: false,
            quality: 80,
            bitrate_kbps: 2_000,
            burn_captions: false,
            part_of_batch: false,
            prores_profile: String::new(),
            dnx_profile: String::new(),
            sdr: true,
            signal: ColorSignal::default(),
            sink: None,
            ..Default::default()
        };
        let Some(Ok(mut encoder)) = factory(Format::H264, 64, 64, FrameRate { num: 30, den: 1 }, &settings) else {
            // CI and developer machines without the NVIDIA driver should still run this suite.
            return;
        };
        let rgba = vec![128; 64 * 64 * 4];
        let packets = encoder.encode(&EncoderFrame { width: 64, height: 64, rgba: &rgba, hdr: None, index: 0 }).unwrap();
        assert_eq!(packets.len(), 1);
        assert!(packets[0].key);
        let entry = encoder.sample_entry();
        let filmcraft_isobmff::CodecConfig::Avc(avcc) = entry.codec else { panic!("expected AVC sample entry") };
        assert!(!avcc.sps.is_empty() && !avcc.pps.is_empty());
        assert!(encoder.flush().unwrap().is_empty());
    }

    /// Repeatable backend-only benchmark. Invoke with:
    /// `cargo test -p filmcraft-export compare_cpu_and_nvenc -- --ignored --nocapture`
    /// Optional settings: FILMCRAFT_BENCH_WIDTH, _HEIGHT, _FRAMES, _WARMUP, _BITRATE_KBPS.
    #[test]
    #[ignore = "manual performance comparison; requires NVIDIA NVENC hardware"]
    fn compare_synthetic_frames_cpu_and_nvenc() {
        let width = env_u32("FILMCRAFT_BENCH_WIDTH", 1280);
        let height = env_u32("FILMCRAFT_BENCH_HEIGHT", 720);
        let frames_count = env_u32("FILMCRAFT_BENCH_FRAMES", 180) as usize;
        let warmup = env_u32("FILMCRAFT_BENCH_WARMUP", 24) as usize;
        let bitrate_kbps = env_u32("FILMCRAFT_BENCH_BITRATE_KBPS", 8_000);
        assert!(width >= 64 && height >= 64 && width.is_multiple_of(2) && height.is_multiple_of(2), "dimensions must be even and at least 64x64");
        assert!(frames_count > 0);

        let rate = FrameRate { num: 30, den: 1 };
        let settings = ExportSettings {
            format: Format::H264,
            path: String::new(),
            range: None,
            scale: 1.0,
            include_audio: false,
            quality: 80,
            bitrate_kbps,
            burn_captions: false,
            part_of_batch: false,
            prores_profile: String::new(),
            dnx_profile: String::new(),
            sdr: true,
            signal: ColorSignal::default(),
            sink: None,
            video_encoder: VideoEncoderPreference::Auto,
            ..Default::default()
        };
        // Build the exact same small set of non-static images once, outside the timed region.
        // This excludes project rendering and source generation while still exercising the real
        // RGBA→YUV conversion performed by both production encoders.
        let images = benchmark_images(width, height);
        let nvenc =
            measure_backend("NVIDIA NVENC", width, height, rate, &settings, frames_count, warmup, &images, |s| factory(Format::H264, width, height, rate, s));
        let cpu = measure_backend("Software H.264", width, height, rate, &settings, frames_count, warmup, &images, |s| {
            crate::h264_factory(Format::H264, width, height, rate, s)
        });

        println!("\nH.264 backend-only comparison (same {}x{}, 30 fps input, target {} kb/s, {} measured frames)", width, height, bitrate_kbps, frames_count);
        println!("Rendering/source generation excluded; encoder-side pixel conversion and host↔GPU copies included.");
        for result in [&nvenc, &cpu] {
            println!(
                "{:<18} {:>8.1} frames/s | {:>7.1} Mpixel/s | {:>7.1} Mb/s actual | {:>7.2} MiB output",
                result.name, result.fps, result.mpix_s, result.actual_mbps, result.mib
            );
        }
        println!("NVENC throughput vs CPU: {:.2}x", nvenc.fps / cpu.fps);
    }

    struct BenchmarkResult {
        name: &'static str,
        fps: f64,
        mpix_s: f64,
        actual_mbps: f64,
        mib: f64,
    }

    fn measure_backend<F>(
        name: &'static str,
        width: u32,
        height: u32,
        rate: FrameRate,
        settings: &ExportSettings,
        frame_count: usize,
        warmup: usize,
        images: &[Vec<u8>],
        make_encoder: F,
    ) -> BenchmarkResult
    where
        F: Fn(&ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>>,
    {
        let create = || make_encoder(settings).expect("backend unavailable").expect("encoder initialization failed");
        // Warm up one disposable encoder, then time a fresh session so initialization is excluded.
        if warmup > 0 {
            let mut encoder = create();
            for i in 0..warmup {
                let rgba = &images[i % images.len()];
                let _ = encoder.encode(&EncoderFrame { width, height, rgba, hdr: None, index: i as u64 }).unwrap();
            }
            let _ = encoder.flush().unwrap();
        }

        let mut encoder = create();
        let mut elapsed = Duration::ZERO;
        let mut encoded_bytes = 0u64;
        for i in 0..frame_count {
            let rgba = &images[i % images.len()];
            let input = EncoderFrame { width, height, rgba, hdr: None, index: i as u64 };
            let start = Instant::now();
            let packets = encoder.encode(&input).unwrap();
            elapsed += start.elapsed();
            encoded_bytes += packets.iter().map(|p| p.data.len() as u64).sum::<u64>();
        }
        let start = Instant::now();
        let tail = encoder.flush().unwrap();
        elapsed += start.elapsed();
        encoded_bytes += tail.iter().map(|p| p.data.len() as u64).sum::<u64>();

        let seconds = elapsed.as_secs_f64();
        let fps = frame_count as f64 / seconds;
        BenchmarkResult {
            name,
            fps,
            mpix_s: fps * width as f64 * height as f64 / 1_000_000.0,
            actual_mbps: encoded_bytes as f64 * 8.0 * rate.num as f64 / (frame_count as f64 * rate.den as f64) / 1_000_000.0,
            mib: encoded_bytes as f64 / (1024.0 * 1024.0),
        }
    }

    fn benchmark_images(width: u32, height: u32) -> Vec<Vec<u8>> {
        (0..4)
            .map(|variant| {
                let mut rgba = vec![255; (width * height * 4) as usize];
                for y in 0..height {
                    for x in 0..width {
                        let i = ((y * width + x) * 4) as usize;
                        rgba[i] = (x.wrapping_mul(13).wrapping_add(y * 7).wrapping_add(variant * 53) & 255) as u8;
                        rgba[i + 1] = (x.wrapping_mul(3).wrapping_add(y * 17).wrapping_add(variant * 29) & 255) as u8;
                        rgba[i + 2] = (x ^ y.wrapping_mul(11) ^ variant.wrapping_mul(71)) as u8;
                    }
                }
                rgba
            })
            .collect()
    }

    fn env_u32(name: &str, default: u32) -> u32 {
        std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
    }
}
