//! Premiere-style docking: a tree of splits and tab groups. Panels are rounded frames separated by
//! thin gutters; each group has a tab strip (active tab bright, with a panel menu "≡"); the focused
//! panel gets a blue outline. Gutters drag to resize; workspaces are serialized trees.

use egui::{Align2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::icons::{self, Icon};
use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelKind {
    Project,
    MediaBrowser,
    Libraries,
    Info,
    Effects,
    Markers,
    History,
    Source,
    EffectControls,
    AudioClipMixer,
    Metadata,
    Program,
    Timeline,
    Tools,
    AudioMeters,
    AudioTrackMixer,
    LumetriColor,
    LumetriScopes,
    EssentialGraphics,
    EssentialSound,
    Properties,
    Text,
    Events,
    Progress,
    ReferenceMonitor,
    Timecode,
}

impl PanelKind {
    pub const ALL: [PanelKind; 26] = [
        PanelKind::Project,
        PanelKind::MediaBrowser,
        PanelKind::Libraries,
        PanelKind::Info,
        PanelKind::Effects,
        PanelKind::Markers,
        PanelKind::History,
        PanelKind::Source,
        PanelKind::EffectControls,
        PanelKind::AudioClipMixer,
        PanelKind::Metadata,
        PanelKind::Program,
        PanelKind::Timeline,
        PanelKind::Tools,
        PanelKind::AudioMeters,
        PanelKind::AudioTrackMixer,
        PanelKind::LumetriColor,
        PanelKind::LumetriScopes,
        PanelKind::EssentialGraphics,
        PanelKind::EssentialSound,
        PanelKind::Properties,
        PanelKind::Text,
        PanelKind::Events,
        PanelKind::Progress,
        PanelKind::ReferenceMonitor,
        PanelKind::Timecode,
    ];
    pub fn title(self) -> &'static str {
        match self {
            PanelKind::Project => "Project",
            PanelKind::MediaBrowser => "Media Browser",
            PanelKind::Libraries => "Libraries",
            PanelKind::Info => "Info",
            PanelKind::Effects => "Effects",
            PanelKind::Markers => "Markers",
            PanelKind::History => "History",
            PanelKind::Source => "Source",
            PanelKind::EffectControls => "Effect Controls",
            PanelKind::AudioClipMixer => "Audio Clip Mixer",
            PanelKind::Metadata => "Metadata",
            PanelKind::Program => "Program",
            PanelKind::Timeline => "Timeline",
            PanelKind::Tools => "Tools",
            PanelKind::AudioMeters => "Audio Meters",
            PanelKind::AudioTrackMixer => "Audio Track Mixer",
            PanelKind::LumetriColor => "Lumetri Color",
            PanelKind::LumetriScopes => "Lumetri Scopes",
            PanelKind::EssentialGraphics => "Essential Graphics",
            PanelKind::EssentialSound => "Essential Sound",
            PanelKind::Properties => "Properties",
            PanelKind::Text => "Text",
            PanelKind::Events => "Events",
            PanelKind::Progress => "Progress",
            PanelKind::ReferenceMonitor => "Reference Monitor",
            PanelKind::Timecode => "Timecode",
        }
    }
    pub fn id(self) -> String {
        format!("{self:?}")
    }
    pub fn from_name(s: &str) -> Option<PanelKind> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        Self::ALL.iter().copied().find(|p| format!("{p:?}").to_ascii_lowercase() == n || p.title().to_ascii_lowercase().replace(' ', "") == n)
    }
    /// Window-menu shortcut (Premiere: Shift+1..9).
    pub fn window_shortcut(self) -> Option<&'static str> {
        match self {
            PanelKind::Project => Some("Shift+1"),
            PanelKind::Source => Some("Shift+2"),
            PanelKind::Timeline => Some("Shift+3"),
            PanelKind::Program => Some("Shift+4"),
            PanelKind::EffectControls => Some("Shift+5"),
            PanelKind::AudioClipMixer => Some("Shift+9"),
            PanelKind::Effects => Some("Shift+7"),
            PanelKind::MediaBrowser => Some("Shift+8"),
            PanelKind::AudioTrackMixer => Some("Shift+6"),
            _ => None,
        }
    }
    /// Narrow chrome-less panels (no tab strip; a slim grip instead).
    pub fn compact(self) -> bool {
        matches!(self, PanelKind::Tools | PanelKind::AudioMeters)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SplitSize {
    Ratio(f32),
    /// First child has a fixed size in points.
    FixedA(f32),
    /// Second child has a fixed size in points.
    FixedB(f32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DockNode {
    /// `vertical`: children stacked top/bottom; else side by side.
    Split {
        vertical: bool,
        size: SplitSize,
        a: Box<DockNode>,
        b: Box<DockNode>,
    },
    Tabs {
        panels: Vec<PanelKind>,
        active: usize,
    },
}

fn tabs(p: &[PanelKind], active: usize) -> DockNode {
    DockNode::Tabs { panels: p.to_vec(), active }
}
fn hsplit(size: SplitSize, a: DockNode, b: DockNode) -> DockNode {
    DockNode::Split { vertical: false, size, a: Box::new(a), b: Box::new(b) }
}
fn vsplit(size: SplitSize, a: DockNode, b: DockNode) -> DockNode {
    DockNode::Split { vertical: true, size, a: Box::new(a), b: Box::new(b) }
}

pub const WORKSPACES: [&str; 9] = ["Editing", "Assembly", "Color", "Effects", "Audio", "Captions and Graphics", "Learning", "Review", "All Panels"];

/// The default layout of a named workspace.
pub fn workspace(name: &str) -> DockNode {
    use PanelKind::*;
    use SplitSize::*;
    let bottom_editing = || hsplit(FixedA(64.0), tabs(&[Tools], 0), hsplit(FixedB(116.0), tabs(&[Timeline], 0), tabs(&[AudioMeters], 0)));
    match name {
        "Assembly" => vsplit(
            Ratio(0.52),
            hsplit(
                Ratio(0.46),
                tabs(&[Project, MediaBrowser, Libraries, Info, Effects, Markers, History], 0),
                hsplit(Ratio(0.5), tabs(&[Source, EffectControls, AudioClipMixer, Metadata], 0), tabs(&[Program], 0)),
            ),
            bottom_editing(),
        ),
        "Color" => hsplit(
            FixedB(340.0),
            vsplit(
                Ratio(0.58),
                hsplit(Ratio(0.42), tabs(&[LumetriScopes, Source, EffectControls], 0), tabs(&[Program], 0)),
                hsplit(Ratio(0.3), tabs(&[Project, Info, Markers, History], 0), bottom_editing()),
            ),
            tabs(&[LumetriColor, Effects], 0),
        ),
        "Effects" => hsplit(
            FixedB(320.0),
            vsplit(
                Ratio(0.5),
                hsplit(Ratio(0.5), tabs(&[Source, EffectControls, AudioClipMixer], 1), tabs(&[Program], 0)),
                hsplit(Ratio(0.3), tabs(&[Project, MediaBrowser, Info, Markers, History], 0), bottom_editing()),
            ),
            vsplit(Ratio(0.5), tabs(&[Effects], 0), tabs(&[EffectControls, EssentialGraphics], 1)),
        ),
        "Audio" => hsplit(
            FixedB(320.0),
            vsplit(
                Ratio(0.5),
                hsplit(Ratio(0.5), tabs(&[AudioTrackMixer, Source, AudioClipMixer, EffectControls], 0), tabs(&[Program], 0)),
                hsplit(Ratio(0.3), tabs(&[Project, MediaBrowser, Effects, Markers, History], 0), bottom_editing()),
            ),
            tabs(&[EssentialSound], 0),
        ),
        "Captions and Graphics" => hsplit(
            FixedB(330.0),
            vsplit(
                Ratio(0.5),
                hsplit(Ratio(0.4), tabs(&[Text, Source, EffectControls], 0), tabs(&[Program], 0)),
                hsplit(Ratio(0.3), tabs(&[Project, MediaBrowser, Libraries], 0), bottom_editing()),
            ),
            tabs(&[EssentialGraphics, Properties], 0),
        ),
        "All Panels" => vsplit(
            Ratio(0.5),
            hsplit(
                Ratio(0.5),
                tabs(&[Source, EffectControls, AudioClipMixer, Metadata, LumetriScopes, Text], 0),
                hsplit(Ratio(0.6), tabs(&[Program], 0), tabs(&[LumetriColor, EssentialGraphics, EssentialSound, Properties, AudioTrackMixer], 0)),
            ),
            hsplit(Ratio(0.26), tabs(&[Project, MediaBrowser, Libraries, Info, Effects, Markers, History], 0), bottom_editing()),
        ),
        // Editing (default), Learning, Review — Premiere 26 factory layout:
        // top: Source group | Program | Properties; bottom: Project group | Tools | Timeline | Meters.
        _ => vsplit(
            Ratio(0.57),
            hsplit(
                FixedB(450.0),
                hsplit(Ratio(0.5), tabs(&[Source, EffectControls, AudioClipMixer, Metadata], 0), tabs(&[Program], 0)),
                tabs(&[Properties, EssentialGraphics, Text], 0),
            ),
            hsplit(
                Ratio(0.29),
                tabs(&[Project, MediaBrowser, Libraries, Info, Effects, Markers, History], 0),
                hsplit(FixedA(64.0), tabs(&[Tools], 0), hsplit(FixedB(116.0), tabs(&[Timeline], 0), tabs(&[AudioMeters], 0))),
            ),
        ),
    }
}

impl DockNode {
    /// Every panel in the tree.
    pub fn panels(&self, out: &mut Vec<PanelKind>) {
        match self {
            DockNode::Split { a, b, .. } => {
                a.panels(out);
                b.panels(out);
            }
            DockNode::Tabs { panels, .. } => out.extend(panels.iter().copied()),
        }
    }
    pub fn contains(&self, p: PanelKind) -> bool {
        let mut v = Vec::new();
        self.panels(&mut v);
        v.contains(&p)
    }
    /// Make `p` the active tab of its group. Returns false if not present.
    pub fn activate(&mut self, p: PanelKind) -> bool {
        match self {
            DockNode::Split { a, b, .. } => a.activate(p) || b.activate(p),
            DockNode::Tabs { panels, active } => {
                if let Some(i) = panels.iter().position(|x| *x == p) {
                    *active = i;
                    true
                } else {
                    false
                }
            }
        }
    }
    pub fn is_visible(&self, p: PanelKind) -> bool {
        match self {
            DockNode::Split { a, b, .. } => a.is_visible(p) || b.is_visible(p),
            DockNode::Tabs { panels, active } => panels.get(*active) == Some(&p),
        }
    }
    /// Close a panel (remove its tab; empty groups collapse their split).
    pub fn close(&mut self, p: PanelKind) {
        if let DockNode::Split { a, b, .. } = self {
            a.close(p);
            b.close(p);
            let empty = |n: &DockNode| matches!(n, DockNode::Tabs { panels, .. } if panels.is_empty());
            if empty(a) {
                *self = (**b).clone();
            } else if empty(b) {
                *self = (**a).clone();
            }
        } else if let DockNode::Tabs { panels, active } = self {
            panels.retain(|x| *x != p);
            *active = (*active).min(panels.len().saturating_sub(1));
        }
    }
    /// Add a panel as a tab next to `near` (or into the first group).
    pub fn open_near(&mut self, p: PanelKind, near: PanelKind) {
        if self.contains(p) {
            self.activate(p);
            return;
        }
        fn add(n: &mut DockNode, p: PanelKind, near: PanelKind) -> bool {
            match n {
                DockNode::Split { a, b, .. } => add(a, p, near) || add(b, p, near),
                DockNode::Tabs { panels, active } => {
                    if panels.contains(&near) {
                        panels.push(p);
                        *active = panels.len() - 1;
                        true
                    } else {
                        false
                    }
                }
            }
        }
        if !add(self, p, near) && !add(self, p, PanelKind::Project) {
            add(self, p, PanelKind::Program);
        }
    }
}

/// One laid-out tab group.
pub struct Group {
    pub path: String,
    pub rect: Rect,
    pub content: Rect,
    pub panels: Vec<PanelKind>,
    pub active: usize,
}

/// Actions produced by interacting with the dock chrome.
#[derive(Debug, Clone, PartialEq)]
pub enum DockAction {
    Activate(PanelKind),
    Focus(PanelKind),
    Close(PanelKind),
    PanelMenu(PanelKind, egui::Pos2),
}

/// Lay out the tree into group rects (with `gap` gutters) and handle gutter dragging.
pub fn layout(ui: &mut egui::Ui, node: &mut DockNode, rect: Rect, t: &Tokens, path: &str, out: &mut Vec<Group>, reg: &mut crate::automation::Registry) {
    match node {
        DockNode::Tabs { panels, active } => {
            let tab_h = if panels.len() == 1 && panels[0].compact() { 8.0 } else { t.tab_h };
            let content = Rect::from_min_max(pos2(rect.min.x, rect.min.y + tab_h), rect.max);
            out.push(Group { path: path.to_string(), rect, content, panels: panels.clone(), active: *active });
        }
        DockNode::Split { vertical, size, a, b } => {
            let g = t.gap;
            let total = if *vertical { rect.height() } else { rect.width() };
            let avail = (total - g).max(0.0);
            let first = match *size {
                SplitSize::Ratio(r) => avail * r,
                SplitSize::FixedA(px) => px.min(avail - 20.0),
                SplitSize::FixedB(px) => avail - px.min(avail - 20.0),
            }
            .clamp(20.0_f32.min(avail), (avail - 20.0).max(0.0));
            let (ra, gutter, rb) = if *vertical {
                (
                    Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + first)),
                    Rect::from_min_max(pos2(rect.min.x, rect.min.y + first), pos2(rect.max.x, rect.min.y + first + g)),
                    Rect::from_min_max(pos2(rect.min.x, rect.min.y + first + g), rect.max),
                )
            } else {
                (
                    Rect::from_min_max(rect.min, pos2(rect.min.x + first, rect.max.y)),
                    Rect::from_min_max(pos2(rect.min.x + first, rect.min.y), pos2(rect.min.x + first + g, rect.max.y)),
                    Rect::from_min_max(pos2(rect.min.x + first + g, rect.min.y), rect.max),
                )
            };
            // gutter drag (a slightly larger hit area than the visible gap)
            let hit = gutter.expand2(if *vertical { vec2(0.0, 3.0) } else { vec2(3.0, 0.0) });
            let id = egui::Id::new(("dock-gutter", path.to_string()));
            let resp = ui.interact(hit, id, Sense::drag());
            reg.add(&format!("dock.gutter.{path}"), hit, "gutter");
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(if *vertical { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
            }
            if resp.dragged() {
                let d = if *vertical { resp.drag_delta().y } else { resp.drag_delta().x };
                let nf = (first + d).clamp(40.0, (avail - 40.0).max(40.0));
                *size = match *size {
                    SplitSize::Ratio(_) => SplitSize::Ratio(nf / avail.max(1.0)),
                    SplitSize::FixedA(_) => SplitSize::FixedA(nf),
                    SplitSize::FixedB(_) => SplitSize::FixedB(avail - nf),
                };
            }
            if resp.dragged() || resp.hovered() {
                ui.painter().rect_filled(gutter, 0.0, t.focus.gamma_multiply(if resp.dragged() { 0.9 } else { 0.4 }));
            }
            layout(ui, a, ra, t, &format!("{path}a"), out, reg);
            layout(ui, b, rb, t, &format!("{path}b"), out, reg);
        }
    }
}

/// Draw a group's frame + tab strip. Returns actions (tab clicks, panel menu, focus).
pub fn draw_group_chrome(ui: &mut egui::Ui, g: &Group, focused: PanelKind, t: &Tokens, reg: &mut crate::automation::Registry) -> Vec<DockAction> {
    let mut actions = Vec::new();
    let painter = ui.painter().clone();
    painter.rect_filled(g.rect, t.radius, t.panel_bg);
    let active_panel = g.panels.get(g.active).copied();
    let compact = g.panels.len() == 1 && g.panels[0].compact();
    if compact {
        // grip dots
        let c = pos2(g.rect.center().x, g.rect.min.y + 4.0);
        for dx in [-4.0, 0.0, 4.0] {
            painter.circle_filled(c + vec2(dx, 0.0), 1.0, t.text_faint);
        }
    } else {
        let strip = Rect::from_min_size(g.rect.min, vec2(g.rect.width(), t.tab_h));
        let mut x = strip.min.x + 12.0;
        let text_y = strip.min.y + 16.0;
        for (i, p) in g.panels.iter().enumerate() {
            let is_active = i == g.active;
            let galley = painter.layout_no_wrap(p.title().to_string(), Tokens::ui(12.0), if is_active { t.tab_text_active } else { t.tab_text });
            let menu_w = if is_active { 20.0 } else { 0.0 };
            let w = galley.size().x + 16.0 + menu_w;
            if x + w > strip.max.x - 20.0 && i > g.active {
                let r = Rect::from_min_size(pos2(strip.max.x - 22.0, strip.min.y + 6.0), vec2(18.0, 20.0));
                let resp = ui.interact(r, egui::Id::new(("tab-overflow", g.path.clone())), Sense::click());
                icons::paint(&painter, r.shrink(4.0).translate(vec2(-2.0, 0.0)), Icon::ChevronRight, t.tab_text);
                icons::paint(&painter, r.shrink(4.0).translate(vec2(2.0, 0.0)), Icon::ChevronRight, t.tab_text);
                if resp.clicked() {
                    let next = g.panels[(g.active + 1) % g.panels.len()];
                    actions.push(DockAction::Activate(next));
                }
                break;
            }
            let tab = Rect::from_min_size(pos2(x, strip.min.y), vec2(w, t.tab_h));
            let resp = ui.interact(tab, egui::Id::new(("tab", g.path.clone(), i)), Sense::click());
            reg.add(&format!("panel.tab.{}", p.id()), tab, p.title());
            let label_x = tab.min.x + 8.0;
            let label_w = galley.size().x;
            let col = if is_active || resp.hovered() { t.tab_text_active } else { t.tab_text };
            painter.galley_with_override_text_color(pos2(label_x, text_y - galley.size().y / 2.0), galley, col);
            if is_active {
                let mr = Rect::from_center_size(pos2(label_x + label_w + 12.0, text_y), vec2(12.0, 10.0));
                let mresp = ui.interact(mr.expand(3.0), egui::Id::new(("tab-menu", g.path.clone())), Sense::click());
                reg.add(&format!("panel.menu.{}", p.id()), mr, "panel menu");
                let mc = if mresp.hovered() { t.tab_text_active } else { t.tab_text };
                for dy in [-3.5, 0.0, 3.5] {
                    painter.line_segment([pos2(mr.min.x, mr.center().y + dy), pos2(mr.max.x, mr.center().y + dy)], Stroke::new(1.5, mc));
                }
                // 1 pt underline spanning label + ≡, 23 pt below the frame top
                let uy = strip.min.y + 23.0;
                painter.line_segment([pos2(label_x, uy), pos2(mr.max.x, uy)], Stroke::new(1.0, t.tab_text_active));
                if mresp.clicked() {
                    actions.push(DockAction::PanelMenu(*p, mr.left_bottom()));
                }
            }
            if resp.clicked() {
                actions.push(DockAction::Activate(*p));
                actions.push(DockAction::Focus(*p));
            }
            if resp.middle_clicked() {
                actions.push(DockAction::Close(*p));
            }
            x += w + 8.0;
        }
    }
    // focus outline
    if active_panel == Some(focused) {
        painter.rect_stroke(g.rect, 0.0, Stroke::new(1.0, t.focus), StrokeKind::Inside);
    }
    // clicking anywhere in the panel focuses it
    if let Some(p) = active_panel
        && ui.rect_contains_pointer(g.rect)
        && ui.input(|i| i.pointer.any_pressed())
    {
        actions.push(DockAction::Focus(p));
    }
    actions
}

/// Placeholder body for panels that are not implemented yet.
pub fn placeholder(ui: &mut egui::Ui, rect: Rect, t: &Tokens, text: &str) {
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, Tokens::ui(12.0), t.text_faint);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspaces_contain_core_panels() {
        for w in WORKSPACES {
            let d = workspace(w);
            assert!(d.contains(PanelKind::Timeline), "{w}");
            assert!(d.contains(PanelKind::Program), "{w}");
        }
    }

    #[test]
    fn close_and_open() {
        let mut d = workspace("Editing");
        d.close(PanelKind::Metadata);
        assert!(!d.contains(PanelKind::Metadata));
        d.open_near(PanelKind::LumetriColor, PanelKind::Program);
        assert!(d.is_visible(PanelKind::LumetriColor));
        let s = serde_json::to_string(&d).unwrap();
        let back: DockNode = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }
}
