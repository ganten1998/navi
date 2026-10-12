//! Headless frames through the real NaviApp::update (no window, no GL):
//! the side panel's keyboard and pointer rules, persistence and pacing.
//! Widgets are found from egui's own record of the last pass.

use super::*;
use egui::{Event, Modifiers, PointerButton, RawInput};
use navi_core::{groups::Group, RawNode};
use std::path::PathBuf;

const W: f32 = 1400.0;
const H: f32 = 900.0;

struct Harness {
    app: NaviApp,
    ctx: egui::Context,
    t:   f64,
    dir: PathBuf,
}

fn node(id: &str, title: &str, file: &str) -> RawNode {
    RawNode { id: id.into(), title: title.into(), file: file.into(), ..Default::default() }
}

impl Harness {
    /// a-b-c, d alone; one manual group "Rust" when `group` is set.
    fn new(name: &str, group: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("navi-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = ViewStore::load(&dir.join("views.json"));
        store.ensure_db("k", None);
        if group { store.active_mut("k").groups.push(Group::new("Rust", "tag:rust")); }
        let graph = Graph::new(
            vec![node("a", "Alpha", "/n/a.org"), node("b", "Beta", "/n/b.org"), node("c", "Gamma", "/n/c.org"), node("d", "Delta", "/n/d.org")],
            vec![("a".into(), "b".into()), ("b".into(), "c".into())],
        );
        let app = NaviApp::new(graph, Config::default(), "/nonexistent/org-roam.db".into(), store, "k".into());
        let mut h = Harness { app, ctx: egui::Context::default(), t: 0.0, dir };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(W, H))),
            time: Some(self.t),
            events,
            ..RawInput::default()
        };
        self.t += 1.0 / 60.0;
        let app = &mut self.app;
        let _ = self.ctx.run(raw, |ctx| app.update(ctx));
    }

    fn key(&mut self, key: Key, modifiers: Modifiers, text: Option<&str>) {
        let mut ev = vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }];
        if let Some(t) = text { ev.push(Event::Text(t.into())); }
        self.frame(ev);
        self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
    }

    fn click(&mut self, pos: Pos2) {
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    }

    /// Widgets of the last pass inside the panel (right of the graph), top to bottom.
    fn panel_widgets(&self) -> Vec<egui::WidgetRect> {
        let right = self.app.graph_rect.right();
        let mut w: Vec<egui::WidgetRect> = self.ctx.viewport(|vp| {
            vp.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter().cloned()).collect()
        });
        w.retain(|r| r.rect.left() >= right && r.rect.is_positive());
        w.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()).then(a.rect.left().total_cmp(&b.rect.left())));
        w
    }

    /// The first text field in the panel (click + drag + focusable).
    fn first_text_field(&self) -> Pos2 {
        self.panel_widgets().into_iter()
            .find(|r| r.sense.click && r.sense.drag && r.sense.focusable && r.rect.height() < 30.0 && r.rect.width() > 40.0)
            .expect("a text field in the panel").rect.center()
    }

    fn saved(&mut self) -> String {
        self.app.shutdown();
        std::fs::read_to_string(self.dir.join("views.json")).unwrap_or_default()
    }
}

impl Drop for Harness {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
}

#[test]
fn backslash_opens_the_panel_and_the_graph_gets_what_is_left() {
    let mut h = Harness::new("open", false);
    assert!((h.app.graph_rect.width() - W).abs() < 1.0, "closed: the graph is full width");
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    assert!(h.app.active_view.panel_open);
    let w = h.app.graph_rect.width();
    assert!(w < W - panel::PANEL_W_MIN + 1.0 && w > W - panel::PANEL_W_MAX - 1.0, "graph width {w}");
    assert!(h.saved().contains("\"panel_open\": true"), "remembered in the view");
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    assert!(!h.app.active_view.panel_open);
    assert!((h.app.graph_rect.width() - W).abs() < 1.0);
}

/// While a panel field has focus no graph shortcut fires; Escape leaves the
/// field (and doesn't quit); after that keys reach the graph again.
#[test]
fn typing_in_the_panel_never_fires_graph_shortcuts() {
    let mut h = Harness::new("typing", true);
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    let field = h.first_text_field();
    h.click(field);
    assert!(h.ctx.wants_keyboard_input(), "the click focused the field");
    for (k, c) in [(Key::D, "d"), (Key::O, "o"), (Key::Q, "q"), (Key::Num2, "2"), (Key::G, "g"), (Key::Slash, "/")] {
        h.key(k, Modifiers::NONE, Some(c));
    }
    assert_eq!(h.app.view_filter.dailies, DailiesMode::Show, "D typed, not toggled");
    assert_eq!(h.app.view_filter.orphans, OrphansMode::Show);
    assert!(!h.app.active_view.colour_groups && !h.app.search_active && !h.app.quit_requested);
    assert_eq!(h.app.active_view.groups[0].name, "Rustdoq2g/", "the keys went into the field");
    h.key(Key::Escape, Modifiers::NONE, None);
    assert!(!h.app.quit_requested, "Escape leaves the field first");
    assert!(!h.ctx.wants_keyboard_input());
    h.key(Key::D, Modifiers::NONE, Some("d"));
    assert_eq!(h.app.view_filter.dailies, DailiesMode::Hide, "the graph has the keyboard again");
    assert!(h.saved().contains("Rustdoq2g/"), "the edit reached views.json");
    h.key(Key::Escape, Modifiers::NONE, None);
    assert!(h.app.quit_requested, "and Escape quits again");
}

/// Tab cycles nodes even with the panel open; egui doesn't take it to focus
/// the panel's first widget.
#[test]
fn tab_stays_with_the_graph() {
    let mut h = Harness::new("tab", true);
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    h.key(Key::Tab, Modifiers::NONE, None);
    assert_eq!(h.app.selected, Some(0));
    assert!(!h.ctx.wants_keyboard_input(), "no panel widget got focus");
    h.key(Key::Tab, Modifiers::NONE, None);
    assert_eq!(h.app.selected, Some(1));
    h.key(Key::D, Modifiers::NONE, Some("d"));
    assert_eq!(h.app.view_filter.dailies, DailiesMode::Hide);
}

/// Clicks, drags and scrolling over the panel never pan, select or zoom.
#[test]
fn the_panel_keeps_the_pointer() {
    let mut h = Harness::new("pointer", false);
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    h.app.graph.physics_on = false;
    let (pan, zoom) = (h.app.pan, h.app.zoom);
    let inside = Pos2::new(W - 40.0, H - 80.0); // empty panel space
    h.app.selected = Some(2);
    h.click(inside);
    assert_eq!(h.app.selected, Some(2), "a click on the panel doesn't clear the selection");
    h.frame(vec![Event::PointerMoved(inside), Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, 120.0), modifiers: Modifiers::NONE }]);
    for _ in 0..30 { h.frame(vec![]); }
    assert_eq!((h.app.pan, h.app.zoom), (pan, zoom), "scrolling over the panel doesn't zoom");
    // a drag that starts on the panel doesn't pan
    let to = Pos2::new(200.0, 200.0);
    h.frame(vec![Event::PointerButton { pos: inside, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
    h.frame(vec![Event::PointerMoved(to)]);
    h.frame(vec![Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    assert_eq!(h.app.pan, pan);
    // the graph still zooms
    let g = h.app.graph_rect.center();
    h.frame(vec![Event::PointerMoved(g), Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, 120.0), modifiers: Modifiers::NONE }]);
    for _ in 0..30 { h.frame(vec![]); }
    assert!(h.app.zoom != zoom, "scrolling over the graph zooms");
}

/// A group dragged by its handle lands where it's dropped.
#[test]
fn dragging_a_group_handle_reorders() {
    let mut h = Harness::new("drag", true);
    h.app.active_view.groups.push(Group::new("Blog", "tag:blog"));
    h.app.active_view.groups.push(Group::new("Misc", "level:0"));
    h.app.groups_changed();
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    h.frame(vec![]);
    // the handles: drag-only widgets
    let grips: Vec<Pos2> = h.panel_widgets().into_iter().filter(|r| r.sense.drag && !r.sense.click && r.rect.width() <= 20.0 && r.rect.height() <= 24.0).map(|r| r.rect.center()).collect();
    assert!(grips.len() >= 3, "{} handles", grips.len());
    let (from, below_last) = (grips[0], grips[2] + Vec2::new(0.0, 40.0));
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
    for k in 1..=10 { h.frame(vec![Event::PointerMoved(from + (below_last - from) * (k as f32 / 10.0))]); }
    assert_eq!(h.app.panel.drag_from, Some(0));
    h.frame(vec![Event::PointerButton { pos: below_last, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    h.frame(vec![]);
    let names: Vec<&str> = h.app.active_view.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, vec!["Blog", "Misc", "Rust"]);
    assert_eq!(h.app.panel.drag_from, None);
    assert_eq!(h.app.groups.groups[2].label, "Rust", "precedence follows at once");
}

#[test]
fn view_management_from_the_panel() {
    let mut h = Harness::new("views", true);
    h.app.apply_view_op(ViewOp::New);
    assert_eq!(h.app.views.views("k").len(), 2);
    assert_eq!(h.app.active_view.name, "View");
    assert!(h.app.active_view.panel_open && h.app.active_view.groups.is_empty());
    h.app.apply_view_op(ViewOp::Rename("Default".into()));
    assert!(h.app.panel.rename_error.is_some(), "taken name refused");
    h.app.apply_view_op(ViewOp::Rename("Writing".into()));
    assert_eq!(h.app.active_view.name, "Writing");
    h.app.apply_view_op(ViewOp::Switch(0));
    assert_eq!((h.app.active_view.name.as_str(), h.app.active_view.groups.len()), ("Default", 1));
    assert!(h.app.active_view.panel_open, "switching from the panel keeps it open");
    h.app.apply_view_op(ViewOp::Duplicate);
    assert_eq!(h.app.active_view.name, "Default copy");
    assert_eq!(h.app.active_view.groups.len(), 1);
    h.app.apply_view_op(ViewOp::Delete);
    assert_eq!(h.app.views.views("k").len(), 2);
    let text = h.saved();
    assert!(text.contains("\"Writing\"") && !text.contains("Default copy"), "{text}");
}

/// The panel never holds the full frame rate by itself.
#[test]
fn an_idle_panel_lets_the_app_idle() {
    let mut h = Harness::new("idle", false);
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    for _ in 0..5 { h.frame(vec![]); }
    h.app.graph.physics_on = false;
    h.app.pan_vel = Vec2::ZERO;
    h.app.last_active_at = Instant::now() - Duration::from_secs(60);
    h.app.set_egui_busy(false);
    assert!(!h.app.needs_full_speed());
    h.app.set_egui_busy(true);
    assert!(!h.app.needs_full_speed(), "an egui repaint request long after input doesn't wake it");
    h.app.last_active_at = Instant::now() - h.app.idle_grace - Duration::from_millis(500);
    assert!(h.app.needs_full_speed(), "an animation started near input may finish");
}

/// Rename puts the keyboard in its field (typing doesn't fire shortcuts),
/// and Escape cancels a rename or a delete prompt instead of quitting.
#[test]
fn rename_takes_the_keyboard_and_escape_cancels() {
    let mut h = Harness::new("rename", false);
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    h.app.panel.rename = Some("Default".into()); // what the Rename button does
    h.app.panel.rename_focus = true;
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(text_field_focused(&h.ctx), "the rename field has the keyboard");
    h.key(Key::O, Modifiers::NONE, Some("o"));
    assert_eq!(h.app.view_filter.orphans, OrphansMode::Show, "O typed, not cycled");
    assert!(h.app.panel.rename.as_deref().is_some_and(|b| b.contains('o')), "{:?}", h.app.panel.rename);
    h.key(Key::Escape, Modifiers::NONE, None);
    assert!(h.app.panel.rename.is_none() && !h.app.quit_requested, "Escape cancels the rename");
    // unfocused (clicked away) rename and the delete prompt: Escape cancels, never quits
    h.app.panel.rename = Some("Default".into());
    h.frame(vec![]);
    h.key(Key::Escape, Modifiers::NONE, None);
    assert!(h.app.panel.rename.is_none() && !h.app.quit_requested);
    h.app.panel.confirm_delete = true;
    h.frame(vec![]);
    h.key(Key::Escape, Modifiers::NONE, None);
    assert!(!h.app.panel.confirm_delete && !h.app.quit_requested);
    assert_eq!(h.app.views.views("k")[0].name, "Default");
}

/// Tab in a panel text field cycles nodes and leaves the field; it never
/// walks egui's focus onto a toggle (where graph keys would die and Space
/// would flip it).
#[test]
fn tab_in_a_text_field_goes_back_to_the_graph() {
    let mut h = Harness::new("tabfield", true);
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    h.click(h.first_text_field());
    assert!(text_field_focused(&h.ctx));
    h.key(Key::Tab, Modifiers::NONE, None);
    assert_eq!(h.app.selected, Some(0), "Tab cycled nodes");
    h.key(Key::Tab, Modifiers::NONE, None);
    assert_eq!(h.app.selected, Some(1));
    assert!(!h.ctx.wants_keyboard_input(), "no panel widget holds the keyboard");
    h.key(Key::D, Modifiers::NONE, Some("d"));
    assert_eq!(h.app.view_filter.dailies, DailiesMode::Hide, "graph keys work");
    assert!(h.app.active_view.groups[0].visible);
}

fn settle(h: &mut Harness) {
    for k in 0..6000 {
        if !h.app.graph.is_layout_animating() && !h.app.graph.physics_on { break; }
        h.app.graph.step(1.0 / 60.0);
        if k % 20 == 0 { h.frame(vec![]); }
    }
    h.frame(vec![]);
    assert!(!h.app.graph.is_layout_animating() && !h.app.graph.physics_on, "never settled");
}

/// Column / Tree hold their layout: turning on groups that pull doesn't
/// start force-directed physics there.
#[test]
fn group_changes_keep_the_tree_layout_still() {
    let mut h = Harness::new("treepull", false);
    let mut g = Group::new("ABG", "Alpha OR Beta OR Gamma");
    g.pull = true;
    h.app.active_view.groups.push(g);
    h.app.active_view.group_pull = 0.5;
    h.app.groups_changed();
    h.key(Key::V, Modifiers::NONE, Some("v"));
    h.key(Key::V, Modifiers::NONE, Some("v"));
    assert_eq!(h.app.layout_mode, 2);
    settle(&mut h);
    h.key(Key::G, Modifiers::NONE, Some("g"));
    assert!(h.app.active_view.colour_groups && h.app.graph.group_pull_active());
    assert!(!h.app.graph.physics_on, "the tree stays put");
    // in Disk the pull does wake physics
    h.key(Key::V, Modifiers::NONE, Some("v"));
    settle(&mut h);
    h.app.active_view.group_pull = 0.3;
    h.app.apply_group_pull();
    assert!(h.app.graph.physics_on);
}

/// Hiding a group is a filter: the layout re-forms like it does for D / O.
#[test]
fn hiding_a_group_reforms_the_layout() {
    let mut h = Harness::new("hidegroup", false);
    h.app.active_view.groups.push(Group::new("G", "title:gamma"));
    h.app.groups_changed();
    settle(&mut h);
    h.app.active_view.groups[0].visible = false;
    h.app.groups_changed();
    assert!(!h.app.graph.is_visible(2), "Gamma hidden");
    assert!(h.app.graph.physics_on, "Disk: physics resumes around the hole");
    // Tree: the layout re-runs for what's left
    h.app.active_view.groups[0].visible = true;
    h.app.groups_changed();
    h.key(Key::V, Modifiers::NONE, Some("v"));
    h.key(Key::V, Modifiers::NONE, Some("v"));
    settle(&mut h);
    h.app.active_view.groups[0].visible = false;
    h.app.groups_changed();
    assert!(h.app.graph.is_layout_animating(), "Tree re-runs");
    // a group change that hides nothing moves nothing
    settle(&mut h);
    h.app.active_view.groups[0].hull = true;
    h.app.groups_changed();
    assert!(!h.app.graph.is_layout_animating() && !h.app.graph.physics_on);
}

/// Search text that doesn't parse still searches, as plain text.
#[test]
fn search_falls_back_to_plain_text() {
    let mut h = Harness::new("search", false);
    h.key(Key::Slash, Modifiers::NONE, Some("/"));
    for (k, c) in [(Key::OpenBracket, "("), (Key::A, "a"), (Key::L, "l")] { h.key(k, Modifiers::NONE, Some(c)); }
    assert_eq!(h.app.search_query, "(al");
    assert!(h.app.search_error.is_some());
    assert_eq!(h.app.search_hits, 0, "no title contains \"(al\"");
    h.app.search_query = "Beta - x".into();
    h.frame(vec![]);
    assert!(h.app.search_error.is_none(), "a lone '-' is a word");
    h.app.search_query = "(beta".into();
    h.frame(vec![]);
    assert!(h.app.search_error.is_some());
    h.app.graph.node_list[1].title = "(Beta) notes".into();
    h.app.view_gen = h.app.view_gen.wrapping_add(1);
    h.frame(vec![]);
    assert_eq!(h.app.search_hits, 1, "matched as plain text");
    h.key(Key::Enter, Modifiers::NONE, None);
    assert_eq!(h.app.selected, Some(1), "Enter jumps to it");
}

/// A refused rename (empty or taken name) keeps the keyboard in the field:
/// the next keys edit the name, never fire D or quit with Q.
#[test]
fn a_refused_rename_keeps_the_keyboard() {
    let mut h = Harness::new("renamerefused", false);
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    h.app.apply_view_op(ViewOp::New);
    h.app.panel.rename = Some("Default".into());
    h.app.panel.rename_focus = true;
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(text_field_focused(&h.ctx));
    h.key(Key::Enter, Modifiers::NONE, None);
    assert!(h.app.panel.rename_error.is_some(), "taken name refused");
    h.frame(vec![]);
    assert!(text_field_focused(&h.ctx), "the field has the keyboard again");
    h.key(Key::D, Modifiers::NONE, Some("d"));
    h.key(Key::Q, Modifiers::NONE, Some("q"));
    assert_eq!(h.app.view_filter.dailies, DailiesMode::Show, "d typed, not toggled");
    assert!(!h.app.quit_requested, "q typed, not quit");
    assert_eq!(h.app.panel.rename.as_deref(), Some("Defaultdq"));
}

/// The active view's own key (or ] with one view) reloads nothing: physics
/// stays asleep, Tree doesn't re-run, a pending prompt stays, nothing saves.
#[test]
fn the_active_views_key_reloads_nothing() {
    let mut h = Harness::new("samekey", false);
    settle(&mut h);
    h.app.panel.confirm_delete = true;
    h.app.views_dirty_at = None;
    h.key(Key::Num1, Modifiers::NONE, Some("1"));
    assert!(!h.app.graph.physics_on, "Disk: physics stays asleep");
    assert!(h.app.panel.confirm_delete, "the prompt stays");
    assert!(h.app.views_dirty_at.is_none(), "nothing to save");
    h.app.panel.confirm_delete = false;
    h.key(Key::V, Modifiers::NONE, Some("v"));
    h.key(Key::V, Modifiers::NONE, Some("v"));
    settle(&mut h);
    h.key(Key::CloseBracket, Modifiers::NONE, Some("]"));
    assert!(!h.app.graph.is_layout_animating(), "Tree doesn't re-run");
    assert!(h.app.popup.as_ref().is_some_and(|(m, _)| m.contains("(1/1)")), "it still says where you are");
}

/// A save that fails stays pending and is written once it can be (here at
/// exit), not dropped with the error.
#[test]
fn a_failed_save_is_retried() {
    let mut h = Harness::new("saveretry", false);
    let path = h.dir.join("views.json");
    let _ = std::fs::remove_file(&path);
    std::fs::create_dir_all(&path).unwrap(); // a directory where the file goes: every save fails
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    h.app.shutdown();
    assert!(h.app.views_dirty_at.is_some(), "still pending after the failure");
    std::fs::remove_dir_all(&path).unwrap();
    h.app.shutdown();
    assert!(h.app.views_dirty_at.is_none());
    assert!(std::fs::read_to_string(&path).unwrap().contains("\"panel_open\": true"), "the edit reached views.json");
}

/// Dragging a group's colour (or typing its name) patches colours in place:
/// no re-resolve per step, and the graph shows the new colour.
#[test]
fn a_colour_edit_does_not_recompute_the_view() {
    let mut h = Harness::new("restyle", false);
    h.app.active_view.groups.push(Group::new("AB", "Alpha OR Beta"));
    h.app.active_view.colour_groups = true;
    h.app.groups_changed();
    h.frame(vec![]);
    let gen = h.app.view_gen;
    h.app.active_view.groups[0].colour = Some([1, 2, 3]);
    h.app.active_view.groups[0].name = "Renamed".into();
    h.app.apply_panel(PanelOut { group_style: true, ..PanelOut::default() });
    h.frame(vec![]);
    assert_eq!(h.app.view_gen, gen, "no recompute");
    assert_eq!(h.app.node_colours[0], Some(Color32::from_rgb(1, 2, 3)), "the new colour is on the graph");
    assert_eq!(h.app.groups.groups[0].label, "Renamed");
    assert!(h.saved().contains("Renamed"), "and saved");
}

/// "Add group" from a search that only matched as plain text makes a group
/// that parses and matches the same notes.
#[test]
fn add_group_from_a_plain_text_search_matches_the_same_notes() {
    let mut h = Harness::new("addgroup", false);
    h.app.graph.node_list[1].title = "Beta (draft".into();
    h.app.last_search = "beta (draft".into();
    panel::add_group(&mut h.app, [0, 0, 0]);
    h.app.groups_changed();
    let g = h.app.active_view.groups.last().expect("Add group clicked");
    assert_eq!(g.query, "\"beta (draft\"");
    let info = &h.app.groups.groups[h.app.active_view.groups.len() - 1];
    assert!(info.error.is_none() && info.members == 1, "{info:?}");
}

/// The automatic groups list lays out only the rows in sight: hundreds of
/// tags cost a screenful of widgets.
#[test]
fn a_long_auto_group_list_builds_only_visible_rows() {
    let mut h = Harness::new("autolist", false);
    let nodes: Vec<RawNode> = (0..400).map(|i| RawNode { tags: vec![format!("t{i:03}")], ..node(&format!("n{i}"), &format!("N{i}"), &format!("/n/n{i}.org")) }).collect();
    h.app.graph = Graph::new(nodes, vec![]);
    h.app.active_view.auto = AutoGrouper::Tag;
    h.app.active_view.colour_groups = true;
    h.app.groups_changed();
    h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
    // open every collapsed header in the panel (the list starts closed past 12)
    let mut opened = 0;
    for w in h.panel_widgets() {
        if let Some(mut st) = egui::collapsing_header::CollapsingState::load(&h.ctx, w.id) {
            if !st.is_open() { st.set_open(true); st.store(&h.ctx); opened += 1; }
        }
    }
    for _ in 0..3 { h.frame(vec![]); }
    assert!(opened > 0, "the list was opened");
    let n = h.panel_widgets().len();
    assert_eq!(h.app.groups.groups.len(), 400);
    assert!(n < 200, "{n} widgets for 400 groups");
}

/// A rename or delete prompt left open when the panel closes is dropped:
/// Escape with the panel closed quits, first time.
#[test]
fn a_prompt_hidden_with_the_panel_never_takes_escape() {
    for rename in [false, true] {
        let mut h = Harness::new(if rename { "hidden-rename" } else { "hidden-delete" }, false);
        h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
        if rename { h.app.panel.rename = Some("Default".into()); } else { h.app.panel.confirm_delete = true; }
        h.frame(vec![]);
        h.key(Key::Backslash, Modifiers::NONE, Some("\\"));
        assert!(!h.app.active_view.panel_open);
        assert!(h.app.panel.rename.is_none() && !h.app.panel.confirm_delete, "closing the panel drops its prompt");
        h.key(Key::Escape, Modifiers::NONE, None);
        assert!(h.app.quit_requested, "Escape quits");
    }
}

/// "Add group" with no search makes a group that matches nothing yet: the
/// graph keeps its colours, and hiding the group hides nothing.
#[test]
fn add_group_with_no_search_takes_nothing() {
    let mut h = Harness::new("addempty", false);
    h.app.active_view.auto = AutoGrouper::Component;
    h.app.active_view.colour_groups = true;
    h.app.groups_changed();
    h.frame(vec![]);
    let before = h.app.node_colours.clone();
    assert!(h.app.last_search.is_empty() && !h.app.search_active);
    panel::add_group(&mut h.app, [0, 0, 0]);
    h.app.groups_changed();
    h.frame(vec![]);
    assert_eq!(h.app.active_view.groups[0].query, "");
    assert_eq!(h.app.groups.groups[0].members, 0);
    assert_eq!(h.app.node_colours, before, "no node takes the new group's colour");
    h.app.active_view.groups[0].visible = false;
    h.app.groups_changed();
    h.frame(vec![]);
    assert_eq!(h.app.view.visible.iter().filter(|&&v| v).count(), 4, "nothing hidden");
}

/// The panel's "Group by", like Shift-G, turns the group display on.
#[test]
fn choosing_a_grouper_in_the_panel_shows_groups() {
    let mut v = View::named("V");
    assert!(!v.colour_groups);
    panel::choose_grouper(&mut v, AutoGrouper::Tag);
    assert_eq!((v.auto, v.colour_groups), (AutoGrouper::Tag, true));
    v.colour_groups = false;
    panel::choose_grouper(&mut v, AutoGrouper::None);
    assert!(!v.colour_groups, "None leaves it as it is");
}

/// The controls overlay fits the window: a narrow one narrows it, and no
/// key label starts left of the window.
#[test]
fn the_controls_overlay_fits_a_narrow_window() {
    for w in [395.0_f32, 400.0, 735.0, 1400.0] {
        let r = Rect::from_min_size(Pos2::new(10.0, 0.0), Vec2::new(w, 800.0));
        let (x, pw, val) = help_columns(r);
        assert!(x >= r.left() && x + pw <= r.right() + 0.5, "{w}: {x} + {pw}");
        assert!(val < pw && val > 100.0, "{w}: value column {val}");
    }
    assert_eq!(help_columns(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 800.0))).1, 510.0);
}

/// views.json edited by hand while navi runs: the working view takes the
/// edit, and navi's next save keeps it.
#[test]
fn a_hand_edit_of_views_json_reaches_the_graph() {
    let mut h = Harness::new("handedit", false);
    h.app.views_dirty_at = Some(Instant::now());
    h.app.flush_views(true);
    let path = h.dir.join("views.json");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("\"groups\": []").count(), 1, "{text}");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&path, text.replace("\"groups\": []", r#""groups": [ { "name": "Hand", "query": "Alpha" } ]"#)).unwrap();
    h.app.views_polled_at = Instant::now() - VIEWS_POLL;
    h.frame(vec![]);
    assert_eq!(h.app.active_view.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["Hand"]);
    assert_eq!(h.app.groups.groups[0].members, 1, "and the graph shows it");
    h.key(Key::D, Modifiers::NONE, Some("d"));
    let saved = h.saved();
    assert!(saved.contains("\"Hand\"") && saved.contains("\"dailies\": \"hide\""), "{saved}");
}
