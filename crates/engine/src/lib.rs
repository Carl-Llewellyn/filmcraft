//! The FilmCraft engine façade.
//!
//! Every user-visible action is a command with a stable id (`sequence.addEdit`,
//! `timeline.trim`, `markers.markIn`…) and JSON parameters, dispatched through
//! [`Session::execute`]. The egui UI, the CLI, the control channel and the MCP server all use this
//! one entry point; that is what makes the UI swappable and the whole app agent-drivable.
//!
//! The project is an `Arc<Project>` edited copy-on-write; undo keeps whole-project snapshots (cheap
//! thanks to structural sharing of untouched items).

pub mod autosave;
pub mod captions;
pub mod clip_ops;
pub mod color;
pub mod commands;
pub mod demo;
pub mod essential_sound;
pub mod graphics;
pub mod interchange;
pub mod masks;
pub mod media_pool;
pub mod mixer;
pub mod multicam;
pub mod presets;
pub mod previews;
pub mod project_manager;
pub mod project_tools;
pub mod proxies;
pub mod relink;
pub mod scene_detect;
pub mod sequence_extras;
pub mod sequence_tools;
pub mod settings;
pub mod shortcut_presets;
pub mod shortcuts;
pub mod sync;
pub mod transcript;
pub mod trim;

use std::sync::Arc;

use filmcraft_edit::EditCtx;
use filmcraft_project::{ClipId, ItemId, Project, Sequence, TrackId, TrackKind};
use filmcraft_time::{FrameRate, Tick};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use commands::{CommandSpec, command_specs, find as find_command};
pub use filmcraft_export as export;
pub use filmcraft_project as project;
pub use filmcraft_render as render;
pub use filmcraft_time as time;
pub use media_pool::MediaPool;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("command `{0}` is not available right now: {1}")]
    Disabled(String, String),
    #[error("invalid parameters for `{cmd}`: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("no active sequence")]
    NoSequence,
    #[error("edit failed: {0}")]
    Edit(#[from] filmcraft_edit::EditError),
    #[error("media: {0}")]
    Media(#[from] filmcraft_media::MediaError),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Host services (file access, clipboard…) injected by the frontend.
pub trait Services: Send + Sync {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>>;
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()>;
    /// Size of a file in bytes (an error when it is missing). Hosts should override this: the
    /// default reads the whole file.
    fn file_size(&self, path: &str) -> std::io::Result<u64> {
        self.read_file(path).map(|b| b.len() as u64)
    }
    /// Up to `len` bytes of a file starting at `offset` (fewer at the end of the file).
    fn read_range(&self, path: &str, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        let b = self.read_file(path)?;
        let a = (offset as usize).min(b.len());
        Ok(b[a..(a + len).min(b.len())].to_vec())
    }
    /// A random-access reader for a media file, so containers are opened without reading the
    /// whole file (web: `Blob` range reads). `None`: the host has none; media are read whole.
    fn reader(&self, _path: &str) -> Option<std::io::Result<filmcraft_media::SharedReader>> {
        None
    }
    /// Exports are encoded in memory and handed to [`Services::write_file`] (hosts without a
    /// filesystem, e.g. the web: the file is then offered as a download). Otherwise they stream
    /// to the output path.
    fn export_in_memory(&self) -> bool {
        false
    }
}

/// Native filesystem services.
pub struct FsServices;
impl Services for FsServices {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        // Atomic + durable: temp file in the same directory, fsync, rename, fsync the directory.
        filmcraft_format::atomic_write(std::path::Path::new(path), data)
    }
    fn file_size(&self, path: &str) -> std::io::Result<u64> {
        let m = std::fs::metadata(path)?;
        if m.is_file() { Ok(m.len()) } else { Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{path} is not a file"))) }
    }
    fn read_range(&self, path: &str, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut out = Vec::with_capacity(len);
        f.take(len as u64).read_to_end(&mut out)?;
        Ok(out)
    }
}

/// Undo history of whole-project snapshots.
#[derive(Clone, Default)]
pub struct History {
    pub undo: Vec<(String, Arc<Project>)>,
    pub redo: Vec<(String, Arc<Project>)>,
    /// Labels of all applied states, oldest first (History panel).
    pub limit: usize,
    /// Key of the last [`Session::edit_merged`] step: a continuous gesture (a fader drag) with the
    /// same key folds into that one undo step.
    pub merge_key: Option<String>,
}

impl History {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

/// Source patching / track targeting (the track header "V1/A1" source buttons + target toggles).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Targeting {
    /// Timeline tracks targeted for navigation / paste / match frame.
    pub targeted: Vec<TrackId>,
    /// Destination track for the source's video (source patch), None = unpatched.
    pub video_dest: Option<TrackId>,
    pub audio_dest: Option<TrackId>,
}

/// Editing state that commands depend on (not project data, but headless-relevant).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EditorState {
    pub active_sequence: Option<ItemId>,
    /// Playhead per sequence.
    pub playheads: std::collections::BTreeMap<ItemId, Tick>,
    /// Item loaded in the Source monitor and its playhead (media time).
    pub source_item: Option<ItemId>,
    pub source_playhead: Tick,
    /// Selected timeline items.
    pub selection: Vec<ClipId>,
    /// Selected project panel items.
    pub project_selection: Vec<ItemId>,
    pub targeting: std::collections::BTreeMap<ItemId, Targeting>,
    pub snapping: bool,
    pub linked_selection: bool,
    /// Open sequences (timeline tabs), in tab order.
    pub open_sequences: Vec<ItemId>,
    /// Timeline clipboard (serialized track items with their relative track index).
    #[serde(skip)]
    pub clipboard: Vec<(TrackKind, usize, filmcraft_project::TrackItem)>,
    pub default_video_transition: String,
    pub default_audio_transition: String,
    /// Selected edit points (trim mode).
    #[serde(default)]
    pub edit_points: Vec<trim::EditPoint>,
    /// Trim Monitor Out/In shift counters for the selected edit point.
    #[serde(default)]
    pub trim_shift: trim::TrimShift,
    /// Selected captions (caption tracks / Captions panel).
    #[serde(default)]
    pub caption_selection: Vec<ClipId>,
    /// Selected layers (indices among the graphic layers, 0 = back) of the selected graphic clip.
    #[serde(default)]
    pub graphic_layers: Vec<usize>,
    /// The mask being edited on the Program monitor (Effect Controls selection).
    #[serde(default)]
    pub selected_mask: Option<masks::MaskSel>,
    /// Multi-Camera Audio Follows Video: switching a multi-camera clip's angle switches its linked
    /// audio clips too.
    #[serde(default)]
    pub multicam_audio_follows_video: bool,
    /// Sequence ▸ Selection Follows Playhead: moving the playhead selects the clips under it on
    /// targeted tracks.
    #[serde(default)]
    pub selection_follows_playhead: bool,
    /// Sequence ▸ Show Through Edits: the timeline marks cuts between continuous pieces of a clip.
    #[serde(default)]
    pub show_through_edits: bool,
    /// Markers ▸ Ripple Sequence Markers: ripple edits move (and delete) sequence markers too.
    #[serde(default)]
    pub ripple_sequence_markers: bool,
    /// Markers ▸ Copy Paste Includes Sequence Markers.
    #[serde(default)]
    pub copy_paste_sequence_markers: bool,
    /// Marker colours hidden in the Markers panel (Show All Marker Colors clears this).
    #[serde(default)]
    pub hidden_marker_colors: Vec<filmcraft_project::Label>,
    /// Sequence markers copied with the clipboard clips (start relative to the copied range).
    #[serde(skip)]
    pub clipboard_markers: Vec<filmcraft_project::Marker>,
    /// The last Edit ▸ Find… (Find Next continues it).
    #[serde(default)]
    pub find: Option<project_tools::FindState>,
}

/// Events for frontends (drained each frame).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Event {
    ProjectChanged { revision: u64 },
    Toast { message: String, error: bool },
    OpenSequence(ItemId),
    OpenSource(ItemId),
}

pub struct Session {
    pub project: Arc<Project>,
    pub history: History,
    pub revision: u64,
    pub saved_revision: u64,
    pub path: Option<String>,
    pub state: EditorState,
    pub media: Arc<MediaPool>,
    pub services: Arc<dyn Services>,
    pub events: Vec<Event>,
    /// Commands executed (for macros/debugging): (id, params).
    pub journal: Vec<(String, Value)>,
    /// Background jobs (exports, render previews…).
    pub jobs: Vec<Job>,
    /// Media-linking work started while importing an interchange document.
    pub import_jobs: Vec<interchange::PendingImport>,
    /// User preferences (`prefs.*` commands) and where they persist (None = not persisted).
    pub prefs: autosave::Preferences,
    pub prefs_path: Option<std::path::PathBuf>,
    /// Auto-save ring + crash-recovery journal (native frontends start it; None = off).
    pub persistence: Option<autosave::Persistence>,
    /// Schema version of the file at `path` as found on disk (older = upgraded on load; the first
    /// save over it keeps a backup of the original).
    pub loaded_schema: u32,
    /// Render preview files + render-bar segments (shared with the frontend's frame workers).
    pub previews: Arc<previews::PreviewStore>,
    /// Audio Track Mixer automation pass in progress.
    pub mixrec: mixer::Recorder,
    /// Multi-camera live switching pass in progress.
    pub mcrec: multicam::Recorder,
    /// Dynamic (J/K/L) trimming and trim-mode loop playback in progress.
    pub trim_play: trim::TrimPlayback,
    /// Keyboard shortcuts (active bindings, presets; `shortcuts.*` commands).
    pub shortcuts: shortcuts::Shortcuts,
    /// Missing / offline media found by the last scan (Link Media dialog).
    pub offline: relink::OfflineState,
    /// Proxy / ingest / project-manager jobs whose results still have to be applied to the project.
    pub media_jobs: Vec<proxies::PendingJob>,
    /// Mask tracking jobs whose keyframes are still being written.
    pub mask_jobs: Vec<masks::PendingTrack>,
    /// Scene Edit Detection jobs whose results are applied when they finish.
    pub scene_jobs: Vec<scene_detect::PendingScene>,
    /// Effect presets (built-in + the user's, persisted in the data directory).
    pub presets: presets::PresetLibrary,
    /// Exports run a batch at a time by [`Session::pump_jobs`] (hosts without threads: web).
    pub stepped: Vec<SteppedJob>,
    /// Speech recogniser for `transcript.generate` (None = the Whisper model named by the command,
    /// feature `whisper`). Hosts and tests install one here.
    pub transcriber: Option<Arc<dyn filmcraft_speech::Transcriber>>,
    /// Nesting depth of [`Session::execute`] (commands that run other commands).
    exec_depth: u32,
}

/// An export advanced a batch at a time on the host's thread ([`Session::pump_jobs`]).
pub struct SteppedJob {
    pub job: u64,
    pub exporter: filmcraft_export::Exporter,
    pub provider: media_pool::PoolProvider,
    pub progress: Arc<filmcraft_export::Progress>,
    pub result: Arc<std::sync::Mutex<Option<std::result::Result<filmcraft_export::Report, String>>>>,
}

/// A background job with shared progress.
#[derive(Clone)]
pub struct Job {
    pub id: u64,
    pub label: String,
    pub progress: Arc<filmcraft_export::Progress>,
    pub result: Arc<std::sync::Mutex<Option<std::result::Result<filmcraft_export::Report, String>>>>,
}

impl Job {
    pub fn to_json(&self) -> Value {
        use std::sync::atomic::Ordering;
        let res = self.result.lock().unwrap_or_else(|e| e.into_inner()).clone();
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "progress": self.progress.fraction(),
            "done": self.progress.done.load(Ordering::Relaxed),
            "total": self.progress.total.load(Ordering::Relaxed),
            "status": self.progress.status.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            "finished": res.is_some(),
            "result": match res { Some(Ok(r)) => serde_json::to_value(r).unwrap_or_default(), Some(Err(e)) => serde_json::json!({"error": e}), None => Value::Null },
        })
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Arc::new(FsServices))
    }
}

impl Session {
    pub fn new(services: Arc<dyn Services>) -> Self {
        Self {
            project: Arc::new(Project::new("Untitled")),
            history: History { limit: 200, ..Default::default() },
            revision: 1,
            saved_revision: 1,
            path: None,
            state: EditorState {
                snapping: true,
                linked_selection: true,
                default_video_transition: "cross_dissolve".into(),
                default_audio_transition: "constant_power".into(),
                ..Default::default()
            },
            media: Arc::new(MediaPool::default()),
            services,
            events: Vec::new(),
            journal: Vec::new(),
            jobs: Vec::new(),
            import_jobs: Vec::new(),
            prefs: Default::default(),
            prefs_path: None,
            persistence: None,
            loaded_schema: filmcraft_format::SCHEMA_VERSION,
            previews: Arc::new(previews::PreviewStore::temp()),
            mixrec: Default::default(),
            mcrec: Default::default(),
            trim_play: Default::default(),
            shortcuts: shortcuts::Shortcuts::new(),
            offline: Default::default(),
            media_jobs: Vec::new(),
            mask_jobs: Vec::new(),
            scene_jobs: Vec::new(),
            presets: Default::default(),
            stepped: Vec::new(),
            transcriber: None,
            exec_depth: 0,
        }
    }

    /// Advance stepped jobs ([`SteppedJob`]) for up to `budget` (at least one step). Returns
    /// whether jobs remain. A job whose media is still loading waits for the next call.
    pub fn pump_jobs(&mut self, budget: std::time::Duration) -> bool {
        let t0 = web_time::Instant::now();
        while let Some(j) = self.stepped.first_mut() {
            match j.exporter.step(&j.provider, &j.progress) {
                Ok(filmcraft_export::Step::Progress) => {}
                Ok(filmcraft_export::Step::Pending) => return true,
                Ok(filmcraft_export::Step::Done(r)) => {
                    *j.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(Ok(r));
                    self.stepped.remove(0);
                }
                Err(e) => {
                    let e = e.to_string();
                    *j.progress.error.lock().unwrap_or_else(|x| x.into_inner()) = Some(e.clone());
                    j.progress.finished.store(true, std::sync::atomic::Ordering::Relaxed);
                    *j.result.lock().unwrap_or_else(|x| x.into_inner()) = Some(Err(e));
                    self.stepped.remove(0);
                }
            }
            if t0.elapsed() >= budget {
                break;
            }
        }
        !self.stepped.is_empty()
    }

    /// Start auto-save and the crash-recovery journal (native frontends). Loads preferences from
    /// the data directory and finds unsaved changes left by sessions that died
    /// ([`Session::recovery_candidates`]).
    pub fn start_autosave(&mut self, cfg: autosave::AutosaveConfig) -> std::io::Result<()> {
        let prefs_path = cfg.data_dir.join("preferences.json");
        self.prefs = autosave::Preferences::load(&prefs_path);
        self.media.set_use_proxies(self.prefs.media.enable_proxies);
        self.shortcuts.set_dir(&cfg.data_dir);
        self.presets.set_dir(&cfg.data_dir);
        self.prefs_path = Some(prefs_path);
        self.apply_media_cache();
        self.persistence = Some(autosave::Persistence::start(&cfg, self.prefs.auto_save.clone())?);
        self.sync_persistence();
        Ok(())
    }

    /// Clean shutdown: flush and stop the worker. Unsaved changes stay in the journal (offered on
    /// the next launch); otherwise the session's journal directory is removed.
    pub fn shutdown(&mut self) {
        self.sync_persistence();
        if let Some(p) = self.persistence.take() {
            p.close();
        }
    }

    pub fn recovery_candidates(&self) -> &[autosave::RecoveryCandidate] {
        self.persistence.as_ref().map(|p| p.candidates.as_slice()).unwrap_or(&[])
    }

    /// Tell the worker about the current project state if it changed (cheap: an `Arc` clone and a
    /// channel send; serialization happens on the worker). Called after every command.
    pub fn sync_persistence(&mut self) {
        let dirty = self.is_dirty();
        let Some(p) = self.persistence.as_mut() else { return };
        let cur = autosave::Sent { revision: self.revision, saved_revision: self.saved_revision, path: self.path.clone() };
        if p.last_sent.as_ref() == Some(&cur) {
            return;
        }
        if dirty {
            p.send_dirty(self.project.clone(), self.revision, self.path.clone());
        } else {
            p.send_clean();
        }
        p.last_sent = Some(cur);
    }

    /// Apply worker notifications (call regularly, e.g. once per UI frame). Also applies the
    /// results of finished proxy / ingest jobs.
    pub fn poll_persistence(&mut self) {
        proxies::poll(self);
        interchange::poll_imports(self);
        masks::poll(self);
        scene_detect::poll(self);
        let Some(p) = self.persistence.as_mut() else { return };
        for ev in p.drain_events() {
            match ev {
                autosave::WorkerEvent::SavedProject { path, revision } => {
                    if self.path.as_deref() == Some(path.as_str()) && revision > self.saved_revision && revision <= self.revision {
                        self.saved_revision = revision;
                    }
                }
                autosave::WorkerEvent::Error(m) => self.events.push(Event::Toast { message: m, error: true }),
                _ => {}
            }
        }
        self.sync_persistence();
    }

    /// Replace preferences (persisting them and updating the worker).
    pub fn set_prefs(&mut self, p: autosave::Preferences) -> std::io::Result<()> {
        let cache_changed = self.prefs.media_cache != p.media_cache;
        self.prefs = p;
        if cache_changed {
            self.apply_media_cache();
        }
        if self.media.use_proxies() != self.prefs.media.enable_proxies {
            self.media.set_use_proxies(self.prefs.media.enable_proxies);
            self.bump_view();
        }
        if let Some(w) = &self.persistence {
            w.set_prefs(self.prefs.auto_save.clone());
        }
        match &self.prefs_path {
            Some(path) => self.prefs.save(path),
            None => Ok(()),
        }
    }

    /// Keep render previews next to the saved project (moves an unsaved project's previews).
    pub fn previews_follow_path(&mut self) {
        if let Some(p) = &self.path
            && !cfg!(target_arch = "wasm32")
        {
            self.previews.move_to(project_tools::previews_dir(self).unwrap_or_else(|| previews::dir_for_project(p)));
        }
    }

    /// A change of what the project *looks like* that isn't an edit (proxies toggled): bumps the
    /// revision so frame caches refresh, without marking a clean project as modified.
    pub fn bump_view(&mut self) {
        let clean = !self.is_dirty();
        self.bump();
        if clean {
            self.saved_revision = self.revision;
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// Run a command by id.
    pub fn execute(&mut self, id: &str, params: Value) -> Result<Value> {
        let spec = commands::find(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        if self.exec_depth == 0 && spec.journal && self.trim_play.active() {
            self.settle_trim_playback(id);
        }
        (spec.enabled)(self).map_err(|why| EngineError::Disabled(id.to_string(), why))?;
        self.exec_depth += 1;
        let r = (spec.run)(self, &params);
        self.exec_depth -= 1;
        self.sync_persistence();
        // playback reads the newest snapshot (mixer moves, mutes… are heard while playing)
        self.previews.live.publish_project(self.project.clone());
        if r.is_ok() && spec.journal {
            self.journal.push((id.to_string(), params));
            if self.journal.len() > 10_000 {
                self.journal.drain(..1000);
            }
        }
        r
    }

    /// Another command arrives while trim-mode playback runs: a dynamic trim is committed first
    /// (so e.g. Undo undoes it as one step); loop playback stops unless it is a trim command.
    fn settle_trim_playback(&mut self, id: &str) {
        const LIVE: [&str; 5] = ["trim.shuttle", "trim.tick", "trim.shuttleStop", "trim.cancelDynamic", "trim.playAround"];
        if LIVE.contains(&id) {
            return;
        }
        if self.trim_play.dynamic.is_some() {
            trim::commit(self);
        }
        if !id.starts_with("trim.") {
            self.trim_play.around = None;
        }
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        commands::find(id).is_some_and(|c| (c.enabled)(self).is_ok())
    }

    /// Apply an undoable project edit. The closure gets a mutable copy; on error nothing changes.
    pub fn edit<R>(&mut self, label: &str, f: impl FnOnce(&mut Project, &mut EditorState) -> Result<R>) -> Result<R> {
        let mut p = (*self.project).clone();
        let mut st = self.state.clone();
        let r = f(&mut p, &mut st)?;
        let old = std::mem::replace(&mut self.project, Arc::new(p));
        self.history.undo.push((label.to_string(), old));
        if self.history.undo.len() > self.history.limit {
            self.history.undo.remove(0);
        }
        self.history.redo.clear();
        self.history.merge_key = None;
        self.state = st;
        self.bump();
        Ok(r)
    }

    /// Like [`Session::edit`], but consecutive calls with the same `key` (and no other edit in
    /// between) share one undo step: dragging a control is one undoable change.
    pub fn edit_merged<R>(&mut self, label: &str, key: &str, f: impl FnOnce(&mut Project, &mut EditorState) -> Result<R>) -> Result<R> {
        let merge = self.history.merge_key.as_deref() == Some(key) && self.history.undo.last().is_some_and(|u| u.0 == label);
        if !merge {
            let r = self.edit(label, f)?;
            self.history.merge_key = Some(key.to_string());
            return Ok(r);
        }
        let mut p = (*self.project).clone();
        let mut st = self.state.clone();
        let r = f(&mut p, &mut st)?;
        self.project = Arc::new(p);
        self.state = st;
        self.bump();
        Ok(r)
    }

    /// Edit the active sequence with the edit-algebra context.
    pub fn edit_sequence<R>(&mut self, label: &str, f: impl FnOnce(&mut Sequence, &mut EditCtx, &mut EditorState) -> Result<R>) -> Result<R> {
        let seq_id = self.state.active_sequence.ok_or(EngineError::NoSequence)?;
        let media = self.media.clone();
        self.edit(label, move |p, st| {
            let project_snapshot = p.clone();
            let durations = move |id: ItemId| -> Option<Tick> { media_duration(&project_snapshot, &media, id) };
            let min = p.sequence(seq_id).map(|s| s.settings.frame_rate.frame_duration()).unwrap_or(Tick(1));
            let mut next = p.next_id;
            let r = {
                let seq = p.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
                let mut ctx = EditCtx { next_id: &mut next, media_duration: &durations, min_duration: min };
                f(seq, &mut ctx, st)?
            };
            p.next_id = next;
            if let Some(s) = p.sequence(seq_id) {
                s.check().map_err(EngineError::Other)?;
            }
            Ok(r)
        })
    }

    pub fn undo(&mut self) -> Option<String> {
        let (label, prev) = self.history.undo.pop()?;
        self.history.merge_key = None;
        let cur = std::mem::replace(&mut self.project, prev);
        self.history.redo.push((label.clone(), cur));
        self.fix_state();
        self.bump();
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let (label, next) = self.history.redo.pop()?;
        self.history.merge_key = None;
        let cur = std::mem::replace(&mut self.project, next);
        self.history.undo.push((label.clone(), cur));
        self.fix_state();
        self.bump();
        Some(label)
    }

    /// Drop dangling references after undo/redo/delete.
    pub fn fix_state(&mut self) {
        let p = self.project.clone();
        if let Some(m) = self.state.selected_mask {
            let ok =
                self.active_sequence().and_then(|q| q.find_item(m.clip)).and_then(|(_, it)| it.effects.get(m.effect)).is_some_and(|e| m.mask < e.masks.len());
            if !ok {
                self.state.selected_mask = None;
            }
        }
        if let Some(s) = self.state.active_sequence
            && p.sequence(s).is_none()
        {
            self.state.active_sequence = p.sequences().next().map(|i| i.id);
        }
        self.state.open_sequences.retain(|s| p.sequence(*s).is_some());
        if let Some(s) = self.state.source_item
            && p.item(s).is_none()
        {
            self.state.source_item = None;
        }
        if let Some(seq) = self.state.active_sequence.and_then(|s| p.sequence(s)) {
            self.state.selection.retain(|c| seq.find_item(*c).is_some());
            self.state.caption_selection.retain(|c| seq.find_caption(*c).is_some());
        } else {
            self.state.selection.clear();
            self.state.caption_selection.clear();
        }
        self.state.project_selection.retain(|i| p.item(*i).is_some());
    }

    fn bump(&mut self) {
        self.revision += 1;
        self.events.push(Event::ProjectChanged { revision: self.revision });
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.events.push(Event::Toast { message: msg.into(), error: false });
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    // ---- accessors ----

    pub fn active_sequence(&self) -> Option<&Sequence> {
        self.state.active_sequence.and_then(|s| self.project.sequence(s))
    }

    pub fn playhead(&self) -> Tick {
        self.state.active_sequence.and_then(|s| self.state.playheads.get(&s).copied()).unwrap_or_default()
    }

    pub fn set_playhead(&mut self, t: Tick) {
        if let Some(s) = self.state.active_sequence {
            let rate = self.active_sequence().map(|s| s.settings.frame_rate).unwrap_or_default();
            self.state.playheads.insert(s, rate.snap(t.max(Tick::ZERO)));
            if self.state.selection_follows_playhead {
                sequence_tools::select_under_playhead(self);
            }
        }
    }

    pub fn sequence_rate(&self) -> FrameRate {
        self.active_sequence().map(|s| s.settings.frame_rate).unwrap_or_default()
    }

    pub fn targeting(&self) -> Targeting {
        let Some(sid) = self.state.active_sequence else { return Targeting::default() };
        if let Some(t) = self.state.targeting.get(&sid) {
            return t.clone();
        }
        // Default: V1/A1 patched and all tracks targeted.
        let seq = self.active_sequence();
        Targeting {
            targeted: seq.map(|s| s.all_tracks().map(|t| t.id).collect()).unwrap_or_default(),
            video_dest: seq.and_then(|s| s.video_tracks.first().map(|t| t.id)),
            audio_dest: seq.and_then(|s| s.audio_tracks.first().map(|t| t.id)),
        }
    }

    /// The media source for an item (creating it on first use).
    pub fn source(&self, item: ItemId) -> Option<filmcraft_media::SharedSource> {
        self.media.source_for(&self.project, item, &*self.services)
    }

    /// Render the active sequence at the playhead in its working colour space (HDR values kept;
    /// for scopes and analysis).
    pub fn render_program_working(&self, scale: f32) -> Option<filmcraft_render::Image> {
        let seq = self.state.active_sequence?;
        let provider = self.media.provider(self.project.clone(), self.services.clone());
        let opts = filmcraft_render::RenderOptions { scale, working_output: true, ..Default::default() };
        Some(filmcraft_render::render_sequence(&self.project, seq, self.playhead(), opts, &provider))
    }

    /// Render the active sequence at the playhead (CPU reference path).
    pub fn render_program(&self, scale: f32) -> Option<filmcraft_render::Image> {
        let seq = self.state.active_sequence?;
        let provider = self.media.provider(self.project.clone(), self.services.clone());
        let opts = filmcraft_render::RenderOptions { scale, captions: true, ..Default::default() };
        Some(filmcraft_render::render_sequence(&self.project, seq, self.playhead(), opts, &provider))
    }
}

/// Media duration of an item (None for stills/adjustment layers = unlimited handles).
pub fn media_duration(p: &Project, _pool: &MediaPool, id: ItemId) -> Option<Tick> {
    let it = p.item(id)?;
    match &it.kind {
        filmcraft_project::ItemKind::Media(m) => match m.info.kind {
            filmcraft_media::MediaKind::Still | filmcraft_media::MediaKind::Synthetic => None,
            _ => Some(m.info.duration),
        },
        filmcraft_project::ItemKind::Sequence(s) => Some(s.duration()),
        // a subclip that restricts trims ends at its Out point; otherwise its parent's media is the limit
        filmcraft_project::ItemKind::Subclip { range, restrict_trims: true, .. } => Some(range.end()),
        filmcraft_project::ItemKind::Subclip { parent, .. } => media_duration(p, _pool, *parent),
        filmcraft_project::ItemKind::AdjustmentLayer { .. } | filmcraft_project::ItemKind::Graphic { .. } => None,
    }
}

#[cfg(test)]
mod audio_effects_tests;
#[cfg(test)]
mod autosave_tests;
#[cfg(test)]
mod clip_ops_tests;
#[cfg(test)]
mod color_tests;
#[cfg(test)]
mod encoding_benchmark_tests;
#[cfg(test)]
mod essential_sound_tests;
#[cfg(test)]
mod file_tests;
#[cfg(test)]
mod masks_tests;
#[cfg(test)]
mod media_test_util;
#[cfg(test)]
mod mixer_tests;
#[cfg(test)]
mod multicam_tests;
#[cfg(test)]
mod presets_tests;
#[cfg(test)]
mod previews_tests;
#[cfg(test)]
mod project_manager_tests;
#[cfg(test)]
mod proxies_tests;
#[cfg(test)]
mod relink_tests;
#[cfg(test)]
mod sequence_tools_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod shortcuts_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transcript_tests;
#[cfg(test)]
mod trim_tests;
#[cfg(test)]
mod vfx_tests;
