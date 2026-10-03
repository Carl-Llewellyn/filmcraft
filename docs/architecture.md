# Architecture

FilmCraft is a Cargo workspace of small crates with strictly enforced layering. The engine is
headless: every feature can be reached without a window, and the egui UI is one client among the
CLI, the JSON control channel and the MCP server.

Design principles:

1. **Engine-first.** Project-changing actions go through `Session::execute(id, params)`.
2. **Everything is a command.** Stable id, label, menu path, shortcut, parameter doc, `enabled()`
   predicate with a human-readable reason, and `run()`.
3. **Exact time.** Integer ticks, rational frame rates. No `f64` seconds in edit math.
4. **Copy-on-write snapshots.** The project is an `Arc<Project>`. Undo is a stack of snapshots, and
   background readers (playback, export) hold a snapshot without locking.
5. **CPU reference, GPU fast path.** The CPU compositor is the oracle. The GPU path is tested
   against it.
6. **Pure Rust, clean-room.** Codecs and containers are written from public specifications
   (see [AGENTS.md](../AGENTS.md)).

## 1. Layers

```text
 L6  apps/filmcraft · apps/filmcraft-cli · apps/filmcraft-web
 L5  ui-egui · automation
 L4  engine
 L3  render · gpu · export · golden (test-only)
 L2  edit · codecs · interchange · captions · speech
 L1  frame · media · project · audio-dsp · text
 L0  foundation: time · geom · color · bitstream · testkit (dev-dependency only)
     codecs/containers: isobmff · matroska · h264 · h264enc · hevc · vp9 · av1 · prores · dnx · aac · opus
```

Crates are named `filmcraft-<dir>` (`crates/time` is `filmcraft-time`). The apps are `filmcraft`
and `filmcraft-cli`.

| Crate | Layer | Purpose |
|---|---|---|
| `time` | L0 | `Tick`, `FrameRate`, `TimeRange`, timecode parse/format (NDF/DF, frames, feet+frames, samples) |
| `geom` | L0 | `Vec2`, `Rect`, `Affine`, Motion-transform composition |
| `color` | L0 | colour spaces, transfer functions, YUV↔RGB matrices, LUTs |
| `bitstream` | L0 | bit reader/writer, Exp-Golomb, emulation prevention |
| `isobmff` | L0 | MP4/MOV demux and mux |
| `matroska` | L0 | MKV/WebM demux |
| `h264`, `h264enc` | L0 | H.264 decoder; H.264 encoder |
| `hevc` | L0 | H.265 Main/Main 10 decoder |
| `prores` | L0 | ProRes decoder and encoder |
| `dnx` | L0 | DNxHD / DNxHR (SMPTE ST 2019-1 VC-3) decoder and DNxHR encoder |
| `av1` | L0 | AV1 decoder (Main profile; bit-exact with libdav1d; see its README for the stage table) |
| `aac` | L0 | AAC-LC decoder and encoder |
| `testkit` | L0 | test-only helpers, used only as a dev-dependency: ffmpeg/ffprobe discovery, fixture dirs, golden images ([testing.md](testing.md)) |
| `frame` | L1 | `VideoFrame` (planar YUV / RGBA8 / linear RGBA f32, colour metadata), `AudioBuffer` |
| `media` | L1 | `MediaSource` trait, probing/openers, frame cache, generators, stills, WAV |
| `project` | L1 | document model, effect definitions, keyframes |
| `audio-dsp` | L1 | loudness metering (BS.1770 / R128) and audio effects; no dependencies |
| `text` | L1 | text engine: font database (bundled OFL fonts + system fonts), shaping (harfrust), bidi, line breaking, paragraph layout, glyph/path rasteriser, strokes ([crates/text/README.md](../crates/text/README.md)) |
| `edit` | L2 | pure edit algebra (insert, overwrite, razor, ripple, roll, slip, slide, rate stretch…; text-based editing: `edit::transcript`) |
| `speech` | L2 | speech-to-text: `Transcriber` trait, Whisper model catalogue + verified downloader (feature `download`), pure-Rust Whisper inference on candle with word timestamps (feature `whisper`), speaker labelling ([transcripts.md](transcripts.md)) |
| `codecs` | L2 | container + codec hub: MP4/MOV and MKV sources, GOP-aware seeking, decoder registry, audio decoding |
| `interchange` | L2 | EDL, FCP7 XML, FCPXML and OTIO import/export (no file I/O) |
| `render` | L3 | sequence evaluation, CPU compositor, video effects (`effects`, `vfx`; effects needing other frames or tracks read them through `vfx::FxEnv`), transitions, audio mix |
| `gpu` | L3 | wgpu compositor (WGSL) |
| `golden` | L3 | test-only: golden-image tests of the CPU renderer and GPU-vs-CPU parity; empty library, dev-dependencies only |
| `export` | L3 | render → encode → mux pipeline, progress/cancel |
| `engine` | L4 | `Session`, command registry, undo history, media pool, jobs, interchange glue |
| `ui-egui` | L5 | the egui frontend: docking, panels, timeline, monitors, playback, control-channel handlers |
| `automation` | L5 | MCP server (`rmcp`, stdio), headless or bridged to the running app |
| `filmcraft` | L6 | desktop binary: eframe/wgpu window, cpal audio output, file dialogs, native macOS menu, TCP control server |
| `filmcraft-cli` | L6 | headless CLI: `exec`, `run`, `inspect`, `describe`, `commands`, `import`, `export`, `render`, `probe`, `mcp`; `--bridge` targets the running app |
| `filmcraft-web` | L6 | the browser app (wasm32): eframe web runner on WebGPU/WebGL2, Blob-backed services, OPFS recovery, WebAudio, WebCodecs, `window.filmcraft` API ([web.md](web.md)) |

### What `cargo xtask layers` enforces

The table of layers lives in `xtask/src/main.rs` (`LAYERS`). It also reserves names for planned
crates. The check reads `cargo metadata` and looks at normal and build dependencies (dev-dependencies
are exempt):

| Rule | Detail |
|---|---|
| Every crate has a layer | A new crate fails the check until it is added to `LAYERS`. |
| Only downward edges | A crate may not depend on a crate in a higher layer. |
| Same-layer edges are listed | From L1 up, a same-layer edge must be in `SAME_LAYER`: `media→frame`, `project→media`, `project→frame`, `gpu→render`, `export→render`, `cli→filmcraft`, plus a few reserved for planned crates. |
| L0 codecs stay standalone | L0 crates other than `time`, `geom`, `color`, `bitstream`, `testkit` may depend on no workspace crate except `filmcraft-bitstream`. External crates such as `thiserror` and `rayon` are allowed. |
| No UI/OS crates below L5 | `egui`, `eframe`, `egui-wgpu`, `winit`, `rfd`, `cpal`, `muda` are allowed only in L5 and L6. |

`cargo xtask wasm` runs `cargo check --target wasm32-unknown-unknown` on every L0–L4 crate, the
egui UI and the web app, so everything up to the engine stays web-portable and the web app builds
([web.md](web.md)). `unsafe_code = "deny"` applies workspace-wide.

## 2. Time base

All time is `filmcraft_time::Tick(i64)` at `TICKS_PER_SECOND = 254_016_000_000`.

That number divides evenly into the frame duration of every broadcast rate (23.976, 24, 25, 29.97,
30, 48, 50, 59.94, 60, 120…) and the sample duration of every common audio rate (8 kHz to 192 kHz,
including the 44.1 kHz family). So frame and sample positions are exact integers, edits never drift
at 29.97, and audio and video line up to the sample.

| Type | Use |
|---|---|
| `Tick` | timeline and media positions and durations |
| `FrameRate { num, den }` | `frame_duration()`, `tick_of(frame)`, `frame_at(tick)`, `snap(tick)` |
| `TimeRange` | half-open `start + duration` |
| Timecode | display only (SMPTE NDF/DF, frames, feet+frames, samples); `parse_timecode` and `format_time` |

Commands take time as `time` (ticks), `frame`, `seconds` or `timecode`; the engine converts once at
the boundary.

## 3. Data model (`filmcraft-project`)

```text
Project
├─ root: Bin                         tree of bins
├─ items: map ItemId → ProjectItem   flat
│    kind: Media(MediaClip) | Sequence(Sequence) | Subclip{..} | AdjustmentLayer{..} | Graphic{..}
└─ next_id

Sequence
├─ settings: frame rate, size, sample rate, …
├─ video_tracks / audio_tracks: Vec<Track>
├─ markers, mark_in / mark_out
Track
├─ locked, sync lock, targeting, mute/solo/visibility
├─ items: Vec<TrackItem>             sorted, never overlapping
├─ transitions: Vec<Transition>
└─ audio: volume_db, pan, effects (mixer inserts), mixer: MixerStrip
     (automation mode + lanes, sends, output, record arm, solo safe, input map)
Sequence (audio) ─ submix_tracks: Vec<Track>, master_volume_db / master_effects / master_mixer
TrackItem (a clip instance)
├─ item: ItemId, start (timeline ticks), source_in (media ticks), duration, speed
├─ link group, label, enabled
└─ effects: Vec<EffectInstance>      intrinsic Motion/Opacity/Volume… first, then standard effects
EffectInstance
├─ effect id, enabled, params: id → constant value or keyframe track
└─ masks: Vec<Mask>                  path (keyframable Bézier), feather, opacity, expansion, inverted, mode
```

- **Graphic clips** ([graphics.md](graphics.md)) reference a `Graphic` canvas item; their text and
  shape layers are hidden `graphic_text` / `graphic_shape` effect instances on the track item.
- Everything is plain serde data. `Sequence::check()` validates the invariants (no overlaps, unique
  ids), and the engine runs it after every sequence edit.
- Timeline positions are sequence ticks; `source_in` and keyframes are in media time, so trims and
  splits never move keyframes.
- **Effect definitions are data.** `project::effect::effect_defs()` lists every video effect,
  audio effect and transition with its parameter schema. The Effects panel tree, the Effect
  Controls rows and the parameter docs agents see are all generated from it.
- **Project files** (`.fcproj`) are the project serialised as JSON. Saves are atomic: write a
  temporary sibling file, then rename it over the target.

## 4. Command system (`filmcraft-engine`)

```rust
pub struct CommandSpec {
    pub id: &'static str,                 // "sequence.addEdit"
    pub label: &'static str,              // "Add Edit"
    pub menu: &'static [&'static str],    // ["Sequence"]; empty = not in menus
    pub shortcut: Option<&'static str>,   // "Cmd+K" (Cmd = ⌘ on macOS, Ctrl elsewhere)
    pub params: &'static str,             // r#"{"time":ticks?}"#, shown to agents
    pub enabled: fn(&Session) -> Result<(), String>,  // Err carries the reason
    pub run: fn(&mut Session, &Value) -> Result<Value>,
    pub journal: bool,                    // false for read-only queries
}
```

- All commands are in `crates/engine/src/commands.rs` (`cmd!` for actions, `query!` for read-only
  queries such as `project.inspect`, `sequence.inspect`, `effects.list`, `jobs.list`). Ids follow
  the menu structure: `file.*`, `edit.*`, `clip.*`, `sequence.*`, `markers.*`, `timeline.*`,
  `effects.*`…
- `Session::execute(id, params)` finds the spec, checks `enabled`, runs it and appends it to the
  journal.
- **Undo.** Edits go through `Session::edit(label, |project, state| …)` or
  `Session::edit_sequence(label, |seq, ctx, state| …)`. These clone the project, apply the closure,
  and on success push the old `Arc<Project>` with the label onto the undo stack (200 entries).
  On error nothing changes. `edit.undo` and `edit.redo` swap snapshots. Thanks to structural sharing
  a snapshot costs little.
- **Editor state** (`EditorState`: active sequence, playheads, selection, targeting, edit points…)
  is serde, so agents can read it with `state.inspect`.
- **Events** (`ProjectChanged`, `Toast`, `OpenSequence`, `OpenSource`) are drained by frontends
  each frame.
- **UI-only commands** (tools, playback, zoom, panels, workspaces) live in
  `crates/ui-egui/src/menus.rs` (`UI_COMMANDS`). The menu bar is built from the engine registry plus
  this table, and `menus::invoke` is the single entry point for menus, shortcuts and the control
  channel.

## 5. Media, render and playback pipeline

```text
file ──► codecs (MP4/MOV, MKV, audio)        demux + decode, GOP-aware seek
          │   decoder registry: h264, hevc, vp9, av1, prores, dnx, mjpeg (+ any registered first)
          ▼
        media::MediaSource ──► frame cache (byte-budgeted LRU, shared)
          ▼
        render::render_sequence(project, seq, t, scale)        CPU reference
          per track bottom→top: map timeline t → media t (speed), fetch frame,
          standard effects → Motion → Opacity/blend, transitions, composite
          in linear-light premultiplied f32
          │
          └─ render::plan::plan_frame → gpu::GpuCompositor     GPU path
               layers = decoded YUV/RGBA frames + matrix + opacity;
               Brightness & Contrast runs in WGSL; other unsupported work is pre-rendered on CPU
          ▼
        ui-egui frames.rs worker pool ──► monitors (program/source), thumbnails, prefetch
```

- **Sources.** `media::MediaSource` yields `video_frame(FrameRequest)` and
  `audio(start, frames, rate)` in media time. Sources are `Send + Sync` and shared by monitors,
  thumbnails, playback and export. The engine's `MediaPool` creates one per project item, lazily,
  through registered openers (`codecs::openers()`: MP4/MOV, MKV/WebM, audio files).
- **Offline media and proxies.** The pool caches each item's source per reference (path, offline
  flag), so relinking and undo take effect at once. Media that can't be opened renders the offline
  slate (`render::offline`) instead of failing. With proxies enabled, an item with a proxy reads
  it through `ProxySource`, which reports the original's size. Export always uses
  `MediaPool::full_res_provider`. See [project-files.md](project-files.md#media-offline-relinking-proxies-ingest).
- **Seeking.** `codecs::Mp4Source` seeks to the preceding sync sample and decodes forward, caching
  every frame of the GOP. Sequential playback reuses the decoder. Decoders implement
  `codecs::VideoDecoder`. `register_video_decoder` puts a factory in front of the built-in ones, so a
  hardware decoder can take precedence. The GOP cache never holds its lock while decoding.
  Decoders run slices on rayon, so an export worker waiting inside a decode can pick up another
  frame of the same source. A request that finds the shared decoder busy decodes with a private
  decoder.
- **Compositor.** `render` is the reference for monitors, thumbnails and export. `render::plan`
  turns a frame into GPU layers. Brightness & Contrast currently runs in WGSL and is checked against
  the CPU effect implementation. Other standard effects, Non-Normal blend modes, adjustment layers,
  nested sequences and non-dissolve transitions are rendered on the CPU for that layer or frame and
  handed to the GPU as an image. On Linux, the desktop app prefers the AMD wgpu adapter when it is
  surface-compatible; video decoding still uses the software codec path. Setting
  `FILMCRAFT_CPU_COMPOSITE=1` forces the CPU compositor in the desktop app.
- **Frame scheduling.** `crates/ui-egui/src/frames.rs` runs a small pool of worker threads with
  prioritised jobs: the frame on screen first, then playback prefetch, then thumbnails. The UI never
  decodes. It shows the exact frame when it is ready and holds the nearest cached frame meanwhile.
  Play waits for the first frames (`PREROLL_FRAMES`, at most 0.5 s) before starting the clock.
  Every refresh, `schedule_playback` asks for the next frames and drops (or cancels, through
  `filmcraft_media::cancel`) jobs for frames the playhead has passed; a new on-screen frame
  replaces the one asked for before (scrubbing); Stop cancels the prefetch. When frames cost more
  than the workers can render in real time (CPU effects), it starts only frames that can still be
  on time and spaces them evenly (`playback_plan`). GPU plans carry their texels already
  converted for upload (`filmcraft_gpu::prepare`), so the UI thread only copies them.
- **Audio clock.** The desktop app passes a cpal output (`apps/filmcraft/src/audio.rs`) to the UI as
  `AudioOut`. While playing, the samples played by the sound card drive the playhead and video follows.
  Without an audio device, playback falls back to the wall clock. `PlaybackMeter` counts timeline
  frames: shown when the exact picture was on screen while due, dropped otherwise (including frames
  passed over without a refresh, not while the window is hidden). `cargo xtask bench-playback`
  measures the whole path headlessly ([testing.md](testing.md) §5).
  Sequence audio goes through the mixer graph (`render::mixer`, §5.1); clip audio effects run on
  `audio-dsp` via `render::audio_fx`.

### 5.1 Audio mixer

```text
clip: gain → clip effects → Volume / Channel Volume / Panner (clip keyframes, media time)
      → audio transitions → summed per track                           render::audio::track_input
track / submix strip:  input map, mono fold → pre-fader inserts → pre-fader sends → mute
      → fader (volume) → meter → post-fader inserts → post-fader sends → pan / balance → output
Mix:  bus sum → pre-fader inserts → fader → meter → post-fader inserts → out    render::mixer
```

- **Model** (`project::mixer`). Every audio track, submix and the Mix has a `MixerStrip`. Static
  values stay in `Track::volume_db`, `pan`, `muted` and the send/effect parameters; automation is
  keyframes in sequence ticks: lanes `volume`, `pan`, `mute` (hold), `send.<i>.level` in
  `MixerStrip::lanes`, and insert parameters (`fx.<slot>.<param>`) in the effect's own keyframes.
  Up to 5 inserts (`EffectInstance::post_fader` picks the side) and 5 sends per strip. Submixes feed
  the Mix or a submix after them (no feedback). All fields have serde defaults, so older projects
  load unchanged.
- **Graph** (`render::mixer::mix_graph`). Lanes are evaluated per sample; effect parameters update
  on an absolute 64-sample grid, so the output does not depend on how callers cut the timeline into
  requests (export batches and device callbacks give identical samples). Inserts that report
  latency delay their strip; each route into a bus gets a compensation delay and the graph is read
  ahead by its total latency. Graph state (DSP, delay lines) is cached per structure and continued by
  sequential readers; other requests start fresh with a pre-roll (effect tails, ≤ 3 s). Tracks run
  in parallel (rayon), buses in order. Mono tracks pan with the −3 dB constant-power law; stereo
  tracks and sends use balance. Solo keeps soloed and solo-safe strips plus everything feeding them
  or fed by them. 24 tracks × (EQ + Dynamics + Studio Reverb) + a compressed submix renders at
  ~6× realtime on one core (release).
- **Automation modes** (Premiere semantics). Off ignores lanes; Read plays them; Latch records from
  the first touch and holds the last value until playback stops; Touch records while held and ramps
  back to the existing automation over the **automatch time** (Preferences ▸ Audio, 1 s); Write
  records every control from playback start (then switches to Touch unless "Switch to Touch after
  Write" is off).
- **Recording** (`engine::mixer`). Playback start runs `mixer.recordStart`, stop runs
  `mixer.recordStop`. Fader and knob drags send `mixer.touch` (value, playhead) while held and
  `mixer.release` when let go. Held values go to `render::mixer::LiveMix`, which the playing mix
  reads, so moves are heard at once. At stop each gesture stream is thinned (linear keyframe
  thinning, optional minimum time interval) and written over its time range as one undo step,
  with boundary keyframes that keep the automation outside the range unchanged.
- **Live state.** `PreviewStore::live` (`LiveMix`) also carries per-strip meter peaks posted by the
  mix (Track Mixer and Audio Meters read them) and the newest project snapshot, which the audio
  callback uses, so edits made during playback are heard.

### 5.2 Essential Sound

```text
clip.essential (type + settings)  ──apply()──►  clip effects marked `essential`, clip gain, Volume, Panner
essentialSound.autoMatch   BS.1770 integrated loudness of render::audio::clip_signal → match gain (clip gain)
essentialSound.generateDucking   trigger clips' summed level (10 ms hops) → activity regions → Volume keyframes
```

- **Model** (`project::essential`). A clip's `essential: Option<EssentialSound>` holds its audio type
  (Dialogue / Music / SFX / Ambience) and per-type sections: Loudness (match gain, measured and target
  LUFS), Repair (Reduce Noise, Reduce Rumble, DeHum 50/60 Hz, DeEss, Reduce Reverb), Clarity
  (Dynamics, EQ preset + amount, Enhance Speech), Creative (Reverb preset + amount, Stereo Width for
  Ambience), Ducking (against types, sensitivity, reduce by, fades), Pan, Clip Volume and Mute.
- **Effects under the hood.** `essential::apply(item, old, t)` turns the settings into ordinary clip
  effects (Highpass, DeNoise, DeHummer, DeEsser, DeReverb, Dynamics Processing, Parametric Equalizer,
  Enhance Speech, Stereo Width, Studio Reverb) flagged `EffectInstance::essential`, in that order, ahead
  of the user's own effects. Only parameters whose derived value changed are written (at the playhead),
  so keyframes added in Effect Controls survive. A section switch bypasses its effects, a slot switch
  removes its effect, clearing the type removes them all. Auto-match gain, Clip Volume (an offset on
  the Volume level or on all its keyframes) and Pan are applied as deltas, so clearing restores the
  clip. Because these are normal effects, playback, the mixer and export need nothing special.
- **Loudness.** The clip signal (clip gain + effects, before Volume) is measured with the BS.1770
  meter; the gain is linear after the effects, so one measurement hits the target exactly. Targets are
  preferences (`audio.dialogueTargetLufs` −23, `musicTargetLufs` −25, `sfxTargetLufs` −21,
  `ambienceTargetLufs` −30).
- **Ducking.** Sensitivity 0…10 maps to a threshold −20 − 4·s dBFS on the summed trigger signal (50 ms
  window); regions shorter than 100 ms are dropped and pauses under 250 ms bridged; each region gets a
  fade-down before it and a fade-up after it (`audio_dsp::ducking::duck_keyframes`), written as Volume
  level keyframes that replace earlier ones.
- **Presets** (our own names and values) per type; user presets are saved in preferences
  (`essentialSound.userPresets`). Music remixing to a duration is out of scope (the Duration section
  says so).
- **Enhance Speech** is a DSP chain (high-pass, de-mud, presence and air EQ, expander, compressor), not a
  model: no speech-enhancement model with an open licence that we could ship and verify is bundled.
  DeepFilterNet (MIT/Apache-2.0, Rust inference via tract) is the candidate for a future optional
  integration behind a trait.

### 5.3 Masks

Every video effect and the intrinsic Opacity carry `EffectInstance::masks` (`project::mask`).
A mask is a closed cubic Bézier `MaskPath` in clip pixels (ellipse = four smooth vertices with
circular tangents, 4-point polygon = corner vertices, the pen draws arbitrary vertices), stored as a
`ParamValue::Path` so the ordinary keyframe engine animates it (vertex-wise interpolation; paths
with different vertex counts hold). Feather, Opacity and Expansion are ordinary float parameters.

- **Coverage** (`render::mask`): the path is flattened (≤ 0.05 working px chord error); the
  signed distance to the polygon (nonzero winding) plus Expansion goes through a falloff of width
  max(Feather, 1) centred on the edge (linear = exact box-filtered antialiasing at Feather 0,
  blending into smoothstep as Feather grows). Masks combine top to bottom with Add / Subtract /
  Intersect / Lighten / Darken / Difference; Inverted and Opacity apply per mask.
- **Semantics.** A masked effect is `lerp(original, effected, coverage)` per premultiplied channel
  (the effect only applies inside); Opacity masks scale the clip's layer before Motion, so they
  follow the clip's transform. Adjustment-layer masks are in sequence pixels.
- **GPU.** `filmcraft-gpu::GpuMask` evaluates the same coverage and mix in WGSL (compute), tested
  to agree with the CPU within 3·10⁻⁶. Layers with masks are CPU-rendered images in frame plans.
- **Editing.** `masks.*` commands (add / remove / set / moveVertex / translate / addVertex /
  removeVertex / toggleVertexSmooth / select / list); keyframe commands take `"mask": n`. The
  Program monitor overlay drags vertices, Bézier handles, the whole mask and the feather /
  expansion handles; drags merge into one undo step.
- **Tracking** (`render::track`, `masks.track`): Shi–Tomasi features inside the mask, pyramidal
  Lucas–Kanade (4 levels, 15×15 window) with a forward–backward check, then RANSAC + least squares
  for Position / Position & Rotation / Position, Scale & Rotation (2D Procrustes). Each frame's
  transform moves the path, written as Mask Path keyframes while the background job runs (one undo
  step per run; `jobs.cancel` stops and keeps what was tracked). Frames are tracked at ≤ 960 px
  wide. On synthetic footage with known similarity motion the path stays within 0.3 px over 20 frames.

### 5.4 Colour management

```text
frame (Y'CbCr/RGB + metadata) ─► source colour space: Interpret Footage override, else VUI/colr/MKV Colour
  ─► decode table (sRGB/BT.709, PQ, HLG scene light, camera log) ─► HLG OOTF ─► 3×3 to working gamut
  ─► BT.2390 tone map (HDR/log into an SDR sequence, Auto Tone Map Media) ─► gamut compression
  ─► effects + compositing in working linear (1.0 = reference white = SDR white = 203 cd/m²)
  ─► monitors: working → SDR BT.709 (tone map from HDR, gamut map from BT.2020)
  ─► HDR export: working → PQ/HLG BT.2020 R'G'B' → Y'CbCr (BT.2020 NCL) + VUI/colr/mdcv/clli/SEI
```

- **Model.** `SequenceSettings::color` (`ColorPipeline`: working space Rec. 709 / Rec. 2100 PQ /
  Rec. 2100 HLG, wide gamut, auto tone map) and `Interpretation::color_space` (per media item;
  `None` = from metadata). Commands: `sequence.colorSettings`, `clip.interpretFootage`,
  `color.spaces`, `media.colorInfo`.
- **Maths** in `filmcraft-color` (`transform`, `log`, `spaces`; formulas and sources in
  [crates/color/README.md](../crates/color/README.md)); the renderer side is `render::colorman`.
- **Fast path.** A Rec. 709 sequence without wide gamut and media whose metadata says Rec. 709 /
  sRGB decodes exactly as before, and its layers stay on the GPU. Log, HDR or wide-gamut media,
  and every layer of an HDR/wide-gamut sequence, are converted on the CPU (the GPU path draws them
  as pre-rendered images, like layers with effects).
- **Outputs.** `RenderOptions::working_output` returns working-space pixels (HDR exports,
  scopes); otherwise the top-level render is converted for an SDR monitor. Lumetri and the other
  colour effects work on display-encoded values clamped to 0..1, so in an HDR sequence they clip
  highlights above reference white (HDR-aware grading is future work).
- **HDR export.** H.264 and ProRes exports of a PQ/HLG sequence encode BT.2020 PQ/HLG (ProRes
  10-bit; our H.264 encoder is 8-bit, so H.264 HDR is 8-bit) and signal it in the VUI / ProRes
  frame header, `colr`, and for PQ `mdcv` (BT.2020 / D65, 1000 / 0.0001 cd/m²) and `clli`
  (MaxCLL/MaxFALL 0 = unknown, they are not measured) plus the matching H.264 SEI. `sdr: true`
  exports the tone-mapped SDR picture instead; render previews always do.
- **Monitors / display colour management.** The monitors are SDR (sRGB-encoded RGBA8 textures):
  HDR sequences are shown tone mapped. macOS EDR (extended-range `CAMetalLayer` output) is not
  wired up: there is no `platform` crate yet and eframe/wgpu do not expose an EDR surface, so HDR
  values above SDR white are never sent to the display. The scopes of an HDR sequence show the
  working-space values (waveform in cd/m² on a PQ scale, BT.2020 vectorscope).
- **LUTs.** Lumetri Input LUT and Creative Look reference `lib:<id>` (the project's LUT library,
  `Project::luts`, embedded `.cube`/`.3dl` text) or `builtin:<id>` (code-generated camera
  conversions and looks). `filmcraft-gpu::GpuLut` is the WGSL tetrahedral counterpart, tested for
  parity.

### 5.4 Multi-camera and synchronisation

```text
clips ──sync (in | out | timecode[±hours] | marker | audio)──► anchors (media time ↔ common instant)
  ├─ clip.synchronize        moves selected timeline clips (link groups together) onto the reference
  ├─ clip.mergeClips         video + ≤16 audio clips → a merged-clip sequence
  └─ clip.createMulticam     cameras → a multi-camera source sequence (one video track per angle)
multi-camera clip = nested source + TrackItem::multicam {enabled, angle}
  render: only the angle's video track · audio: camera 1 | all | the angle (Switch Audio)
```

- **Model** (`project::multicam`). `Sequence::multicam` (`MulticamSource`: cameras with their video
  track, audio tracks, name, shown flag and source item; audio mode) marks a multi-camera source;
  `Sequence::merged` a merged clip. `TrackItem::multicam` (`MulticamSel`) makes a nested clip a
  multi-camera clip; any nest can be one (its video tracks are then the angles,
  `Sequence::cameras()`). Editing a multi-camera source into a sequence gives an enabled clip on the
  first angle (`Project::make_track_item`). Project schema v7.
- **Sync** (`engine::sync`). Each method reduces a clip to an *anchor* (the media time that lines up
  with the common instant). Audio uses `audio_dsp::sync::find_offset`: DC removal, windowed-sinc
  decimation to ≤ 8 kHz, GCC-PHAT-β (β = 0.75) via one packed complex FFT for the coarse lag, then
  the same at the full rate on the loudest common window (≤ 2.7 s) and parabolic interpolation.
  Recordings with different gains, microphones (filtered), 0 dB SNR noise or a strong echo are
  aligned to the sample; two 10-minute recordings take ~2.4 s (release). Clips are placed on frame
  boundaries with the sub-frame remainder taken from their source In, so video stays on frames
  while audio keeps sample accuracy.
- **Render.** `render::item_layer` renders a multi-camera clip's angle track only
  (`render_seq_tracks`); `render::audio` mixes the nested source with only the audible tracks
  (`Sequence::with_angle_audio`). Nested audio now plays at all (it was skipped when the nest had
  no media source) and is limited to the clip's range. `render::multicam::render_grid` renders the
  shown angles at the cell scale in parallel (rayon) and tiles them: the Multi-Camera view is one
  frame job (`frames::Target::MulticamGrid`), prefetched while playing like the program.
- **Editing** (`edit::multicam`, `engine::multicam`). `multicam.switchAngle` (click an angle,
  Ctrl/⌘-click for video only), `multicam.selectCamera1…9` (keys 1–9) and `cutToCamera1…9`
  (Ctrl+1–9), Enable/Flatten, Edit Cameras, Audio Follows Video (`EditorState`). Live switching:
  playback in the Multi-Camera view runs `multicam.recordStart`; each key/click is a
  `multicam.cut` that is applied at once (the program shows it) by re-applying the whole pass to
  the project from before the pass (`edit_merged`), so a pass is one undo step; Stop runs
  `multicam.recordStop`, which ends the last angle at the stop point. Through edits inside the
  recorded range are healed, so pressing the angle already showing adds no edit. Flatten replaces
  a clip by the clip(s) its angle shows (outer effects carried over; linked pairs stay linked).

## 6. Export jobs (`filmcraft-export`)

```text
file.exportMedia {path, format, scale, audio, quality}
  → engine creates a Job {id, label, progress, result} and runs it on a background thread
  → export: render frames in parallel batches → encode in order → mux; audio mixed per batch
  → jobs.list shows progress; jobs.cancel sets the shared cancel flag
```

| Format | Encoder | Container |
|---|---|---|
| `h264` | `filmcraft-h264enc` + `filmcraft-aac` | MP4 (`isobmff`) |
| `prores` | `filmcraft-prores` | MOV |
| `dnxhr` | `filmcraft-dnx` (LB / SQ / HQ / HQX) | MOV (`AVdh`) |
| `mjpeg` | built in | MOV |
| `png`, `gif`, `wav` | built in | image sequence / GIF / WAV |

Video encoders implement `export::VideoEncoder`. Codec crates plug in with `register_encoder` and
`register_audio_encoder`.

Timelines can be exchanged as EDL, FCP7 XML, FCPXML or OTIO. `file.import` detects these formats and
merges the result into the project as one undoable step, and `file.exportInterchange`,
`file.exportEdl`, `file.exportFcpxml` and `file.exportOtio` write them.

## 7. Automation surfaces

All of these dispatch the same command ids.

| Surface | Where | Scope |
|---|---|---|
| UI | `ui-egui` menus, shortcuts, panels | `menus::invoke` → engine or UI command |
| CLI | `filmcraft-cli` | `exec <id> key=value…`, `run script.jsonl` (one `{"id","params"}` per line), `inspect`, `import`, `export`, `render`, `probe`; `--save`, `--bridge` |
| Control channel | `filmcraft --control <port>` | JSON lines on loopback TCP: engine commands plus synthetic input, inspection and screenshots of the live UI |
| MCP | `filmcraft-cli mcp` | stdio MCP server: headless in-process session, or `--bridge` to the control channel |

- **Automation ids.** Every interactive widget calls `app.auto.add(id, rect, label)` each frame
  (`crates/ui-egui/src/automation.rs`). Agents click by id, e.g. `tools.Razor`,
  `timeline.clip.<id>`, `effects.item.gaussian_blur`, `panel.Timeline`.
- **UI state** that is not project data (tool, workspace, dock layout, zoom, scroll, monitor
  settings) is in `crates/ui-egui/src/state.rs` as serde structs, so `ui.inspect` and `ui.set` can
  read and write it.

Protocol reference: [control-protocol.md](control-protocol.md). Agent guide: [agents.md](agents.md).

## 8. Not built yet

The layer table reserves names for crates that don't exist yet: `riff`, `mjpeg`,
`keyframe`, `effects`, `audio`, `scopes`, `playback`, `format` and `platform`.
Until they exist, that work lives elsewhere: keyframes and effect definitions in `project`, effects
and the audio mix in `render`, scopes and playback in `ui-egui`, and OS integration (cpal, rfd,
native menus) in `apps/filmcraft`. [ROADMAP.md](../ROADMAP.md) has the milestone status.
