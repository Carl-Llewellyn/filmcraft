//! Effect Controls: the selected clip's effects (fixed Motion/Opacity/Time Remapping first, as in
//! Premiere, then standard effects), generated from parameter schemas, with stopwatches and a
//! keyframe lane on the right. Also hosts the Lumetri Color panel body (same editor, grouped).

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use filmcraft_project::{ClipId, EffectInstance, Keyframe, ParamKind, ParamValue, TrackItem};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

const ROW_H: f32 = 22.0;

#[derive(Clone, Debug)]
pub(crate) struct KeyframeClipboardEntry {
    effect: String,
    param: String,
    mask: Option<usize>,
    offset: i64,
    keyframe: Keyframe,
}

fn selected_clip(app: &FilmcraftApp) -> Option<(ClipId, TrackItem, filmcraft_project::TrackKind)> {
    let seq = app.session.active_sequence()?;
    let mut best = None;
    for c in &app.session.state.selection {
        if let Some((tid, it)) = seq.find_item(*c) {
            let kind = seq.track(tid)?.kind;
            if kind == filmcraft_project::TrackKind::Video {
                return Some((*c, it.clone(), kind));
            }
            best.get_or_insert((*c, it.clone(), kind));
        }
    }
    best
}

pub(crate) fn copy_selected_keyframes(app: &mut FilmcraftApp) -> bool {
    let Some((clip, item, _)) = selected_clip(app) else { return false };
    let mut selected = Vec::new();
    for selection in app.ui.effect_keyframes.iter().filter(|selection| selection.clip == clip.0) {
        let Some(effect) = item.effects.iter().find(|effect| effect.effect == selection.effect) else { continue };
        let param = match selection.mask {
            Some(mask) => effect.masks.get(mask).and_then(|mask| mask.param(&selection.param)),
            None => effect.params.get(&selection.param),
        };
        let Some(keyframe) = param.and_then(|param| param.keyframes.iter().find(|key| key.time.0 == selection.media_time)).cloned() else { continue };
        let timeline_time = item.start + Tick(((keyframe.time - item.source_in).0 as f64 / item.speed.abs().max(1e-6)) as i64);
        selected.push((selection.effect.clone(), selection.param.clone(), selection.mask, timeline_time, keyframe));
    }
    let Some(anchor) = selected.iter().map(|entry| entry.3).min() else {
        app.ui.status = "Select keyframes in Effect Controls to copy".into();
        return true;
    };
    app.keyframe_clipboard = selected
        .into_iter()
        .map(|(effect, param, mask, time, keyframe)| KeyframeClipboardEntry { effect, param, mask, offset: (time - anchor).0, keyframe })
        .collect();
    app.ui.status = format!("Copied {} keyframe{}", app.keyframe_clipboard.len(), if app.keyframe_clipboard.len() == 1 { "" } else { "s" });
    true
}

pub(crate) fn paste_keyframes(app: &mut FilmcraftApp) -> bool {
    if app.keyframe_clipboard.is_empty() {
        return false;
    }
    let Some((clip, item, _)) = selected_clip(app) else {
        app.ui.status = "Select a destination clip to paste keyframes".into();
        return true;
    };
    let anchor = app.session.playhead().clamp(item.start, item.end() - Tick(1));
    let mut tracks: Vec<Value> = Vec::new();
    for entry in &app.keyframe_clipboard {
        let timeline_time = anchor + Tick(entry.offset);
        if timeline_time < item.start || timeline_time >= item.end() {
            continue;
        }
        let Some(effect) = item.effects.iter().find(|effect| effect.effect == entry.effect) else { continue };
        let param = match entry.mask {
            Some(mask) => effect.masks.get(mask).and_then(|mask| mask.param(&entry.param)),
            None => effect.params.get(&entry.param),
        };
        let Some(param) = param else { continue };
        if std::mem::discriminant(&entry.keyframe.value) != std::mem::discriminant(&param.value) {
            continue;
        }
        let media_time = item.source_time_at(timeline_time);
        let mut keyframe = entry.keyframe.clone();
        keyframe.time = media_time;
        let serialized = match serde_json::to_value(keyframe) {
            Ok(value) => value,
            Err(error) => {
                app.ui.status = error.to_string();
                return true;
            }
        };
        if let Some(track) = tracks.iter_mut().find(|track| {
            track["effect"] == entry.effect && track["param"] == entry.param && track.get("mask").and_then(Value::as_u64) == entry.mask.map(|mask| mask as u64)
        }) {
            track["keyframes"].as_array_mut().unwrap().push(serialized);
        } else {
            let mut track = json!({"effect": entry.effect, "param": entry.param, "keyframes": [serialized]});
            if let Some(mask) = entry.mask {
                track["mask"] = json!(mask);
            }
            tracks.push(track);
        }
    }
    if tracks.is_empty() {
        app.ui.status = "No matching keyframe parameters on the destination clip".into();
        return true;
    }
    match app.session.execute("effects.pasteKeyframes", json!({"clip": clip.0, "tracks": tracks})) {
        Ok(_) => {
            let count: usize = tracks.iter().filter_map(|track| track["keyframes"].as_array().map(Vec::len)).sum();
            app.ui.status = format!("Pasted {count} keyframe{}", if count == 1 { "" } else { "s" });
        }
        Err(error) => app.ui.status = error.to_string(),
    }
    true
}

/// Delete the selected keyframe when Delete is pressed in Effect Controls. Return true whenever
/// a keyframe selection was handled so the application's general Delete shortcut cannot clear
/// the selected timeline clip instead.
pub(crate) fn delete_selected_keyframe(app: &mut FilmcraftApp) -> bool {
    let selected = std::mem::take(&mut app.ui.effect_keyframes);
    if selected.is_empty() {
        return false;
    }
    let Some((clip, item, _)) = selected_clip(app) else { return true };
    for selected in selected.into_iter().filter(|s| s.clip == clip.0) {
        let exists = item
            .effects
            .iter()
            .find(|e| e.effect == selected.effect)
            .and_then(|e| match selected.mask {
                Some(mask) => e.masks.get(mask).and_then(|m| m.param(&selected.param)),
                None => e.params.get(&selected.param),
            })
            .is_some_and(|p| p.keyframes.iter().any(|k| k.time.0 == selected.media_time));
        if !exists {
            continue;
        }
        let mut params = json!({
            "clip": selected.clip,
            "effect": selected.effect,
            "param": selected.param,
            "mediaTime": selected.media_time,
        });
        if let Some(mask) = selected.mask {
            params["mask"] = json!(mask);
        }
        if let Err(e) = app.session.execute("effects.deleteKeyframe", params) {
            app.ui.status = e.to_string();
        }
    }
    true
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let Some((clip, it, kind)) = selected_clip(app) else {
        crate::dock::placeholder(ui, rect, &t, "(no clip selected)");
        return;
    };
    let seq = app.session.active_sequence().expect("seq").clone();
    let split = rect.min.x + (rect.width() * 0.58).max(260.0).min(rect.width() - 60.0);
    let head = Rect::from_min_size(rect.min + vec2(8.0, 4.0), vec2(split - rect.min.x - 12.0, 24.0));
    // Premiere: two pill tabs — "Source · clip" and "Sequence · clip" (active)
    let pill = |ui: &mut egui::Ui, r: Rect, text: &str, active: bool| {
        ui.painter().rect_filled(r, 4.0, if active { Color32::from_rgb(0x3a, 0x3a, 0x3a) } else { t.panel_bg });
        if !active {
            ui.painter().rect_stroke(r, 4.0, Stroke::new(1.0, t.separator), egui::StrokeKind::Inside);
        }
        let cp = ui.painter().with_clip_rect(r.shrink(2.0));
        cp.text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, text, Tokens::ui(11.5), if active { t.text } else { t.text_dim });
    };
    let half = (head.width() - 6.0) / 2.0;
    pill(ui, Rect::from_min_size(head.min, vec2(half, 24.0)), &format!("Source · {}", it.name), false);
    pill(ui, Rect::from_min_size(head.min + vec2(half + 6.0, 0.0), vec2(half, 24.0)), &format!("{} · {}", seq_name(app), it.name), true);
    // Keyframe lane header: a compact time ruler above the clip strip. Keeping ticks and labels
    // separate from the clip name makes it possible to orient keyframes in time at a glance.
    let lane = Rect::from_min_max(pos2(split + 4.0, rect.min.y + 4.0), pos2(rect.max.x - 6.0, rect.max.y - 26.0));
    ui.painter().rect_filled(lane, 0.0, t.tl_bg);
    let ph = app.session.playhead();
    let dur = it.duration.0.max(1) as f64;
    let lx = |tk: Tick| -> f32 { lane.min.x + (((tk - it.start).0 as f64 / dur) as f32).clamp(0.0, 1.0) * lane.width() };
    let rate = seq.settings.frame_rate;
    let frame_dur = rate.frame_duration().0.max(1);
    let frames = (it.duration.0.max(1) + frame_dur - 1) / frame_dur;
    let ruler_font = Tokens::mono(9.0);
    let label_width = ui.painter().layout_no_wrap("00:00:00:00".into(), ruler_font.clone(), t.tl_ruler_text).size().x;
    let min_label_spacing = label_width + 6.0;
    let target_ticks = (lane.width() / min_label_spacing).max(1.0) as i64;
    let raw_step = (frames / target_ticks).max(1);
    let magnitude = 10_i64.pow((raw_step as f64).log10().floor().max(0.0) as u32);
    let step_frames = [1, 2, 5, 10].into_iter().map(|n| n * magnitude).find(|n| *n >= raw_step).unwrap_or(10 * magnitude);
    let label_spacing = step_frames as f32 / frames.max(1) as f32 * lane.width();
    let show_labels = label_spacing >= min_label_spacing;
    let tick_y = lane.min.y + 16.0;
    let painter = ui.painter().with_clip_rect(lane);
    painter.line_segment([pos2(lane.min.x, tick_y), pos2(lane.max.x, tick_y)], Stroke::new(1.0, t.separator));
    let minor_step = (step_frames / 5).max(1);
    for f in (0..=frames).step_by(minor_step as usize) {
        let x = lane.min.x + (f as f32 / frames.max(1) as f32) * lane.width();
        let major = f % step_frames == 0;
        let height = if major { 6.0 } else { 3.0 };
        painter.line_segment([pos2(x, tick_y - height), pos2(x, tick_y + 2.0)], Stroke::new(1.0, t.tl_ruler_tick));
        // Tick marks are independent of labels: hide a label if this ruler is too narrow, but
        // retain every major and subdivision tick so temporal spacing remains visible.
        if major && show_labels {
            let tc = filmcraft_time::format_time(it.start + Tick(f * frame_dur), rate, seq.settings.drop_frame, filmcraft_time::TimeDisplay::Timecode, 48000);
            if x + label_width + 3.0 <= lane.max.x {
                painter.text(pos2(x + 2.0, lane.min.y + 1.0), Align2::LEFT_TOP, tc, ruler_font.clone(), t.tl_ruler_text);
            }
        }
    }
    let clip_bar = Rect::from_min_max(pos2(lane.min.x, lane.min.y + 19.0), pos2(lane.max.x, lane.min.y + 33.0));
    ui.painter().rect_filled(clip_bar, 2.0, Color32::from_rgb(58, 58, 70));
    ui.painter().text(pos2(lane.min.x + 4.0, clip_bar.center().y), Align2::LEFT_CENTER, &it.name, Tokens::ui(10.0), t.text);
    let scrub = Rect::from_min_max(lane.min, pos2(lane.max.x, lane.min.y + 18.0));
    let sresp = ui.interact(scrub, egui::Id::new(("ec-scrub", clip.0)), Sense::click_and_drag());
    app.auto.add("effectControls.lane", lane, "keyframe lane");
    if (sresp.dragged() || sresp.clicked())
        && let Some(pos) = sresp.interact_pointer_pos()
    {
        let f = ((pos.x - lane.min.x) / lane.width()).clamp(0.0, 1.0) as f64;
        let tk = it.start + Tick((f * it.duration.0 as f64) as i64);
        app.stop();
        app.session.set_playhead(tk);
    }
    // A single marquee spans the whole keyframe column, so it can collect points from
    // different effects and parameters at once.
    let keyframe_area = Rect::from_min_max(pos2(lane.min.x, lane.min.y + 33.0), lane.max);
    let marquee_id = egui::Id::new(("keyframe-marquee", clip.0));
    let marquee = ui.interact(keyframe_area, marquee_id, Sense::click_and_drag());
    let marquee_start: Option<Pos2> = ui.data(|d| d.get_temp(marquee_id));
    if marquee.drag_started() {
        let start = marquee.interact_pointer_pos().unwrap_or(marquee.rect.min);
        ui.data_mut(|d| d.insert_temp(marquee_id, start));
        if !ui.input(|i| i.modifiers.shift) {
            app.ui.effect_keyframes.clear();
        }
    }
    let marquee_rect = marquee_start.and_then(|start| marquee.interact_pointer_pos().map(|pos| Rect::from_two_pos(start, pos)));
    if marquee.drag_stopped() {
        ui.data_mut(|d| d.remove::<Pos2>(marquee_id));
    }
    let body = Rect::from_min_max(pos2(rect.min.x, head.max.y + 4.0), pos2(split, rect.max.y - 26.0));
    let mut actions: Vec<(String, Value)> = Vec::new();
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("ec-body"));
    bui.set_clip_rect(Rect::from_min_max(body.min, pos2(rect.max.x, body.max.y)));
    let mt_now = it.source_time_at(ph.clamp(it.start, it.end() - Tick(1)));
    let heading = if kind == filmcraft_project::TrackKind::Video { "Video" } else { "Audio" };
    bui.painter().text(pos2(body.min.x + 8.0, body.min.y + 8.0), Align2::LEFT_CENTER, heading, Tokens::semibold(11.5), t.text_dim);
    bui.add_space(18.0);
    // Premiere lists the fixed effects (Motion, Opacity, Time Remapping / Volume…) first.
    let mut order: Vec<usize> = (0..it.effects.len()).collect();
    order.sort_by_key(|i| !it.effects[*i].def().is_some_and(|d| d.intrinsic));
    let scroll_out = egui::ScrollArea::vertical().id_salt("ec-scroll").auto_shrink([false, false]).show(&mut bui, |bui| {
        for idx in order {
            let e = &it.effects[idx];
            let Some(def) = e.def() else { continue };
            let key = format!("{}:{}", clip.0, idx);
            let open = !app.ui.collapsed_fx.contains(&key);
            let (r, resp) = bui.allocate_exact_size(vec2(body.width(), ROW_H), Sense::click());
            bui.painter().rect_filled(r, 0.0, t.panel_bg);
            if resp.hovered() {
                bui.painter().rect_filled(r, 0.0, t.hover);
            }
            bui.painter().line_segment([r.left_top(), pos2(lane.max.x, r.min.y)], Stroke::new(1.0, t.separator));
            bui.painter().line_segment([r.left_bottom(), pos2(lane.max.x, r.max.y)], Stroke::new(1.0, t.separator));
            icons::paint(
                bui.painter(),
                Rect::from_center_size(pos2(r.min.x + 10.0, r.center().y), vec2(10.0, 10.0)),
                if open { Icon::ChevronDown } else { Icon::ChevronRight },
                t.text_dim,
            );
            // fx enable toggle
            let fxr = Rect::from_center_size(pos2(r.min.x + 28.0, r.center().y), vec2(18.0, 14.0));
            let fxresp = bui.interact(fxr, egui::Id::new(("fxen", clip.0, idx)), Sense::click());
            bui.painter().text(fxr.center(), Align2::CENTER_CENTER, "fx", Tokens::semibold(10.5), if e.enabled { t.text } else { t.text_faint });
            if !e.enabled {
                bui.painter().line_segment([fxr.left_bottom(), fxr.right_top()], Stroke::new(1.0, t.text_faint));
            }
            if fxresp.clicked() {
                actions.push(("effects.toggleEnabled".into(), json!({"clip": clip.0, "index": idx})));
            }
            bui.painter().text(pos2(r.min.x + 42.0, r.center().y), Align2::LEFT_CENTER, def.name, Tokens::ui(12.0), t.text);
            // reset button
            let rr = Rect::from_center_size(pos2(r.max.x - 14.0, r.center().y), vec2(16.0, 16.0));
            let rresp = bui.interact(rr, egui::Id::new(("fxreset", clip.0, idx)), Sense::click()).on_hover_text("Reset Effect");
            icons::paint(bui.painter(), rr.shrink(2.0), Icon::Reset, if rresp.hovered() { t.text } else { t.text_dim });
            if rresp.clicked() {
                actions.push(("effects.reset".into(), json!({"clip": clip.0, "index": idx})));
            }
            app.auto.add(&format!("effectControls.effect.{}", e.effect), r, def.name);
            if resp.clicked() && !fxresp.clicked() && !rresp.clicked() {
                if open {
                    app.ui.collapsed_fx.push(key.clone());
                } else {
                    app.ui.collapsed_fx.retain(|k| *k != key);
                }
            }
            let mut save_preset = false;
            let fx_id = e.effect.clone();
            resp.context_menu(|ui| {
                let sp = ui.button("Save Preset…");
                app.auto.add(&format!("effectControls.effect.{fx_id}.savePreset"), sp.rect, "Save Preset…");
                if sp.clicked() {
                    save_preset = true;
                    ui.close();
                }
                if !def.intrinsic && ui.button("Clear").clicked() {
                    actions.push(("effects.remove".into(), json!({"clip": clip.0, "index": idx})));
                    ui.close();
                }
            });
            if save_preset {
                crate::panels::presets::open_save(app, clip.0, vec![idx], def.name);
            }
            if !open {
                continue;
            }
            if crate::panels::audio_fx_editor::has_editor(&e.effect) {
                custom_setup_row(app, bui, body, clip, idx, &e.effect);
            }
            for pd in &def.params {
                param_row(app, bui, body, clip, idx, e, None, pd, mt_now, &mut actions, &lane, &lx, &it, marquee_rect);
                if app.ui.expanded_fx.contains(&graph_key(clip, idx, pd.id))
                    && let Some(param) = e.params.get(pd.id)
                    && param.is_animated()
                    && matches!(param.value, ParamValue::Float(_))
                {
                    graph_rows(app, bui, body, clip, idx, None, pd, param, &lane, &it, &mut actions);
                }
            }
            if crate::panels::masks::maskable(e) {
                crate::panels::masks::effect_rows(app, bui, body, clip, idx, e, mt_now, &mut actions, &lane, &lx, &it, marquee_rect);
            }
        }
    });
    let _ = scroll_out;
    if let Some(selection_rect) = marquee_rect.filter(|_| marquee.dragged()) {
        ui.painter().rect_filled(selection_rect, 1.0, Color32::from_rgba_unmultiplied(70, 140, 255, 45));
        ui.painter().rect_stroke(selection_rect, 1.0, Stroke::new(1.0, t.hot_text), egui::StrokeKind::Inside);
    }
    // playhead in lane
    let px = lx(ph);
    ui.painter().line_segment([pos2(px, lane.min.y), pos2(px, lane.max.y)], Stroke::new(1.0, t.playhead));
    // footer timecode
    let tc = filmcraft_time::format_time(ph, seq.settings.frame_rate, seq.settings.drop_frame, filmcraft_time::TimeDisplay::Timecode, 48000);
    ui.painter().text(pos2(rect.min.x + 10.0, rect.max.y - 13.0), Align2::LEFT_CENTER, tc, Tokens::mono(13.0), t.timecode);
    for (cmd, p) in actions {
        if let Err(e) = app.session.execute(&cmd, p) {
            app.ui.status = e.to_string();
        }
    }
}

/// Premiere's "Custom Setup ▸ Edit…" row: opens the effect's Clip Fx Editor window.
fn custom_setup_row(app: &mut FilmcraftApp, ui: &mut egui::Ui, body: Rect, clip: ClipId, idx: usize, effect: &str) {
    let t = app.tokens;
    let (r, _) = ui.allocate_exact_size(vec2(body.width(), ROW_H), Sense::hover());
    ui.painter().text(pos2(r.min.x + 42.0, r.center().y), Align2::LEFT_CENTER, "Custom Setup", Tokens::ui(12.0), t.text_dim);
    let br = Rect::from_min_size(pos2(r.min.x + 160.0, r.min.y + 2.0), vec2(60.0, ROW_H - 4.0));
    let resp = ui.interact(br, egui::Id::new(("fx-custom-setup", clip.0, idx)), Sense::click());
    ui.painter().rect_filled(br, 3.0, if resp.hovered() { t.hover } else { t.field_bg });
    ui.painter().rect_stroke(br, 3.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
    ui.painter().text(br.center(), Align2::CENTER_CENTER, "Edit…", Tokens::ui(11.5), t.text);
    app.auto.add(&format!("effectControls.effect.{effect}.edit"), br, "Edit…");
    if resp.clicked() {
        crate::panels::audio_fx_editor::open(app, crate::panels::audio_fx_editor::FxTarget::Clip { clip: clip.0, index: idx });
    }
}

fn seq_name(app: &FilmcraftApp) -> String {
    app.session.state.active_sequence.and_then(|s| app.session.project.item(s)).map(|i| i.name.clone()).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn param_row(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    body: Rect,
    clip: ClipId,
    idx: usize,
    e: &EffectInstance,
    mask: Option<usize>,
    pd: &filmcraft_project::ParamDef,
    mt: Tick,
    actions: &mut Vec<(String, Value)>,
    lane: &Rect,
    lx: &dyn Fn(Tick) -> f32,
    it: &TrackItem,
    marquee_rect: Option<Rect>,
) {
    let _ = lx;
    let t = app.tokens;
    let param = match mask {
        Some(k) => e.masks.get(k).and_then(|m| m.param(pd.id)),
        None => e.params.get(pd.id),
    };
    let Some(param) = param else { return };
    // mask parameters: automation ids / widget ids get a `m<k>.` prefix, commands a `mask` field
    let pkey: String = match mask {
        Some(k) => format!("mask{k}.{}", pd.id),
        None => pd.id.to_string(),
    };
    let pkey = pkey.as_str();
    let with_mask = |mut v: Value| -> Value {
        if let Some(k) = mask {
            v["mask"] = json!(k);
        }
        v
    };
    let (r, _) = ui.allocate_exact_size(vec2(body.width(), ROW_H), Sense::hover());
    // Extend each parameter's row guide through the keyframe lane, like the edit rows in a
    // conventional effect-controls timeline, so a diamond is visually tied to its parameter.
    ui.painter().line_segment([r.left_bottom(), pos2(lane.max.x, r.max.y)], Stroke::new(1.0, t.separator));
    let mut x = r.min.x + 26.0;
    // twirl-down for the value/velocity graphs (animated scalar params)
    if param.is_animated() && matches!(param.value, ParamValue::Float(_)) {
        let key = graph_key(clip, idx, pkey);
        let open = app.ui.expanded_fx.contains(&key);
        let tw = Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(12.0, 12.0));
        let tresp = ui.interact(tw.expand(2.0), egui::Id::new(("twirl", clip.0, idx, pkey)), Sense::click()).on_hover_text("Show graphs");
        icons::paint(ui.painter(), tw, if open { Icon::ChevronDown } else { Icon::ChevronRight }, t.text_dim);
        app.auto.add(&format!("effectControls.{}.{}.graphs", e.effect, pkey), tw, "Show graphs");
        if tresp.clicked() {
            if open {
                app.ui.expanded_fx.retain(|k| *k != key);
            } else {
                app.ui.expanded_fx.push(key);
            }
        }
    }
    if pd.animatable {
        let sw = Rect::from_center_size(pos2(x, r.center().y), vec2(14.0, 14.0));
        let resp = ui.interact(sw, egui::Id::new(("sw", clip.0, idx, pkey)), Sense::click()).on_hover_text("Toggle animation");
        icons::paint(ui.painter(), sw, Icon::Stopwatch, if param.is_animated() { t.accent } else { t.text_dim });
        if resp.clicked() {
            actions.push(("effects.toggleAnimation".into(), with_mask(json!({"clip": clip.0, "effect": idx, "param": pd.id}))));
        }
        app.auto.add(&format!("effectControls.{}.{}.stopwatch", e.effect, pkey), sw, "Toggle animation");
    }
    x += 14.0;
    ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, pd.label, Tokens::ui(12.0), t.text);
    let vx = r.min.x + (r.width() * 0.5).max(150.0);
    let value = param.value_at(mt);
    let mut vui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(vx, r.min.y + 1.0), pos2(r.max.x - 26.0, r.max.y - 1.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let id = egui::Id::new(("pv", clip.0, idx, pkey));
    let mut set: Option<Value> = None;
    match (&pd.kind, &value) {
        (ParamKind::Float { min, max, soft_min, soft_max, unit, decimals }, ParamValue::Float(v)) => {
            let speed = ((soft_max - soft_min) / 400.0).max(0.01);
            let (_, nv) = crate::widgets::hot_number(&mut vui, id, *v, speed, (*min, *max), *decimals as usize, unit, &t);
            if let Some(nv) = nv {
                set = Some(json!(nv));
            }
        }
        (ParamKind::Angle, ParamValue::Float(v)) => {
            let (_, nv) = crate::widgets::hot_number(&mut vui, id, *v, 0.5, (-36000.0, 36000.0), 1, "°", &t);
            if let Some(nv) = nv {
                set = Some(json!(nv));
            }
        }
        (ParamKind::Point, ParamValue::Vec2(p)) => {
            let (_, nx) = crate::widgets::hot_number(&mut vui, id.with("x"), p.x, 1.0, (-100_000.0, 100_000.0), 1, "", &t);
            let (_, ny) = crate::widgets::hot_number(&mut vui, id.with("y"), p.y, 1.0, (-100_000.0, 100_000.0), 1, "", &t);
            if nx.is_some() || ny.is_some() {
                set = Some(json!([nx.unwrap_or(p.x), ny.unwrap_or(p.y)]));
            }
        }
        (ParamKind::Bool, ParamValue::Bool(b)) => {
            let mut v = *b;
            if vui.checkbox(&mut v, "").changed() {
                set = Some(json!(v));
            }
        }
        (ParamKind::Choice(opts), ParamValue::Choice(c)) => {
            let mut sel = *c as usize;
            egui::ComboBox::from_id_salt(id).selected_text(opts.get(sel).copied().unwrap_or("")).width(130.0).show_ui(&mut vui, |ui| {
                for (i, o) in opts.iter().enumerate() {
                    if ui.selectable_value(&mut sel, i, *o).changed() {
                        set = Some(json!(i));
                    }
                }
            });
        }
        (ParamKind::Color, ParamValue::Color(c)) => {
            let mut rgba = egui::Rgba::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
            if egui::color_picker::color_edit_button_rgba(&mut vui, &mut rgba, egui::color_picker::Alpha::Opaque).changed() {
                set = Some(json!([rgba.r(), rgba.g(), rgba.b(), rgba.a()]));
            }
        }
        (ParamKind::Path, _) => {
            crate::panels::masks::path_value(app, &mut vui, clip, idx, mask, actions);
        }
        (ParamKind::Text, ParamValue::Text(s)) if !pd.id.ends_with("_lut") => {
            // edited in a buffer; committed (one undo step) when the field loses focus
            let mut buf = vui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| s.clone());
            let r = vui.add(egui::TextEdit::singleline(&mut buf).desired_width(160.0));
            app.auto.add(&format!("effectControls.text.{}.{}", idx, pd.id), r.rect, pd.label);
            if r.lost_focus() {
                if buf != *s {
                    set = Some(json!(buf));
                }
                vui.data_mut(|d| d.remove::<String>(id));
            } else if r.has_focus() {
                vui.data_mut(|d| d.insert_temp(id, buf));
            } else {
                vui.data_mut(|d| d.remove::<String>(id));
            }
        }
        _ => {
            vui.label(param_text(&app.session.project, pd.id, &value));
        }
    }
    if let Some(v) = set {
        actions.push(("effects.setParam".into(), with_mask(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": v}))));
    }
    // keyframe navigator ◀ ◆ ▶ (when animated)
    let eff_json = json!(idx);
    if pd.animatable && param.is_animated() {
        let nx = r.max.x - 58.0;
        let cy = r.center().y;
        let at_key = param.keyframes.iter().any(|k| k.time == mt);
        let prev = param.prev_keyframe(mt);
        let next = param.next_keyframe(mt);
        let to_tl = |k: Tick| it.start + Tick(((k - it.source_in).0 as f64 / it.speed.abs().max(1e-6)) as i64);
        let pr = Rect::from_center_size(pos2(nx, cy), vec2(12.0, 14.0));
        let kr = Rect::from_center_size(pos2(nx + 16.0, cy), vec2(12.0, 12.0));
        let nr = Rect::from_center_size(pos2(nx + 32.0, cy), vec2(12.0, 14.0));
        let arrow = |p: &egui::Painter, r: Rect, left: bool, on: bool| {
            let c = r.center();
            let pts = if left {
                vec![c + vec2(3.0, -4.0), c + vec2(3.0, 4.0), c + vec2(-3.0, 0.0)]
            } else {
                vec![c + vec2(-3.0, -4.0), c + vec2(-3.0, 4.0), c + vec2(3.0, 0.0)]
            };
            p.add(egui::Shape::convex_polygon(pts, if on { t.text } else { t.text_faint }, Stroke::NONE));
        };
        arrow(ui.painter(), pr, true, prev.is_some());
        arrow(ui.painter(), nr, false, next.is_some());
        icons::paint(ui.painter(), kr, Icon::Keyframe, if at_key { t.hot_text } else { t.text_dim });
        if ui.interact(pr, egui::Id::new(("kprev", clip.0, idx, pkey)), Sense::click()).clicked()
            && let Some(k) = prev
        {
            actions.push(("playhead.set".into(), json!({"time": to_tl(k).0})));
        }
        if ui.interact(nr, egui::Id::new(("knext", clip.0, idx, pkey)), Sense::click()).clicked()
            && let Some(k) = next
        {
            actions.push(("playhead.set".into(), json!({"time": to_tl(k).0})));
        }
        if ui.interact(kr, egui::Id::new(("kadd", clip.0, idx, pkey)), Sense::click()).on_hover_text("Add/Remove Keyframe").clicked() {
            actions.push(("effects.addKeyframe".into(), with_mask(json!({"clip": clip.0, "effect": eff_json, "param": pd.id}))));
        }
        app.auto.add(&format!("effectControls.{}.{}.addKeyframe", e.effect, pkey), kr, "Add/Remove Keyframe");
    }
    // keyframes in the lane: draggable diamonds; right-click for interpolation
    if param.is_animated() {
        let y = r.center().y;
        let dur = it.duration.0.max(1) as f64;
        let rate = app.session.sequence_rate();
        if let Some(selection_rect) = marquee_rect {
            for k in &param.keyframes {
                let tl = it.start + Tick(((k.time - it.source_in).0 as f64 / it.speed.abs().max(1e-6)) as i64);
                let f = ((tl - it.start).0 as f64 / dur) as f32;
                let point = pos2(lane.min.x + f * lane.width(), y);
                if selection_rect.expand(3.0).contains(point) {
                    let item =
                        crate::state::EffectKeyframeSelection { clip: clip.0, effect: e.effect.clone(), param: pd.id.to_string(), mask, media_time: k.time.0 };
                    if !app.ui.effect_keyframes.contains(&item) {
                        app.ui.effect_keyframes.push(item);
                    }
                }
            }
        }
        for k in &param.keyframes {
            let tl = it.start + Tick(((k.time - it.source_in).0 as f64 / it.speed.abs().max(1e-6)) as i64);
            let f = ((tl - it.start).0 as f64 / dur) as f32;
            if !(-0.01..=1.01).contains(&f) {
                continue;
            }
            let id = egui::Id::new(("kf", clip.0, idx, pkey, k.time.0));
            let drag_off: Option<f32> = ui.data(|d| d.get_temp(id));
            let kx = lane.min.x + f * lane.width() + drag_off.unwrap_or(0.0);
            let kr = Rect::from_center_size(pos2(kx, y), vec2(11.0, 11.0));
            let resp = ui
                .interact(kr.expand(2.0), id, Sense::click_and_drag())
                .on_hover_text("Select keyframe · drag on the lane to select multiple · press Delete to remove");
            app.auto.add(&format!("effectControls.{}.{}.keyframe.{}", e.effect, pkey, k.time.0), kr, "keyframe");
            let sel = app
                .ui
                .effect_keyframes
                .iter()
                .any(|s| s.clip == clip.0 && s.effect == e.effect && s.param == pd.id && s.mask == mask && s.media_time == k.time.0)
                || resp.dragged();
            let col = if sel { t.hot_text } else { Color32::from_rgb(0xb0, 0xb0, 0xb0) };
            match k.interp {
                filmcraft_project::Interpolation::Hold => {
                    ui.painter().rect_filled(Rect::from_center_size(kr.center(), vec2(8.0, 8.0)), 0.0, col);
                }
                filmcraft_project::Interpolation::Linear => icons::paint(ui.painter(), kr, Icon::Keyframe, col),
                _ => {
                    ui.painter().circle_filled(kr.center(), 4.5, col);
                }
            }
            if resp.dragged() {
                let off = drag_off.unwrap_or(0.0) + resp.drag_delta().x;
                ui.data_mut(|d| d.insert_temp(id, off));
            }
            if resp.drag_stopped() {
                let off = drag_off.unwrap_or(0.0);
                ui.data_mut(|d| d.remove::<f32>(id));
                let new_tl =
                    rate.snap_nearest(it.start + Tick(((f + off / lane.width()) as f64 * dur) as i64)).clamp(it.start, it.end() - rate.frame_duration());
                let new_media = it.source_in + Tick(((new_tl - it.start).0 as f64 * it.speed.abs()) as i64);
                if new_media != k.time {
                    actions.push((
                        "effects.moveKeyframe".into(),
                        with_mask(json!({"clip": clip.0, "effect": eff_json, "param": pd.id, "mediaTime": k.time.0, "to": new_media.0})),
                    ));
                }
            }
            if resp.clicked() {
                app.ui.effect_keyframes.clear();
                app.ui.effect_keyframes.push(crate::state::EffectKeyframeSelection {
                    clip: clip.0,
                    effect: e.effect.clone(),
                    param: pd.id.to_string(),
                    mask,
                    media_time: k.time.0,
                });
                actions.push(("playhead.set".into(), json!({"time": tl.0})));
            }
            resp.context_menu(|ui| {
                ui.label(egui::RichText::new("Temporal Interpolation").color(t.text_dim));
                for (label, key) in [
                    ("Linear", "linear"),
                    ("Bezier", "bezier"),
                    ("Auto Bezier", "autoBezier"),
                    ("Continuous Bezier", "continuousBezier"),
                    ("Hold", "hold"),
                    ("Ease In", "easeIn"),
                    ("Ease Out", "easeOut"),
                ] {
                    if ui.button(label).clicked() {
                        actions.push((
                            "effects.setInterpolation".into(),
                            with_mask(json!({"clip": clip.0, "effect": eff_json, "param": pd.id, "mediaTime": k.time.0, "interpolation": key})),
                        ));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Delete Keyframe").clicked() {
                    actions
                        .push(("effects.deleteKeyframe".into(), with_mask(json!({"clip": clip.0, "effect": eff_json, "param": pd.id, "mediaTime": k.time.0}))));
                    ui.close();
                }
            });
        }
    }
}

/// Lumetri Color panel: edits (or adds) the Lumetri effect on the selected clip, grouped by section.
pub fn lumetri_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let Some((clip, it, _)) = selected_clip(app) else {
        crate::dock::placeholder(ui, rect, &t, "Select a clip to grade");
        return;
    };
    let idx = it.effects.iter().position(|e| e.effect == "lumetri");
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(6.0)).id_salt("lumetri"));
    bui.label(egui::RichText::new(format!("Master · {}", it.name)).color(t.text_dim));
    let Some(idx) = idx else {
        if bui.button("Add Lumetri Color to clip").clicked() {
            let _ = app.session.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"}));
        }
        return;
    };
    let e = it.effects[idx].clone();
    let def = e.def().expect("lumetri def");
    let ph = app.session.playhead();
    let mt = it.source_time_at(ph.clamp(it.start, it.end() - Tick(1)));
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut bui, |ui| {
        let mut groups: Vec<&str> = Vec::new();
        for p in &def.params {
            if let Some(g) = p.group
                && !groups.contains(&g)
            {
                groups.push(g);
            }
        }
        for g in groups {
            let key = format!("lumetri:{g}");
            let open = !app.ui.collapsed_fx.contains(&key);
            let (resp, now_open) = crate::widgets::section_header(ui, egui::Id::new(&key), g, open, &t, true);
            app.auto.add(&format!("lumetri.section.{g}"), resp.rect, g);
            if now_open != open {
                if now_open {
                    app.ui.collapsed_fx.retain(|k| *k != key);
                } else {
                    app.ui.collapsed_fx.push(key.clone());
                }
            }
            if !now_open {
                continue;
            }
            for pd in def.params.iter().filter(|p| p.group == Some(g)) {
                let v = e.params.get(pd.id).map(|p| p.value_at(mt)).unwrap_or(pd.default.clone());
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    ui.add_sized(vec2(110.0, 18.0), egui::Label::new(egui::RichText::new(pd.label).size(12.0)));
                    if let (ParamKind::Float { min, max, soft_min, soft_max, .. }, ParamValue::Float(x)) = (&pd.kind, &v) {
                        let mut val = *x;
                        let s = ui.add(egui::Slider::new(&mut val, *soft_min..=*soft_max).show_value(false));
                        let (_, nv) =
                            crate::widgets::hot_number(ui, egui::Id::new(("lum", pd.id)), val, (soft_max - soft_min) / 300.0, (*min, *max), 1, "", &t);
                        if s.changed() {
                            actions.push(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": val}));
                        } else if let Some(nv) = nv {
                            actions.push(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": nv}));
                        }
                        if s.double_clicked() {
                            actions.push(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": pd.default.as_f64().unwrap_or(0.0)}));
                        }
                    } else if let ParamValue::Color(c) = v {
                        let mut rgba = egui::Rgba::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
                        if egui::color_picker::color_edit_button_rgba(ui, &mut rgba, egui::color_picker::Alpha::Opaque).changed() {
                            actions.push(json!({"clip": clip.0, "effect": idx, "param": pd.id, "value": [rgba.r(), rgba.g(), rgba.b(), 1.0]}));
                        }
                    }
                });
            }
        }
    });
    for a in actions {
        if let Err(e) = app.session.execute("effects.setParam", a) {
            app.ui.status = e.to_string();
        }
    }
}

/// Properties panel (Premiere 26): a compact inspector for the selected clip.
pub fn properties_panel(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let Some((clip, it, kind)) = selected_clip(app) else {
        crate::dock::placeholder(ui, rect, &t, "Select a clip to see its properties");
        return;
    };
    let ph = app.session.playhead();
    let mt = it.source_time_at(ph.clamp(it.start, it.end() - Tick(1)));
    let mut actions: Vec<(String, Value)> = Vec::new();
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(14.0, 8.0))).id_salt("props"));
    // header: clip name + menu
    let (hr, _) = bui.allocate_exact_size(vec2(bui.available_width(), 30.0), Sense::hover());
    let sw = Rect::from_center_size(pos2(hr.min.x + 8.0, hr.center().y), vec2(12.0, 12.0));
    let lc = app.session.prefs.labels.rgb(it.label);
    bui.painter().rect_filled(sw, 2.0, Color32::from_rgb(lc[0], lc[1], lc[2]));
    bui.painter().text(pos2(hr.min.x + 22.0, hr.center().y), Align2::LEFT_CENTER, &it.name, Tokens::semibold(12.5), t.text);
    let row = |ui: &mut egui::Ui, label: &str| -> Rect {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        ui.painter().text(pos2(r.min.x + 18.0, r.center().y), Align2::LEFT_CENTER, label, Tokens::ui(12.0), t.text_dim);
        r
    };
    let section = |ui: &mut egui::Ui, app: &mut FilmcraftApp, name: &str, reset: Option<(usize, u64)>, actions: &mut Vec<(String, Value)>| -> bool {
        ui.add_space(4.0);
        let key = format!("props:{name}");
        let open = !app.ui.collapsed_fx.contains(&key);
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        ui.painter().line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, t.separator));
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(r.min.x + 6.0, r.center().y), vec2(10.0, 10.0)),
            if open { Icon::ChevronDown } else { Icon::ChevronRight },
            t.text_dim,
        );
        ui.painter().text(pos2(r.min.x + 18.0, r.center().y), Align2::LEFT_CENTER, name, Tokens::semibold(13.0), t.text);
        if let Some((idx, c)) = reset {
            let rr = Rect::from_center_size(pos2(r.max.x - 10.0, r.center().y), vec2(14.0, 14.0));
            icons::paint(ui.painter(), rr, Icon::Reset, t.text_dim);
            if ui.interact(rr, egui::Id::new(("props-reset", name, c)), Sense::click()).clicked() {
                actions.push(("effects.reset".into(), json!({"clip": c, "index": idx})));
            }
        }
        if resp.clicked() {
            if open {
                app.ui.collapsed_fx.push(key);
            } else {
                app.ui.collapsed_fx.retain(|k| *k != key);
            }
        }
        open
    };
    let diamond = |ui: &mut egui::Ui, r: Rect, animated: bool, id: egui::Id| -> bool {
        let dr = Rect::from_center_size(pos2(r.max.x - 10.0, r.center().y), vec2(10.0, 10.0));
        let resp = ui.interact(dr.expand(3.0), id, Sense::click());
        if animated {
            icons::paint(ui.painter(), dr, Icon::Keyframe, t.hot_text);
        } else {
            let c = dr.center();
            let s = 4.5;
            ui.painter()
                .add(egui::Shape::closed_line(vec![c + vec2(0.0, -s), c + vec2(s, 0.0), c + vec2(0.0, s), c + vec2(-s, 0.0)], Stroke::new(1.2, t.text_dim)));
        }
        resp.clicked()
    };
    let eff_idx = |id: &str| it.effects.iter().position(|e| e.effect == id);
    let val = |eid: &str, p: &str| it.effect(eid).and_then(|e| e.param(p)).map(|p| p.value_at(mt));
    let anim = |eid: &str, p: &str| it.effect(eid).and_then(|e| e.param(p)).is_some_and(|p| p.is_animated());
    if kind == filmcraft_project::TrackKind::Video {
        if section(&mut bui, app, "Transform", eff_idx("motion").map(|i| (i, clip.0)), &mut actions) {
            for (label, p, unit, speed, range) in [
                ("Position", "position", "", 1.0, (-100_000.0, 100_000.0)),
                ("Anchor point", "anchor", "", 1.0, (-100_000.0, 100_000.0)),
                ("Scale", "scale", " %", 0.5, (0.0, 10000.0)),
                ("Rotation", "rotation", " °", 0.5, (-36000.0, 36000.0)),
            ] {
                let r = row(&mut bui, label);
                let mut vui = bui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(Rect::from_min_max(pos2(r.min.x + 150.0, r.min.y + 4.0), pos2(r.max.x - 24.0, r.max.y - 4.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                match val("motion", p) {
                    Some(ParamValue::Vec2(v)) => {
                        let (_, nx) = crate::widgets::hot_number(&mut vui, egui::Id::new(("pp", p, "x", clip.0)), v.x, speed, range, 0, " X", &t);
                        vui.add_space(10.0);
                        let (_, ny) = crate::widgets::hot_number(&mut vui, egui::Id::new(("pp", p, "y", clip.0)), v.y, speed, range, 0, " Y", &t);
                        if nx.is_some() || ny.is_some() {
                            actions.push((
                                "effects.setParam".into(),
                                json!({"clip": clip.0, "effect": "motion", "param": p, "value": [nx.unwrap_or(v.x), ny.unwrap_or(v.y)]}),
                            ));
                        }
                    }
                    Some(ParamValue::Float(v)) => {
                        let (_, nv) =
                            crate::widgets::hot_number(&mut vui, egui::Id::new(("pp", p, clip.0)), v, speed, range, if p == "scale" { 0 } else { 1 }, unit, &t);
                        if let Some(nv) = nv {
                            actions.push(("effects.setParam".into(), json!({"clip": clip.0, "effect": "motion", "param": p, "value": nv})));
                        }
                    }
                    _ => {}
                }
                if diamond(&mut bui, r, anim("motion", p), egui::Id::new(("pd", p, clip.0))) {
                    actions.push(("effects.toggleAnimation".into(), json!({"clip": clip.0, "effect": "motion", "param": p})));
                }
            }
            let r = row(&mut bui, "Opacity");
            let mut vui = bui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(pos2(r.min.x + 150.0, r.min.y + 4.0), pos2(r.max.x - 24.0, r.max.y - 4.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            if let Some(ParamValue::Float(v)) = val("opacity", "opacity") {
                let (_, nv) = crate::widgets::hot_number(&mut vui, egui::Id::new(("pp-op", clip.0)), v, 0.5, (0.0, 100.0), 0, " %", &t);
                if let Some(nv) = nv {
                    actions.push(("effects.setParam".into(), json!({"clip": clip.0, "effect": "opacity", "param": "opacity", "value": nv})));
                }
            }
            if diamond(&mut bui, r, anim("opacity", "opacity"), egui::Id::new(("pd-op", clip.0))) {
                actions.push(("effects.toggleAnimation".into(), json!({"clip": clip.0, "effect": "opacity", "param": "opacity"})));
            }
        }
        let crop = eff_idx("crop");
        if section(&mut bui, app, "Crop", crop.map(|i| (i, clip.0)), &mut actions) {
            for (label, p) in [("Left", "left"), ("Top", "top"), ("Right", "right"), ("Bottom", "bottom")] {
                let r = row(&mut bui, label);
                let mut vui = bui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(Rect::from_min_max(pos2(r.min.x + 150.0, r.min.y + 4.0), pos2(r.max.x - 24.0, r.max.y - 4.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let v = val("crop", p).and_then(|v| v.as_f64()).unwrap_or(0.0);
                let (_, nv) = crate::widgets::hot_number(&mut vui, egui::Id::new(("pp-crop", p, clip.0)), v, 0.2, (0.0, 100.0), 1, " %", &t);
                if let Some(nv) = nv {
                    if crop.is_none() {
                        actions.push(("effects.apply".into(), json!({"clips": [clip.0], "effect": "crop"})));
                    }
                    actions.push(("effects.setParam".into(), json!({"clip": clip.0, "effect": "crop", "param": p, "value": nv})));
                }
                diamond(&mut bui, r, anim("crop", p), egui::Id::new(("pd-crop", p, clip.0)));
            }
        }
    } else if section(&mut bui, app, "Audio", eff_idx("volume").map(|i| (i, clip.0)), &mut actions) {
        let r = row(&mut bui, "Level");
        let mut vui = bui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(pos2(r.min.x + 150.0, r.min.y + 4.0), pos2(r.max.x - 24.0, r.max.y - 4.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let v = val("volume", "level").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let (_, nv) = crate::widgets::hot_number(&mut vui, egui::Id::new(("pp-vol", clip.0)), v, 0.1, (-96.0, 15.0), 1, " dB", &t);
        if let Some(nv) = nv {
            actions.push(("effects.setParam".into(), json!({"clip": clip.0, "effect": "volume", "param": "level", "value": nv})));
        }
        if diamond(&mut bui, r, anim("volume", "level"), egui::Id::new(("pd-vol", clip.0))) {
            actions.push(("effects.toggleAnimation".into(), json!({"clip": clip.0, "effect": "volume", "param": "level"})));
        }
        let r = row(&mut bui, "Pan");
        let mut vui = bui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(pos2(r.min.x + 150.0, r.min.y + 4.0), pos2(r.max.x - 24.0, r.max.y - 4.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let v = val("panner", "balance").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let (_, nv) = crate::widgets::hot_number(&mut vui, egui::Id::new(("pp-pan", clip.0)), v, 0.5, (-100.0, 100.0), 0, "", &t);
        if let Some(nv) = nv {
            actions.push(("effects.setParam".into(), json!({"clip": clip.0, "effect": "panner", "param": "balance", "value": nv})));
        }
    }
    // speed footer
    bui.add_space(12.0);
    let (br, bresp) = bui.allocate_exact_size(vec2(118.0, 26.0), Sense::click());
    bui.painter().rect_filled(br, 4.0, if bresp.hovered() { t.hover } else { t.panel_bg });
    bui.painter().rect_stroke(br, 4.0, Stroke::new(1.0, t.separator), egui::StrokeKind::Inside);
    bui.painter().text(br.center(), Align2::CENTER_CENTER, format!("Speed {:.0}%", it.speed * 100.0), Tokens::ui(12.0), t.text);
    for (cmd, p) in actions {
        if let Err(e) = app.session.execute(&cmd, p) {
            app.ui.status = e.to_string();
        }
    }
}

pub(crate) fn graph_key(clip: ClipId, idx: usize, pid: &str) -> String {
    format!("graph:{}:{}:{}", clip.0, idx, pid)
}

/// Value and velocity graphs of an animated scalar parameter, drawn across the keyframe lane.
/// Keyframes drag vertically (value); Bezier influence handles drag horizontally.
#[allow(clippy::too_many_arguments)]
pub(crate) fn graph_rows(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    body: Rect,
    clip: ClipId,
    idx: usize,
    mask: Option<usize>,
    pd: &filmcraft_project::ParamDef,
    param: &filmcraft_project::Param,
    lane: &Rect,
    it: &TrackItem,
    actions: &mut Vec<(String, Value)>,
) {
    let with_mask = |mut v: Value| -> Value {
        if let Some(k) = mask {
            v["mask"] = json!(k);
        }
        v
    };
    let t = app.tokens;
    let (vr, _) = ui.allocate_exact_size(vec2(body.width(), 110.0), Sense::hover());
    let (velr, _) = ui.allocate_exact_size(vec2(body.width(), 64.0), Sense::hover());
    let speed = it.speed.abs().max(1e-6);
    let dur = it.duration.0.max(1) as f64;
    let to_media = |f: f64| it.source_in + Tick((f * dur * speed) as i64);
    let to_f = |m: Tick| ((m - it.source_in).0 as f64 / speed / dur) as f32;
    let x_of = |f: f32| lane.min.x + f * lane.width();
    // samples
    let n = (lane.width() / 2.0).max(8.0) as usize;
    let vals: Vec<f64> = (0..=n).map(|i| param.f64_at(to_media(i as f64 / n as f64))).collect();
    let (mut lo, mut hi) = vals.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    for k in &param.keyframes {
        if let ParamValue::Float(v) = k.value {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if hi - lo < 1e-6 {
        lo -= 1.0;
        hi += 1.0;
    }
    let pad = (hi - lo) * 0.12;
    let (mut lo, mut hi) = (lo - pad, hi + pad);
    if let ParamKind::Float { min, max, .. } = pd.kind {
        lo = lo.max(min);
        hi = hi.min(max);
    }
    let area = Rect::from_min_max(pos2(lane.min.x, vr.min.y + 4.0), pos2(lane.max.x, vr.max.y - 4.0));
    let y_of = |v: f64| area.max.y - ((v - lo) / (hi - lo)) as f32 * area.height();
    let v_of = |y: f32| lo + ((area.max.y - y) / area.height()) as f64 * (hi - lo);
    let p = ui.painter();
    p.rect_filled(area, 0.0, Color32::from_rgb(0x19, 0x19, 0x19));
    for g in 1..4 {
        let y = area.min.y + area.height() * g as f32 / 4.0;
        p.line_segment([pos2(area.min.x, y), pos2(area.max.x, y)], Stroke::new(1.0, Color32::from_rgb(0x2a, 0x2a, 0x2a)));
    }
    // range labels in the property column
    let dec = if let ParamKind::Float { decimals, .. } = pd.kind { decimals as usize } else { 1 };
    p.text(pos2(vr.max.x - 30.0, area.min.y + 6.0), Align2::RIGHT_CENTER, format!("{hi:.dec$}"), Tokens::ui(10.0), t.text_dim);
    p.text(pos2(vr.max.x - 30.0, area.max.y - 6.0), Align2::RIGHT_CENTER, format!("{lo:.dec$}"), Tokens::ui(10.0), t.text_dim);
    p.text(pos2(vr.min.x + 44.0, area.center().y), Align2::LEFT_CENTER, "Value", Tokens::ui(11.0), t.text_dim);
    let line: Vec<Pos2> = vals.iter().enumerate().map(|(i, v)| pos2(x_of(i as f32 / n as f32), y_of(*v))).collect();
    p.add(egui::Shape::line(line, Stroke::new(1.5, t.accent)));
    // velocity (units per second, derivative of the sampled value)
    let secs = dur * speed / filmcraft_time::TICKS_PER_SECOND as f64 / n as f64;
    let vel: Vec<f64> = (0..n).map(|i| (vals[i + 1] - vals[i]) / secs.max(1e-9)).collect();
    let vmax = vel.iter().fold(1e-6f64, |a, v| a.max(v.abs())) * 1.15;
    let varea = Rect::from_min_max(pos2(lane.min.x, velr.min.y + 2.0), pos2(lane.max.x, velr.max.y - 4.0));
    p.rect_filled(varea, 0.0, Color32::from_rgb(0x19, 0x19, 0x19));
    let vy = |v: f64| varea.center().y - (v / vmax) as f32 * varea.height() / 2.0;
    p.line_segment([pos2(varea.min.x, varea.center().y), pos2(varea.max.x, varea.center().y)], Stroke::new(1.0, Color32::from_rgb(0x33, 0x33, 0x33)));
    let vline: Vec<Pos2> = vel.iter().enumerate().map(|(i, v)| pos2(x_of((i as f32 + 0.5) / n as f32), vy(*v))).collect();
    p.add(egui::Shape::line(vline, Stroke::new(1.2, Color32::from_rgb(0xd0, 0xa0, 0x40))));
    p.text(pos2(velr.min.x + 44.0, varea.center().y), Align2::LEFT_CENTER, "Velocity", Tokens::ui(11.0), t.text_dim);
    p.text(pos2(velr.max.x - 30.0, varea.min.y + 6.0), Align2::RIGHT_CENTER, format!("{vmax:.1}/s"), Tokens::ui(10.0), t.text_dim);
    let p = p.clone();
    // keyframes + handles
    let ks = &param.keyframes;
    for (i, k) in ks.iter().enumerate() {
        let ParamValue::Float(v) = k.value else { continue };
        let f = to_f(k.time);
        if !(-0.01..=1.01).contains(&f) {
            continue;
        }
        let id = egui::Id::new(("kfg", clip.0, idx, mask, pd.id, k.time.0));
        let dy: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
        let c = pos2(x_of(f), y_of(v) + dy);
        let r = Rect::from_center_size(c, vec2(10.0, 10.0));
        let resp = ui.interact(r.expand(2.0), id, Sense::drag());
        app.auto.add(&format!("effectControls.{}.graph.keyframe.{}", pd.id, k.time.0), r, "keyframe value");
        // influence handles: flat (ease) tangents with length ∝ influence × neighbouring segment
        let eases_out =
            !matches!(k.interp, filmcraft_project::Interpolation::Linear | filmcraft_project::Interpolation::Hold | filmcraft_project::Interpolation::EaseIn);
        let eases_in =
            !matches!(k.interp, filmcraft_project::Interpolation::Linear | filmcraft_project::Interpolation::Hold | filmcraft_project::Interpolation::EaseOut);
        for (side, on, nb) in [(1.0f32, eases_out, ks.get(i + 1)), (-1.0f32, eases_in && i > 0, if i > 0 { ks.get(i - 1) } else { None })] {
            let (true, Some(nb)) = (on, nb) else { continue };
            let seg = (x_of(to_f(nb.time)) - c.x).abs();
            let infl = if side > 0.0 { k.out_influence } else { k.in_influence } as f32;
            let hid = id.with(if side > 0.0 { "out" } else { "in" });
            let hdx: f32 = ui.data(|d| d.get_temp(hid)).unwrap_or(0.0);
            let hx = c.x + side * (infl * seg + hdx * side).clamp(seg * 0.01, seg);
            let hp = pos2(hx, c.y);
            p.line_segment([c, hp], Stroke::new(1.0, Color32::from_rgb(0x90, 0x90, 0x90)));
            p.circle_filled(hp, 3.5, Color32::from_rgb(0xd0, 0xd0, 0xd0));
            let hr = ui.interact(Rect::from_center_size(hp, vec2(10.0, 10.0)), hid.with("h"), Sense::drag());
            if hr.dragged() {
                let nx = hdx + hr.drag_delta().x;
                ui.data_mut(|d| d.insert_temp(hid, nx));
            }
            if hr.drag_stopped() {
                ui.data_mut(|d| d.remove::<f32>(hid));
                let ni = ((infl * seg + hdx * side) / seg.max(1.0)).clamp(0.01, 1.0);
                let key = if side > 0.0 { "outInfluence" } else { "inInfluence" };
                actions.push(("effects.setKeyframe".into(), with_mask(json!({"clip": clip.0, "effect": idx, "param": pd.id, "mediaTime": k.time.0, key: ni}))));
            }
        }
        p.circle_filled(c, 4.5, if resp.dragged() { t.hot_text } else { Color32::from_rgb(0xe0, 0xe0, 0xe0) });
        if resp.dragged() {
            let ny = dy + resp.drag_delta().y;
            ui.data_mut(|d| d.insert_temp(id, ny));
            p.text(c + vec2(8.0, -10.0), Align2::LEFT_BOTTOM, format!("{:.dec$}", v_of(c.y)), Tokens::ui(10.5), t.hot_text);
        }
        if resp.drag_stopped() {
            ui.data_mut(|d| d.remove::<f32>(id));
            let nv = v_of(c.y);
            let nv = if let ParamKind::Float { min, max, .. } = pd.kind { nv.clamp(min, max) } else { nv };
            actions.push(("effects.setKeyframe".into(), with_mask(json!({"clip": clip.0, "effect": idx, "param": pd.id, "mediaTime": k.time.0, "value": nv}))));
        }
    }
}

/// Read-only text for parameters without an inline editor (LUT references, free text, curves).
fn param_text(project: &filmcraft_project::Project, id: &str, value: &ParamValue) -> String {
    match value {
        ParamValue::Text(s) if id.ends_with("_lut") => filmcraft_render::luts::label(Some(project), s),
        ParamValue::Text(s) if s.is_empty() => "None".into(),
        ParamValue::Text(s) => s.clone(),
        ParamValue::Curve(pts) if pts.is_empty() || *pts == [[0.0, 0.0], [1.0, 1.0]] => "Default".into(),
        ParamValue::Curve(pts) => format!("Custom ({} points)", pts.len()),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod param_text_tests {
    use super::*;

    #[test]
    fn readable_values_for_text_lut_and_curve_params() {
        let p = filmcraft_project::Project::default();
        assert_eq!(param_text(&p, "input_lut", &ParamValue::Text(String::new())), "None");
        assert_eq!(param_text(&p, "look_lut", &ParamValue::Text("builtin:look-teal-orange".into())), "Teal & Orange");
        assert_eq!(param_text(&p, "label", &ParamValue::Text(String::new())), "None");
        assert_eq!(param_text(&p, "label", &ParamValue::Text("Reel 3".into())), "Reel 3");
        assert_eq!(param_text(&p, "curve_luma", &ParamValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]])), "Default");
        assert_eq!(param_text(&p, "hue_vs_sat", &ParamValue::Curve(vec![])), "Default");
        assert_eq!(param_text(&p, "curve_luma", &ParamValue::Curve(vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]])), "Custom (3 points)");
    }
}
