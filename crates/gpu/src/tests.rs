use super::*;
use filmcraft_geom::{Affine, Vec2};
use filmcraft_render::plan::execute_cpu;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

fn yuv_frame(w: u32, h: u32) -> Arc<VideoFrame> {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y: Vec<u8> = (0..w * h).map(|i| (16 + ((i % w) * 219 / w)) as u8).collect();
    let u: Vec<u8> = (0..cw * ch).map(|i| (64 + (i / cw) * 128 / ch) as u8).collect();
    let v: Vec<u8> = (0..cw * ch).map(|i| (200 - (i % cw) * 100 / cw) as u8).collect();
    Arc::new(VideoFrame {
        width: w,
        height: h,
        data: PixelData::Yuv8 { planes: [Arc::new(y), Arc::new(u), Arc::new(v)], chroma: Chroma::C420, alpha: None },
        color: filmcraft_color::ColorInfo::REC709,
        par: (1, 1),
        pts: Default::default(),
    })
}

#[test]
fn gpu_matches_cpu_plan() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let pixels = filmcraft_media::generators::render(
        &filmcraft_media::Generator::Demo(filmcraft_media::DemoScene::OceanSunset),
        320,
        180,
        1.0,
        24,
        filmcraft_time::FrameRate::FPS_24,
    );
    let rgba = Arc::new(VideoFrame::rgba8(320, 180, pixels));
    let (w, h) = (320usize, 180usize);
    let plan = FramePlan::Layers {
        width: w,
        height: h,
        layers: vec![
            PlanLayer { frame: yuv_frame(640, 360), matrix: Affine::scale(0.5, 0.5), opacity: 1.0, effect: None },
            PlanLayer {
                frame: rgba,
                matrix: Affine::motion(Vec2::new(200.0, 100.0), Vec2::new(0.4, 0.4), 12.0, Vec2::new(160.0, 90.0)),
                opacity: 0.7,
                effect: None,
            },
        ],
    };
    let cpu = execute_cpu(&plan).over_black_rgba8();
    c.composite(&plan);
    let (gw, gh, gpu) = c.read_output().expect("readback");
    assert_eq!((gw as usize, gh as usize), (w, h));
    // compare away from antialiased edges: mean abs error and 99th percentile
    let mut diffs: Vec<u32> =
        cpu.chunks(4).zip(gpu.chunks(4)).map(|(a, b)| (0..3).map(|k| (a[k] as i32 - b[k] as i32).unsigned_abs()).max().unwrap_or(0)).collect();
    diffs.sort_unstable();
    let p99 = diffs[diffs.len() * 99 / 100];
    let mean = diffs.iter().sum::<u32>() as f64 / diffs.len() as f64;
    assert!(p99 <= 6 && mean < 1.5, "p99 {p99}, mean {mean}");
    // cache: compositing the same plan again uploads nothing
    let before = c.uploaded_bytes;
    c.composite(&plan);
    assert_eq!(c.uploaded_bytes, before);
}

#[test]
fn gpu_brightness_contrast_matches_cpu_effect() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (w, h) = (48u32, 32u32);
    let pixels: Vec<f32> = (0..w * h)
        .flat_map(|i| {
            let x = (i % w) as f32 / (w - 1) as f32;
            let y = (i / w) as f32 / (h - 1) as f32;
            [x * 0.8, y * 0.7, (1.0 - x) * 0.6, 0.25 + (i % 4) as f32 * 0.25]
        })
        .collect();
    let plan = FramePlan::Layers {
        width: w as usize,
        height: h as usize,
        layers: vec![PlanLayer {
            frame: Arc::new(VideoFrame::rgba_f32(w, h, pixels)),
            matrix: Affine::IDENTITY,
            opacity: 1.0,
            effect: Some(filmcraft_render::plan::PlanEffect::BrightnessContrast { brightness: 22.0, contrast: 38.0 }),
        }],
    };
    let cpu = execute_cpu(&plan).over_black_rgba8();
    let mut compositor = GpuCompositor::new(&dev, &q);
    compositor.composite(&plan);
    let (_, _, gpu) = compositor.read_output().expect("readback");
    let max = cpu.as_chunks::<4>().0.iter().zip(gpu.as_chunks::<4>().0).flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c]))).max().unwrap_or(0);
    assert!(max <= 2, "brightness/contrast GPU output differs by up to {max} code values");
}

#[test]
fn half_float_conversion() {
    for v in [0.0f32, 1.0, 0.5, 0.123, 65504.0, -2.0, 1e-5] {
        let h = f32_to_f16(v);
        // decode
        let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
        let e = ((h >> 10) & 0x1f) as i32;
        let m = (h & 0x3ff) as f32;
        let d = if e == 0 { s * m * 2f32.powi(-24) } else { s * (1.0 + m / 1024.0) * 2f32.powi(e - 15) };
        assert!((d - v).abs() <= v.abs() * 1e-3 + 1e-6, "{v} → {d}");
    }
}

#[test]
fn prepared_upload_matches_inline_conversion() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (w, h) = (64u32, 36u32);
    let yuv16 = Arc::new(VideoFrame {
        width: w,
        height: h,
        data: PixelData::Yuv16 {
            planes: [
                Arc::new((0..w * h).map(|i| (64 + (i * 7) % 876) as u16).collect()),
                Arc::new((0..w * h / 2).map(|i| (64 + (i * 13) % 896) as u16).collect()),
                Arc::new((0..w * h / 2).map(|i| (960 - (i * 5) % 896) as u16).collect()),
            ],
            chroma: Chroma::C422,
            bits: 10,
            alpha: None,
        },
        color: filmcraft_color::ColorInfo::REC709,
        par: (1, 1),
        pts: Default::default(),
    });
    let f32_layer: Vec<f32> = (0..w * h * 4).map(|i| ((i * 37) % 1000) as f32 / 1000.0 * if i % 4 == 3 { 1.0 } else { 0.6 }).collect();
    let rgbaf = Arc::new(VideoFrame::rgba_f32(w, h, f32_layer.clone()));
    let layers = FramePlan::Layers {
        width: w as usize,
        height: h as usize,
        layers: vec![
            PlanLayer { frame: yuv16, matrix: Affine::IDENTITY, opacity: 1.0, effect: None },
            PlanLayer { frame: rgbaf, matrix: Affine::scale(0.5, 0.5), opacity: 0.8, effect: None },
        ],
    };
    let image = FramePlan::Image(filmcraft_render::Image { w: w as usize, h: h as usize, px: f32_layer });
    for plan in [layers, image] {
        let mut a = GpuCompositor::new(&dev, &q);
        a.composite(&plan);
        let inline = a.read_output().expect("readback");
        let mut b = GpuCompositor::new(&dev, &q);
        let prep = prepare(&plan);
        assert!(prep.bytes() > 0);
        b.composite_prepared(&plan, Some(&prep));
        let prepared = b.read_output().expect("readback");
        assert!(inline == prepared, "prepared upload differs");
    }
}

/// The upload cache is keyed by pixel-buffer address, so it must keep the buffer alive: otherwise
/// a new frame allocated at a freed frame's address is drawn with the stale texture (seen as
/// whole frames from a previous composite in `crates/golden` GPU parity).
#[test]
fn upload_cache_keeps_buffers_alive() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let px = Arc::new(vec![200u8; 16 * 8 * 4]);
    let frame = Arc::new(VideoFrame { width: 16, height: 8, data: PixelData::Rgba8(px.clone()), ..(*yuv_frame(16, 8)).clone() });
    let plan = FramePlan::Layers { width: 16, height: 8, layers: vec![PlanLayer { frame, matrix: Affine::IDENTITY, opacity: 1.0, effect: None }] };
    c.composite(&plan);
    drop(plan);
    assert!(Arc::strong_count(&px) > 1, "cached upload must own its pixel buffer");
}

/// The WGSL tetrahedral LUT matches `Lut3d::apply` on the CPU.
#[test]
fn gpu_lut_matches_cpu_tetrahedral() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let g = GpuLut::new(&dev, &q);
    for size in [2usize, 17, 33, 65] {
        let mut lut = filmcraft_color::Lut3d::from_fn(size, |c| {
            [(c[0] * c[1] + c[2] * c[2]).sin(), c[1].powf(0.45) * (1.0 - 0.3 * c[0]), (c[0] - c[2]).abs() + 0.1 * c[1]]
        });
        if size == 17 {
            lut.domain_min = [-0.1, 0.0, 0.0];
            lut.domain_max = [1.2, 1.0, 2.0];
        }
        let mut s = 99u64;
        let mut px = Vec::new();
        for _ in 0..70_000 {
            for k in 0..4 {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                px.push(if k == 3 { 0.5 } else { (s % 10_001) as f32 / 10_000.0 * 1.2 - 0.1 });
            }
        }
        let gpu = g.apply(&lut, &px).expect("gpu lut");
        let mut worst = 0f32;
        for (i, p) in px.chunks_exact(4).enumerate() {
            let c = lut.apply([p[0], p[1], p[2]]);
            for k in 0..3 {
                worst = worst.max((c[k] - gpu[i * 4 + k]).abs());
            }
            assert_eq!(gpu[i * 4 + 3], 0.5);
        }
        eprintln!("LUT {size}³: max |cpu − gpu| = {worst:e}");
        assert!(worst < 1e-4, "size {size}: max |cpu - gpu| = {worst}");
    }
}

fn test_masks(scale: f32) -> Vec<filmcraft_render::mask::FlatMask> {
    use filmcraft_project::{Mask, MaskMode, MaskPath, ParamValue};
    let mut a = Mask::new("a", MaskPath::ellipse(Vec2::new(150.0, 90.0), Vec2::new(90.0, 55.0)));
    a.feather.value = ParamValue::Float(24.0);
    a.expansion.value = ParamValue::Float(6.0);
    let mut b = Mask::new("b", MaskPath::polygon(&[Vec2::new(40.0, 20.0), Vec2::new(260.0, 50.0), Vec2::new(120.0, 170.0)]));
    b.mode = MaskMode::Subtract;
    b.feather.value = ParamValue::Float(0.0);
    b.opacity.value = ParamValue::Float(70.0);
    let mut c = Mask::new("c", MaskPath::rect(200.0, 100.0, 300.0, 175.0));
    c.mode = MaskMode::Difference;
    c.inverted = true;
    c.feather.value = ParamValue::Float(3.5);
    filmcraft_render::mask::prepare(&[a, b, c], filmcraft_time::Tick(0), scale)
}

#[test]
fn gpu_mask_coverage_matches_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let g = crate::GpuMask::new(&dev, &q);
    for scale in [1.0f32, 0.5] {
        let masks = test_masks(scale);
        let (w, h) = ((320.0 * scale) as usize, (180.0 * scale) as usize);
        let cpu = filmcraft_render::mask::coverage(&masks, w, h).unwrap();
        let gpu = g.coverage(&masks, w, h).expect("gpu coverage");
        let worst = cpu.iter().zip(&gpu).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        eprintln!("mask coverage @{scale}: max |cpu − gpu| = {worst:e}");
        assert!(worst < 2e-4, "scale {scale}: {worst}");
        assert!(cpu.iter().any(|v| *v > 0.99) && cpu.iter().any(|v| *v < 0.01) && cpu.iter().any(|v| *v > 0.2 && *v < 0.8));
    }
}

#[test]
fn gpu_masked_mix_matches_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let g = crate::GpuMask::new(&dev, &q);
    let masks = test_masks(1.0);
    let (w, h) = (320usize, 180usize);
    let original = filmcraft_render::Image { w, h, px: (0..w * h * 4).map(|i| ((i * 37) % 101) as f32 / 100.0).collect() };
    let mut effected = original.clone();
    effected.px.iter_mut().for_each(|v| *v = 1.0 - *v * 0.5);
    let gpu = g.mix(&masks, w, h, &original.px, &effected.px).expect("gpu mix");
    let cov = filmcraft_render::mask::coverage(&masks, w, h).unwrap();
    let mut cpu = effected.clone();
    filmcraft_render::mask::mix(&mut cpu, &original, &cov);
    let worst = cpu.px.iter().zip(&gpu).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    eprintln!("masked mix: max |cpu − gpu| = {worst:e}");
    assert!(worst < 2e-4, "{worst}");
}
