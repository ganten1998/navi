use egui::{Color32, Key, Pos2, Rect, Sense, Stroke, Vec2, epaint::FontId};
use navi_core::{
    config::Config,
    groups::{AutoGrouper, GroupResolution, GroupSpec, Grouping},
    load_graph, DailiesMode, EmacsClient, EvalCtx, Graph, GraphData, OrphansMode, Query,
    QueryError, View, ViewFilter, ViewState, ViewStore,
};
use std::time::{Duration, Instant};

use crate::painter::{GraphPainter, HullDraw};
use crate::theme::{Theme, THEMES};

mod panel;
use panel::{PanelOut, PanelState, ViewOp};
#[cfg(test)]
mod ui_tests;

const ZOOM_MIN: f32 = 0.04;
const ZOOM_MAX: f32 = 20.0;
const ZOOM_STEP: f32 = 1.12;
const PAN_DECAY: f32 = 0.92;
const BASE_DT: f32 = 1.0 / 60.0;
const STATUS_H: f32 = 30.0;
const SAVE_DEBOUNCE: Duration = Duration::from_millis(600); // views.json writes after the last edit
const VIEWS_POLL:    Duration = Duration::from_millis(700); // how often views.json is checked for a hand edit
const DB_POLL:       Duration = Duration::from_millis(1000); // how often org-roam.db is checked for a write


pub struct NaviApp {
    graph: Graph,
    cfg: Config,
    db_path: String,
    emacs: EmacsClient,

    pan: Vec2,
    zoom: f32,
    pan_vel: Vec2,

    // Node indices into graph.node_list (ids only at the reload boundary).
    hovered: Option<usize>,
    selected: Option<usize>,

    pan_dragging: bool,
    pan_origin_m: Pos2,
    pan_origin_v: Vec2,
    drag_samples: Vec<(Instant, f32, f32)>,

    node_dragging: Option<usize>,
    node_drag_origin_w: (f64, f64),
    node_drag_origin_s: Pos2,
    node_drag_samples: Vec<(Instant, f64, f64)>,

    last_click: Option<(usize, Instant)>,

    // ── views (views.json): the active one's filters + groups ──
    views:       ViewStore,
    db_key:      String,                                // this db's key in views.json
    active_view: View,                                  // working copy of the active view
    views_dirty_at: Option<Instant>,                    // unsaved edits (debounced save)
    views_polled_at: Instant,                           // views.json last checked for an outside edit

    // ── side panel (backslash) ──
    panel:         PanelState,
    kb_focus_prev: bool,                                // a panel widget had keyboard focus last frame
    tab_to_graph:  bool,                                // Tab cycled nodes: keep egui from focusing the panel
    egui_busy:     bool,                                // egui asked for an immediate repaint (animation, drag)
    visuals_for:   Option<usize>,                       // theme the egui visuals were built for
    root_depth:    Option<(u64, usize)>,                // (view_gen, notes root depth) for the inspector
    last_search:   String,                              // last search committed (prefills "Add group")

    // ── view pipeline: recomputed only when the filter, groups or graph change ──
    view_filter: ViewFilter,
    view:        ViewState,
    view_dirty:  bool,
    view_gen:    u64,                                   // bumps on every recompute
    grouping:    Grouping,                              // colour memory, partitions, community worker
    group_spec:  GroupSpec,                             // the active view's groups, queries compiled
    groups:      GroupResolution,
    node_colours: Vec<Option<Color32>>,                 // group colour per node (None = theme colour)
    hulls:       Vec<HullDraw>,
    colours_key: Option<(u64, usize, bool)>,            // (view_gen, theme, colouring on)
    faded:       Vec<bool>,                             // search / local-graph fade
    faded_key:   Option<(String, usize, Option<usize>, u64)>,
    search_error: Option<QueryError>,                   // parse error of the query being typed
    search_hits: usize,
    hov_adj:     Vec<bool>,                             // hovered node + visible neighbours
    hov_key:     Option<(Option<usize>, u64)>,
    local_hops: usize,
    show_age: bool,
    show_fps: bool,
    layout_mode: u8, // 0 = Disk, 1 = Column, 2 = Tree
    theme_idx: usize,
    popup: Option<(String, f64)>, // transient notification (theme, layout, …)

    search_active: bool,
    search_query: String,
    search_swallow_slash: bool,

    help_anim: f32,
    help_visible: bool,

    status_msg: Option<(String, Instant, Duration)>,
    reload_pending: Option<std::sync::mpsc::Receiver<Result<GraphData, String>>>,

    frame_times:         std::collections::VecDeque<f32>,
    last_frame:          Instant,
    last_prof_log:       Option<Instant>,
    last_update_end:     Option<Instant>,
    post_overhead:       Duration,
    now_secs:            f64,
    physics_accumulator: f64,

    // Idle / focus tracking (drives the 240↔30 fps cadence in main.rs).
    window_focused:      bool,
    last_active_at:      Instant,
    idle_grace:          Duration,

    // Cached graph_rect from last frame for coordinate math during input
    graph_rect: Rect,
    // The pre-frame fit in new() assumes a 1400×870 viewport; tiling WMs
    // (i3, AeroSpace) hand us an arbitrary size on map, so we re-fit once we
    // see the real rect.
    initial_fit_done: bool,

    // Q/Escape sets this; main.rs routes it through event_loop.exit() so GL
    // teardown runs (a raw process::exit skips Drop and can upset drivers).
    quit_requested: bool,

    // ── Emacs (`--emacs`: started by org-roam-navi.el, talking over stdio) ──
    emacs_mode:   bool,                                 // open nodes over the pipe, not emacsclient
    emacs_theme:  Option<Theme>,                        // Emacs's colours, the theme after THEMES
    glide_to:     Option<usize>,                        // node the view is gliding to (a `show`)
    pending_show: Option<(Option<String>, Option<String>)>, // a `show` waiting on a reload
    db_stamp:     Option<Vec<Option<std::time::SystemTime>>>, // mtimes of the db and its -wal
    db_polled_at: Instant,
    reload_quiet: bool,                                 // a reload navi started itself: no status chatter
    quiet_failures: u32,                                // quiet reloads failed in a row
}

impl NaviApp {
    pub fn new(graph: Graph, cfg: Config, db_path: String, mut views: ViewStore, db_key: String) -> Self {
        let emacs = EmacsClient::new(&cfg.emacsclient, &cfg.server_name);
        let show_fps = cfg.show_fps;
        let active_view = views.active(&db_key);
        let view_filter = active_view.filter(&cfg.dailies_dirs);
        let group_spec  = active_view.spec();
        let mut grouping = Grouping::default();
        grouping.memory.slots = views.auto_colours(&db_key); // the db's, shared by its views
        let mut app = NaviApp {
            graph,
            cfg,
            db_path,
            emacs,
            pan: Vec2::ZERO,
            zoom: 1.0,
            pan_vel: Vec2::ZERO,
            hovered: None,
            selected: None,
            pan_dragging: false,
            pan_origin_m: Pos2::ZERO,
            pan_origin_v: Vec2::ZERO,
            drag_samples: Vec::new(),
            node_dragging: None,
            node_drag_origin_w: (0.0, 0.0),
            node_drag_origin_s: Pos2::ZERO,
            node_drag_samples: Vec::new(),
            last_click: None,
            views,
            db_key,
            active_view,
            views_dirty_at: None,
            views_polled_at: Instant::now(),
            panel: PanelState::default(),
            kb_focus_prev: false,
            tab_to_graph: false,
            egui_busy: false,
            visuals_for: None,
            root_depth: None,
            last_search: String::new(),
            view_filter,
            view: ViewState::default(),
            view_dirty: true,
            view_gen: 0,
            grouping,
            group_spec,
            groups: GroupResolution::default(),
            node_colours: Vec::new(),
            hulls: Vec::new(),
            colours_key: None,
            faded: Vec::new(),
            faded_key: None,
            search_error: None,
            search_hits: 0,
            hov_adj: Vec::new(),
            hov_key: None,
            local_hops: 0,
            show_age: true,
            show_fps,
            layout_mode: 0,
            theme_idx: 0,
            popup: None,
            search_active: false,
            search_query: String::new(),
            search_swallow_slash: false,
            help_anim: 0.0,
            help_visible: false,
            status_msg: None,
            reload_pending: None,
            frame_times:         std::collections::VecDeque::new(),
            last_frame:          Instant::now(),
            last_prof_log:       None,
            last_update_end:     None,
            post_overhead:       Duration::from_micros(500),
            // Default: stay at full speed for 10 s after the last activity, then
            // drop to the idle (~30 fps) tier. Override with NAVI_IDLE_GRACE_SECS.
            window_focused:      true,
            last_active_at:      Instant::now(),
            idle_grace:          Duration::from_secs_f64(
                std::env::var("NAVI_IDLE_GRACE_SECS").ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(10.0),
            ),
            now_secs:            unix_now(),
            physics_accumulator: 0.0,
            graph_rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 870.0)),
            initial_fit_done: false,
            quit_requested: false,
            emacs_mode: false,
            emacs_theme: None,
            glide_to: None,
            pending_show: None,
            db_stamp: None,
            db_polled_at: Instant::now(),
            reload_quiet: false,
            quiet_failures: 0,
        };
        app.db_stamp = Some(app.db_mtimes());
        // main.rs built the graph with Graph::from_data_with_view, so the
        // visible subset of a persisted filter is already laid out on its own.
        app.pin_root();
        app.refresh_view();
        // positions_disk() already pre-relaxes internally, so nodes are at
        // equilibrium.  Resume physics with an ease-in so nothing snaps.
        app.graph.resume_physics();
        app.fit_to_nodes();
        app
    }

    // ── view pipeline ─────────────────────────────────────────────────────────

    /// Recompute the view if the filter, groups or graph changed; push it
    /// into the graph (physics/layouts, group pull) and drop interaction
    /// state on hidden nodes. Groups are resolved inside (see groups.rs:
    /// hidden groups filter before orphan classification).
    /// True when the set of visible nodes changed (the caller decides whether
    /// the layout re-forms around it).
    fn refresh_view(&mut self) -> bool {
        if self.grouping.community.poll() { self.view_dirty = true; } // a community partition landed
        if !self.view_dirty { return false; }
        self.view_dirty = false;
        let (view, groups) = self.grouping.compute(&self.graph, &self.view_filter, &self.group_spec);
        let changed = view.visible != self.view.visible;
        self.view   = view;
        self.groups = groups;
        self.graph.apply_view(&self.view);
        self.apply_group_pull();
        // auto-group colours persist with the db (only when new keys appeared)
        if self.views.set_auto_colours(&self.db_key, &self.grouping.memory.slots) { self.views_dirty_at = Some(Instant::now()); }
        self.view_gen = self.view_gen.wrapping_add(1);
        let vis = |i: &usize| self.graph.is_visible(*i);
        self.selected = self.selected.filter(vis);
        self.hovered  = self.hovered.filter(vis);
        if self.node_dragging.is_some_and(|i| !self.graph.is_visible(i)) { self.node_dragging = None; }
        if self.last_click.as_ref().is_some_and(|(i, _)| !self.graph.is_visible(*i)) { self.last_click = None; }
        changed
    }

    /// Group pull follows the group display (G): off, nothing pulls. Column
    /// and Tree hold their layout: a group change never starts physics there
    /// (the pull applies once physics runs, P).
    fn apply_group_pull(&mut self) {
        let lists = if self.active_view.colour_groups { self.groups.pull_lists(&self.view.visible) } else { Vec::new() };
        let asleep = !self.graph.physics_on;
        self.graph.set_group_pull(lists, self.active_view.group_pull);
        if asleep && self.layout_mode != 0 { self.graph.physics_on = false; }
    }

    /// Write the working view back into the store; views.json is saved
    /// SAVE_DEBOUNCE after the last edit (typing a query doesn't write per key).
    fn store_view(&mut self) {
        *self.views.active_mut(&self.db_key) = self.active_view.clone();
        self.views_dirty_at = Some(Instant::now());
    }

    /// Save views.json when an edit is pending and has settled (or now, `force`).
    /// A failed save stays pending: a passing failure (a file another program
    /// holds, a full disk) is retried after SAVE_DEBOUNCE and at exit. Only a
    /// views.json that couldn't be read at startup is never retried.
    fn flush_views(&mut self, force: bool) {
        let Some(at) = self.views_dirty_at else { return };
        if !force && at.elapsed() < SAVE_DEBOUNCE { return; }
        match self.views.save() {
            Ok(outside) => {
                self.views_dirty_at = None;
                if outside { self.adopt_outside_edit(); }
            }
            Err(e) => {
                self.views_dirty_at = self.views.writable().then(Instant::now);
                self.set_msg(format!("Couldn't save {}: {e}", self.views.path.display()), Duration::from_secs(6));
            }
        }
    }

    /// views.json changed outside navi (a hand edit, another window): take it
    /// in now, before an edit here is saved over it. A stat every VIEWS_POLL.
    fn poll_views_file(&mut self) {
        if self.views_polled_at.elapsed() < VIEWS_POLL { return; }
        self.views_polled_at = Instant::now();
        if !self.views.changed_on_disk() { return; }
        match self.views.sync() {
            Ok(true) => self.adopt_outside_edit(),
            Ok(false) => {}
            Err(e) => eprintln!("navi: couldn't read {}: {e}", self.views.path.display()),
        }
    }

    /// The store took an outside edit of this db's views: the working view,
    /// its filter, groups and the colour memory follow, and the graph shows
    /// them (a prompt or drag in the panel is dropped, as on a view switch).
    fn adopt_outside_edit(&mut self) {
        self.active_view = self.views.active(&self.db_key);
        self.view_filter = self.active_view.filter(&self.cfg.dailies_dirs);
        self.group_spec  = self.active_view.spec();
        self.grouping.memory.slots = self.views.auto_colours(&self.db_key);
        self.panel = PanelState::default();
        self.colours_key = None;
        self.popup = Some(("views.json changed: view reloaded".into(), self.now_secs));
        self.view_dirty = true;
        if self.refresh_view() { self.reform_layout(); }
    }

    /// The notes root: the one remembered for this db while every note is
    /// still under it (a folder that empties never re-keys folder groups or
    /// re-aims `dir:`), remembered again when it changes.
    fn pin_root(&mut self) {
        self.graph.root_pin = self.views.notes_root(&self.db_key);
        if let Some(root) = self.graph.notes_root_path() {
            if self.views.set_notes_root(&self.db_key, &root) { self.views_dirty_at = Some(Instant::now()); }
        }
        self.root_depth = None;
    }

    /// The window is closing (main.rs `exiting`): write any pending edit.
    pub fn shutdown(&mut self) { self.flush_views(true); }

    /// The active view's groups changed (G, Shift+G, the panel). Hiding a
    /// group is a filter: when nodes came or went, the layout re-forms.
    fn groups_changed(&mut self) {
        self.group_spec = self.active_view.spec();
        self.store_view();
        self.view_dirty = true;
        if self.refresh_view() { self.reform_layout(); }
    }

    /// The active view's groups changed in colour or name only (a colour
    /// drag, typing a name): patched in place, since neither moves a node
    /// into or out of a group; no re-resolve per drag step or key.
    fn group_restyled(&mut self) {
        self.group_spec.manual = self.active_view.groups.clone(); // queries unchanged: `compiled` still matches
        if !self.groups.restyle_manual(&self.active_view.groups) { return self.groups_changed(); }
        self.colours_key = None; // node colours, hull colours and labels
        self.store_view();
    }

    /// Switch to view `idx` of this db (1-9, [ / ], the panel).
    fn switch_view(&mut self, idx: usize) {
        let n = self.views.views(&self.db_key).len();
        if idx >= n {
            self.popup = Some((format!("No view {}  ({n} saved)", idx + 1), self.now_secs));
            return;
        }
        // its own key (or [ / ] with one view): say where you are, reload
        // nothing (no layout re-run, physics stays asleep, prompts stay)
        if idx == self.views.active_index(&self.db_key) {
            self.popup = Some((format!("View  {}  ({}/{n})", self.active_view.name, idx + 1), self.now_secs));
            return;
        }
        self.store_view(); // the working copy, before it's replaced
        self.views.set_active(&self.db_key, idx);
        self.load_active_view();
    }

    /// The store's active view became a different one (switch, new,
    /// duplicate, delete): make it the working copy and show it.
    fn load_active_view(&mut self) {
        self.active_view = self.views.active(&self.db_key);
        self.view_filter = self.active_view.filter(&self.cfg.dailies_dirs);
        self.group_spec  = self.active_view.spec();
        self.grouping.forget_partitions(); // keys from what's remembered, not the last view's partition
        self.panel = PanelState::default();
        let n = self.views.views(&self.db_key).len();
        let k = self.views.active_index(&self.db_key);
        self.popup = Some((format!("View  {}  ({}/{n})", self.active_view.name, k + 1), self.now_secs));
        self.filter_changed();
        self.flush_views(true);
    }

    /// New / duplicate / rename / delete / switch from the panel.
    fn apply_view_op(&mut self, op: ViewOp) {
        self.store_view();
        let key = self.db_key.clone();
        match op {
            ViewOp::Switch(k) => {
                // switching from the panel keeps the panel in front of you
                if let Some(v) = self.views.dbs.get_mut(&key).and_then(|d| d.views.get_mut(k)) { v.panel_open = true; }
                self.switch_view(k);
            }
            ViewOp::New => {
                let v = View { panel_open: true, ..View::named("View") };
                self.views.add_view(&key, v);
                self.load_active_view();
            }
            ViewOp::Duplicate => {
                let cur = self.views.active_index(&key);
                self.views.duplicate_view(&key, cur);
                self.load_active_view();
            }
            ViewOp::Rename(name) => {
                let cur = self.views.active_index(&key);
                match self.views.rename_view(&key, cur, &name) {
                    Ok(()) => {
                        self.active_view.name = self.views.active(&key).name;
                        self.panel.rename = None;
                        self.panel.rename_error = None;
                        self.flush_views(true);
                    }
                    Err(e) => {
                        // Enter (or the button) took the keyboard from the
                        // field: give it back, or the next keys fire shortcuts
                        self.panel.rename_error = Some(e);
                        self.panel.rename_focus = true;
                    }
                }
            }
            ViewOp::Delete => {
                let cur = self.views.active_index(&key);
                if self.views.delete_view(&key, cur) {
                    if let Some(d) = self.views.dbs.get_mut(&key) { let i = d.active_index(); d.views[i].panel_open = true; }
                    self.load_active_view();
                }
            }
        }
    }

    /// Apply what the side panel changed this frame (before the graph paints).
    fn apply_panel(&mut self, out: PanelOut) {
        if out.panel { self.toggle_panel(); }
        if out.filter { self.filter_changed(); }
        if out.groups { self.groups_changed(); } else if out.group_style { self.group_restyled(); }
        if out.pull {
            self.store_view();
            self.apply_group_pull();
        }
        if out.physics { self.toggle_physics(); }
        if let Some(i) = out.open {
            self.open_node(i);
            self.popup = Some(("Opening in Emacs…".into(), self.now_secs));
        }
        if let Some(op) = out.view_op { self.apply_view_op(op); }
    }

    fn toggle_panel(&mut self) {
        self.active_view.panel_open = !self.active_view.panel_open;
        self.panel.drag_from = None;
        if !self.active_view.panel_open {
            // a rename or delete prompt you can't see would take the next Escape
            self.panel.rename = None;
            self.panel.rename_error = None;
            self.panel.confirm_delete = false;
        }
        self.store_view();
    }

    fn toggle_physics(&mut self) {
        if self.graph.physics_on {
            self.graph.physics_on = false;
            self.popup = Some(("Physics  Paused".into(), self.now_secs));
        } else {
            self.graph.resume_physics();
            self.popup = Some(("Physics  Active".into(), self.now_secs));
        }
    }

    /// Notes root depth (for paths relative to it), cached per view recompute.
    fn notes_root(&mut self) -> usize {
        if let Some((gen, d)) = self.root_depth { if gen == self.view_gen { return d; } }
        let d = self.graph.notes_root_depth();
        self.root_depth = Some((self.view_gen, d));
        d
    }

    /// A filter changed the view: persist it and let the layout re-form
    /// around what's left (Disk resumes physics; Column/Tree re-run in place).
    fn filter_changed(&mut self) {
        self.active_view.dailies = self.view_filter.dailies;
        self.active_view.orphans = self.view_filter.orphans;
        self.active_view.links_to_dailies_count = self.view_filter.links_to_dailies_count;
        self.store_view();
        self.view_dirty = true;
        self.refresh_view();
        self.reform_layout();
    }

    /// What's on screen changed: Disk resumes physics, Column / Tree re-run
    /// in place.
    fn reform_layout(&mut self) {
        match self.layout_mode {
            1 => { let t = self.graph.positions_column(); self.graph.begin_layout_transition(t, false); }
            2 => { let t = self.graph.positions_tree();   self.graph.begin_layout_transition(t, false); }
            _ => { self.graph.physics_on = true; }
        }
    }

    /// Search + local-graph fade mask; rebuilt only when its inputs change.
    /// The search box speaks the query language (query.rs); while the text
    /// doesn't parse it searches it as plain text, the old way, and the
    /// status bar shows why.
    fn refresh_faded(&mut self) {
        let key = (self.search_query.clone(), self.local_hops, self.selected, self.view_gen);
        if self.faded_key.as_ref() == Some(&key) { return; }
        let n = self.graph.node_list.len();
        let (parsed, error) = Query::search(&self.search_query);
        self.search_error = error;
        let query = Some(parsed).filter(|q| !q.is_empty());
        let cx = EvalCtx::from_view(&self.graph, &self.view);
        let local = match (self.local_hops, self.selected) {
            (h, Some(sel)) if h > 0 => Some(self.graph.bfs_visible(sel, h)),
            _ => None,
        };
        let mut hits = 0;
        self.faded = (0..n).map(|i| {
            if !self.graph.is_visible(i) { return false; }
            let in_search = match &query { Some(q) => q.matches(&cx, i), None => true };
            if in_search { hits += 1; }
            let in_local = match &local { Some(l) => l[i], None => true };
            !in_search || !in_local
        }).collect();
        self.search_hits = hits;
        self.faded_key = Some(key);
    }

    /// Group colour per node and the hulls to draw; rebuilt when the view,
    /// the theme or G changes.
    fn refresh_colours(&mut self) {
        let key = (self.view_gen, self.theme_idx, self.active_view.colour_groups);
        if self.colours_key == Some(key) { return; }
        self.colours_key = Some(key);
        let on = self.active_view.colour_groups;
        let bg = self.theme().bg;
        let bg = [bg.r(), bg.g(), bg.b()];
        let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
        self.node_colours = self.groups.primary.iter()
            .map(|p| p.filter(|_| on).map(|g| rgb(self.groups.groups[g].colour.rgb(bg)))).collect();
        self.hulls = if on {
            self.groups.member_lists(&self.view.visible).into_iter().zip(&self.groups.groups)
                .filter(|(m, g)| g.hull && g.visible && !m.is_empty())
                .map(|(members, g)| HullDraw::new(rgb(g.colour.rgb(bg)), g.label.clone(), members))
                .collect()
        } else { Vec::new() };
    }

    fn refresh_hov_adj(&mut self) {
        let key = (self.hovered, self.view_gen);
        if self.hov_key == Some(key) { return; }
        self.hov_adj = vec![false; self.graph.node_list.len()];
        if let Some(h) = self.hovered {
            self.hov_adj[h] = true;
            for &nb in &self.graph.vis_adj[h] { self.hov_adj[nb] = true; }
        }
        self.hov_key = Some(key);
    }

    fn theme(&self) -> Theme {
        match (THEMES.get(self.theme_idx), self.emacs_theme) {
            (Some(t), _) => *t,
            (None, Some(t)) => t,
            (None, None) => THEMES[0],
        }
    }

    // ── coordinate math (uses cached graph_rect) ──────────────────────────────

    fn w2s(&self, wx: f64, wy: f64) -> Pos2 {
        let cx = self.graph_rect.center().x;
        let cy = self.graph_rect.center().y;
        Pos2::new(cx + wx as f32 * self.zoom + self.pan.x,
                  cy + wy as f32 * self.zoom + self.pan.y)
    }

    fn s2w(&self, sx: f32, sy: f32) -> (f64, f64) {
        let cx = self.graph_rect.center().x;
        let cy = self.graph_rect.center().y;
        (((sx - cx - self.pan.x) / self.zoom) as f64,
         ((sy - cy - self.pan.y) / self.zoom) as f64)
    }

    fn node_at(&self, sx: f32, sy: f32) -> Option<usize> {
        let mut best_d = f32::INFINITY;
        let mut best: Option<usize> = None;
        for &i in &self.graph.vis_idx {
            let nd = &self.graph.node_list[i];
            let sc = self.w2s(nd.x, nd.y);
            let d = (sc - Pos2::new(sx, sy)).length();
            let hit_r = (nd.radius * self.zoom).max(9.0);
            if d <= hit_r && d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        best
    }

    fn fit_to_nodes(&mut self) {
        self.glide_to = None;
        let nds = &self.graph.node_list;
        let vis = &self.graph.vis_idx;
        if vis.is_empty() { return; }
        let xs: Vec<f32> = vis.iter().map(|&i| nds[i].x as f32).collect();
        let ys: Vec<f32> = vis.iter().map(|&i| nds[i].y as f32).collect();
        let min_x = xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_x = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let min_y = ys.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let pad = navi_core::graph::R_MAX * 3.0;
        let world_w = (max_x - min_x + pad * 2.0).max(1.0);
        let world_h = (max_y - min_y + pad * 2.0).max(1.0);
        let vw = self.graph_rect.width().max(1.0);
        let vh = self.graph_rect.height().max(1.0);
        self.zoom = ((vw / world_w).min(vh / world_h) * 0.70).clamp(ZOOM_MIN, ZOOM_MAX);
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        self.pan = Vec2::new(-cx * self.zoom, -cy * self.zoom);
    }

    fn open_node(&mut self, idx: usize) {
        let Some(nd) = self.graph.node_list.get(idx) else { return };
        // started by Emacs: hand it the node over the pipe; that Emacs opens
        // it, so neither a server nor an emacsclient path is needed
        if self.emacs_mode {
            emit_to_emacs(&navi_core::bridge::open_event(nd));
            return;
        }
        if let Err(e) = self.emacs.open_node(nd) {
            self.set_msg(e, Duration::from_secs(6));
        }
    }

    fn set_msg(&mut self, msg: String, dur: Duration) {
        self.status_msg = Some((msg, Instant::now(), dur));
    }

    /// Shown when main() couldn't load the org-roam db: the window still opens
    /// (empty graph) with this pinned in the status bar until a reload succeeds.
    pub fn set_startup_error(&mut self, msg: String) {
        self.set_msg(msg, Duration::from_secs(24 * 3600));
    }

    /// Enter: jump to the best visible match (a title starting with the
    /// first search word ranks first, then node order).
    fn search_commit(&mut self) {
        let (query, _) = Query::search(&self.search_query); // text that doesn't parse: a plain search
        if query.is_empty() { self.search_query.clear(); return; }
        self.last_search = self.search_query.trim().to_string();
        let lead = query.first_bare().unwrap_or_default();
        let cx = EvalCtx::from_view(&self.graph, &self.view);
        let mut matches: Vec<_> = self.graph.vis_idx.iter().copied()
            .filter(|&i| query.matches(&cx, i))
            .map(|i| { let nd = &self.graph.node_list[i]; (i, !lead.is_empty() && nd.title.to_lowercase().starts_with(&lead), nd.x, nd.y) })
            .collect();
        matches.sort_by_key(|(_, starts, _, _)| !starts);
        match matches.first() {
            Some(&(idx, _, nx, ny)) => {
                self.glide_to = None;
                self.selected = Some(idx);
                self.pan = Vec2::new(-(nx as f32) * self.zoom, -(ny as f32) * self.zoom);
            }
            None => self.set_msg("Search: no visible match".into(), Duration::from_secs(3)),
        }
        self.search_query.clear();
    }

    fn cycle_layout(&mut self) {
        self.layout_mode = (self.layout_mode + 1) % 3;
        let (targets, name) = match self.layout_mode {
            1 => (self.graph.positions_column(), "Column"),
            2 => (self.graph.positions_tree(),   "Tree"),
            _ => (self.graph.positions_disk(),   "Disk"),
        };
        self.popup = Some((name.to_string(), self.now_secs));
        self.fit_to_positions(&targets);
        let resume = self.layout_mode == 0; // Disk: resume physics so nodes spread naturally
        self.graph.begin_layout_transition(targets, resume);
    }

    fn fit_to_positions(&mut self, positions: &[(f64, f64)]) {
        self.glide_to = None;
        // Visible nodes only: hidden ones keep (and report) wherever they were frozen.
        let vis = &self.graph.vis_idx;
        if vis.is_empty() || positions.len() != self.graph.node_list.len() { return; }
        let xs: Vec<f32> = vis.iter().map(|&i| positions[i].0 as f32).collect();
        let ys: Vec<f32> = vis.iter().map(|&i| positions[i].1 as f32).collect();
        let min_x = xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_x = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let min_y = ys.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let pad     = navi_core::graph::R_MAX * 4.0;
        let world_w = (max_x - min_x + pad * 2.0).max(1.0);
        let world_h = (max_y - min_y + pad * 2.0).max(1.0);
        let vw = self.graph_rect.width().max(1.0);
        let vh = self.graph_rect.height().max(1.0);
        self.zoom = ((vw / world_w).min(vh / world_h) * 0.72).clamp(ZOOM_MIN, ZOOM_MAX);
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        self.pan = Vec2::new(-cx * self.zoom, -cy * self.zoom);
    }

    fn reload(&mut self) {
        self.reload_quiet = false;
        self.start_reload();
        self.set_msg("Reloading…".to_string(), Duration::from_secs(30));
    }

    /// The db changed on disk (org-roam saved a note): re-read it without a
    /// "Reloading…" in the status bar. Only a change in the set of nodes is
    /// reported.
    fn reload_quietly(&mut self) {
        if self.reload_pending.is_some() { return; }
        self.reload_quiet = true;
        self.start_reload();
    }

    fn start_reload(&mut self) {
        let db = self.db_path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(load_graph(&db).map_err(|e| e.to_string()));
        });
        self.reload_pending = Some(rx);
    }

    /// Modification times of the db and its write-ahead log (org-roam's
    /// sqlite may run in WAL mode, where a write touches only the -wal).
    fn db_mtimes(&self) -> Vec<Option<std::time::SystemTime>> {
        ["", "-wal"].iter()
            .map(|sfx| std::fs::metadata(format!("{}{sfx}", self.db_path)).and_then(|m| m.modified()).ok())
            .collect()
    }

    fn poll_db_file(&mut self) {
        if self.db_polled_at.elapsed() < DB_POLL { return; }
        self.db_polled_at = Instant::now();
        let now = self.db_mtimes();
        if self.db_stamp.as_ref() == Some(&now) { return; }
        if self.reload_pending.is_some() { return; } // look again once this one lands
        self.db_stamp = Some(now);
        self.reload_quietly();
    }

    fn poll_reload(&mut self) {
        let result = match &self.reload_pending {
            Some(rx) => rx.try_recv().ok(),
            None => return,
        };
        if let Some(result) = result {
            self.reload_pending = None;
            match result {
                Ok(data) => {
                    self.quiet_failures = 0;
                    let mut new_graph = Graph::from_data(data);
                    new_graph.transplant_positions(&self.graph);
                    let added = new_graph.node_list.iter().filter(|n| !self.graph.nodes.contains_key(&n.id)).count();
                    let gone  = self.graph.node_list.iter().filter(|n| !new_graph.nodes.contains_key(&n.id)).count();
                    let nn = new_graph.node_list.len();
                    let ne = new_graph.edges.len();
                    let diff = if added + gone > 0 { format!("  +{added}/−{gone}") } else { String::new() };

                    // ids are the only identity that survives a reload
                    let old_sel = self.selected.map(|i| self.graph.node_list[i].id.clone());
                    let old_glide = self.glide_to.and_then(|i| self.graph.node_list.get(i)).map(|n| n.id.clone());
                    self.graph = new_graph;
                    self.hovered = None;
                    self.node_dragging = None;
                    self.last_click = None;
                    self.selected = old_sel.and_then(|s| self.graph.nodes.get(&s).copied());
                    self.glide_to = old_glide.and_then(|s| self.graph.nodes.get(&s).copied());
                    self.pin_root();
                    self.view_dirty = true;
                    self.refresh_view();
                    if !self.reload_quiet || !diff.is_empty() {
                        self.set_msg(format!("Reloaded: {nn} nodes  {ne} edges{diff}"), Duration::from_secs(3));
                    }
                    if let Some((id, file)) = self.pending_show.take() {
                        self.show_node(id.as_deref(), file.as_deref(), false);
                    }
                }
                // a write caught half-done (the db locked mid-save): the next
                // polls retry, but a db that stays unreadable is said once and
                // left alone until it changes again
                Err(e) if self.reload_quiet => {
                    self.quiet_failures += 1;
                    if self.quiet_failures < 3 {
                        self.db_stamp = None;
                    } else {
                        self.quiet_failures = 0;
                        self.set_msg(format!("Reload failed: {e}"), Duration::from_secs(6));
                    }
                }
                Err(e) => self.set_msg(format!("Reload failed: {e}"), Duration::from_secs(6)),
            }
        }
    }

    fn is_animating(&self) -> bool {
        self.graph.physics_on
            || self.pan_vel.length() > 0.5
            || self.node_dragging.is_some()
            || (self.help_anim - if self.help_visible { 1.0 } else { 0.0 }).abs() > 0.01
            // Cap at 6 s so a long-lived (pinned) status message doesn't hold
            // the renderer at full speed for its whole lifetime.
            || self.status_msg.as_ref().map_or(false, |(_, t, d)| t.elapsed() < (*d).min(Duration::from_secs(6)))
            || (self.reload_pending.is_some() && !self.reload_quiet)
            || self.glide_to.is_some()
            || self.grouping.community.pending()
    }

    fn build_status(&self) -> String {
        if self.search_active {
            let tail = match &self.search_error {
                Some(e) => format!("   {} matching as plain text ({e})   ESC: Cancel  Enter: Jump", self.search_hits),
                None if self.search_query.trim().is_empty() => "   ESC: Cancel  Enter: Jump".into(),
                None => format!("   {} matching   ESC: Cancel  Enter: Jump", self.search_hits),
            };
            return format!("  Search: {}█{tail}", self.search_query);
        }
        if let Some((msg, start, dur)) = &self.status_msg {
            if start.elapsed() < *dur { return format!("  {msg}"); }
        }
        let c = &self.view.counts;
        let n = if c.visible < c.total { format!("{}/{}", c.visible, c.total) } else { c.total.to_string() };
        let e = c.visible_edges;
        let mut filters = Vec::<String>::new();
        if self.views.views(&self.db_key).len() > 1 { filters.push(format!("View:{}", self.active_view.name)); }
        if self.active_view.colour_groups {
            let auto = self.active_view.auto;
            let mut what = Vec::new();
            if !self.active_view.groups.is_empty() { what.push(format!("{} manual", self.active_view.groups.len())); }
            if auto != AutoGrouper::None { what.push(auto.label()); }
            let pending = if self.grouping.community.pending() { " (computing)" } else { "" };
            filters.push(format!("Groups:{}{pending}", if what.is_empty() { "None".into() } else { what.join("+") }));
        }
        if c.hidden_by_groups > 0 { filters.push(format!("Hidden by groups {}", c.hidden_by_groups)); }
        if let Some(bad) = self.groups.groups.iter().find(|g| g.error.is_some()) {
            filters.push(format!("Group \"{}\": {}", bad.label, bad.error.as_ref().map(|e| e.to_string()).unwrap_or_default()));
        }
        if self.view_filter.dailies == DailiesMode::Hide { filters.push("Dailies:Hidden".into()); }
        if self.view_filter.orphans != OrphansMode::Show {
            let mut why = Vec::new();
            if c.isolated_by_filters > 0 { why.push(format!("{} isolated by filters", c.isolated_by_filters)); }
            if c.daily_only > 0          { why.push(format!("{} daily-only", c.daily_only)); }
            let iso = if why.is_empty() { String::new() } else { format!(" ({})", why.join(", ")) };
            filters.push(format!("Orphans:{} {}{iso}", self.view_filter.orphans.label(), c.orphans_in_view));
        }
        match self.local_hops { 1 => filters.push("Local:1hop".into()), 2 => filters.push("Local:2hop".into()), 3 => filters.push("Local:3hop".into()), _ => {} }
        let filt = if filters.is_empty() { String::new() } else { format!("  ·  {}", filters.join("  ")) };

        if let Some(sel) = self.selected.filter(|&i| i < self.graph.node_list.len()) {
            let nd = &self.graph.node_list[sel];
            // char-based truncation: byte slicing panics mid-codepoint on
            // CJK titles
            let t: String = nd.title.chars().take(52).collect();
            // every group the node is in; the one that colours it is marked
            let mem = self.groups.members.get(sel).map(|m| m.as_slice()).unwrap_or(&[]);
            let groups = if mem.is_empty() { String::new() } else {
                let primary = self.groups.primary.get(sel).copied().flatten();
                let names: Vec<String> = mem.iter().map(|&g| {
                    let l = &self.groups.groups[g].label;
                    if Some(g) == primary && self.active_view.colour_groups { format!("{l} (colour)") } else { l.clone() }
                }).collect();
                format!("  ·  in {}", names.join(", "))
            };
            return format!("  {t}{groups}  ·  {n} Nodes  ·  {e} Edges{filt}");
        }
        format!("  {n} Nodes  ·  {e} Edges{filt}")
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        // Escape cancels a pending rename / delete in the open panel (never
        // quits past one), whether or not its field has the keyboard.
        let prompt = self.panel.rename.is_some() || self.panel.confirm_delete;
        if prompt && self.active_view.panel_open && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.panel.rename = None;
            self.panel.rename_error = None;
            self.panel.confirm_delete = false;
            return;
        }
        // A panel TEXT field owns the keyboard: no graph shortcut fires. egui
        // drops focus on Escape before this runs, so focus held LAST frame
        // counts too: Escape leaves the field and does nothing else. Any
        // other focused widget (a toggle egui's Tab reached) gives the
        // keyboard back to the graph, so Space can't flip it.
        let typing = text_field_focused(ctx) || self.kb_focus_prev;
        if !typing && ctx.wants_keyboard_input() && !self.tab_to_graph { ctx.memory_mut(|m| m.stop_text_input()); }
        let popup  = ctx.memory(|m| m.any_popup_open());
        if typing || popup {
            self.help_visible = false;
            if typing && self.search_active {
                // a click into the panel ends the search; keep it for "Add group"
                if !self.search_query.trim().is_empty() { self.last_search = self.search_query.trim().to_string(); }
                self.search_active = false;
                self.search_query.clear();
            }
            if popup && !typing && ctx.input(|i| i.key_pressed(Key::Escape)) { ctx.memory_mut(|m| m.close_popup()); }
            // Tab in a text field still cycles nodes (and leaves the field),
            // never walks egui's focus onto the panel's toggles
            let tab = ctx.input(|i| i.events.iter().find_map(|e| match e {
                egui::Event::Key { key: Key::Tab, pressed: true, modifiers, .. } => Some(modifiers.shift),
                _ => None,
            }));
            if let (true, Some(back)) = (typing && !popup, tab) {
                self.tab_cycle(back);
                self.tab_to_graph = true;
                ctx.memory_mut(|m| m.request_focus(egui::Id::new("navi-graph-tab")));
            }
            return;
        }

        let held = ctx.input(|i| i.keys_down.clone());
        self.help_visible = held.contains(&Key::H);

        ctx.input(|i| {
            for ev in &i.events {
                match ev {
                    egui::Event::Key { key, pressed: true, repeat: false, .. } => {
                        if self.search_active {
                            // No early return here: egui delivers the Text
                            // event for this same keystroke *after* the Key
                            // event in the same batch — returning would eat it
                            // and search typing would never register.
                            match key {
                                Key::Escape    => { self.search_active = false; self.search_query.clear(); }
                                Key::Enter     => { self.search_commit(); self.search_active = false; }
                                Key::Backspace => { self.search_query.pop(); }
                                Key::Tab       => { self.tab_to_graph = true; }
                                _ => {}
                            }
                            continue;
                        }
                        match key {
                            Key::Q | Key::Escape => { self.quit_requested = true; }
                            Key::T => {
                                let n = THEMES.len() + self.emacs_theme.is_some() as usize;
                                self.theme_idx = (self.theme_idx + 1) % n;
                                self.popup = Some((self.theme().name.to_string(), self.now_secs));
                            }
                            Key::P => { self.toggle_physics(); }
                            Key::Backslash => {
                                self.toggle_panel();
                                let open = self.active_view.panel_open;
                                self.popup = Some((if open { "Panel  Open".into() } else { "Panel  Closed".into() }, self.now_secs));
                            }
                            Key::D => {
                                self.view_filter.dailies = self.view_filter.dailies.toggled();
                                let hidden = self.view_filter.dailies == DailiesMode::Hide;
                                self.popup = Some((if hidden { "Dailies  Hidden".into() } else { "Dailies  Shown".into() }, self.now_secs));
                                self.filter_changed();
                            }
                            Key::O => {
                                self.view_filter.orphans = self.view_filter.orphans.cycled();
                                self.popup = Some((format!("Orphans  {}", self.view_filter.orphans.label()), self.now_secs));
                                self.filter_changed();
                            }
                            Key::G if i.modifiers.shift => {
                                self.active_view.auto = self.active_view.auto.cycled();
                                self.active_view.colour_groups = true;
                                self.popup = Some((format!("Grouping  {}", self.active_view.auto.label()), self.now_secs));
                                self.groups_changed();
                            }
                            Key::G => {
                                let v = &mut self.active_view;
                                v.set_colour_groups(!v.colour_groups); // nothing to group: the old tag colouring
                                let msg = if v.colour_groups { format!("Groups  On  ·  {}", v.auto.label()) } else { "Groups  Off".into() };
                                self.popup = Some((msg, self.now_secs));
                                self.groups_changed();
                            }
                            Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5
                            | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9 => {
                                let idx = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5,
                                           Key::Num6, Key::Num7, Key::Num8, Key::Num9].iter().position(|k| k == key).unwrap_or(0);
                                self.switch_view(idx);
                            }
                            Key::OpenBracket | Key::CloseBracket => {
                                let n   = self.views.views(&self.db_key).len().max(1);
                                let cur = self.views.active_index(&self.db_key);
                                let idx = if *key == Key::CloseBracket { (cur + 1) % n } else { (cur + n - 1) % n };
                                self.switch_view(idx);
                            }
                            Key::A => {
                                self.show_age = !self.show_age;
                                self.popup = Some((if self.show_age { "Age View  On".into() } else { "Age View  Off".into() }, self.now_secs));
                            }
                            Key::V => { self.cycle_layout(); }
                            Key::F => {
                                self.show_fps = !self.show_fps;
                                self.popup = Some((if self.show_fps { "FPS  On".into() } else { "FPS  Off".into() }, self.now_secs));
                            }
                            Key::W => {
                                self.popup = Some(("Reloading…".into(), self.now_secs));
                                self.reload();
                            }
                            Key::R => {
                                self.fit_to_nodes();
                                self.popup = Some(("View  Reset".into(), self.now_secs));
                            }
                            Key::L => {
                                self.local_hops = if self.local_hops >= 3 { 0 } else { self.local_hops + 1 };
                                let msg = match self.local_hops {
                                    0 => "Local Graph  Off",
                                    1 => "Local Graph  1 Hop",
                                    2 => "Local Graph  2 Hops",
                                    _ => "Local Graph  3 Hops",
                                };
                                self.popup = Some((msg.into(), self.now_secs));
                            }
                            Key::Slash => {
                                self.search_active = true;
                                self.search_query.clear();
                                // The '/' keystroke's own Text event is still
                                // coming in this batch — swallow it so it
                                // doesn't seed the query.
                                self.search_swallow_slash = true;
                            }
                            Key::Tab => {
                                self.tab_to_graph = true; // see below the event loop
                                self.tab_cycle(i.modifiers.shift);
                            }
                            Key::Enter | Key::Space => {
                                if let Some(nid) = self.selected {
                                    self.open_node(nid);
                                    self.popup = Some(("Opening in Emacs…".into(), self.now_secs));
                                }
                            }
                            _ => {}
                        }
                    }
                    egui::Event::Text(text) if self.search_active => {
                        if self.search_swallow_slash && text == "/" {
                            self.search_swallow_slash = false;
                        } else {
                            self.search_query.push_str(text);
                        }
                    }
                    _ => {}
                }
            }
        });
        // Tab is the graph's: without this egui would also hand keyboard
        // focus to the panel's first widget. A dummy focus holds it off for
        // this pass; update() lets go of it once the panel is drawn. (Not
        // inside ctx.input above: that holds the context's lock.)
        if self.tab_to_graph { ctx.memory_mut(|m| m.request_focus(egui::Id::new("navi-graph-tab"))); }
    }

    /// Tab / Shift+Tab: select the next / previous visible node, in node order.
    fn tab_cycle(&mut self, back: bool) {
        let vis = &self.graph.vis_idx;
        if vis.is_empty() { return; }
        let n   = vis.len();
        let cur = self.selected.and_then(|s| vis.iter().position(|&i| i == s));
        let k = match (cur, back) {
            (Some(k), false) => (k + 1) % n,
            (Some(k), true)  => (k + n - 1) % n,
            (None, false)    => 0,
            (None, true)     => n - 1,
        };
        let idx = vis[k];
        self.selected = Some(idx);
        let title = self.graph.node_list[idx].title.clone();
        let t = if title.chars().count() > 28 { title.chars().take(28).collect::<String>() + "…" } else { title };
        self.popup = Some((t, self.now_secs));
    }

    /// Is `pos` over the side panel or one of egui's popups (colour picker,
    /// drop-down) rather than the graph? Those never pan, select or zoom.
    fn on_ui(&self, ctx: &egui::Context, pos: Pos2) -> bool {
        let mut r = self.graph_rect;
        if self.active_view.panel_open { r.max.x -= 6.0; } // the panel's resize grip reaches into the graph
        !r.contains(pos) || ctx.layer_id_at(pos).is_some_and(|l| l.order != egui::Order::Background)
    }

    fn handle_pointer(&mut self, ctx: &egui::Context) {
        let pointer = ctx.input(|i| i.pointer.clone());
        let scroll  = ctx.input(|i| i.smooth_scroll_delta);
        let hover_pos = pointer.hover_pos();

        if std::env::var_os("NAVI_INPUT_DEBUG").is_some() {
            eprintln!(
                "navi-debug: hover_pos={:?} graph_rect={:?} any_pressed={} any_down={} is_moving={} node_at={:?}",
                hover_pos,
                self.graph_rect,
                pointer.any_pressed(),
                pointer.any_down(),
                pointer.is_moving(),
                hover_pos.map(|hp| self.node_at(hp.x, hp.y)),
            );
        }

        // Hover
        let ui_busy = ctx.is_using_pointer(); // a panel widget is being dragged
        if !self.pan_dragging && self.node_dragging.is_none() {
            if let Some(hp) = hover_pos {
                if !ui_busy && !self.on_ui(ctx, hp) {
                    let new_hov = self.node_at(hp.x, hp.y);
                    self.hovered = new_hov;
                } else {
                    self.hovered = None;
                }
            }
        }

        // Scroll/zoom
        if scroll.y.abs() > 0.1 {
            if let Some(hp) = hover_pos {
                if !self.on_ui(ctx, hp) {
                    self.glide_to = None;
                    let steps = (scroll.y / 20.0).clamp(-5.0, 5.0);
                    let factor = ZOOM_STEP.powf(steps);
                    let cx = self.graph_rect.center().x;
                    let cy = self.graph_rect.center().y;
                    let wx = ((hp.x - cx - self.pan.x) / self.zoom) as f64;
                    let wy = ((hp.y - cy - self.pan.y) / self.zoom) as f64;
                    self.zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
                    self.pan.x = hp.x - cx - wx as f32 * self.zoom;
                    self.pan.y = hp.y - cy - wy as f32 * self.zoom;
                }
            }
        }

        // Drag start (a click that closes an open popup is the popup's)
        if pointer.any_pressed() && !ui_busy && !ctx.memory(|m| m.any_popup_open()) {
            if let Some(pos) = pointer.press_origin() {
                if !self.on_ui(ctx, pos) {
                    let hit = self.node_at(pos.x, pos.y);
                    if let Some(idx) = hit {
                        // Node drag — record offset from click point to node centre so
                        // the node doesn't jump when clicked away from its centre
                        let (nw_x, nw_y) = (self.graph.node_list[idx].x, self.graph.node_list[idx].y);
                        let (press_wx, press_wy) = self.s2w(pos.x, pos.y);
                        self.node_drag_origin_s = pos;
                        self.node_drag_origin_w = (press_wx - nw_x, press_wy - nw_y);
                        self.node_dragging = Some(idx);
                        self.node_drag_samples.clear();
                        self.graph.node_list[idx].pinned = true;
                        self.graph.node_list[idx].vx = 0.0;
                        self.graph.node_list[idx].vy = 0.0;
                        self.selected = Some(idx);

                        // Double-click
                        let now_i = Instant::now();
                        let is_double = self.last_click.as_ref()
                            .map_or(false, |&(lid, lt)| lid == idx && now_i.duration_since(lt) < Duration::from_millis(350));
                        if is_double { self.open_node(idx); }
                        self.last_click = Some((idx, now_i));
                    } else {
                        self.pan_dragging = true;
                        self.pan_origin_m = pos;
                        self.pan_origin_v = self.pan;
                        self.pan_vel = Vec2::ZERO;
                        self.drag_samples.clear();
                    }
                }
            }
        }

        // Drag motion
        if pointer.is_moving() || pointer.any_down() {
            if let Some(cur) = hover_pos {
                if let Some(idx) = self.node_dragging {
                    let (wx, wy) = self.s2w(cur.x, cur.y);
                    let (off_x, off_y) = self.node_drag_origin_w;
                    self.graph.node_list[idx].x = wx - off_x;
                    self.graph.node_list[idx].y = wy - off_y;
                    let now_i = Instant::now();
                    self.node_drag_samples.push((now_i, wx - off_x, wy - off_y));
                    let cutoff = now_i - Duration::from_millis(70);
                    self.node_drag_samples.retain(|(t, _, _)| *t >= cutoff);
                } else if self.pan_dragging {
                    self.pan = self.pan_origin_v + (cur - self.pan_origin_m);
                    let now_i = Instant::now();
                    self.drag_samples.push((now_i, cur.x, cur.y));
                    let cutoff = now_i - Duration::from_millis(40);
                    self.drag_samples.retain(|(t, _, _)| *t >= cutoff);
                }
            }
        }

        // Drag release
        if pointer.any_released() {
            if let Some(idx) = self.node_dragging.take() {
                self.graph.node_list[idx].pinned = false;
                let s = &self.node_drag_samples;
                if s.len() >= 2 {
                    let span = s.last().unwrap().0.duration_since(s[0].0).as_secs_f64();
                    if span > 0.001 {
                        let dvx = (s.last().unwrap().1 - s[0].1) / span;
                        let dvy = (s.last().unwrap().2 - s[0].2) / span;
                        let spd = dvx.hypot(dvy);
                        if spd > 6.0 {
                            let cap = 1100.0;
                            let (dvx, dvy) = if spd > cap { (dvx/spd*cap, dvy/spd*cap) } else { (dvx, dvy) };
                            self.graph.node_list[idx].vx = dvx;
                            self.graph.node_list[idx].vy = dvy;
                            self.graph.physics_on = true;
                        }
                    }
                }
                self.node_drag_samples.clear();
            }
            if self.pan_dragging {
                // Small displacement = click on empty space → clear selection
                let pan_delta = (self.pan - self.pan_origin_v).length();
                if pan_delta < 8.0 {
                    self.selected = None;
                }
                let s = &self.drag_samples;
                if s.len() >= 2 {
                    let span = s.last().unwrap().0.duration_since(s[0].0).as_secs_f32();
                    if span > 0.001 {
                        let vx = (s.last().unwrap().1 - s[0].1) / span;
                        let vy = (s.last().unwrap().2 - s[0].2) / span;
                        if vx.hypot(vy) > 120.0 {
                            self.pan_vel = Vec2::new(vx, vy);
                        }
                    }
                }
                self.drag_samples.clear();
                self.pan_dragging = false;
            }
        }
    }

    /// Top-left legend while group colouring is on: each group's colour,
    /// name and how many of its nodes are on screen.
    fn paint_legend(&self, painter: &egui::Painter, rect: Rect, t: Theme) {
        const MAX_ROWS: usize = 12;
        let rows: Vec<(Color32, String, bool)> = {
            let bg = [t.bg.r(), t.bg.g(), t.bg.b()];
            let listed = |g: &&navi_core::groups::GroupInfo| g.manual || g.shown > 0 || !g.visible;
            // only the rows drawn are formatted (thousands of tags or communities)
            let total = self.groups.groups.iter().filter(listed).count();
            let fits  = if total > MAX_ROWS { MAX_ROWS - 1 } else { total };
            let mut v: Vec<(Color32, String, bool)> = self.groups.groups.iter().filter(listed).take(fits)
                .map(|g| {
                    let c = g.colour.rgb(bg);
                    let label = if !g.visible { format!("{}  hidden", g.label) }
                                else if g.error.is_some() { format!("{}  (bad query)", g.label) }
                                else { format!("{}  {}", g.label, g.shown) };
                    (Color32::from_rgb(c[0], c[1], c[2]), label, g.visible && g.error.is_none())
                }).collect();
            if total > fits { v.push((Color32::TRANSPARENT, format!("+{} more", total - fits), false)); }
            v
        };
        let title = if self.grouping.community.pending() && rows.is_empty() { "Computing communities…".to_string() }
                    else if rows.is_empty() { format!("{}: no groups", self.active_view.auto.label()) }
                    else { String::new() };
        let font  = FontId::proportional(12.5);
        let row_h = 18.0;
        let pad   = 10.0;
        let width = rows.iter().chain(std::iter::once(&(Color32::TRANSPARENT, title.clone(), false)))
            .map(|(_, l, _)| painter.layout_no_wrap(l.clone(), font.clone(), t.label).size().x)
            .fold(0.0f32, f32::max).min(320.0) + 22.0 + pad * 2.0;
        let n_rows = rows.len().max(if title.is_empty() { 0 } else { 1 });
        if n_rows == 0 { return; }
        let r = Rect::from_min_size(Pos2::new(rect.left() + 18.0, rect.top() + 52.0),
                                    Vec2::new(width, n_rows as f32 * row_h + pad * 1.2));
        painter.rect_filled(r, 6.0, Color32::from_rgba_unmultiplied(t.bar_bg.r(), t.bar_bg.g(), t.bar_bg.b(), 215));
        painter.rect_stroke(r, 6.0, Stroke::new(1.0_f32, t.bar_line));
        let mut y = r.top() + pad * 0.6 + row_h / 2.0;
        if rows.is_empty() {
            painter.text(Pos2::new(r.left() + pad, y), egui::Align2::LEFT_CENTER, title, font, t.bar_text);
            return;
        }
        for (c, label, bright) in rows {
            if c != Color32::TRANSPARENT {
                let dot = Pos2::new(r.left() + pad + 5.0, y);
                if bright { painter.circle_filled(dot, 5.0, c); } else { painter.circle_stroke(dot, 4.5, Stroke::new(1.2_f32, c)); }
            }
            let col = if bright { t.label_hov } else { t.bar_text };
            let text: String = label.chars().take(44).collect();
            painter.text(Pos2::new(r.left() + pad + 18.0, y), egui::Align2::LEFT_CENTER, text, font.clone(), col);
            y += row_h;
        }
    }

    fn paint_help(&self, painter: &egui::Painter, rect: Rect, t: Theme) {
        let c = &self.view.counts;
        let orphans = match (c.isolated_by_filters, c.daily_only) {
            (0, 0) => format!("{} in view  ·  {} in db", c.orphans_in_view, c.orphans_in_db),
            (i, 0) => format!("{} in view  ({i} isolated by filters)", c.orphans_in_view),
            (0, d) => format!("{} in view  ({d} daily-only)", c.orphans_in_view),
            (i, d) => format!("{} in view  ({i} by filters, {d} daily-only)", c.orphans_in_view),
        };
        let unresolved = c.dangling.to_string();
        let rows: &[(&str, &str)] = &[
            ("Mouse / Trackpad", ""),
            ("  Drag background", "Pan view"),
            ("  Scroll", "Zoom toward cursor"),
            ("  Click node", "Select"),
            ("  Double-click node", "Open in Emacs"),
            ("", ""),
            ("Keyboard", ""),
            ("  Tab / Shift-Tab", "Cycle nodes"),
            ("  Enter / Space", "Open selected in Emacs"),
            ("  T", "Cycle theme"),
            ("  G", "Toggle group colours, hulls, pull"),
            ("  Shift-G", "Cycle auto grouping"),
            ("  1-9  /  [ ]", "Switch view"),
            ("  \\", "Side panel: views, filters, groups, inspector"),
            ("  A", "Toggle age heatmap"),
            ("  D", "Show / hide daily notes"),
            ("  O", "Cycle orphans  (shown → hidden → only)"),
            ("  L", "Cycle local graph (1-3 hops / off)"),
            ("  /", "Search (title, tag:, dir:, OR, -x)"),
            ("  V", "Cycle layout  (Disk → Column → Tree)"),
            ("  W", "Reload graph from database"),
            ("  F", "Toggle FPS counter"),
            ("  P", "Pause / resume physics"),
            ("  R", "Reset view"),
            ("  Q / Escape", "Quit"),
            ("", ""),
            ("Graph", ""),
            ("  Orphans", &orphans),
            ("  Unresolved links", &unresolved),
            ("", ""),
            ("  H", "Hold to show this menu"),
        ];
        let row_h = 22.0;
        let pad = 18.0;
        let (panel_x, panel_w, val_x) = help_columns(rect);
        let head_h = 38.0;
        let n_rows = rows.iter().filter(|&&(k,v)| !k.is_empty()||!v.is_empty()).count();
        let n_gaps = rows.iter().filter(|&&(k,v)| k.is_empty()&&v.is_empty()).count();
        let panel_h = head_h + n_rows as f32 * row_h + n_gaps as f32 * row_h / 2.0 + pad;

        let ease = 1.0 - (1.0 - self.help_anim).powi(3);
        let panel_y = rect.top() + (-panel_h * (1.0 - ease)) + 6.0;
        let panel_rect = Rect::from_min_size(Pos2::new(panel_x, panel_y), Vec2::new(panel_w, panel_h));

        painter.rect_filled(panel_rect, 6.0,
            Color32::from_rgba_unmultiplied(t.bar_bg.r(), t.bar_bg.g(), t.bar_bg.b(), 242));
        painter.rect_stroke(panel_rect, 6.0, Stroke::new(1.0, t.bar_line));
        painter.line_segment(
            [Pos2::new(panel_x+pad, panel_y+head_h), Pos2::new(panel_x+panel_w-pad, panel_y+head_h)],
            Stroke::new(1.0, t.bar_line),
        );
        painter.text(Pos2::new(panel_x+pad, panel_y+head_h/2.0), egui::Align2::LEFT_CENTER,
            "Controls", FontId::proportional(15.0), t.label_hov);

        let mut y = panel_y + head_h + pad / 2.0;
        for &(key_s, val_s) in rows {
            if key_s.is_empty() && val_s.is_empty() { y += row_h / 2.0; continue; }
            let font = FontId::proportional(13.0);
            if !key_s.is_empty() {
                let col = if val_s.is_empty() { t.label_hov } else { t.label };
                painter.text(Pos2::new(panel_x+pad, y+row_h/2.0), egui::Align2::LEFT_CENTER, key_s, font.clone(), col);
            }
            if !val_s.is_empty() {
                painter.text(Pos2::new(panel_x+pad+val_x, y+row_h/2.0), egui::Align2::LEFT_CENTER, val_s, font, t.bar_text);
            }
            y += row_h;
        }
    }
}

/// The controls overlay across `rect` (the window): (left, width, value
/// column offset). 510 wide and centred; a narrower window narrows it and
/// its value column, so no key label is ever cut off on the left.
fn help_columns(rect: Rect) -> (f32, f32, f32) {
    let w = 510.0_f32.min((rect.width() - 12.0).max(200.0));
    let x = (rect.left() + (rect.width() - w) / 2.0).max(rect.left());
    (x, w, 215.0_f32.min(w * 0.42))
}

impl NaviApp {
    /// Whether the app should be rendering at full display rate this frame.
    /// Cadence rules (driven by the custom event loop in main.rs):
    ///   * Anything actively animating (physics, layout, drag, pan inertia) → full speed.
    ///   * Window unfocused → idle (always; saves power when you're elsewhere).
    ///   * Window focused but no input for `idle_grace` → idle.
    ///   * Otherwise → full speed (the "you're using the app" mode).
    pub fn needs_full_speed(&self) -> bool {
        let actively_animating = self.graph.physics_on
            || self.graph.is_layout_animating()
            || self.glide_to.is_some()
            || self.pan_vel.length_sq() > 4.0
            || self.pan_dragging
            || self.node_dragging.is_some();
        if actively_animating {
            return true;
        }
        if !self.window_focused {
            return false;
        }
        // egui asked for an immediate repaint (a panel animation, a drag):
        // honoured, but only near input, so nothing in the panel can hold the
        // full rate while idle. Delayed requests (the text cursor's blink) are
        // met by the ~30 fps idle tier.
        let grace = self.last_active_at.elapsed();
        grace < self.idle_grace || (self.egui_busy && grace < self.idle_grace + Duration::from_secs(2))
    }

    /// main.rs: egui requested a repaint sooner than the idle tier would
    /// paint (`true`) during the last frame.
    pub fn set_egui_busy(&mut self, busy: bool) { self.egui_busy = busy; }

    /// Called by main.rs when the OS reports the window's focus state changes.
    /// On focus gain we immediately reset the activity clock so the next paint
    /// flips us back into full-speed mode without waiting on input.
    pub fn set_focused(&mut self, focused: bool) {
        self.window_focused = focused;
        if focused {
            self.last_active_at = Instant::now();
        }
    }

    /// Called by main.rs for any user input event (mouse, scroll, keyboard, IME).
    /// Bumps the activity clock so the idle grace timer restarts.
    pub fn touch_input(&mut self) {
        self.last_active_at = Instant::now();
    }

    /// Started by org-roam-navi.el (`--emacs`): nodes open in that Emacs.
    pub fn set_emacs_mode(&mut self, on: bool) { self.emacs_mode = on; }

    /// A command from Emacs (raise and quit are main.rs's: they need the
    /// window and the event loop).
    pub fn emacs_command(&mut self, cmd: navi_core::bridge::Command) {
        use navi_core::bridge::Command;
        match cmd {
            Command::Show { id, file } => self.show_node(id.as_deref(), file.as_deref(), true),
            Command::Reload => self.reload(),
            Command::Theme(p) => {
                let first = self.emacs_theme.is_none();
                self.emacs_theme = Some(crate::theme::from_emacs(&p));
                // the first palette takes over the canvas; later ones (a
                // theme switch in Emacs) repaint it only if it's still on
                if first { self.theme_idx = THEMES.len(); }
                self.visuals_for = None;
                self.colours_key = None;
                self.panel.autos = None;
            }
            Command::Raise | Command::Quit => {}
        }
    }

    /// Select the node Emacs is on and glide to it. An id navi doesn't know
    /// yet (a note created since the db was read) is looked up again after
    /// one reload; meanwhile its file's node stands in, when there is one.
    fn show_node(&mut self, id: Option<&str>, file: Option<&str>, may_reload: bool) {
        // a newer show from Emacs replaces one still waiting on a reload
        if may_reload { self.pending_show = None; }
        let found = navi_core::bridge::resolve_show(&self.graph, id, file);
        let exact = id.map_or(true, |id| self.graph.nodes.contains_key(id));
        if !exact && may_reload {
            self.pending_show = Some((id.map(str::to_string), file.map(str::to_string)));
            self.reload_quietly();
        }
        let Some(i) = found else { return };
        if !self.graph.is_visible(i) {
            let title = self.graph.node_list[i].title.clone();
            self.set_msg(format!("{title} is hidden in this view"), Duration::from_secs(4));
            return;
        }
        // the replay after a reload leaves a view the user has moved since alone;
        // a show from Emacs always brings the node back into the middle
        if !may_reload && self.selected == Some(i) { return; }
        self.selected = Some(i);
        self.glide_to = Some(i);
    }

    /// True once the user pressed Q/Escape; main.rs exits the event loop.
    pub fn take_quit_request(&mut self) -> bool {
        std::mem::take(&mut self.quit_requested)
    }

    /// Long-unfocused (>5 min): main.rs stretches the idle wake-up from 33 ms
    /// to 1 s. Wayland/composited-X11 never report full occlusion, so this is
    /// what keeps a covered window from burning 30 paints/s indefinitely.
    pub fn long_idle(&self) -> bool {
        !self.window_focused && self.last_active_at.elapsed() > Duration::from_secs(300)
    }

    /// Current theme background as `[r, g, b]` floats — used by main.rs to
    /// `glClear` the surface before egui paints into it.
    pub fn bg_color(&self) -> [f32; 3] {
        let bg = self.theme().bg;
        [
            bg.r() as f32 / 255.0,
            bg.g() as f32 / 255.0,
            bg.b() as f32 / 255.0,
        ]
    }

    /// Egui frame entry point. Driven by main.rs at exactly the panel's vsync
    /// rate (240 Hz on the user's display) when active, or a 33 ms timer when
    /// idle. Mouse / keyboard events do *not* trigger renders directly — they
    /// only mutate input state via egui_winit, which we read here.
    pub fn update(&mut self, ctx: &egui::Context) {
        // ── Frame timing ──────────────────────────────────────────────────────
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;
        self.now_secs = unix_now();
        self.frame_times.push_back(dt);
        // Keep only the last 1 second of frame times
        while self.frame_times.iter().sum::<f32>() > 1.0 && self.frame_times.len() > 2 {
            self.frame_times.pop_front();
        }

        // NAVI_FPS_LOG output lives in main.rs's `redraw` — that's the only
        // place that has authoritative paint timing. The earlier copy in this
        // function ran out of phase with main.rs and produced doubled prints.

        // ── Updates ───────────────────────────────────────────────────────────
        self.poll_reload();
        self.poll_views_file();
        self.poll_db_file();

        // Fixed 60 Hz physics — decoupled from render rate so vsync at any
        // frequency doesn't change how fast nodes move or how much CPU physics uses.
        const PHYS_DT: f64 = 1.0 / 60.0;
        self.physics_accumulator = (self.physics_accumulator + dt as f64).min(PHYS_DT * 5.0);
        while self.physics_accumulator >= PHYS_DT {
            self.graph.step(PHYS_DT);
            self.physics_accumulator -= PHYS_DT;
        }

        // `show` from Emacs: ease the view onto the node, following it while
        // physics moves it; any hand on the graph takes over
        if self.pan_dragging || self.node_dragging.is_some() { self.glide_to = None; }
        if let Some(i) = self.glide_to.filter(|&i| i < self.graph.node_list.len()) {
            let nd = &self.graph.node_list[i];
            let goal = Vec2::new(-(nd.x as f32) * self.zoom, -(nd.y as f32) * self.zoom);
            self.pan += (goal - self.pan) * (8.0 * dt).min(1.0);
            self.pan_vel = Vec2::ZERO;
            if (goal - self.pan).length() < 0.5 { self.pan = goal; self.glide_to = None; }
        } else {
            self.glide_to = None;
        }

        if !self.pan_dragging && self.pan_vel.length() > 0.5 {
            self.pan += self.pan_vel * dt;
            self.pan_vel *= PAN_DECAY.powf(dt / BASE_DT);
        }

        let help_target = if self.help_visible { 1.0f32 } else { 0.0 };
        self.help_anim += (help_target - self.help_anim) * (10.0 * dt).min(1.0);

        // ── View (recomputed only when the filter or graph changed; a landed
        //    community partition can hide or show a hidden community) ─────────
        if self.refresh_view() { self.reform_layout(); }

        // ── Input (all mutations before painter borrows) ───────────────────────
        self.handle_keyboard(ctx);
        self.handle_pointer(ctx);
        self.refresh_faded();
        self.refresh_hov_adj();
        self.refresh_colours();

        // ── Render ────────────────────────────────────────────────────────────
        let t = self.theme();
        if self.visuals_for != Some(self.theme_idx) {
            ctx.set_visuals(panel::visuals(&t));
            self.visuals_for = Some(self.theme_idx);
        }
        let status_text = self.build_status();

        egui::TopBottomPanel::bottom("status")
            .exact_height(STATUS_H)
            .frame(egui::Frame::none().fill(t.bar_bg).stroke(Stroke::new(1.0, t.bar_line)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(&status_text).color(t.bar_text).size(13.0));
                    if self.show_fps && self.frame_times.len() >= 2 {
                        let total = self.frame_times.iter().sum::<f32>();
                        let fps_str = format!("{:.0} fps", self.frame_times.len() as f32 / total);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new(fps_str).color(t.bar_text).size(12.0));
                        });
                    }
                });
            });

        // ── Side panel: drawn before the graph so the graph gets what's left;
        //    its edits apply now, so the graph below already shows them ──
        let out = panel::show(self, ctx);
        self.apply_panel(out);
        if self.refresh_view() { self.reform_layout(); }
        self.refresh_faded();
        self.refresh_hov_adj();
        self.refresh_colours();
        // the dummy focus that kept Tab on the graph has done its job
        if std::mem::take(&mut self.tab_to_graph) { ctx.memory_mut(|m| m.stop_text_input()); }
        self.kb_focus_prev = text_field_focused(ctx);
        self.flush_views(false);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(t.bg))
            .show(ctx, |ui| {
                let graph_rect = ui.available_rect_before_wrap();
                let rect_changed = (graph_rect.size() - self.graph_rect.size()).length() > 1.0;
                self.graph_rect = graph_rect; // cache for next frame's input
                if !self.initial_fit_done {
                    // First frame with the real viewport (a tiling WM may have
                    // sized us to anything): redo the startup fit before painting.
                    if rect_changed {
                        self.fit_to_nodes();
                    }
                    self.initial_fit_done = true;
                }
                ui.allocate_rect(graph_rect, Sense::hover());
                let painter = ui.painter_at(graph_rect);

                let gp = GraphPainter {
                    graph: &self.graph,
                    theme: t,
                    pan: self.pan,
                    zoom: self.zoom,
                    hovered: self.hovered,
                    selected: self.selected,
                    faded: &self.faded,
                    hov_adj: &self.hov_adj,
                    node_colours: &self.node_colours,
                    hulls: &self.hulls,
                    show_age: self.show_age,
                    now_secs: self.now_secs,
                    graph_rect,
                };

                let prof = std::env::var_os("NAVI_PROF").is_some();
                let no_grid   = std::env::var_os("NAVI_NO_GRID").is_some();
                let no_edges  = std::env::var_os("NAVI_NO_EDGES").is_some();
                let no_nodes  = std::env::var_os("NAVI_NO_NODES").is_some();
                let no_labels = std::env::var_os("NAVI_NO_LABELS").is_some();

                let t_grid   = Instant::now();
                if !no_grid   { gp.paint_grid(&painter);  }
                if !self.hulls.is_empty() { gp.paint_hulls(&painter); }
                let t_edges  = Instant::now();
                if !no_edges  { gp.paint_edges(&painter); }
                let t_nodes  = Instant::now();
                if !no_nodes  { gp.paint_nodes(&painter); }
                let t_labels = Instant::now();
                if !no_labels && self.zoom > 0.25 { gp.paint_labels(&painter); }
                let t_help   = Instant::now();
        if self.active_view.colour_groups { self.paint_legend(&painter, graph_rect, t); }
                if self.help_anim > 0.01 {
                    // over the side panel too: the whole window, never clipped to the graph
                    let over = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("navi-help")));
                    self.paint_help(&over, ui.ctx().screen_rect(), t);
                }
                let t_end    = Instant::now();

                if prof {
                    let due = self.last_prof_log
                        .map_or(true, |ts: Instant| ts.elapsed() >= Duration::from_secs(1));
                    if due {
                        let us = |a: Instant, b: Instant| (b - a).as_micros();
                        eprintln!(
                            "navi: paint  grid={}us edges={}us nodes={}us labels={}us help={}us  post_overhead={}us",
                            us(t_grid, t_edges),
                            us(t_edges, t_nodes),
                            us(t_nodes, t_labels),
                            us(t_labels, t_help),
                            us(t_help,  t_end),
                            self.post_overhead.as_micros(),
                        );
                        self.last_prof_log = Some(Instant::now());
                    }
                }
            });

        // ── Theme name popup ──────────────────────────────────────────────────
        if let Some((ref name, start)) = self.popup.clone() {
            let elapsed = (self.now_secs - start) as f32;
            let alpha: f32 = if elapsed < 0.25 {
                elapsed / 0.25                   // fade in
            } else if elapsed < 1.1 {
                1.0                              // hold
            } else if elapsed < 1.7 {
                (1.7 - elapsed) / 0.6            // fade out
            } else {
                self.popup = None;
                0.0
            };
            if alpha > 0.01 {
                let a  = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
                let t  = self.theme();
                let fid = egui::epaint::FontId::proportional(14.0);
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground, egui::Id::new("popup"),
                ));
                // Measure text first so we can size the background pill
                let galley = ctx.fonts(|f| f.layout_no_wrap(
                    name.clone(), fid.clone(),
                    egui::Color32::WHITE,
                ));
                let pad    = egui::Vec2::new(14.0, 7.0);
                let rect   = egui::Rect::from_min_size(
                    egui::Pos2::new(18.0, 12.0),
                    galley.size() + pad * 2.0,
                );
                painter.rect_filled(rect, 8.0,
                    egui::Color32::from_rgba_unmultiplied(t.bar_bg.r(), t.bar_bg.g(), t.bar_bg.b(), (a as f32 * 0.92) as u8));
                painter.rect_stroke(rect, 8.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(t.bar_line.r(), t.bar_line.g(), t.bar_line.b(), a)));
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, name,
                    fid, egui::Color32::from_rgba_unmultiplied(t.label_hov.r(), t.label_hov.g(), t.label_hov.b(), a));
            }
        }

        // Frame cadence is driven entirely by main.rs:
        //   * macOS: a CADisplayLink running on a dedicated background thread sends
        //     `UserEvent::Vsync` to the main loop on every panel refresh (240 Hz on
        //     the test display). main.rs calls `request_redraw` only on those events
        //     plus the 33 ms idle timer when `needs_full_speed()` is false.
        //   * egui_winit's per-event "please repaint" hint is *deliberately ignored*
        //     in main.rs, so mouse motion no longer drives extra renders. That is
        //     what unlocks the locked-240-fps low-CPU mode.
        //
        // We therefore do nothing here at the end of update() — no `request_repaint`,
        // no sleep, no pacer. The whole loop is cleanly owned by main.rs.
        let _ = ctx; // (still passed in case future code needs it)
        let _ = self.post_overhead;
        let _ = self.last_update_end;
    }
}


/// A text field (TextEdit, or a DragValue being typed into) has the keyboard.
/// egui's `wants_keyboard_input` is true for ANY focused widget, a toggle
/// that Tab reached too; only a text field should silence the shortcuts.
fn text_field_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused()).is_some_and(|id| egui::widgets::text_edit::TextEditState::load(ctx, id).is_some())
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// One event line to the Emacs that started navi. A closed pipe (Emacs gone)
/// is ignored: stdin's EOF is what ends navi then.
pub fn emit_to_emacs(line: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}").and_then(|_| out.flush());
}
