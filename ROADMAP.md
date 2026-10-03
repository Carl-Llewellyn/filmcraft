# FilmCraft Roadmap

Progress toward feature parity with Adobe Premiere Pro, with estimates. Updated as milestones land.

**Last updated:** 2026-10-02 · **Overall parity:** ~72% (measured scorecard below) · **Code:** 34 crates, 1258 tests

## Parity scorecard

Measured against Premiere Pro 26.5 on this machine. Menu items: the native menu bar dump, minus
Adobe-cloud-only items (Team Projects, Productions, Firefly, Stock, Dynamic Link, account/help pages),
matched against `filmcraft-cli commands` and the UI command table. Effects: the Effects panel tree.

| Area | Weight | Measured | Coverage |
|---|---|---|---|
| Editing, timeline, trimming, multicam | 20% | core edit algebra, trim modes, sync, multicam, menu long tail, Scene Edit Detection, Automate to Sequence | ~80% |
| Menus / commands | (cross-check) | ~325 of 344 in-scope menu items | ~95% |
| Effects and transitions | 12% | video effects 93/93 (+Legacy, Obsolete), audio effects 53/53, video transitions 84/84 (+21 Legacy), audio transitions 3/3; some approximations (Warp Stabilizer 2-D, Morph Cut, Auto Reframe) | ~92% |
| Media I/O and codecs | 12% | H.264 (software + Linux NVIDIA NVENC), HEVC, VP9, AV1, ProRes, DNx, MJPEG, Opus, AAC, MP4/MOV/MKV/WebM; no MXF, image sequences, HW decode | ~70% |
| Panels and UI fidelity | 12% | all main panels, audio effect editor windows; Metadata, Media Browser, scopes (parade/histogram), Timecode, Events panels thin or missing | ~65% |
| Audio | 10% | mixer, automation, Essential Sound, meters, all 53 effects; no 5.1 buses, voice-over record | ~70% |
| Colour | 8% | Lumetri complete, LUTs, colour management, HDR | ~80% |
| Graphics and captions | 8% | text engine, shapes, captions, transcripts; no MOGRT, rolls/crawls, responsive design | ~60% |
| Export | 8% | own H.264/AAC, ProRes, DNxHR, PNG/GIF/WAV; no preset library/queue UI, AAF/OMF, MXF | ~60% |
| Preferences and project management | 5% | Settings dialog with 16 categories (most settings wired), project settings, scratch disks, search bins, templates | ~80% |
| Performance | 5% | 1080p real-time, 3×1080p; 4K/8K and AV1 real-time not yet | ~50% |
| **Weighted total** | | | **~72%** |

## Estimate to parity

Measured throughput in the last work block (2026-10-01 night → 10-02): five Opus 5.5 agents in
parallel for ~4.5 wall-clock hours (~18 agent-hours including integration) moved parity by ~4 points
(≈4–5 agent-hours per point), on a machine that was heavily overloaded (load 150–300 on 14 cores) and
once ran out of disk. The remaining points are a long tail (each effect, dialog and preference page is
small, but there are many), so the estimate applies a 1.3–1.5× tail factor.

| | Opus 5.5 agent-hours | Wall-clock (4–5 parallel agents + integrator) |
|---|---|---|
| Feature parity by checklist (~28 points left) | ~130–180 | **~30–45 h** |
| Robust on real-world material (codec edge cases, 4K/8K performance, pro workflows) | +120–200 | **+30–50 h** |
| **Total to "100% and better"** | **~250–380** | **~60–95 h (≈2.5–4 days, 24/7)** |

The 2026-10-02 block moved parity ~62% → ~72% with six agents in ~6 wall-clock hours, despite the
disk filling twice; the remaining work is mostly panels (scopes, Metadata, Media Browser), export
presets/queue, codecs (MXF, image sequences, hardware decode), performance and graphics templates.

Limits on speed: CPU and disk on one machine (more than ~5 agents slows everyone down: each worktree
build is 6–7 GB and a full `cargo xtask ci` takes 20–60 min under load), and a single integrator
merging, resolving conflicts (e.g. two agents bumping the project schema) and re-running CI. With one
agent and no parallelism, multiply wall-clock by ~3–4.

Not reachable clean-room and locally: **Generative Extend** (needs a large video-generation model).
**Enhance Speech** and **Auto Reframe** are feasible only with openly licensed models we can ship.

## Milestones

Status: ✅ done · 🟡 in progress · ⬜ not started. Estimates are remaining agent-hours.

| # | Milestone | Status | Done | Remaining | Est. |
|---|---|---|---|---|---|
| M0 | Skeleton + visual shell | ✅ | Workspace, 20 crates, dock/workspaces, Premiere 26 look, native menus, control channel, MCP, xtask gates (layers, wasm) | — | — |
| M1 | Media I/O | ✅ | MP4/MOV demux+mux, WAV, stills, MJPEG, symphonia audio (MP3/FLAC/ALAC/Vorbis), GOP seek + frame cache, import | Media Browser polish | 2 |
| M2 | H.264 decoder | ✅ | Own decoder, bit-exact on 37+ streams, 500–600 fps 1080p | — | — |
| M3 | Editing core | 🟡 | Edit algebra (insert/overwrite/razor/lift/extract/ripple/roll/slip/slide/rate-stretch/nest/paste), tools, markers, trim mode + Trim Monitor + dynamic J/K/L trimming, Keyboard Shortcuts editor with FilmCraft/Premiere/FCP/Avid presets; Edit/Clip/File menu commands (Label colours and Select Label Group, Paste/Remove Attributes, Select All Matching, Remove Unused, Consolidate Duplicates, Sequence From Clip, Bin From Selection, Offline File, Close Project, Make/Edit Subclip, Modify Audio Channels/Timecode, Frame Hold Options/Add Frame Hold/Insert Frame Hold Segment, Time Interpolation with frame blending, Fit/Fill frame, Breakout to Mono, Extract Audio, Replace With Clip; M3.11: Scene Edit Detection (pure-Rust cut detection, background job), Normalize Mix Track, Simplify Sequence, Transcribe Sequence, Find/Find Next and search bins, Automate to Sequence, Edit Original, Edit Offline, Source Settings, Update Metadata (XMP), Generate Audio Waveform, Project Settings General/Scratch Disks, Get Media File Properties, Save as Template, Selection as FilmCraft Project, Avid Log Exchange export, Flash Cue markers, Dynamic Audio Waveforms, Reveal Log Files, System Compatibility Report); multicam (Create Multi-Camera Source Sequence, Multi-Camera view with live switching on 1–9, angle switching, Enable/Flatten, Edit Cameras) and sync (Synchronize, Merge Clips; In/Out/timecode/marker/audio — GCC-PHAT, sample-accurate) | Multicam paging >16 angles and grid thumbnails, optical flow (renders as frame blending), ~200 Premiere default shortcuts whose commands don't exist yet | 6–10 |
| M4 | Playback | 🟡 | Audio-clock master, prefetch with cancellation, J/K/L, correct dropped-frame stats, playback resolution, render bar + content-hashed render previews, App Nap opt-out; 1080p H.264 and 3 stacked 1080p streams play with 0 dropped frames | 4K under load, 8K, frame-threaded AV1 decode, reduced-resolution decode for multicam grids | 6–10 |
| M5 | Effects, keyframes, GPU | 🟡 | ~60 CPU effects, 30 transitions, keyframes + value/velocity graphs, wgpu compositor, effect + opacity masks (ellipse/polygon/pen, feather, expansion, modes; CPU/WGSL parity), mask tracking (Lucas–Kanade + RANSAC), adjustment layers, effect presets (built-in + user, JSON import/export) | Full ~150-effect catalogue, WGSL parity for all effects and masks in the live GPU path, Warp Stabilizer, Morph Cut | 20–30 |
| M6 | Export | ✅ | Own H.264 encoder (High/Main/Baseline, B-frames, VBR/CBR/2-pass) → MP4 + own AAC; Linux NVIDIA NVENC H.264 backend with Hardware/Software selector (hardware default when available, CPU option/fallback); ProRes, MJPEG, PNG, GIF, WAV; background jobs | Preset library, queue UI, smart render | 6–8 |
| M7 | Audio | 🟡 | Mixer graph (tracks → submixes → Mix, pre/post-fader inserts and sends, latency-compensated, sample-accurate, ~6× realtime for 24 tracks × 3 effects on one core), Audio Track Mixer + Audio Clip Mixer panels, track automation (Off/Read/Latch/Touch/Write, recorded live while playing, thinned to keyframes, timeline lanes with pen editing), solo/solo-safe, channel mapping basics, peak + BS.1770 loudness meters (match ffmpeg), DSP crate with 16 clip/track effects, Audio Gain (set/adjust/normalize), Constant Power / Constant Gain / Exponential Fade, Essential Sound (types, Loudness auto-match, Repair incl. DeEss/DeReverb, Clarity, Creative, Ducking keyframes, presets; full Dialogue chain 22× realtime) | Music duration remix, ML speech enhancement, 5.1 panner and multichannel buses, voice-over record, effect editor windows (EQ curve), remaining effects (multiband, convolution reverb), clip-mixer automation recording | 6–9 |
| M8 | Colour | 🟡 | Lumetri: basic, creative + looks, RGB & hue curves, wheels, HSL secondary, vignette, section bypass; Input LUT / Look LUT (.cube 1D/3D/shaper, .3dl; tetrahedral CPU + WGSL; project LUT library; built-in camera conversions); Colour Match (Oklab tonal-range statistics, skin protection, solved in Lumetri wheels); colour management: Rec. 709 / Rec. 2100 PQ / HLG working spaces + wide gamut, Interpret Footage colour space (S-Log3, V-Log, Canon Log 2/3, LogC3/4, Apple Log, D-Log from published specs), metadata auto-detect, BT.2390 tone mapping, gamut mapping, HDR export signalling (VUI/colr/mdcv/clli/SEI, ffprobe-verified); scopes incl. HDR nits waveform | Parade/histogram/HLS scopes, HDR-aware Lumetri maths, macOS EDR monitors, mastering metadata → tone-map peak, D-Log M (no published formula), HSL Secondary refine | 3–5 |
| M9 | More codecs | 🟡 | ProRes decode+encode, AAC decode+encode, HEVC Main/Main 10 decoder (bit-exact on 41 fixtures, ~225 fps 1080p), VP9 decoder (profiles 0–3, 8/10/12-bit, bit-exact on 50+ fixtures; WebM/MKV `V_VP9` and MP4 `vp09` import with key-frame-checked seeking), Matroska/WebM import (H.264/HEVC/VP9/ProRes/MJPEG + AAC/Opus/FLAC/MP3/Vorbis/PCM), Opus decoder (SILK/CELT/hybrid, 5.1/7.1 multistream; all RFC 8251 vectors range-exact; WebM/MKV/MP4), DNxHD/DNxHR decoder (all SMPTE ST 2019-1 CIDs, 8/10/12-bit, 4:2:2/4:4:4, interlaced, alpha; within IDCT precision of ffmpeg on 22 fixtures) + DNxHR LB/SQ/HQ/HQX/444 encoder and MOV `AVdh` export, AV1 decoder (Main profile, 8/10-bit, all intra/inter tools, loop filters, superres, film grain, intra BC, spatial layers; bit-exact vs libdav1d on 19 SVT fixtures + 22 libaom vectors; MP4 `av01` / WebM `V_AV1` import) | VP9 frame threading, AV1 threading/SIMD (~4 fps 1080p today), AV1 High/Professional profiles, Ogg Opus files, MXF, hardware decode | 12–20 |
| M10 | Graphics & captions | 🟡 | Caption tracks (Subtitle/CEA-608/708/Teletext formats, track style), SRT/WebVTT/SCC import+export (frame-exact, property-tested), caption editing (add/split/merge/trim/move, sync-locked insert/extract), Text panel Captions tab, burn-in in Program monitor and export; text engine (`crates/text`: bundled + system fonts, harfrust shaping, bidi, line breaking, paragraph layout, glyph cache; 3-line 1080p title ≈ 0.15 ms warm); graphic clips with text + shape layers (fill, 2 strokes, background, shadow, keyframable transform), Type tool with on-monitor editing, shape/pen tools, Properties/Essential Graphics editor, align/distribute | Responsive design pins, rolls/crawls, per-character styles, motion graphics templates, MCC/STL/TTML, 608/708 embedding, speech-to-text | 12–18 |
| M11 | Interchange & project management | ✅ | `.fcproj` schema versions + migrations, atomic saves, Save a Copy/Revert, auto-save ring + crash-recovery journal (Preferences ▸ Auto Save, recovery prompt), FCP7 XML, FCPXML, EDL, OTIO; XML media linking runs as a background job with live filename/count progress; offline media (own slate) + Link Media (fingerprint-checked relink, folder remap, search, Align Timecode, Make Offline); proxies (ProRes Proxy/LT, H.264 ¼/½ background jobs, attach/detach/reconnect, monitor toggle, export full-res) + ingest (copy/transcode/proxies); Project Manager (collect, consolidate + transcode with handles, size estimate) | Rename media to clip names, image-sequence conversion, Media Browser-driven relink, smart (cross-drive) path tracking | 1–2 |
| M12–M16 | Multicam, web (WASM), platform, long tail | 🟡 | Multicam: audio sync (GCC-PHAT, sub-sample), Merge Clips, multi-camera source sequences, Multi-Camera view with live switching (1–9); L0–L4 crates compile to wasm32 | Web app shell (file access, WebCodecs, audio), scene detection, auto reframe, ~800 remaining commands and dialogs, performance hardening | 35–55 |
| M17 | Native Premiere project import | ⬜ | — | Direct `.prproj` gzip/XML object-graph reader; import project bins, media, sequences, tracks, clips, timing/retiming, nesting, markers and supported effects; async parse/link with progress; preserve unknown Premiere components and emit a loss report instead of silently dropping them. Validate against the user-provided large project and paired Premiere XML exports; keep private media/project data out of committed fixtures. | Premiere-only plugins, MOGRTs and unsupported effects cannot be promised to render identically; grow mappings from the loss report and tests | 20–35 |

## Running now

- Nothing in progress; next: M17 native `.prproj` import, then remaining Premiere menu items (OMF/AAF, Find) and AV1 single-thread re-measure on an idle machine

## Log

- **2026-10-02 (later):** all 93 video effects (+Legacy/Obsolete bins), all 53 audio effects with Parametric/Graphic EQ, Multiband Compressor and Dynamics editor windows, all 84 video transitions (+21 Legacy), Settings dialog (16 categories, most wired), remaining menu items (Scene Edit Detection, Find, Normalize Mix Track, Simplify Sequence, Automate to Sequence, search bins, templates, Project Settings with scratch disks, ALE and selection-as-project export, system report); project schema v11. 1258 tests.

- **2026-10-02 (NVENC + import planning):** Linux H.264 NVENC export is complete, including hardware/software selection and hardware-first auto selection when available. XML media linking now reports live background progress. Planned M17: direct Premiere `.prproj` import, validated against a large user-provided sample and paired Premiere XML exports; structural completeness is the target, while Premiere-only effects/plugins remain explicitly reported limitations.

- **2026-10-02:** Premiere menu long tail: Sequence/Markers (gaps, split edits, through edits, subsequence, Delete Tracks, range/chapter markers, ripple sequence markers), Clip/Edit/File (Paste/Remove Attributes, subclips, Frame Hold Options, Time Interpolation with frame blending, Audio Channels, Breakout to Mono, Extract Audio, Replace With Clip, Remove Unused, Consolidate Duplicates), View/monitors (paused resolution, alpha/RGB display modes, comparison view, magnification, rulers, guides + templates, snapping), Graphics and Titles (vertical text, shape layers, align/distribute/arrange). Transcripts and text-based editing (Text panel, optional local Whisper). AV1 tile/frame/post-filter threading. Agent-friendly CLI (`exec`, `inspect`, `describe`, `import`, `export`, `run -`, `--save`, `--bridge`). Project schema v10.

- **2026-10-01 (night):** new README hero (Apollo 11 documentary edit, NASA public domain); agents never steal keyboard focus; masks + tracking, adjustment layers, effect presets; multicam + audio sync; DNxHD/DNxHR decode/encode; AV1 decoder bit-exact against libdav1d (all stages, film grain, superres, SVC).

- **2026-10-01 (M12 multicam):** Synchronize / Merge Clips / Create Multi-Camera Source Sequence (sync by In, Out, timecode ± hours, clip markers, or audio: GCC-PHAT cross-correlation, offsets recovered to the sample on noisy multi-mic recordings), multi-camera clips (angle render, Switch Audio), Program monitor Multi-Camera view with live switching (keys 1–9, one undo step per pass), Ctrl+1–9 cuts, Enable / Flatten / Edit Cameras, project schema v7. Fixed: nested sequence audio was silent.
- **2026-10-01 (evening):** M11 done: offline media with our own slate and the Link Media dialog (fingerprint-checked relink, folder remap, search), proxies (create in background / attach / toggle; export stays full-res) with ingest settings, Project Manager (collect, consolidate + transcode). Fixed: GOP cache deadlock when a rayon decoder inside a parallel export re-entered the same source.
- **2026-10-01 (M7.4):** Essential Sound panel: Dialogue/Music/SFX/Ambience types, Loudness auto-match (BS.1770, exact to the target), Repair (noise, rumble, hum, new DeEsser and spectral DeReverb), Clarity (dynamics, EQ presets, Enhance Speech DSP chain), Creative reverb / stereo width, ducking that writes Volume keyframes, presets; all `essentialSound.*` commands, effects visible and keyframable in Effect Controls.
- **2026-10-01 (night):** M8 colour: camera log curves and gamuts from published specs, colour-managed pipeline (PQ/HLG working spaces, Interpret Footage, BT.2390 tone mapping), HDR export signalling verified with ffprobe, LUTs (.cube/.3dl, tetrahedral CPU + WGSL, project library, Lumetri Input LUT / Look), Colour Match, Lumetri section bypass, HDR scopes.

- **2026-10-01 (night):** DNxHD/DNxHR decoder + encoder (SMPTE ST 2019-1) with MOV export; AV1 decoder (spec v1.0.0 + Errata 1, all decoding tools incl. film grain) bit-exact against libdav1d, wired into MP4/WebM/MKV import with key-frame-checked seeking.
- **2026-10-01 (later):** text engine (shaping, bidi, line breaking) + Type/Shape/Pen tools + graphic clips + graphics panel; Trim Monitor + dynamic trimming; Keyboard Shortcuts editor; audio mixer with automation. Fixed: MP4 muxer wrote unreadable all-empty sample tables; system font scan race.

- **2026-10-01:** Audio mixing (M7.2/M7.5/M7.6 basics): mixer graph with submixes, sends, inserts and latency compensation; Audio Track / Clip Mixer panels; Latch/Touch/Write automation recorded live; timeline track keyframes; Audio Gain dialog.
- **2026-10-01:** recovered from a machine crash with no lost work. Merged: VP9 decoder (profiles 0–3, bit-exact, WebM/MKV/MP4), Opus decoder (RFC 8251 range-exact; WebM/MKV/MP4), captions (SRT/VTT/SCC, burn-in), render bar + render previews, project schema versioning + atomic saves + auto-save + crash-recovery journal, test infrastructure (golden images, loudness oracle vs ffmpeg, headless scripted UI tests; fixed a GPU stale-texture bug), public contributor docs and licence files.

- **2026-09-30 (evening):** Opus decoder (RFC 6716/8251, range-exact on every conformance vector, ~80–110× realtime 48 kHz stereo) wired into WebM/MKV and MP4 import.
- **2026-09-30 (afternoon):** Matroska/WebM import; LUFS meters; clip audio effects on the DSP crate.
- **2026-09-30 (midday):** HEVC decoder, Matroska/WebM demuxer and audio DSP merged; HEVC import wired; asset rules (AGENTS.md, ATTRIBUTION.md, `cargo xtask assets`); README with hero screenshot and the Craft family.
- **2026-09-30 (late morning):** keyframe value/velocity graphs; xtask gates; H.264 MP4 export; Lumetri curves, wheels, looks, HSL secondary.
- **2026-09-30 (early morning):** H.264 decoder, ProRes, AAC; GPU compositor; MCP; Premiere 26 visual fidelity pass.
