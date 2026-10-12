//! The side panel (backslash): views, filters, groups, physics and an
//! inspector for the selected node.
//!
//! Everything the panel edits lives in the active view. The panel only
//! records WHAT changed (`PanelOut`); NaviApp applies it right after the
//! panel is drawn, in the same frame, so the graph follows immediately and
//! views.json is saved (debounced) from one place.
//!
//! Input rules (see app.rs): while a panel text field has focus no graph
//! shortcut fires, Escape leaves the field first and Tab cycles nodes;
//! Escape cancels a pending rename or delete; clicks, drags and scrolling
//! over the panel (or one of its popups) never reach the graph.

use egui::{Align, Color32, CursorIcon, Layout, Pos2, RichText, Sense, Stroke, TextEdit, TextStyle, Ui, Vec2};
use navi_core::{
    groups::{luminance, new_group_colour, AutoGrouper, CommunityScope, Group, GroupColour},
    view::relative_to_root,
    DailiesMode, OrphansMode, Query, View,
};

use super::NaviApp;
use crate::theme::Theme;

pub const PANEL_W:     f32 = 340.0;
pub const PANEL_W_MIN: f32 = 270.0;
pub const PANEL_W_MAX: f32 = 600.0;

/// Panel-only state; nothing here is saved.
#[derive(Default)]
pub struct PanelState {
    pub rename:         Option<String>, // the active view's name, being edited
    pub rename_error:   Option<String>,
    pub rename_focus:   bool,           // give the rename field the keyboard the frame it opens
    pub confirm_delete: bool,           // "Delete view?" asked inline
    pub drag_from:      Option<usize>,  // manual group row being dragged by its handle
    autos:              Option<((u64, usize), Vec<AutoRow>)>, // the automatic groups list, per (view_gen, theme)
}

/// One row of the automatic groups list, built once per recompute.
pub struct AutoRow {
    key:     String,
    label:   String,
    short:   String, // the label, cut to fit
    colour:  Color32,
    members: usize,
    shown:   usize,
}

/// A change to the list of views (applied by NaviApp::apply_view_op).
pub enum ViewOp { Switch(usize), New, Duplicate, Rename(String), Delete }

/// What the panel changed this frame.
#[derive(Default)]
pub struct PanelOut {
    pub groups:  bool,           // manual groups, the auto grouper or the group display
    pub group_style: bool,       // only a manual group's colour or name (no re-resolve)
    pub filter:  bool,           // dailies / orphans / links_to_dailies_count
    pub pull:    bool,           // group pull strength
    pub physics: bool,           // pause / resume pressed
    pub view_op: Option<ViewOp>,
    pub open:    Option<usize>,  // open this node in Emacs
    pub panel:   bool,           // the panel's own open/closed state
}

fn rgb(c: [u8; 3]) -> Color32 { Color32::from_rgb(c[0], c[1], c[2]) }
fn bg_of(t: &Theme) -> [u8; 3] { [t.bg.r(), t.bg.g(), t.bg.b()] }

/// egui visuals from a theme: panels, popups and text fields in the theme's
/// bar colours. Light backgrounds start from egui's light visuals.
pub fn visuals(t: &Theme) -> egui::Visuals {
    let mut v = if luminance(bg_of(t)) > 0.35 { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill       = t.bar_bg;
    v.window_fill      = t.bar_bg;
    v.window_stroke    = Stroke::new(1.0_f32, t.bar_line);
    v.extreme_bg_color = t.bg;                                   // text fields
    v.faint_bg_color   = t.grid;
    v.code_bg_color    = t.grid;
    v.hyperlink_color  = t.edge_hi;
    v.selection.bg_fill = t.edge;                                // selected toggles, text selection
    v.selection.stroke  = Stroke::new(1.0_f32, t.label_hov);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill      = t.bar_bg;
    w.noninteractive.weak_bg_fill = t.bar_bg;
    w.noninteractive.bg_stroke    = Stroke::new(1.0_f32, t.bar_line);
    w.noninteractive.fg_stroke    = Stroke::new(1.0_f32, t.bar_text);
    w.inactive.bg_fill            = t.grid;
    w.inactive.weak_bg_fill       = t.grid;
    w.inactive.bg_stroke          = Stroke::NONE;
    w.inactive.fg_stroke          = Stroke::new(1.0_f32, t.bar_text);
    w.hovered.bg_fill             = t.bar_line;
    w.hovered.weak_bg_fill        = t.bar_line;
    w.hovered.bg_stroke           = Stroke::new(1.0_f32, t.edge_hi);
    w.hovered.fg_stroke           = Stroke::new(1.5_f32, t.label_hov);
    w.active.bg_fill              = t.edge;
    w.active.weak_bg_fill         = t.edge;
    w.active.bg_stroke            = Stroke::new(1.0_f32, t.edge_hi);
    w.active.fg_stroke            = Stroke::new(2.0_f32, t.label_hov);
    w.open = w.hovered;
    v
}

/// Draw the panel (when the active view has it open) and report changes.
pub fn show(app: &mut NaviApp, ctx: &egui::Context) -> PanelOut {
    let mut out = PanelOut::default();
    if !app.active_view.panel_open { app.panel.drag_from = None; return out; }
    let t = app.theme();
    egui::SidePanel::right("navi-side-panel")
        .resizable(true)
        .default_width(PANEL_W)
        .width_range(PANEL_W_MIN..=PANEL_W_MAX)
        .frame(egui::Frame::none().fill(t.bar_bg).stroke(Stroke::new(1.0_f32, t.bar_line)).inner_margin(egui::Margin::symmetric(12.0, 8.0)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Navi").size(15.0).color(t.label_hov));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Close").on_hover_text("Hide the panel (\\)").clicked() { out.panel = true; }
                });
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("navi-panel-scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                section(ui, &t, "View", true, |ui| views_section(app, ui, &t, &mut out));
                section(ui, &t, "Filters", true, |ui| filters_section(app, ui, &t, &mut out));
                section(ui, &t, "Groups", true, |ui| groups_section(app, ui, &t, &mut out));
                section(ui, &t, "Physics", false, |ui| physics_section(app, ui, &t, &mut out));
                section(ui, &t, "Inspector", true, |ui| inspector_section(app, ui, &t, &mut out));
                ui.add_space(12.0);
            });
        });
    out
}

fn section(ui: &mut Ui, t: &Theme, title: &str, open: bool, body: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).size(14.0).color(t.label_hov))
        .id_salt(("navi-section", title))
        .default_open(open)
        .show(ui, body);
    ui.add_space(2.0);
}

fn weak(t: &Theme, s: impl Into<String>) -> RichText { RichText::new(s.into()).color(t.label).size(12.0) }

fn error_colour(t: &Theme) -> Color32 { rgb(GroupColour::Slot(0).rgb(bg_of(t))) } // the palette's red, readable on this theme

// ── 1. View ───────────────────────────────────────────────────────────────────

fn views_section(app: &mut NaviApp, ui: &mut Ui, t: &Theme, out: &mut PanelOut) {
    let cur   = app.views.active_index(&app.db_key);
    let names: Vec<String> = app.views.views(&app.db_key).iter().map(|v| v.name.clone()).collect();
    for (k, name) in names.iter().enumerate() {
        let active = k == cur;
        ui.horizontal(|ui| {
            // the active view: a filled dot; the others: a ring
            let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
            if active { ui.painter().circle_filled(r.center(), 4.5, t.node_sel); }
            else { ui.painter().circle_stroke(r.center(), 4.0, Stroke::new(1.2_f32, t.label)); }
            let key  = if k < 9 { format!("{}", k + 1) } else { " ".into() };
            let resp = ui.selectable_label(active, RichText::new(format!("{key}   {name}")).color(if active { t.label_hov } else { t.bar_text }));
            if resp.clicked() && !active { out.view_op = Some(ViewOp::Switch(k)); }
            if !active { resp.on_hover_text(if k < 9 { format!("Switch to this view (key {})", k + 1) } else { "Switch to this view".into() }); }
        });
    }

    let focus = std::mem::take(&mut app.panel.rename_focus);
    if let Some(buf) = app.panel.rename.as_mut() {
        let mut commit = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let r = ui.add(TextEdit::singleline(buf).desired_width(ui.available_width() - 110.0));
            if focus { r.request_focus(); } // else typing the name fires graph shortcuts
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { commit = true; }
            if ui.small_button("Rename").clicked() { commit = true; }
            if ui.small_button("Cancel").clicked() { cancel = true; }
        });
        if let Some(e) = &app.panel.rename_error { ui.label(RichText::new(e).color(error_colour(t)).size(12.0)); }
        if commit { out.view_op = Some(ViewOp::Rename(buf.clone())); }
        if cancel { app.panel.rename = None; app.panel.rename_error = None; }
    } else if app.panel.confirm_delete {
        let name = names.get(cur).cloned().unwrap_or_default();
        ui.label(RichText::new(format!("Delete the view \"{name}\"? Its groups and filters go with it.")).color(t.label_hov));
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Delete").color(error_colour(t))).clicked() { out.view_op = Some(ViewOp::Delete); app.panel.confirm_delete = false; }
            if ui.button("Cancel").clicked() { app.panel.confirm_delete = false; }
        });
    } else {
        ui.horizontal_wrapped(|ui| {
            if ui.button("New").on_hover_text("A fresh view with no groups or filters").clicked() { out.view_op = Some(ViewOp::New); }
            if ui.button("Duplicate").on_hover_text("A copy of this view").clicked() { out.view_op = Some(ViewOp::Duplicate); }
            if ui.button("Rename").clicked() {
                app.panel.rename = Some(names.get(cur).cloned().unwrap_or_default());
                app.panel.rename_error = None;
                app.panel.rename_focus = true;
            }
            let can_delete = names.len() > 1;
            let del = ui.add_enabled(can_delete, egui::Button::new("Delete"));
            let del = if can_delete { del } else { del.on_disabled_hover_text("The last view can't be deleted") };
            if del.clicked() { app.panel.confirm_delete = true; }
        });
    }
}

// ── 2. Filters ────────────────────────────────────────────────────────────────

fn filters_section(app: &mut NaviApp, ui: &mut Ui, t: &Theme, out: &mut PanelOut) {
    let f = &mut app.view_filter;
    egui::Grid::new("navi-filters").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Dailies");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (m, l) in [(DailiesMode::Show, "Show"), (DailiesMode::Hide, "Hide")] {
                if ui.selectable_value(&mut f.dailies, m, l).changed() { out.filter = true; }
            }
        }).response.on_hover_text("D");
        ui.end_row();
        ui.label("Orphans");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (m, l) in [(OrphansMode::Show, "Show"), (OrphansMode::Hide, "Hide"), (OrphansMode::Only, "Only")] {
                if ui.selectable_value(&mut f.orphans, m, l).changed() { out.filter = true; }
            }
        }).response.on_hover_text("O");
        ui.end_row();
    });
    if ui.checkbox(&mut f.links_to_dailies_count, "Links to dailies count")
        .on_hover_text("Off: a note linked only to dailies counts as an orphan (\"daily-only\"). Only matters while dailies are shown.")
        .changed() { out.filter = true; }

    let c = &app.view.counts;
    ui.add_space(2.0);
    egui::Grid::new("navi-counts").num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
        let row = |ui: &mut Ui, k: &str, v: String| { ui.label(weak(t, k)); ui.label(RichText::new(v).color(t.bar_text).size(12.0)); ui.end_row(); };
        row(ui, "Visible", format!("{} of {} nodes, {} links", c.visible, c.total, c.visible_edges));
        row(ui, "Orphans in view", format!("{}  ({} in db)", c.orphans_in_view, c.orphans_in_db));
        row(ui, "Isolated by filters", c.isolated_by_filters.to_string());
        if c.daily_only > 0 { row(ui, "Daily-only", c.daily_only.to_string()); }
        if c.hidden_by_groups > 0 { row(ui, "Hidden by groups", c.hidden_by_groups.to_string()); }
        row(ui, "Dangling links", c.dangling.to_string());
    });
}

// ── 3. Groups ─────────────────────────────────────────────────────────────────

/// The drag handle: a 2 × 3 grip of dots.
fn grip(ui: &mut Ui, t: &Theme, dragging: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(14.0, 20.0), Sense::drag());
    let c = if dragging || resp.hovered() { t.label_hov } else { t.label };
    for row in 0..3 {
        for col in 0..2 {
            let p = Pos2::new(rect.center().x - 2.5 + col as f32 * 5.0, rect.center().y - 5.0 + row as f32 * 5.0);
            ui.painter().circle_filled(p, 1.5, c);
        }
    }
    resp.on_hover_cursor(CursorIcon::Grab).on_hover_text("Drag to reorder: the first visible group a node is in gives its colour")
}

fn groups_section(app: &mut NaviApp, ui: &mut Ui, t: &Theme, out: &mut PanelOut) {
    let bg = bg_of(t);
    let mut shown = app.active_view.colour_groups;
    if ui.checkbox(&mut shown, "Show groups  (G)")
        .on_hover_text("Group colours, hulls, pull and the legend. Hidden groups hide nodes either way.")
        .changed() { app.active_view.set_colour_groups(shown); out.groups = true; } // exactly what G does

    // manual groups, highest precedence first
    let n = app.active_view.groups.len();
    if n == 0 { ui.label(weak(t, "No groups yet. A group is a query, like tag:rust OR dir:posts.")); }
    let mut rects: Vec<egui::Rect> = Vec::with_capacity(n);
    let mut remove: Option<usize> = None;
    let mut swap:   Option<(usize, usize)> = None;
    let drag_from = app.panel.drag_from.filter(|&k| k < n);
    for gi in 0..n {
        let info = app.groups.groups.get(gi).filter(|g| g.manual);
        let g    = &mut app.active_view.groups[gi];
        let frame = egui::Frame::group(ui.style())
            .stroke(Stroke::new(1.0_f32, if drag_from == Some(gi) { t.node_sel } else { t.bar_line }))
            .inner_margin(egui::Margin::same(6.0));
        let resp = frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                if grip(ui, t, drag_from == Some(gi)).drag_started() { app.panel.drag_from = Some(gi); }
                let shown = g.colour.unwrap_or_else(|| info.map_or([128, 128, 128], |i| i.colour.rgb(bg)));
                let mut c = shown;
                if ui.color_edit_button_srgb(&mut c).on_hover_text("Colour").changed() { g.colour = Some(c); out.group_style = true; }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    if ui.small_button("\u{1F5D1}").on_hover_text("Delete this group").clicked() { remove = Some(gi); }
                    if ui.add_enabled(gi + 1 < n, egui::Button::new("\u{23F7}").small()).on_hover_text("Lower precedence").clicked() { swap = Some((gi, gi + 1)); }
                    if ui.add_enabled(gi > 0, egui::Button::new("\u{23F6}").small()).on_hover_text("Higher precedence").clicked() { swap = Some((gi - 1, gi)); }
                    if let Some(i) = info {
                        ui.label(RichText::new(i.members.to_string()).color(t.bar_text).size(12.0))
                            .on_hover_text(format!("{} matching, {} on screen", i.members, i.shown));
                    }
                    if ui.add(TextEdit::singleline(&mut g.name).hint_text("Name").desired_width(f32::INFINITY)).changed() {
                        // a colour taken from the name's hash would change with every key: keep the one shown
                        g.colour.get_or_insert(shown);
                        out.group_style = true;
                    }
                });
            });
            if ui.add(TextEdit::singleline(&mut g.query).hint_text("tag:x OR dir:posts").font(TextStyle::Monospace).desired_width(f32::INFINITY))
                .changed() { out.groups = true; }
            if let Some(e) = info.and_then(|i| i.error.as_ref()) {
                ui.label(RichText::new(e.to_string()).color(error_colour(t)).size(12.0));
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if ui.toggle_value(&mut g.visible, "Visible").on_hover_text("Off: hides nodes whose every group is hidden").changed() { out.groups = true; }
                if ui.toggle_value(&mut g.pull, "Pull").on_hover_text("Members drift toward their centre (strength under Physics)").changed() { out.groups = true; }
                if ui.toggle_value(&mut g.hull, "Hull").on_hover_text("A faint outline behind the members").changed() { out.groups = true; }
            });
        }).response;
        rects.push(resp.rect);
    }

    // drag to reorder: an insertion line follows the pointer; release drops
    if let Some(from) = drag_from {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        let y  = ui.ctx().pointer_interact_pos().map(|p| p.y);
        let to = y.map_or(from, |y| rects.iter().filter(|r| r.center().y < y).count());
        let line_y = if to == 0 { rects[0].top() - 2.0 } else { rects[to - 1].bottom() + 2.0 };
        ui.painter().hline(rects[0].x_range(), line_y, Stroke::new(2.0_f32, t.node_sel));
        if ui.input(|i| !i.pointer.any_down()) {
            let dest = if to > from { to - 1 } else { to };
            if dest != from {
                let g = app.active_view.groups.remove(from);
                app.active_view.groups.insert(dest, g);
                out.groups = true;
            }
            app.panel.drag_from = None;
        }
    } else if app.panel.drag_from.is_some_and(|k| k >= n) {
        app.panel.drag_from = None; // its row is gone
    }
    if let Some((a, b)) = swap { app.active_view.groups.swap(a, b); out.groups = true; }
    if let Some(k) = remove { app.active_view.groups.remove(k); out.groups = true; }

    let prefill = add_group_query(app);
    let add = ui.button("Add group");
    let add = if prefill.trim().is_empty() { add } else { add.on_hover_text(format!("Query: {prefill}")) };
    if add.clicked() {
        add_group(app, bg);
        out.groups = true;
    }

    // the automatic grouper
    ui.add_space(4.0);
    let v = &mut app.active_view;
    let (groups, graph) = (&app.groups, &app.graph);
    ui.horizontal(|ui| {
        ui.label("Group by");
        let kinds = [
            AutoGrouper::None,
            match v.auto { a @ AutoGrouper::Folder { .. } => a, _ => AutoGrouper::Folder { depth: 1 } },
            AutoGrouper::Tag,
            AutoGrouper::Component,
            match v.auto { a @ AutoGrouper::Community { .. } => a, _ => AutoGrouper::Community { resolution: 1.0, scope: CommunityScope::All } },
            AutoGrouper::File,
        ];
        let name = |a: &AutoGrouper| match a { AutoGrouper::Folder { .. } => "Folder".to_string(), a => a.label() };
        egui::ComboBox::from_id_salt("navi-auto-grouper").selected_text(name(&v.auto)).width(130.0).show_ui(ui, |ui| {
            for k in kinds {
                let selected = std::mem::discriminant(&k) == std::mem::discriminant(&v.auto);
                if ui.selectable_label(selected, name(&k)).clicked() && !selected { choose_grouper(v, k); out.groups = true; }
            }
        }).response.on_hover_text("Shift-G cycles");
    });
    match &mut v.auto {
        AutoGrouper::Folder { depth } => {
            ui.horizontal(|ui| {
                ui.label(weak(t, "Folder depth"));
                if ui.add(egui::DragValue::new(depth).range(1..=8)).changed() { v.colour_groups = true; out.groups = true; }
            });
        }
        AutoGrouper::Community { resolution, scope } => {
            let mut changed = ui.add(egui::Slider::new(resolution, 0.1..=5.0).logarithmic(true).text("resolution"))
                .on_hover_text("Above 1: more, smaller communities").changed();
            ui.horizontal(|ui| {
                ui.label(weak(t, "Computed on"));
                ui.spacing_mut().item_spacing.x = 2.0;
                changed |= ui.selectable_value(scope, CommunityScope::All, "All links").on_hover_text("Every real link: communities don't move when a filter changes").changed();
                changed |= ui.selectable_value(scope, CommunityScope::Visible, "Visible").on_hover_text("Only what the dailies filter leaves").changed();
            });
            if changed { v.colour_groups = true; out.groups = true; }
        }
        _ => {}
    }
    if v.auto != AutoGrouper::None {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if ui.toggle_value(&mut v.auto_pull, "Pull").on_hover_text("Pull for every automatic group").changed() { out.groups = true; }
            if ui.toggle_value(&mut v.auto_hull, "Hull").on_hover_text("Hulls for every automatic group").changed() { out.groups = true; }
            if app.grouping.community.pending() { ui.label(weak(t, "computing...")); }
        });
        // built once per recompute, and only the rows in sight are laid out:
        // a db with thousands of tags or communities costs a screenful
        let at = (app.view_gen, app.theme_idx);
        if app.panel.autos.as_ref().map(|(k, _)| *k) != Some(at) {
            let rows = app.groups.groups.iter().filter(|g| !g.manual).map(|g| AutoRow {
                key: g.key.clone(), label: g.label.clone(), short: g.label.chars().take(36).collect(),
                colour: rgb(g.colour.rgb(bg)), members: g.members, shown: g.shown,
            }).collect();
            app.panel.autos = Some((at, rows));
        }
        let autos = app.panel.autos.take().map(|(_, r)| r).unwrap_or_default();
        if autos.is_empty() {
            ui.label(weak(t, "Nothing to group here."));
        } else {
            egui::CollapsingHeader::new(weak(t, format!("{} automatic groups", autos.len())))
                .id_salt("navi-auto-groups").default_open(autos.len() <= 12)
                .show(ui, |ui| {
                    let row_h = ui.spacing().interact_size.y;
                    egui::ScrollArea::vertical().id_salt("navi-auto-scroll").max_height(220.0).auto_shrink([false, true])
                        .show_rows(ui, row_h, autos.len(), |ui, range| {
                            for row in &autos[range] {
                                ui.horizontal(|ui| {
                                    ui.set_min_height(row_h);
                                    let mut vis = !v.auto_hidden.contains(&row.key);
                                    if ui.checkbox(&mut vis, "").on_hover_text("Visible").changed() {
                                        if vis {
                                            // every key hiding it (a merge of two hidden clusters has two): one click shows it
                                            let drop = groups.hiding_keys(graph, &row.key, &v.auto_hidden);
                                            v.auto_hidden.retain(|k| !drop.contains(k));
                                        } else { v.auto_hidden.push(row.key.clone()); }
                                        out.groups = true;
                                    }
                                    let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                                    ui.painter().circle_filled(r.center(), 5.0, row.colour);
                                    ui.label(RichText::new(&row.short).color(if vis { t.bar_text } else { t.label }).size(12.5))
                                        .on_hover_ui(|ui| { ui.label(format!("{}\n{} matching, {} on screen", row.label, row.members, row.shown)); });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| { ui.label(weak(t, row.members.to_string())); });
                                });
                            }
                        });
                });
        }
        app.panel.autos = Some((at, autos));
    }
}

/// The panel's "Group by": as Shift-G, a grouper turns the group display on
/// (one you can't see would look like nothing happened).
pub(super) fn choose_grouper(v: &mut View, k: AutoGrouper) {
    v.auto = k;
    if k != AutoGrouper::None { v.colour_groups = true; }
}

/// "Add group" starts from the search being typed, else the last one, as a
/// group query matching what that search matched (text the search only
/// matched as plain text is quoted, so the group isn't a parse error).
fn add_group_query(app: &NaviApp) -> String {
    let typed = if app.search_active && !app.search_query.trim().is_empty() { &app.search_query } else { &app.last_search };
    Query::group_source(typed)
}

/// Append a group from `add_group_query`, with a colour no group has.
pub(super) fn add_group(app: &mut NaviApp, bg: [u8; 3]) {
    let query = add_group_query(app);
    let names: Vec<&str> = app.active_view.groups.iter().map(|g| g.name.as_str()).collect();
    let name = (app.active_view.groups.len() + 1..).map(|k| format!("Group {k}")).find(|s| !names.contains(&s.as_str())).expect("unbounded");
    let mut g = Group::new(&name, &query);
    g.colour = Some(new_group_colour(&app.active_view.groups, &name, bg));
    app.active_view.groups.push(g);
    app.active_view.colour_groups = true; // a group you can't see would look like nothing happened
}

// ── 4. Physics ────────────────────────────────────────────────────────────────

fn physics_section(app: &mut NaviApp, ui: &mut Ui, t: &Theme, out: &mut PanelOut) {
    if ui.add(egui::Slider::new(&mut app.active_view.group_pull, 0.0..=1.0).text("group pull"))
        .on_hover_text("0.1 is a slow drift, 0.5 makes tight clusters").changed() { out.pull = true; }
    ui.label(weak(t, "Pulls groups with Pull on, while groups are shown (G)."));
    let label = if app.graph.physics_on { "Pause physics  (P)" } else { "Resume physics  (P)" };
    if ui.button(label).clicked() { out.physics = true; }
}

// ── 5. Inspector ──────────────────────────────────────────────────────────────

fn inspector_section(app: &mut NaviApp, ui: &mut Ui, t: &Theme, out: &mut PanelOut) {
    let Some(sel) = app.selected.filter(|&i| i < app.graph.node_list.len()) else {
        ui.label(weak(t, "Click a node (or Tab) to inspect it."));
        return;
    };
    let root = app.notes_root();
    let nd   = &app.graph.node_list[sel];
    ui.label(RichText::new(&nd.title).size(15.0).color(t.label_hov));
    let list = |v: &[String], prefix: &str| if v.is_empty() { "none".to_string() } else { v.iter().map(|s| format!("{prefix}{s}")).collect::<Vec<_>>().join(", ") };
    let dangling: Vec<(String, String)> = app.graph.dangling.iter().filter(|d| d.source == nd.id)
        .map(|d| (d.dest.clone(), d.link_type.clone())).collect();
    let view = &app.view;
    egui::Grid::new("navi-inspector").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
        let row = |ui: &mut Ui, k: &str, v: String| {
            ui.label(weak(t, k));
            ui.add(egui::Label::new(RichText::new(v).color(t.bar_text).size(12.5)).wrap());
            ui.end_row();
        };
        row(ui, "File", relative_to_root(&nd.file, root));
        row(ui, "Node", if nd.level == 0 { "File".into() } else { format!("Heading, level {}", nd.level) });
        row(ui, "Tags", list(&nd.tags, "#"));
        row(ui, "Aliases", list(&nd.aliases, ""));
        let vis_deg = view.visible_degree.get(sel).copied().unwrap_or(0);
        row(ui, "Links", format!("{vis_deg} in view, {} in db", nd.degree));
        row(ui, "Orphan", view.orphan_kind(sel).label().to_string());
        if view.daily.get(sel).copied().unwrap_or(false) { row(ui, "Daily", "yes".into()); }
        row(ui, "Dangling", dangling.len().to_string());
    });
    for (dest, kind) in dangling.iter().take(20) {
        let d: String = dest.chars().take(60).collect();
        ui.label(RichText::new(format!("    \u{2022} {d}  ({kind})")).color(t.label).size(12.0));
    }
    if dangling.len() > 20 { ui.label(weak(t, format!("    +{} more", dangling.len() - 20))); }

    // every group, the one that gives the colour marked
    ui.add_space(3.0);
    let mem = app.groups.members.get(sel).cloned().unwrap_or_default();
    if mem.is_empty() {
        ui.label(weak(t, "In no group (theme colour)."));
    } else {
        ui.label(weak(t, "Groups"));
        let primary = app.groups.primary.get(sel).copied().flatten();
        let bg = bg_of(t);
        for gid in mem {
            let g = &app.groups.groups[gid];
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 5.0, rgb(g.colour.rgb(bg)));
                let kind = if g.manual { "" } else { "  (auto)" };
                ui.label(RichText::new(format!("{}{kind}", g.label)).color(if g.visible { t.bar_text } else { t.label }).size(12.5));
                if Some(gid) == primary {
                    let why = if app.active_view.colour_groups { "colour" } else { "colour, when shown" };
                    ui.label(RichText::new(why).color(t.node_sel).size(11.5));
                }
                if !g.visible { ui.label(weak(t, "hidden")); }
            });
        }
    }
    ui.add_space(3.0);
    if ui.button("Open in Emacs  (Enter)").clicked() { out.open = Some(sel); }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The glyphs the panel uses exist in egui's bundled fonts, so the panel
    /// reads the same with no system fonts (Linux / Windows fallbacks vary).
    #[test]
    fn panel_glyphs_are_in_the_default_fonts() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let font = egui::FontId::proportional(13.0);
        let probe = ["\u{1F5D1}", "\u{23F6}", "\u{23F7}", "\u{2022}"];
        let missing: Vec<&str> = probe.iter().copied().filter(|g| !ctx.fonts(|f| f.has_glyphs(&font, g))).collect();
        assert!(missing.is_empty(), "missing glyphs {missing:?}");
    }

    #[test]
    fn visuals_follow_the_theme() {
        for t in crate::theme::THEMES {
            let v = visuals(t);
            assert_eq!(v.panel_fill, t.bar_bg);
            assert_eq!(v.extreme_bg_color, t.bg);
            assert!(v.dark_mode, "every theme today is dark");
        }
    }
}
