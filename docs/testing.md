# Testing

```sh
cargo test --workspace                          # everything; oracle tests skip without ffmpeg
cargo test -p filmcraft-h264                    # one crate
cargo test --release -p filmcraft-hevc          # codec tests are much faster in release
cargo xtask ci                                  # all gates (fmt, clippy, tests, layers, assets, wasm)
cargo xtask fixtures                            # pre-generate the ffmpeg fixture matrix
FILMCRAFT_REQUIRE_ORACLES=1 cargo test --workspace   # CI: missing ffmpeg fails instead of skipping
```

## 1. Kinds of tests

| Kind | Where | Examples |
|---|---|---|
| Unit | `src/` `#[cfg(test)]` modules, `src/tests.rs` | CABAC engines, VLC tables (prefix-freeness, Kraft sums), colour matrices, frame cache |
| Property (`proptest`) | `time`, `bitstream`, `edit`, `isobmff/tests/roundtrip.rs`, `interchange/tests/*` | tick↔frame round trips for every rate, DF timecode, edit invariants (no overlaps, durations conserved), mux→demux round trips, interchange export→import |
| Synthetic streams | `h264/src/synth_tests.rs`, `hevc/src/synth_tests.rs` | hand-built bitstreams for features the reference encoders don't emit; expected output known exactly |
| Scripted UI | `crates/ui-egui/tests/scripted.rs` | headless app (egui_kittest) driven over the control channel: razor/undo, insert, apply effect, playback, click by automation id (§4) |
| Monitor view / Graphics menu | `crates/ui-egui/tests/view_ui.rs`, `crates/engine/src/graphics_tests.rs` | View menu items and checkmarks, playback/paused resolution, channel / comparison / multi-camera / waveform display modes, magnification and Hand-tool panning, guides dragged from the rulers (move, lock, remove, Add Guide…, templates in the preferences), snapping a graphic to the frame centre and a guide; align to frame / as group / to selection, distribute (centres and gaps), arrange, select next/previous graphic and layer, reset parameters / duration, vertical text, New Layer from file (`FILMCRAFT_UI_SHOTS=<dir>` writes `view-*.png`) |
| Settings | `crates/engine/src/settings_tests.rs`, `crates/ui-egui/tests/settings_ui.rs` | schema keys and defaults, persistence in the data directory, v1 → v2 migration and repair of bad values, validation, per-category reset; each wired setting (still / transition durations, step many, label defaults and colours, media scaling, timebase, recent projects, media cache policy, output mapping, smart quotes, auto-transcribe); the dialog: every category from its command and Cmd+,, edits by automation id, OK / Cancel / Escape / Reset…, theme, label names in Edit ▸ Label, tooltips, frame cache (`FILMCRAFT_UI_SHOTS=<dir>` writes `settings-*.png`) |
| Scopes and panels (M8.9 / M12.6) | `crates/scopes/src/tests.rs`, `crates/engine/src/{scopes,panels}_tests.rs`, `crates/ui-egui/tests/panels_ui.rs` | scope maths on generated frames (flat colours → exact histogram bins, ramps → waveform row = code value, 75 % bars → parade levels and vectorscope target cells for BT.601/709/2020, HLS hue angles, YC chroma, Clamp Signal, decimation, NaN); `scopes.read` on a colour matte (exact bins, 78.43 %) and on Bars and Tone (the six targets); metadata edits (one undo step, read-only fields refused, saved in the project), the event log (failed / disabled commands, repeats, jobs started / finished / failed / cancelled); every panel from Window ▸, the scopes' wrench menu, presets, five-scope grid, typing into a Metadata field, Timecode rows and modes, Events filter / Clear All, Progress cancel, Reference Monitor park / gang / scopes (`FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `panels-*.png`); `perf_scopes_at_1080p` (ignored) times each scope |
| Golden images | `crates/golden/tests/golden.rs` | CPU renders vs committed PNGs; GPU vs CPU on the same scenes (§3) |
| Engine / command | `crates/engine/src/tests.rs` | run commands on the demo project, assert the sequence, undo/redo, disabled cases |
| Render | `crates/render/src/tests.rs` | compositing, opacity, Motion, cross dissolve midpoint, ½-res vs full, GPU plan vs reference, audio mix, audio-effect continuity |
| Multi-camera / sync | `crates/audio-dsp/src/sync.rs`, `crates/engine/src/multicam_tests.rs`, `crates/ui-egui/tests/multicam_ui.rs` | audio offsets within one sample: Rust-generated multi-mic speech (gains −20…+10 dB, SNR 40/25/10/3 dB, zero-phase colouring, fractional delays, a 0 dB-SNR room with a −4 dB echo) and ffmpeg-made camera/recorder files (pink-noise bursts, independent noise; exact lags recovered); timecode (± hours) and marker sync within a frame; Merge Clips; timeline Synchronize keeps frame-aligned starts; multicam structure, angle render (PSNR vs the camera), Switch Audio mix = the angle's track, live switching from a simulated playback clock (cuts, healing, one undo step), Ctrl+N cuts, flatten (frames identical), save/load; `perf_grid_of_four_1080p_angles` (ignored) prints the grid cost |
| Menu long tail (M3.11) | `crates/render/src/scene.rs`, `crates/engine/src/{scene_detect,sequence_extras,project_tools}_tests.rs`, `crates/interchange/src/ale.rs`, `crates/project/src/find.rs`, `crates/ui-egui/tests/menus_ui.rs` | Scene Edit Detection on synthetic frames (hard cuts vs pans, flashes, sensitivity) and on generated movies with known cut points (our own ProRes render of demo scenes; an ffmpeg-made H.264 concat of testsrc2 / SMPTE bars / Mandelbrot): cuts with linked audio, clip markers, subclips, one undo step, background job and cancel; Normalize Mix Track to a target peak; Simplify Sequence; Find / Find Next and search bins; Automate to Sequence (overlap, transitions, insert, unnumbered markers); ALE round trip; Project Settings and scratch disks; menu order and shortcuts against Premiere's; the dialogs by automation id |
| Mixer | `crates/render/src/mixer_tests.rs`, `crates/engine/src/mixer_tests.rs` | sample-exact fader gain and pan laws, automation at block boundaries (bit-identical however requests are cut), solo/mute/solo-safe, sends and submix routing, latency-compensation alignment, render-vs-playback identity, Touch ramp-back, recorder modes (Latch/Touch/Write), thinning, Audio Gain, transition curves; `perf_24_tracks_3_effects_realtime_factor` prints the realtime factor |
| Oracle | `crates/*/tests/*oracle*.rs`, `conformance.rs` | compare with ffmpeg/ffprobe (§2) |
| Robustness / fuzz | `*/tests/robustness.rs`, `*/tests/fuzz.rs` | seeded mutation and truncation of real and synthetic files; nothing may panic |
| Performance | `*/tests/perf.rs` (`#[ignore]`) | §5 |

Commit `*.proptest-regressions` files so failing cases are re-run.

## 2. ffmpeg oracle tests

ffmpeg and ffprobe are **external processes** used to generate fixtures and to check results. They
are never linked or shipped ([AGENTS.md](../AGENTS.md) §2).

- The tools are found by `filmcraft-testkit` (a dev-dependency-only crate, `crates/testkit`), in
  this order: `FILMCRAFT_FFMPEG` / `FILMCRAFT_FFPROBE` (explicit paths; a wrong path is an error),
  then every directory on `PATH` (`ffmpeg` and `ffmpeg.exe`), then well-known install directories
  (`/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`, `C:\ffmpeg\bin`, …).
- If a tool is missing the test prints `SKIPPED (<test>): ffmpeg not found …` and passes. Set
  **`FILMCRAFT_REQUIRE_ORACLES=1`** (CI) to make every such skip a failure. In new tests use
  `let ff = filmcraft_testkit::require_ffmpeg!();` (also `require_ffprobe!`, `require_oracles!`).
- Fixtures are generated on first use into `<workspace>/target/fixtures/<crate>/`
  (`filmcraft_testkit::fixtures_dir`; independent of `CARGO_TARGET_DIR`, so agents with private
  target dirs share them; `FILMCRAFT_FIXTURES_DIR` overrides the root) and reused afterwards.
  Generators write to a per-process/thread temporary name (`testkit::temp_path`) and rename it into
  place, so concurrent tests never see half-written files. Delete the directory to regenerate.
  Never commit media.
- Every crate's `tests/common/mod.rs` delegates discovery and fixture paths to testkit, except
  `crates/codecs/src/tests.rs`, which still has its own lookup (the crate was being edited
  concurrently; migrate it when convenient).
- `cargo xtask fixtures [crate…]` pre-generates the whole fixture matrix up front (useful before a
  parallel test run or on a fresh machine). It runs each crate's ignored `generate_fixtures` test —
  the same generators the oracle tests call — for `h264`, `hevc`, `isobmff`, `matroska`, `dnx`,
  `prores` and `codecs` (MXF and Ogg), and prints one `made` / `cached` / `skipped` line per fixture plus a summary. The
  `aac`, `opus` and `h264enc` oracles generate small per-test signals on demand and are not part of
  the matrix.
- Fixture sources are synthetic: `testsrc2`, `mandelbrot`, SMPTE bars, noise and fades, sine tones.
  H.264 and HEVC fixtures need ffmpeg built with libx264 and libx265. VideoToolbox fixtures are
  generated only on macOS.

### Pass criteria per codec

| Crate | Oracle check | Criterion |
|---|---|---|
| `h264` | decode every fixture single-threaded and frame-threaded; compare with `ffmpeg -f rawvideo -pix_fmt yuv420p` | **bit-exact**, every frame |
| `hevc` | same, `yuv420p` / `yuv420p10le` | **bit-exact** (8- and 10-bit) |
| `h264enc` | `ffmpeg -ec 0` decodes our stream | no errors, exact frame count, **bit-identical to the encoder's reconstruction**; B-frame order checked with ffprobe; rate-control targets |
| `prores` decode | ffmpeg `prores_ks` / `prores_aw` fixtures, compared at native depth | within **±1 LSB** (different integer IDCT); alpha bit-exact |
| `prores` encode | ffmpeg decodes with `-xerror` | no errors; agrees with our decoder within ±1 |
| `dnx` decode | ffmpeg VC-3 fixtures (13 DNxHD CIDs, DNxHR LB/SQ/HQ/HQX/444), compared at native depth | within **±2 LSB**, ≤ 100 samples per million beyond ±1 (ffmpeg's integer IDCT; we evaluate the exact IDCT) |
| `dnx` encode | ffmpeg decodes with `-xerror` | no errors; agrees with our decoder within ±2; HQ luma PSNR ≥ 45 dB |
| `aac` decode | ffmpeg's decode of the same stream | max abs error ~1e-7 (float) |
| `aac` encode | ffmpeg decodes our stream | no errors, no clipping, CBR within ±5% of target; SNR reported |
| `isobmff` | `ffprobe -show_packets` on ffmpeg-made MP4/MOV | packet offsets, sizes, pts/dts, durations, key flags and stream parameters equal; remuxed files decode with `ffmpeg -v error` silent |
| `matroska` | `ffprobe -show_packets` | every packet (stream, size, key flag, pts, duration) equal; seeks land on the latest keyframe ≤ target |
| `audio-dsp` loudness | `tests/loudness_oracle.rs`: signals generated in Rust (997 Hz sine, pink noise at 48/96 kHz, speech-like bursts at 48/44.1 kHz, stereo with silence and sub-gate passages, an fs/4 inter-sample-peak tone) written as float WAV and measured with `ffmpeg -af ebur128=peak=true:metadata=1` | momentary and short-term every 100 ms **±0.1 LU**, integrated **±0.1 LU**, LRA **±0.5 LU**, true peak **±0.2 dB** (against the analytic value when one exists). Measured: ΔM/ΔS ≤ 0.0005 LU, ΔI ≤ 0.007 LU, ΔLRA ≤ 0.04 LU, ΔTP ≤ 0.045 dB against ffmpeg. On the fs/4 tone ffmpeg's own true peak is +0.6 dB high (−0.32 vs the analytic −0.92 dBTP); ours is −0.05 dB |
| `audio-dsp` effects | `src/effects/premiere_tests.rs`, `src/design.rs`, registry tests in `src/effects/mod.rs`: generated tones, noise and impulses | every effect: neutral settings = delayed identity (≤ 2·10⁻⁴), bit-exact determinism, latency constant and equal to where an impulse lands, block-size independence, finite / denormal-free tails at extreme settings, > 1× realtime; per effect a level or frequency-response check (measured tone vs analytic `response_db` within 0.1–0.2 dB, Butterworth vs closed form 10⁻⁶ dB, Chebyshev/elliptic ripple and stop-band bounds, LR4 crossover sum flat within 0.02 dB, comb-notch depth, echo positions to the sample, Schroeder RT60 of generated impulses within 25 %, click repair −20 dB) |
| `mxf` (via `codecs/tests/mxf_oracle.rs`) | ffmpeg-written OP1a / OP-Atom / D-10 files decoded by ffmpeg | H.264 (long GOP, B pictures without temporal offsets) **bit-exact** every frame and at 25 random seeks; DNxHR ±2, ProRes ±1; PCM / AES3 **sample-exact**; frame count = ffprobe packets; timecode (25 fps, 29.97 DF); MPEG-2 reported unsupported; truncation/mutation never panics |
| `ogg` (via `codecs/tests/ogg_oracle.rs`) | ffmpeg (libopus, Vorbis) files decoded by libopus / ffmpeg | length **exact** (pre-skip, end trimming); SNR ≥ 50 dB CELT (measured 77.7), ≥ 10 dB SILK, Vorbis 139 dB; random seeks within 6·10⁻⁶ of the continuous decode |
| image sequences / BWF (`codecs/tests/media_oracle.rs`) | ffmpeg-written PNG/TIFF/BMP/JPEG stills read by ffmpeg's image2 demuxer; ffmpeg BWF (`-write_bext`) | PNG/TIFF/BMP **exact**, JPEG ±12 (measured 3); missing frames hold the previous one; BWF TimeReference = ffprobe's, start timecode, samples exact |
| `export` | our own demuxer/decoder reads the file back; ffprobe counts frames when present | expected size, duration, colour, audio level; exact frame count |
| `export` settings and presets (M6.5) | `export/src/settings_tests.rs`, engine `export_tests.rs`: every built-in preset exports a slice of the demo sequence; ffprobe reads codec, profile, size, rate, audio format and data rate; our decoders read pixels back | codec / profile / container / size / `r_frame_rate` / sample format exact; H.264 bitrate ≤ VBR max × 1.1 and ≥ ½ target over 3 s; ProRes ≤ 1.5 × nominal; DNxHR within 25 % of nominal; loudness normalization within **0.5 LU** of the target (WAV and AAC), true peak ≤ ceiling + 0.1 dB; image sequences numbered `<name>000…`; limiter, overlays, metadata (`title`/`copyright` tags), MOV multiplexer, two-pass/CBR progress |

Each codec README has the full fixture matrix and the measured results.

## 3. Rendering: CPU reference and GPU parity

- The CPU compositor in `filmcraft-render` is the reference. Its tests check computed values
  (opacity blends, dissolve midpoints, Motion placement, ½-resolution against downsampled full
  resolution) and that `render::plan` matches `render_sequence`.
- `crates/gpu/src/tests.rs` composites the same frame plan (a YUV layer plus a transformed,
  semi-transparent RGBA layer) on the GPU and on the CPU. It requires a 99th-percentile channel
  difference of ≤ 6 and a mean of < 1.5 (8-bit levels). It skips when no GPU adapter is available.
- Effects implemented only on the CPU need no GPU test: the plan pre-renders those layers on the
  CPU.
- **Golden images** (`crates/golden`, a test-only crate): `tests/golden.rs` builds small procedural
  projects (demo-generator footage, bars, no media files) at 320×180 and renders one frame of each
  through the CPU compositor: Motion transform + opacity, four blend modes (Multiply, Screen,
  Overlay, Difference), Gaussian Blur, Lumetri basic correction, Crop, Cross Dissolve at 50 %, Dip
  to Black at 25 %, Wipe at 50 %, Timecode / Clip Name burn-in text, graphic clips, and colour
  management: S-Log3/S-Gamut3.Cine footage interpreted into Rec. 709 (`log_to_rec709`) and
  Rec. 2100 PQ footage tone mapped into Rec. 709 (`hdr_tone_map`; the footage is demo frames
  re-encoded by the test's `Encoded` source). Each frame is compared
  with `crates/golden/goldens/<scene>.png` by `filmcraft_testkit::golden`:
  **PSNR ≥ 45 dB, max abs ≤ 12, 99th-percentile per-pixel max channel difference ≤ 2** (8-bit
  sRGB levels; `Tolerance::RENDER`). On failure the actual frame and a ×8 difference image go to
  `<workspace>/target/golden-failures/`. Six `vfx_*` scenes cover the M5.11 effects (Corner Pin over Turbulent
  Displace, Ultra Key, Wonder Glow + Glint, Spacer + Stroke + Long Shadow over Gradient, VR Rotate Sphere,
  Channel Mix + Color Emboss + Rounded Crop); per-effect unit tests live in `crates/render/src/vfx/tests.rs`
  (identity at neutral parameters, determinism, range, a known value each, temporal/track effects through
  the compositor). `scenes_are_distinct` guards against blank or duplicate
  scenes.
- **Video transitions** (`crates/render/src/transitions/tests.rs`) are checked for every transition
  (84 modern, 21 Legacy, 7 hidden Obsolete): p = 0 is exactly the outgoing frame and p = 1 exactly
  the incoming one, near both ends the output is close to that frame (continuity), the midpoint differs
  from both, the output is finite and deterministic (also on 1×1 / odd sizes and NaN progress), every
  parameter visibly changes the picture, and every direction differs. Golden *fingerprints* (4×4 mean
  RGB grid at p = 0.25/0.5/0.75, ±4 levels) live in `src/transitions/goldens.txt`; re-bless with
  `FILMCRAFT_BLESS=1 cargo test -p filmcraft-render golden_fingerprints` and review the diff.
  `FILMCRAFT_TRANSITION_SHEETS=<dir> cargo test -p filmcraft-render contact_sheets -- --ignored`
  writes one contact-sheet PNG per folder for eyeballing (never commit them).
- **Blessing:** `FILMCRAFT_BLESS=1 cargo test -p filmcraft-golden` rewrites the references (and
  writes a `.attribution` sidecar for any new one; it prints the `ATTRIBUTION.md` row to add).
  References are original work rendered by FilmCraft, must stay under 50 KB (enforced on bless),
  and every one needs its sidecar and index row (`cargo xtask assets`). Look at a re-blessed PNG
  before committing it.
- The same scenes run through `plan_frame` + `GpuCompositor` when a GPU adapter exists
  (`gpu_matches_cpu_on_golden_scenes`, same p99 ≤ 6 / mean < 1.5 criterion; skipped otherwise).
  This found a real bug: the GPU upload cache was keyed by pixel-buffer address without keeping
  the buffer alive, so a new frame allocated at a freed frame's address was drawn with the stale
  texture (fixed; regression test `upload_cache_keeps_buffers_alive`).

## 4. UI: control channel and screenshots

### Scripted UI tests (headless)

`crates/ui-egui/tests/scripted.rs` runs the real `FilmcraftApp` under
[`egui_kittest`](https://docs.rs/egui_kittest) (`build_eframe`): no window, no GPU, no OS event loop.
A small `Driver` opens the demo project, sends requests through the same control channel agents use
(`ControlRequest` → `control::handle`, exactly as the TCP server does) and steps egui frames until
each reply arrives. Synthetic input queued by `ui.click` / `ui.drag` / `ui.key` is moved into the
next frame by the app's own `raw_input_hook`, so clicks by automation id work headless. Covered:

| Test | Asserts |
|---|---|
| `demo_project_opens_headless_and_registers_widgets` | active sequence, > 50 registered widgets, panel and tool ids, 6 clips on V1 (`sequence.inspect`) |
| `razor_then_undo_through_the_control_channel` | `timeline.razor` adds a clip, the timeline can locate it on screen, `edit.undo` via `ui.menu.invoke`, redo |
| `insert_from_source_ripples_the_sequence` | `source.open` + marks + `source.insert` lengthens the sequence |
| `apply_effect_appears_in_effect_controls` | `effects.apply` adds Gaussian Blur in the model and `effectControls.effect.gaussian_blur` appears in the panel; undo removes it |
| `playback_toggle_and_stop` | `playback.toggle` / `ui.playback stop` and `ui.inspect` playback state |
| `clicking_a_tool_button_by_automation_id` | `ui.click {id: "tools.Razor"}` changes the tool (real egui input path) |
| `unknown_methods_and_commands_fail_cleanly` | errors come back as `{"ok": false}` |

`crates/ui-egui/tests/mixer_ui.rs` drives the Audio Track Mixer, Audio Clip Mixer, timeline track
keyframes and the Audio Gain dialog by automation id and with multi-frame pointer drags (press, move
over several frames, release). With `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` it also renders the window
offscreen through wgpu (`Harness::render`) and writes `mixer-*.png`; this works without a visible
window (for example on a locked screen, where `ui.screenshot` cannot capture).
`crates/ui-egui/tests/essential_sound_ui.rs` does the same for the Essential Sound panel (type buttons,
switches, a slider drag as one undo step, section bypass, Auto-Match, ducking, Browse presets;
`essential-sound-*.png`).

Essential Sound engine tests (`crates/engine/src/essential_sound_tests.rs`) build projects from
generated speech-like and tonal WAVs: Auto-Match lands within ±0.5 LU of the target (measured: 0.000 LU,
with and without a repair/clarity chain); ducking on a dialogue + music project gives keyframes within
60 ms of the expected times (measured 20 ms) and −15.00 dB in the mix; each repair stage improves its
metric through the render path (hum −39 dB, rumble −22 dB, noise floor −13 dB, sibilance −19 dB,
reverb tail −10 dB); the mix is bit-identical however requests are cut and the WAV export equals it.
`perf_full_dialogue_chain_realtime_factor` (ignored; run with `--release`) prints the realtime factor of
all nine Dialogue effects on one clip (22× on one core).

`crates/ui-egui/tests/export_ui.rs` drives Export mode: the settings column and Summary, editing a
setting turning the preset into Custom, the Preset Manager (search, favourite star, save, apply,
delete), the queue panel (Send to Queue, Up/Down, Cancel, Start, Retry, Clear) with real encodes,
the Export button and the header's Quick Export popup (`export-*.png` snapshots).

`crates/ui-egui/tests/color_ui.rs` drives colour features the same way (`color-*.png` snapshots):
Lumetri Input/Look LUT menus and section switches, the Interpret Footage ▸ Color Management and
Sequence Color Management dialogs, and the HDR scopes of a PQ sequence.

Colour science tests: `filmcraft-color` (curve round trips and published reference values per
camera log curve, BT.709/BT.2087 matrices, BT.2390 EETF, gamut mapping, LUT tetrahedral vs a
brute-force barycentric reference, `.cube`/`.3dl` round trips), `filmcraft-gpu`
(`gpu_lut_matches_cpu_tetrahedral`: WGSL vs CPU, max |Δ| ≈ 2e-7), `render::colorman`,
`render::color_match`, engine `color_tests.rs`, and `export::tests::hdr_exports_signal_pq_and_hlg`
(PQ/HLG exports read back by our demuxer and by ffprobe: `color_transfer`, `color_primaries`,
`color_space`, mastering display and content light side data).

Not covered headless: `ui.screenshot` (needs a real viewport), the wgpu monitor path (the harness
runs the CPU texture path), audio output, and wall-clock playback advance (kittest frames do not
advance real time). Use `Driver` for new UI regressions: `d.exec(command, params)`,
`d.ok(method, params)`, `d.frames(n)`.

### Interactive checks

For visual work, drive the real app:

```sh
cargo run --release -p filmcraft -- --control 9876
```

Then script it over the control channel or MCP: run commands, click by automation id,
`ui.inspect` / `ui.elements` to assert state, and `ui.screenshot {"path": "…"}` to capture the window
or a single panel (`{"panel": "Timeline"}`). Look at every screenshot. Examples are in
[agents.md](agents.md). The headless path (`filmcraft-cli run script.jsonl`, MCP `--demo`) covers
engine behaviour without a window.

## 5. Performance

| Benchmark | Command |
|---|---|
| H.264 decode, 1080p, threads sweep | `cargo test --release -p filmcraft-h264 --test perf -- --ignored --nocapture` |
| HEVC decode, 1080p / 2160p | `cargo test --release -p filmcraft-hevc --test perf -- --ignored --nocapture` |
| ProRes decode/encode | `cargo test --release -p filmcraft-prores --test perf -- --ignored --nocapture` |
| Encoder speed / PSNR / bitrate | `cargo run --release -p filmcraft-h264enc --example h264enc_synth -- …` |
| Decode a real file through the media stack | `cargo run --release -p filmcraft-cli -- bench-decode file.mp4 --frames 120` |
| Coding-tool coverage of the fixtures | `cargo test --release -p filmcraft-h264 --test conformance coverage_report -- --ignored --nocapture` (same for `hevc`) |
| **Whole-app suite**: decode fps per codec, playback, scrubbing, timeline UI, export, project save/open, peak RSS | `cargo xtask bench` ([performance.md](performance.md)) |

`cargo xtask bench` (the example `crates/ui-egui/examples/bench/`) runs six sections, each in its
own process under `/usr/bin/time` so peak RSS is per section: `decode` (every frame of 1080p and
2160p H.264, HEVC, VP9, AV1 and ProRes through the media stack), `playback` (the scenarios below),
`scrub` (random seeks and playhead drags: time until the exact frame is on screen), `timeline`
(the real app under `egui_kittest` with a 1000-clip / 20-track sequence: update, tessellation and
wgpu render ms per frame), `export` (H.264 + AAC and ProRes) and `project` (save / open 5000 clips).
Options: `--sections decode,scrub`, `--only <substring>` (fixture or scenario), `--repeat N`,
`--quick`, `--cpu`, `--label NAME` (output `target/bench/bench-<label>.{json,md}`),
`--section NAME --json FILE` (one section in-process, e.g. under a profiler). Fixtures are made
with ffmpeg in `target/fixtures/playback/` (or `$FILMCRAFT_FIXTURES/playback`).

The perf tests check bit-exactness before they time anything. Results go in the crate README's
performance table, with machine and thread count. Headline numbers go in
[ROADMAP.md](../ROADMAP.md). Measure on an idle machine: parallel agent builds distort timings.

### Playback benchmark

`cargo xtask bench-playback` (the example `crates/ui-egui/examples/bench_playback.rs`) plays
sequences headlessly through the Program monitor's own frame scheduler: the `FrameServer` worker
pool, `schedule_playback` (prefetch order and stale-job dropping), its caches, and the
`PlaybackMeter` that counts shown/dropped frames in the app. On the GPU path it also composites
each plan with `filmcraft-gpu`, as the monitor does on the UI thread.

```sh
cargo xtask bench-playback                                   # every scenario, GPU path, Full and Half
cargo xtask bench-playback --scenario stack3 --res full --cpu
cargo xtask bench-playback --json target/bench-playback.json # machine-readable results
```

| Scenario | What plays |
|---|---|
| `h264-1080` | one 1080p23.976 H.264 clip (testsrc2 + grain, ~45 Mbit/s, 250-frame GOP) |
| `stack3` | three 1080p H.264 clips on V1–V3; V2/V3 scaled, positioned, rotated, 70–85 % opacity |
| `h264-2160` | one 2160p23.976 H.264 clip |
| `demo` | the built-in demo project (procedural footage, transitions, effects) |
| `after-preview` | 1080p H.264 + Lumetri/Sharpen/Levels/Tint: a live play, Render Effects In to Out, then two plays of the green segment |
| `seek-storm` | 40 jumps to random frames 150 ms apart (scrubbing), time until the exact frame shows |

Options: `--res full,half,quarter`, `--cpu` (CPU compositor + texture conversion instead of the
GPU path), `--seconds`, `--refresh` (display Hz), `--workers`, `--repeat`, `--json <file>`.
Fixtures are made with ffmpeg in `target/fixtures/playback/` (set `FILMCRAFT_FIXTURES` to share one
set between worktrees).

Columns: **shown/drop** as counted in the app (a frame is shown when its exact picture was on
screen at a refresh while it was due; frames passed over without a refresh count as dropped);
**ontime** = due frames whose job finished before they were due; **lat** = queue→ready per job,
**svc** = worker time per job; **cpu/j**, **src/j**, **srcC/j** = worker thread CPU, source fetch
(decode) wall and thread CPU per job; **ui** = time on the UI thread to present a frame (GPU upload
and draw, or texture conversion); **cpu ms/f** = process CPU per frame and **cores** = the cores that
needs at the sequence frame rate; **seeks/dec** = decoder restarts and samples decoded; **waste** =
jobs for frames that were never due.

Wall-clock columns (shown/drop, ontime, latencies) depend on machine load, so each row prints the
load average. CPU columns (thread and process CPU time) and the structural counters (seeks,
samples decoded, wasted jobs) do not, and are what to compare between runs on a busy machine.

Results on an M4 Pro (14 cores), GPU path, 8 s plays, median of 2 alternating runs of the M4.6
baseline (commit `5170376`) and the result of M4.6, on a machine shared with parallel agent
builds (**load average 86–207**, so wall-clock columns are pessimistic; CPU ms/frame is not):

| Scenario | shown/dropped before | after | CPU ms/frame before → after | decoder seeks |
|---|---|---|---|---|
| h264-1080 Full / Half | 106/86, 118/74 | **192/0, 192/0** | 80 → 46, 95 → 46 | 3–4 → 1 |
| stack3 (3 × 1080p) Full / Half | 185/7, 174/18 | **192/0, 192/0** | 136 → 109, 117 → 111 | 4 → 3 (one per source) |
| h264-2160 Full / Half | 0/192, 0/192 | 5/187, 19/173 | 110 → 135, 170 → 145 | 4–5 → 1–2 |
| demo Full / Half | 63/129, 124/68 | 97/95, 152/40 | 140 → 80, 93 → 40 | |
| after-preview, 1st play after render (Full) | 80/112 | **190/2** | 119 → 26 | 104 → 0 |
| after-preview, 2nd play (Full) | 145/47 | **192/0** | 76 → 26 | 132 → 0 |
| after-preview, live effects (Half) | 18/174 | 69/123 (frames skipped evenly) | 109 → 95 | |
| seek-storm (40 jumps, 150 ms each) | 0/40 shown | 6/40 | 827 → 666 per jump | |

At load ~25–60 the same final build plays h264-1080, stack3 and h264-2160 at Full with 0 dropped
in 3 of 3 runs (192/0) and render previews 192/0. The 4K fixture needs ~4 cores of decode per
real-time second (≈160 ms CPU per frame at 170 Mbit/s); the demo project's procedural footage
~190 ms per Full-resolution frame.
