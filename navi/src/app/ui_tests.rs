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
