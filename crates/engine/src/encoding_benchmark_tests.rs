//! Manual end-to-end-render / isolated-encode comparison using the real built-in demo sequence.
//!
//! Run with `cargo test -p filmcraft-engine compare_demo_cpu_and_nvenc -- --ignored --nocapture`.
//! Set `FILMCRAFT_BENCH_FRAMES` / `FILMCRAFT_BENCH_WARMUP` to adjust the sampled sequence range.

use std::time::{Duration, Instant};

use filmcraft_export::{EncoderFrame, ExportSettings, Format, VideoEncoderPreference, create_video_encoder};
use filmcraft_render::{RenderOptions, render_sequence};
use serde_json::json;

use crate::Session;

#[test]
#[ignore = "manual performance comparison; requires NVIDIA NVENC hardware"]
fn compare_demo_cpu_and_nvenc() {
    let frame_count = env_u32("FILMCRAFT_BENCH_FRAMES", 90) as usize;
    let warmup = env_u32("FILMCRAFT_BENCH_WARMUP", 8) as usize;
    let bitrate_kbps = env_u32("FILMCRAFT_BENCH_BITRATE_KBPS", 20_000);
    assert!(frame_count > 0, "frame count must be nonzero");

    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).expect("open built-in FilmCraft demo");
    let seq_id = session.state.active_sequence.expect("demo sequence");
    let seq = session.project.sequence(seq_id).expect("demo sequence settings");
    let (width, height, rate) = (seq.settings.width, seq.settings.height, seq.settings.frame_rate);
    assert_eq!((width, height), (1920, 1080), "benchmark expects the default 1080p demo sequence");
    assert!(!seq.settings.color.working.is_hdr(), "benchmark currently exercises SDR H.264");

    let project = session.project.clone();
    let provider = session.media.full_res_provider(project.clone(), session.services.clone());
    let base_settings = ExportSettings { format: Format::H264, bitrate_kbps, include_audio: false, sdr: true, ..Default::default() };

    let nvenc = measure_backend(
        "NVIDIA NVENC",
        &project,
        seq_id,
        &provider,
        width,
        height,
        rate,
        frame_count,
        warmup,
        &ExportSettings { video_encoder: VideoEncoderPreference::Hardware, ..base_settings.clone() },
    );
    let cpu = measure_backend(
        "Software H.264",
        &project,
        seq_id,
        &provider,
        width,
        height,
        rate,
        frame_count,
        warmup,
        &ExportSettings { video_encoder: VideoEncoderPreference::Software, ..base_settings },
    );

    println!(
        "\nFilmCraft demo H.264 backend comparison ({}x{} @ {}, target {} kb/s; {} measured frames)",
        width,
        height,
        rate.label(),
        bitrate_kbps,
        frame_count
    );
    println!("Same CPU-rendered demo frames and settings; render time excluded. Encoder-side color conversion and host↔GPU copies included.");
    assert_eq!(nvenc.input_checksum, cpu.input_checksum, "backends did not receive identical rendered frame bytes");
    println!("Input RGBA checksum: {:016x}", nvenc.input_checksum);
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
    input_checksum: u64,
}

#[allow(clippy::too_many_arguments)]
fn measure_backend(
    name: &'static str,
    project: &filmcraft_project::Project,
    seq_id: filmcraft_project::ItemId,
    provider: &dyn filmcraft_render::SourceProvider,
    width: u32,
    height: u32,
    rate: filmcraft_time::FrameRate,
    frame_count: usize,
    warmup: usize,
    settings: &ExportSettings,
) -> BenchmarkResult {
    let create = || create_video_encoder(Format::H264, width, height, rate, settings).expect("encoder initialization");
    let render = |index: usize| {
        let image = render_sequence(project, seq_id, rate.tick_of(index as i64), RenderOptions::default(), provider);
        image.over_black_rgba8()
    };

    // Warm up a disposable encoder with real demo frames; exclude encoder initialization and all
    // rendering from the timed session.
    if warmup > 0 {
        let mut encoder = create();
        for i in 0..warmup {
            let rgba = render(i);
            let input = EncoderFrame { width, height, rgba: &rgba, hdr: None, index: i as u64 };
            let _ = encoder.encode(&input).expect("warm-up encode");
        }
        let _ = encoder.flush().expect("warm-up flush");
    }

    let mut encoder = create();
    let mut elapsed = Duration::ZERO;
    let mut encoded_bytes = 0u64;
    let mut input_checksum = 0xcbf29ce484222325u64;
    for i in 0..frame_count {
        let frame_index = warmup + i;
        let rgba = render(frame_index);
        for &byte in &rgba {
            input_checksum = (input_checksum ^ byte as u64).wrapping_mul(0x100000001b3);
        }
        let input = EncoderFrame { width, height, rgba: &rgba, hdr: None, index: frame_index as u64 };
        let start = Instant::now();
        let packets = encoder.encode(&input).expect("timed encode");
        elapsed += start.elapsed();
        encoded_bytes += packets.iter().map(|p| p.data.len() as u64).sum::<u64>();
    }
    let start = Instant::now();
    let tail = encoder.flush().expect("timed flush");
    elapsed += start.elapsed();
    encoded_bytes += tail.iter().map(|p| p.data.len() as u64).sum::<u64>();

    let fps = frame_count as f64 / elapsed.as_secs_f64();
    BenchmarkResult {
        name,
        fps,
        mpix_s: fps * width as f64 * height as f64 / 1_000_000.0,
        actual_mbps: encoded_bytes as f64 * 8.0 * rate.num as f64 / (frame_count as f64 * rate.den as f64) / 1_000_000.0,
        mib: encoded_bytes as f64 / (1024.0 * 1024.0),
        input_checksum,
    }
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
}
