//! Interchange documents (CMX 3600 EDL, FCP7 XML, FCPXML, OTIO) ↔ the session's project.

use serde_json::{Value, json};
use std::sync::mpsc::Receiver;

use filmcraft_interchange::{ExportOptions, Format, ImportOptions};
use filmcraft_project::{ItemId, ItemKind, MediaRef};

use crate::{EngineError, Result, Session};

/// The interchange format of a file, if it is one (by content, with the extension as a hint).
pub fn detect(path: &str, bytes: &[u8]) -> Option<Format> {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if !matches!(ext.as_deref(), Some("edl" | "xml" | "fcpxml" | "otio")) {
        return None;
    }
    filmcraft_interchange::detect(bytes, ext.as_deref())
}

fn report_json(r: &filmcraft_interchange::Report) -> Vec<String> {
    r.entries.iter().map(|e| if e.count > 1 { format!("{} (×{})", e.message, e.count) } else { e.message.clone() }).collect()
}

/// Import a document: merge its bins, media and sequences into the project (one undo step), then
/// link each media file that exists on disk. Returns the new sequences.
pub fn import(s: &mut Session, path: &str, bytes: &[u8], format: Format) -> Result<Value> {
    let p = std::path::Path::new(path);
    let opts = ImportOptions {
        base_dir: p.parent().map(|d| d.to_string_lossy().to_string()),
        name: p.file_stem().map(|n| n.to_string_lossy().to_string()),
        ..Default::default()
    };
    let (fragment, report) = filmcraft_interchange::import_with(bytes, format, &opts).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let before: std::collections::HashSet<ItemId> = s.project.items.keys().copied().collect();
    let name = opts.name.clone().unwrap_or_else(|| format.name().to_string());
    let seqs = s.edit(&format!("Import {name}"), |proj, _| Ok(filmcraft_interchange::merge_into(proj, fragment, None)))?;
    // Link media: probe each new file-backed item that exists.
    let new_media: Vec<(ItemId, String)> = s
        .project
        .items
        .values()
        .filter(|i| !before.contains(&i.id))
        .filter_map(|i| match &i.kind {
            ItemKind::Media(m) => match &m.media {
                MediaRef::File { path } => Some((i.id, path.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let mut linked = 0;
    let mut offline = Vec::new();
    for (id, mpath) in new_media {
        let opened = s.services.read_file(&mpath).ok().and_then(|b| {
            let fname = std::path::Path::new(&mpath).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            s.media.open_bytes(&fname, b.into()).ok()
        });
        match opened {
            Some(src) => {
                let info = src.info().clone();
                let identity = crate::relink::identity_of(&*s.services, &mpath).ok();
                let rebase = (format == Format::Edl).then_some(info.start_timecode).flatten();
                s.edit("Link Media", |proj, _| {
                    if let Some(item) = proj.items.get_mut(&id)
                        && let ItemKind::Media(m) = &mut item.kind
                    {
                        let rate = info.video.as_ref().map(|v| v.frame_rate);
                        m.info = info.clone();
                        m.offline = false;
                        m.identity = identity;
                        if let (Some(tc), Some(rate)) = (rebase, rate) {
                            filmcraft_interchange::rebase_source_timecode(proj, id, tc, rate);
                        }
                    }
                    Ok(())
                })?;
                s.media.insert_file(id, &mpath, src);
                linked += 1;
            }
            None => offline.push(mpath),
        }
    }
    if let Some(&first) = seqs.first() {
        s.state.active_sequence = Some(first);
        if !s.state.open_sequences.contains(&first) {
            s.state.open_sequences.push(first);
        }
    }
    Ok(json!({
        "format": format.name(),
        "sequences": seqs.iter().map(|i| i.0).collect::<Vec<_>>(),
        "linkedMedia": linked,
        "offlineMedia": offline,
        "report": report_json(&report),
    }))
}

/// A media file opened by the interchange-import worker and waiting to be applied on the UI
/// thread. Media sources are Send + Sync, but project edits remain on the Session thread.
struct LinkedMedia {
    id: ItemId,
    path: String,
    opened: Option<(filmcraft_media::SharedSource, filmcraft_media::MediaInfo, Option<filmcraft_project::MediaIdentity>)>,
}

pub struct PendingImport {
    job: u64,
    rx: Receiver<LinkedMedia>,
    linked: Vec<LinkedMedia>,
}

/// Import an interchange document without blocking the frontend while its referenced media are
/// opened. Parsing and merging are quick; probing all the linked movie files is the expensive part.
pub fn import_background(s: &mut Session, path: &str, bytes: &[u8], format: Format) -> Result<Value> {
    let p = std::path::Path::new(path);
    let opts = ImportOptions {
        base_dir: p.parent().map(|d| d.to_string_lossy().to_string()),
        name: p.file_stem().map(|n| n.to_string_lossy().to_string()),
        ..Default::default()
    };
    let (fragment, report) = filmcraft_interchange::import_with(bytes, format, &opts).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let before: std::collections::HashSet<ItemId> = s.project.items.keys().copied().collect();
    let name = opts.name.clone().unwrap_or_else(|| format.name().to_string());
    let seqs = s.edit(&format!("Import {name}"), |proj, _| Ok(filmcraft_interchange::merge_into(proj, fragment, None)))?;
    let media: Vec<(ItemId, String)> = s
        .project
        .items
        .values()
        .filter(|i| !before.contains(&i.id))
        .filter_map(|i| match &i.kind {
            ItemKind::Media(m) => match &m.media {
                MediaRef::File { path } => Some((i.id, path.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect();
    if let Some(&first) = seqs.first() {
        s.state.active_sequence = Some(first);
        if !s.state.open_sequences.contains(&first) {
            s.state.open_sequences.push(first);
        }
    }
    let job_id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let job = crate::Job { id: job_id, label: "Importing Media".into(), progress: Default::default(), result: Default::default() };
    job.progress.total.store(media.len() as u64, std::sync::atomic::Ordering::Relaxed);
    if media.is_empty() {
        job.progress.finished.store(true, std::sync::atomic::Ordering::Relaxed);
        *job.progress.status.lock().unwrap_or_else(|e| e.into_inner()) = "Import complete".into();
        *job.result.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Ok(filmcraft_export::Report { path: path.into(), frames: 0, seconds: 0.0, bytes: 0, render_fps: 0.0 }));
    } else {
        let (tx, rx) = std::sync::mpsc::channel();
        let services = s.services.clone();
        let pool = s.media.clone();
        let progress = job.progress.clone();
        let total = media.len();
        std::thread::Builder::new()
            .name("filmcraft-import-media".into())
            .spawn(move || {
                use std::sync::atomic::Ordering;
                for (index, (id, media_path)) in media.into_iter().enumerate() {
                    if progress.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let filename =
                        std::path::Path::new(&media_path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| media_path.clone());
                    *progress.status.lock().unwrap_or_else(|e| e.into_inner()) = format!("Importing: {filename} — {}/{}", index + 1, total);
                    let opened = services.read_file(&media_path).ok().and_then(|bytes| {
                        let src = pool.open_bytes(&filename, bytes.into()).ok()?;
                        let info = src.info().clone();
                        let identity = crate::relink::identity_of(&*services, &media_path).ok();
                        Some((src, info, identity))
                    });
                    if tx.send(LinkedMedia { id, path: media_path, opened }).is_err() {
                        break;
                    }
                    progress.done.store((index + 1) as u64, Ordering::Relaxed);
                }
                progress.finished.store(true, Ordering::Relaxed);
            })
            .map_err(|e| EngineError::Other(format!("could not start media import: {e}")))?;
        s.import_jobs.push(PendingImport { job: job_id, rx, linked: Vec::new() });
    }
    s.jobs.push(job);
    Ok(json!({
        "format": format.name(),
        "sequences": seqs.iter().map(|i| i.0).collect::<Vec<_>>(),
        "linkedMedia": 0,
        "offlineMedia": [],
        "job": job_id,
        "report": report_json(&report),
    }))
}

/// Drain media probe results and commit them to the project from the Session/UI thread.
pub fn poll_imports(s: &mut Session) {
    use std::sync::atomic::Ordering;
    for task in &mut s.import_jobs {
        while let Ok(result) = task.rx.try_recv() {
            task.linked.push(result);
        }
    }
    let finished: Vec<usize> = s
        .import_jobs
        .iter()
        .enumerate()
        .filter(|(_, task)| s.jobs.iter().find(|j| j.id == task.job).is_none_or(|j| j.progress.finished.load(Ordering::Relaxed)))
        .map(|(i, _)| i)
        .collect();
    for index in finished.into_iter().rev() {
        let task = s.import_jobs.remove(index);
        let mut opened = Vec::new();
        let mut offline = Vec::new();
        for item in task.linked {
            if let Some((src, info, identity)) = item.opened {
                opened.push((item.id, item.path, src, info, identity));
            } else {
                offline.push((item.id, item.path));
            }
        }
        let linked_count = opened.len() as u64;
        if !opened.is_empty() {
            let updates: Vec<_> = opened.iter().map(|(id, _, _, info, identity)| (*id, info.clone(), *identity)).collect();
            let _ = s.edit("Link Imported Media", |proj, _| {
                for (id, info, identity) in updates {
                    if let Some(item) = proj.items.get_mut(&id)
                        && let ItemKind::Media(m) = &mut item.kind
                    {
                        m.info = info;
                        m.offline = false;
                        m.identity = identity;
                    }
                }
                Ok(())
            });
            for (id, path, src, _, _) in opened {
                s.media.insert_file(id, &path, src);
            }
        }
        if let Some(job) = s.jobs.iter().find(|j| j.id == task.job) {
            let total = job.progress.total.load(Ordering::Relaxed);
            *job.progress.status.lock().unwrap_or_else(|e| e.into_inner()) =
                format!("Import complete: {linked_count}/{total} linked; {} offline", offline.len());
            *job.result.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Ok(filmcraft_export::Report { path: String::new(), frames: linked_count, seconds: 0.0, bytes: 0, render_fps: 0.0 }));
        }
    }
}

/// `file.exportInterchange {format: "edl"|"xml"|"fcpxml"|"otio", path, sequence?}`
pub fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let format = match p.get("format").and_then(Value::as_str).unwrap_or("xml") {
        "edl" => Format::Edl,
        "fcpxml" => Format::Fcpxml,
        "otio" => Format::Otio,
        _ => Format::Fcp7Xml,
    };
    let path = p.get("path").and_then(Value::as_str).ok_or_else(|| EngineError::Other("need `path`".into()))?.to_string();
    let seq = p.get("sequence").and_then(Value::as_u64).map(ItemId).or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
    let opts = ExportOptions {
        relative_to: std::path::Path::new(&path).parent().map(|d| d.to_string_lossy().to_string()),
        name: s.project.item(seq).map(|i| i.name.clone()),
        ..Default::default()
    };
    let (bytes, report) = filmcraft_interchange::export(&s.project, seq, format, &opts).map_err(|e| EngineError::Other(e.to_string()))?;
    s.services.write_file(&path, &bytes).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"path": path, "bytes": bytes.len(), "report": report_json(&report)}))
}
