//! Export mode (header "Export"): destination list, settings and a live preview. The encode
//! pipeline lands in M6; the page already shows the sequence preview and settings summary.

use egui::{Align2, Color32, Rect, pos2, vec2};

use crate::FilmcraftApp;
use crate::frames::{FrameKey, Target};
use crate::theme::Tokens;

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let left = Rect::from_min_size(rect.min, vec2(220.0, rect.height()));
    let right = Rect::from_min_max(pos2(rect.max.x - 360.0, rect.min.y), rect.max);
    let mid = Rect::from_min_max(pos2(left.max.x + 4.0, rect.min.y), pos2(right.min.x - 4.0, rect.max.y));
    for r in [left, mid, right] {
        ui.painter().rect_filled(r, t.radius, t.panel_bg);
    }
    ui.painter().text(left.min + vec2(14.0, 20.0), Align2::LEFT_CENTER, "Destinations", Tokens::semibold(13.0), t.text);
    for (i, d) in ["Media File", "YouTube", "Vimeo", "TikTok", "Instagram", "FTP"].iter().enumerate() {
        let r = Rect::from_min_size(left.min + vec2(8.0, 40.0 + i as f32 * 30.0), vec2(left.width() - 16.0, 26.0));
        if i == 0 {
            ui.painter().rect_filled(r, 4.0, t.row_selected);
        }
        ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, *d, Tokens::ui(12.5), t.text);
    }
    // preview
    if let Some(seq_id) = app.session.state.active_sequence {
        let q = app.session.active_sequence().expect("seq").clone();
        let rate = q.settings.frame_rate;
        let frame = rate.frame_at(app.session.playhead());
        let key = FrameKey { target: Target::Sequence(seq_id), frame, size: 500, revision: app.session.revision };
        let project = app.session.project.clone();
        app.frames.request(key, rate.tick_of(frame), 0.5, &project, 0);
        let pic = crate::panels::monitor::fit(mid.shrink(24.0), q.settings.width as f32, q.settings.height as f32);
        ui.painter().rect_filled(pic, 0.0, Color32::BLACK);
        if let Some(img) = app.frames.get(&key) {
            let ctx = ui.ctx().clone();
            let tex = app.texture_for(&ctx, "export-preview", key, &img);
            ui.painter().image(tex, pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        // settings
        let seq_name = app.session.project.item(seq_id).map(|i| i.name.clone()).unwrap_or_default();
        let fmt = filmcraft_engine::export::Format::from_name(&app.ui.export_format).unwrap_or(filmcraft_engine::export::Format::H264);
        let hardware_available = filmcraft_engine::export::hardware_encoder_available();
        if app.ui.export_video_encoder.is_empty() {
            app.ui.export_video_encoder = if hardware_available { "hardware" } else { "software" }.into();
        }
        if app.ui.export_path.is_empty() || !app.ui.export_path.ends_with(fmt.extension()) {
            app.ui.export_path = format!("~/Movies/{}.{}", seq_name.replace(' ', "_"), fmt.extension());
        }
        let mut sui = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink(16.0)).id_salt("export-settings"));
        sui.label(egui::RichText::new("Settings").strong().size(14.0));
        sui.add_space(8.0);
        sui.label(egui::RichText::new("File name / location").color(t.text_dim));
        sui.add(egui::TextEdit::singleline(&mut app.ui.export_path).desired_width(f32::INFINITY));
        sui.add_space(6.0);
        sui.label(egui::RichText::new("Format").color(t.text_dim));
        egui::ComboBox::from_id_salt("export-format").selected_text(fmt.label()).width(sui.available_width()).show_ui(&mut sui, |ui| {
            for f in filmcraft_engine::export::Format::ALL {
                let avail = filmcraft_engine::export::available(f);
                let label = if avail { f.label().to_string() } else { format!("{} (encoder in progress)", f.label()) };
                if ui.add_enabled(avail, egui::Button::selectable(f == fmt, label)).clicked() {
                    app.ui.export_format = format!("{f:?}").to_ascii_lowercase();
                    app.ui.export_path.clear();
                }
            }
        });
        sui.add_space(6.0);
        if fmt == filmcraft_engine::export::Format::H264 {
            sui.label(egui::RichText::new("Video encoder").color(t.text_dim));
            let encoder_label = if app.ui.export_video_encoder == "hardware" { "Hardware (NVIDIA)" } else { "Software (CPU)" };
            egui::ComboBox::from_id_salt("export-video-encoder").selected_text(encoder_label).width(sui.available_width()).show_ui(&mut sui, |ui| {
                if ui
                    .add_enabled(hardware_available, egui::Button::selectable(app.ui.export_video_encoder == "hardware", "Hardware (NVIDIA)"))
                    .on_hover_text(if hardware_available { "Use NVIDIA NVENC for H.264 export" } else { "No NVIDIA NVENC device detected" })
                    .clicked()
                {
                    app.ui.export_video_encoder = "hardware".into();
                }
                if ui.selectable_label(app.ui.export_video_encoder == "software", "Software (CPU)").clicked() {
                    app.ui.export_video_encoder = "software".into();
                }
                if !hardware_available {
                    ui.label(egui::RichText::new("NVIDIA NVENC not detected").weak().small());
                }
            });
            if !hardware_available && app.ui.export_video_encoder == "hardware" {
                sui.colored_label(t.danger, "NVENC is unavailable; choose Software.");
            }
            sui.add_space(6.0);
        }
        let lines = [
            ("Video", format!("{}x{} · {} fps", q.settings.width, q.settings.height, q.settings.frame_rate.label())),
            ("Audio", format!("{} Hz · Stereo", q.settings.sample_rate)),
            ("Range", if q.mark_in.is_some() || q.mark_out.is_some() { "Sequence In/Out".to_string() } else { "Entire Sequence".to_string() }),
        ];
        for (k, v) in lines {
            sui.horizontal(|ui| {
                ui.add_sized(vec2(60.0, 16.0), egui::Label::new(egui::RichText::new(k).color(t.text_dim)));
                ui.label(v);
            });
        }
        if !q.caption_tracks.is_empty() {
            sui.add_space(6.0);
            sui.label(egui::RichText::new("Captions").color(t.text_dim));
            let r = sui.checkbox(&mut app.ui.export_burn_captions, "Burn Captions Into Video");
            app.auto.add("export.burnCaptions", r.rect, "Burn Captions Into Video");
        }
        sui.add_space(12.0);
        // jobs
        let jobs: Vec<serde_json::Value> = app.session.jobs.iter().rev().take(4).map(|j| j.to_json()).collect();
        for j in &jobs {
            let frac = j["progress"].as_f64().unwrap_or(0.0) as f32;
            let label = j["label"].as_str().unwrap_or("");
            let err = j["result"]["error"].as_str();
            let done = j["finished"].as_bool().unwrap_or(false);
            sui.label(egui::RichText::new(label).size(12.0));
            sui.add(egui::ProgressBar::new(frac).text(match (done, err) {
                (true, Some(e)) => format!("Failed: {e}"),
                (true, None) => format!("Done · {:.1} fps", j["result"]["render_fps"].as_f64().unwrap_or(0.0)),
                _ => format!("{:.0}%", frac * 100.0),
            }));
            if !done {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let b = Rect::from_min_size(pos2(right.max.x - 120.0, right.max.y - 44.0), vec2(104.0, 30.0));
        let resp = ui.interact(b, egui::Id::new("export-go"), egui::Sense::click());
        ui.painter().rect_filled(b, 15.0, if resp.hovered() { t.accent_hover } else { t.accent });
        ui.painter().text(b.center(), Align2::CENTER_CENTER, "Export", Tokens::semibold(13.0), Color32::WHITE);
        app.auto.add("export.button", b, "Export");
        if resp.clicked() {
            let r = app.session.execute(
                "file.exportMedia",
                serde_json::json!({"path": expand_home(&app.ui.export_path), "format": app.ui.export_format, "burnCaptions": app.ui.export_burn_captions, "videoEncoder": app.ui.export_video_encoder}),
            );
            if let Err(e) = r {
                app.ui.status = e.to_string();
            }
        }
    } else {
        crate::dock::placeholder(ui, mid, &t, "Open a sequence to export");
    }
}

/// `~/…` → the user's home directory (the path field shows the short form).
fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) => format!("{h}/{rest}"),
        _ => p.to_string(),
    }
}
