# Control protocol & MCP

## Desktop control channel
`filmcraft --control 9876` (or `FILMCRAFT_CONTROL_PORT`) listens on `127.0.0.1:<port>` (loopback only).
One JSON request per line → one JSON reply per line. The app opts out of macOS App Nap
(`apps/filmcraft/src/app_nap.rs`): a hidden window would otherwise drop the whole process to
background priority, and on a busy machine it would stop answering.

```json
{"id": 1, "method": "engine.execute", "params": {"command": "sequence.addEdit", "params": {"seconds": 3}}}
{"id": 1, "ok": true, "result": {"cuts": 2}}
```

Methods (handlers in `crates/ui-egui/src/control.rs`):

| Method | Params | |
|---|---|---|
| `engine.execute` / `ui.menu.invoke` | `{command, params}` | any engine or UI command id |
| `engine.commands` / `ui.menu.list` | – | command registry / menu tree |
| `ui.inspect` | – | UI state (tool, workspace, dock, timeline view, playback, fps…) |
| `ui.elements` | `{prefix?}` | every on-screen interactive element: id, label, rect |
| `ui.set` | `{tool, workspace, mode, theme, focused, playbackRes, timeline:{pps,scroll,fit}, program:{…}, source:{…}, clipDialog:{param: value}, menuDialog:{param: value}, panels:{scopes, timecode, reference, events, progress}, export:{…}}` | `program` / `source` merge fields into the monitor view state (`res`, `paused_res`, `high_quality`, `display`, `zoom`, `pan`, `show_rulers`, `show_guides`, `lock_guides`, `snap`, `guides`, `compare_ref`, …; see [monitors.md](monitors.md)); `clipDialog` sets fields of the open Edit / Clip / File dialog (Paste Attributes, Make Subclip, Frame Hold Options, …); `menuDialog` those of the M3.11 dialogs (Find, Create Search Bin, Project Settings, Scene Edit Detection, Simplify Sequence, Automate to Sequence…; see `panels::menu_dialogs`); `panels` merges into the panel settings (`ui.inspect` → `ui.panels`; see `panels::panel_state`): Lumetri Scopes `{shown: ["vectorscopeYuv"|"vectorscopeHls"|"histogram"|"parade"|"waveform"], waveformType, paradeType, colorSpace, brightness, scale, clamp, targets}`, Timecode `{rows: [{source, mode, display}], showName}`, Reference Monitor `{ganged, time, display: "composite"|"scopes", scopes}`, Events `{level, selected}`, Progress `{showFinished}` `export` deep-merges into Export mode state (`preset` — applied first —, `settings` (ExportSettings JSON), `fileName`, `location`, `range`, `customStart`, `customEnd`, `openSections`, `manager` (Preset Manager: `query`, `favoritesOnly`, `selected`, `saveName`), `quickOpen`, `quickPath`, `quickPreset`); `program` / `source` merge fields into the monitor view state (`res`, `paused_res`, `high_quality`, `display`, `zoom`, `pan`, `show_rulers`, `show_guides`, `lock_guides`, `snap`, `guides`, `compare_ref`, …; see [monitors.md](monitors.md)); `clipDialog` sets fields of the open Edit / Clip / File dialog (Paste Attributes, Make Subclip, Frame Hold Options, …); `menuDialog` those of the M3.11 dialogs (Find, Create Search Bin, Project Settings, Scene Edit Detection, Simplify Sequence, Automate to Sequence…; see `panels::menu_dialogs`) |
| `ui.panel.show` / `ui.panel.close` | `{panel}` | |
| `ui.click` / `ui.move` | `{id}` or `{x,y}`, `button`, `count`, `modifiers` | synthetic pointer input |
| `ui.drag` | `{from, to, steps, modifiers}` | press–move–release |
| `ui.scroll` | `{id|x,y, dx, dy, modifiers}` | wheel / trackpad |
| `ui.key` / `ui.type` | `{key}` (`Cmd+K`, `Space`…) / `{text}` | keyboard |
| `ui.timeline.hit` / `ui.timeline.locate` | `{x,y}` / `{clip, edge?}` | timeline hit-testing |
| `ui.playback` | `{action: play|stop|toggle, speed?}` | |
| `ui.screenshot` | `{path?, panel?}` | PNG of the window or one panel; fails after 10 s when no frame is presented (window hidden, display asleep) |
| `ui.resize`, `ui.focus`, `app.quit` | | `ui.focus` is the only request that activates the app and takes keyboard focus |
| `perf.stats` | – | performance counters (also the command `perf.stats`, so `engine.execute` and MCP `command_run` reach it; headless sessions return the engine part). `decode`: GOP-cache requests / hits / `cacheHitRate`, decoder seeks, samples decoded and skipped while catching up, decoder ms (total and per sample); `playback`: playing, shown / dropped frames and `dropRate` of the current or last play, resolution; `frames`: frame-worker jobs, cancelled, `requestHitRate`, `decodeMs` / `renderMs` (mean, p50, p95 of the last 256 jobs), queue, render-cost estimate, cache use; `ui`: fps and frame ms; `process`: CPU seconds; `media`, `jobs`. Counters are cumulative: diff two readings to measure an interval ([performance.md](performance.md)) |

**Focus.** Driving the app never steals the user's keyboard. Started with `--control`, the app opens
without activating itself. UI requests and screenshots that need a rendered frame bring the window
forward without making it key (macOS `orderFrontRegardless`), so the user's typing keeps going to
the app they are in. Only an explicit `ui.focus` activates FilmCraft.

Element ids are stable, e.g. `timeline.clip.<id>`, `timeline.track.V1.lock`, `tools.Razor`,
`project.item.<id>`, `effects.item.gaussian_blur`, `panel.tab.Timeline`, `program.transport.playback.toggle`.

Audio mixing: `mixer.*` commands (strips `"A1"`, `"S1"`, `"Mix"` or ids; lanes `volume`, `pan`,
`mute`, `send.<i>.level`, `fx.<slot>.<param>`): `mixer.inspect`, `mixer.setStrip` (volume, pan, mute,
solo, record arm, solo safe, mode Off/Read/Latch/Touch/Write, output, input map, channels),
`mixer.setValue`, `mixer.touch` / `mixer.release` (a fader gesture; recorded during an automation pass),
`mixer.recordStart` / `mixer.recordStop` (playback runs them), `mixer.addSubmix`, `mixer.deleteSubmix`,
`mixer.addInsert` / `removeInsert` / `setInsert`, `mixer.addSend` / `setSend` / `removeSend`,
`mixer.setKeyframe` / `deleteKeyframe` / `moveKeyframe` / `clearLane`, `mixer.writeAutomation`;
`clipMixer.set`; `clip.audioGain {mode: set|adjust|normalizeMax|normalizeAll, db}`, `clip.audioPeak`;
`effects.setDefaultTransition`. UI ids: `mixer.<A1|S1|Mix>.<fader|value|pan|panValue|mode|mute|solo|
record|soloSafe|output|input|fx.<n>|send.<n>|meter|name>` (popup entries below them, e.g.
`mixer.A1.mode.Touch`, `mixer.A1.fx.0.studio_reverb`), `mixer.showEffects`, `mixer.transport.<cmd>`,
`clipMixer.A1.<fader|pan|mute|solo|keyframe|value>`, `timeline.track.A1.keyframes[.<lane>|.clip]`,
`timeline.track.A1.lane[.kf.<n>]`, `audioGain.<set|adjust|normalizeMax|normalizeAll|ok|cancel|peak>`.

Essential Sound: `essentialSound.inspect`, `essentialSound.setType {type: dialogue|music|sfx|ambience}`,
`essentialSound.clearType`, `essentialSound.set {key, value}` or `{values: {key: value}}` (keys such as
`repair.noise.on`, `repair.noise.amount` (0–10), `repair.humHz`, `clarity.eqPreset`, `clarity.enhanceTone`,
`creative.reverbPreset`, `ducking.against`, `ducking.reduceDb`, `ducking.fadeS`, `pan.value`,
`volume.levelDb`, `mute`, section switches `<section>.enabled`), `essentialSound.applyPreset {preset, type?}`,
`essentialSound.savePreset {name}`, `essentialSound.deletePreset {name}`, `essentialSound.autoMatch {target?}`,
`essentialSound.generateDucking`; all take `clips` (default: the selected audio clips). UI ids:
`essentialSound.tab.<Browse|Edit>`, `essentialSound.type.<Dialogue|Music|SFX|Ambience>`,
`essentialSound.clearType`, `essentialSound.preset[.save|.delete|.name|.ok]`,
`essentialSound.section.<Name>[.toggle]`, a row per setting key (`essentialSound.repair.noise.on`,
`essentialSound.repair.noise.amount`, `essentialSound.repair.humHz.<i>`), `essentialSound.autoMatch`,
`essentialSound.ducking.against.<Type>`, `essentialSound.generateDucking`, `essentialSound.volume.on`,
`essentialSound.volume.levelDb`, `essentialSound.mute`, `essentialSound.browse.<Type>.<preset>`.

Project files, auto-save, crash recovery and preferences commands (`file.recover`, `prefs.set`, …) and their
automation ids are listed in [project-files.md](project-files.md). That file also lists the media
management commands and dialog ids: offline media and relinking (`media.findMissing`,
`media.relink`, `media.autoRelink`, `media.search`, `media.makeOffline`, `media.status`, `linkMedia.*`),
proxies (`media.createProxies`, `media.attachProxies`, `media.toggleProxies`, `proxies.*`), ingest
(`project.ingestSettings`) and the Project Manager (`file.projectManager`, `pm.*`).

**Trim mode** (`crates/engine/src/trim.rs`): `trim.selectEditPoint` / `trim.selectNearest` enter trim
mode (the Program monitor becomes the Trim Monitor, ids `trimMonitor.*`); `trim.monitor` returns what
it shows. Dynamic trimming takes an explicit clock (seconds) so it is deterministic: `trim.shuttle
{direction, slow?, clock}` (J/L), `trim.tick {clock}`, `trim.shuttleStop {clock?}` (K, one undo step),
`trim.cancelDynamic` (Esc), `trim.playAround {clock, loop?}` (Space / Shift+K).

**Colour** (`crates/engine/src/color.rs`): `sequence.colorSettings {workingSpace: rec709|rec2100-pq|
rec2100-hlg, wideGamut, autoToneMap}`, `clip.interpretFootage {items?, colorSpace: auto|<id>}`,
`color.spaces`, `media.colorInfo {item}`; LUTs: `lut.import {path, name?}`, `lut.list`,
`lut.remove {id}`, `lut.export {lut, path, format?}`, `lumetri.setInputLut` / `lumetri.setLook
{clip?, lut: lib:<id>|builtin:<id>|"", path?}`, `lumetri.setSection {clip?, section, on?}`,
`lumetri.applyMatch {clip?, referenceTime|referenceFrame|referenceTimecode, faceDetection?}`. Without
params the menu entries open dialogs (ids `colorDialog.space.<id>`, `colorDialog.working.<id>`,
`colorDialog.wideGamut`, `colorDialog.autoToneMap`, `colorDialog.ok|cancel`). Lumetri panel ids:
`lumetri.input_lut`, `lumetri.look_lut`, `lumetri.switch.<section>`, `lumetri.section.<section>`,
`lumetri.match.*`; HDR scopes: `scopes.hdrWaveform`. `file.exportMedia {sdr: true}` exports an HDR
sequence as tone-mapped SDR.

**Keyboard shortcuts** (`crates/engine/src/shortcuts.rs`): `shortcuts.list {query?, panel?}`,
`shortcuts.get`, `shortcuts.set {command, keys, panel?, add?, keepConflicts?}`, `shortcuts.clear`,
`shortcuts.undo` / `shortcuts.redo`, `shortcuts.conflicts {platform?}`, `shortcuts.forKey {key}`,
`shortcuts.resolve {keys, panel?}`, `shortcuts.presets`, `shortcuts.loadPreset` / `savePreset` /
`deletePreset {name}`, `shortcuts.export` / `import {path}`, `shortcuts.audit`. Keys use `Cmd` (⌘ /
Ctrl), `Ctrl` (macOS ⌃), `Alt`, `Shift`. The dialog (Edit ▸ Keyboard Shortcuts…, ⌥⌘K) uses ids
`shortcuts.*` (`shortcuts.key.K`, `shortcuts.cell.<command>`, `shortcuts.ok`, …).

## MCP
`filmcraft-cli mcp` serves MCP on stdio: headless (in-process session; `--demo` / `--project p.fcproj`)
or `--bridge 127.0.0.1:9876` to drive the running app. Tools: `command_list`, `command_run`,
`project_inspect`, `sequence_inspect`, `media_import`, `render_frame`, `ui_inspect`, `ui_elements`,
`ui_click`, `ui_drag`, `ui_key`, `ui_type`, `ui_screenshot`, `ui_control`. `.mcp.json` registers both.

Time is in ticks (254 016 000 000 per second); commands also accept `seconds`, `frame` or `timecode`.
