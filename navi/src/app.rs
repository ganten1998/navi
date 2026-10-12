use egui::{Color32, Key, Pos2, Rect, Sense, Stroke, Vec2, epaint::FontId};
use navi_core::{
    config::Config,
    compute_view, load_graph, DailiesMode, EmacsClient, Graph, GraphData, OrphansMode, ViewFilter, ViewState,
};
use std::time::{Duration, Instant};

use crate::painter::GraphPainter;
use crate::theme::{Theme, THEMES};

const ZOOM_MIN: f32 = 0.04;
const ZOOM_MAX: f32 = 20.0;
const ZOOM_STEP: f32 = 1.12;
const PAN_DECAY: f32 = 0.92;
const BASE_DT: f32 = 1.0 / 60.0;
const STATUS_H: f32 = 30.0;


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

    // ── view pipeline: recomputed only when the filter or the graph changes ──
    view_filter: ViewFilter,
    view:        ViewState,
    view_dirty:  bool,
    view_gen:    u64,                                   // bumps on every recompute
    faded:       Vec<bool>,                             // search / local-graph fade
    faded_key:   Option<(String, usize, Option<usize>, u64)>,
    hov_adj:     Vec<bool>,                             // hovered node + visible neighbours
    hov_key:     Option<(Option<usize>, u64)>,
    local_hops: usize,
    show_tags: bool,
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
}

impl NaviApp {
    pub fn new(graph: Graph, cfg: Config, db_path: String) -> Self {
        let emacs = EmacsClient::new(&cfg.emacsclient, &cfg.server_name);
        let show_fps = cfg.show_fps;
        let view_filter = cfg.view_filter();
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
            view_filter,
            view: ViewState::default(),
            view_dirty: true,
            view_gen: 0,
            faded: Vec::new(),
            faded_key: None,
            hov_adj: Vec::new(),
            hov_key: None,
            local_hops: 0,
            show_tags: false,
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
        };
        // main.rs built the graph with Graph::from_data_with_view, so the
        // visible subset of a persisted filter is already laid out on its own.
        app.refresh_view();
        // positions_disk() already pre-relaxes internally, so nodes are at
        // equilibrium.  Resume physics with an ease-in so nothing snaps.
        app.graph.resume_physics();
        app.fit_to_nodes();
        app
    }

    // ── view pipeline ─────────────────────────────────────────────────────────

    /// Recompute the view if the filter or graph changed; push it into the
    /// graph (physics/layouts) and drop interaction state on hidden nodes.
    fn refresh_view(&mut self) {
        if !self.view_dirty { return; }
        self.view_dirty = false;
        self.view = compute_view(&self.graph, &self.view_filter);
        self.graph.apply_view(&self.view);
        self.view_gen = self.view_gen.wrapping_add(1);
        let vis = |i: &usize| self.graph.is_visible(*i);
        self.selected = self.selected.filter(vis);
        self.hovered  = self.hovered.filter(vis);
        if self.node_dragging.is_some_and(|i| !self.graph.is_visible(i)) { self.node_dragging = None; }
        if self.last_click.as_ref().is_some_and(|(i, _)| !self.graph.is_visible(*i)) { self.last_click = None; }
    }

    /// A filter key changed the view: persist it and let the layout re-form
    /// around what's left (Disk resumes physics; Column/Tree re-run in place).
    fn filter_changed(&mut self) {
        self.cfg.dailies = self.view_filter.dailies;
        self.cfg.orphans = self.view_filter.orphans;
        self.cfg.save();
        self.view_dirty = true;
        self.refresh_view();
        match self.layout_mode {
            1 => { let t = self.graph.positions_column(); self.graph.begin_layout_transition(t, false); }
            2 => { let t = self.graph.positions_tree();   self.graph.begin_layout_transition(t, false); }
            _ => { self.graph.physics_on = true; }
        }
    }

    /// Search + local-graph fade mask; rebuilt only when its inputs change.
    fn refresh_faded(&mut self) {
        let key = (self.search_query.clone(), self.local_hops, self.selected, self.view_gen);
        if self.faded_key.as_ref() == Some(&key) { return; }
        let n = self.graph.node_list.len();
        let q = self.search_query.to_lowercase();
        let local = match (self.local_hops, self.selected) {
            (h, Some(sel)) if h > 0 => Some(self.graph.bfs_visible(sel, h)),
            _ => None,
        };
        self.faded = (0..n).map(|i| {
            if !self.graph.is_visible(i) { return false; }
            let nd = &self.graph.node_list[i];
            let in_search = q.is_empty() || nd.title.to_lowercase().contains(&q)
                || nd.aliases.iter().any(|a| a.to_lowercase().contains(&q));
            let in_local = match &local { Some(l) => l[i], None => true };
            !in_search || !in_local
        }).collect();
        self.faded_key = Some(key);
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

    fn theme(&self) -> Theme { THEMES[self.theme_idx] }

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
        if let Some(nd) = self.graph.node_list.get(idx) {
            if let Err(e) = self.emacs.open_node(nd) {
                self.set_msg(e, Duration::from_secs(6));
            }
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

    fn search_commit(&mut self) {
        let q = self.search_query.to_lowercase();
        if q.is_empty() { self.search_query.clear(); return; }
        let mut matches: Vec<_> = self.graph.vis_idx.iter().map(|&i| (i, &self.graph.node_list[i]))
            .filter(|(_, nd)|
                nd.title.to_lowercase().contains(&q) ||
                nd.aliases.iter().any(|a| a.to_lowercase().contains(&q))
            )
            .map(|(i, nd)| (i, nd.title.to_lowercase().starts_with(&q), nd.x, nd.y))
            .collect();
        matches.sort_by_key(|(_, starts, _, _)| !starts);
        if let Some(&(idx, _, nx, ny)) = matches.first() {
            self.selected = Some(idx);
            self.pan = Vec2::new(-(nx as f32) * self.zoom, -(ny as f32) * self.zoom);
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
        let db = self.db_path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(load_graph(&db).map_err(|e| e.to_string()));
        });
        self.reload_pending = Some(rx);
        self.set_msg("Reloading…".to_string(), Duration::from_secs(30));
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
                    let mut new_graph = Graph::from_data(data);
                    new_graph.transplant_positions(&self.graph);
                    let added = new_graph.node_list.iter().filter(|n| !self.graph.nodes.contains_key(&n.id)).count();
                    let gone  = self.graph.node_list.iter().filter(|n| !new_graph.nodes.contains_key(&n.id)).count();
                    let nn = new_graph.node_list.len();
                    let ne = new_graph.edges.len();
                    let diff = if added + gone > 0 { format!("  +{added}/−{gone}") } else { String::new() };

                    // ids are the only identity that survives a reload
                    let old_sel = self.selected.map(|i| self.graph.node_list[i].id.clone());
                    self.graph = new_graph;
                    self.hovered = None;
                    self.node_dragging = None;
                    self.last_click = None;
                    self.selected = old_sel.and_then(|s| self.graph.nodes.get(&s).copied());
                    self.view_dirty = true;
                    self.refresh_view();
                    self.set_msg(format!("Reloaded — {nn} nodes  {ne} edges{diff}"), Duration::from_secs(3));
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
            || self.reload_pending.is_some()
    }

    fn build_status(&self) -> String {
        if self.search_active {
            return format!("  Search: {}█   ESC: Cancel  Enter: Jump", self.search_query);
        }
        if let Some((msg, start, dur)) = &self.status_msg {
            if start.elapsed() < *dur { return format!("  {msg}"); }
        }
        let c = &self.view.counts;
        let n = if c.visible < c.total { format!("{}/{}", c.visible, c.total) } else { c.total.to_string() };
        let e = c.visible_edges;
        let mut filters = Vec::<String>::new();
        if self.show_tags { filters.push("Tags:On".into()); }
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

        if let Some(nd) = self.selected.and_then(|i| self.graph.node_list.get(i)) {
            // char-based truncation — byte slicing panics mid-codepoint on
            // CJK titles
            let t: String = nd.title.chars().take(52).collect();
            return format!("  {t}  ·  {n} Nodes  ·  {e} Edges{filt}");
        }
        format!("  {n} Nodes  ·  {e} Edges{filt}")
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
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
                                _ => {}
                            }
                            continue;
                        }
                        match key {
                            Key::Q | Key::Escape => { self.quit_requested = true; }
                            Key::T => {
                                self.theme_idx = (self.theme_idx + 1) % THEMES.len();
                                self.popup = Some((THEMES[self.theme_idx].name.to_string(), self.now_secs));
                            }
                            Key::P => {
                                if self.graph.physics_on {
                                    self.graph.physics_on = false;
                                    self.popup = Some(("Physics  Paused".into(), self.now_secs));
                                } else {
                                    self.graph.resume_physics();
                                    self.popup = Some(("Physics  Active".into(), self.now_secs));
                                }
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
                            Key::G => {
                                self.show_tags = !self.show_tags;
                                self.popup = Some((if self.show_tags { "Tag Colors  On".into() } else { "Tag Colors  Off".into() }, self.now_secs));
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
                                // Visible nodes only, in node order
                                let vis = &self.graph.vis_idx;
                                if !vis.is_empty() {
                                    let n   = vis.len();
                                    let cur = self.selected.and_then(|s| vis.iter().position(|&i| i == s));
                                    let k = match (cur, i.modifiers.shift) {
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
        if !self.pan_dragging && self.node_dragging.is_none() {
            if let Some(hp) = hover_pos {
                if self.graph_rect.contains(hp) {
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
                if self.graph_rect.contains(hp) {
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

        // Drag start
        if pointer.any_pressed() {
            if let Some(pos) = pointer.press_origin() {
                if self.graph_rect.contains(pos) {
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
            ("  G", "Toggle tag colouring"),
            ("  A", "Toggle age heatmap"),
            ("  D", "Show / hide daily notes"),
            ("  O", "Cycle orphans  (shown → hidden → only)"),
            ("  L", "Cycle local graph (1–3 hops / off)"),
            ("  /", "Search by title or alias"),
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
        let panel_w = 510.0;
        let head_h = 38.0;
        let n_rows = rows.iter().filter(|&&(k,v)| !k.is_empty()||!v.is_empty()).count();
        let n_gaps = rows.iter().filter(|&&(k,v)| k.is_empty()&&v.is_empty()).count();
        let panel_h = head_h + n_rows as f32 * row_h + n_gaps as f32 * row_h / 2.0 + pad;

        let ease = 1.0 - (1.0 - self.help_anim).powi(3);
        let panel_x = rect.left() + (rect.width() - panel_w) / 2.0;
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
                painter.text(Pos2::new(panel_x+pad+215.0, y+row_h/2.0), egui::Align2::LEFT_CENTER, val_s, font, t.bar_text);
            }
            y += row_h;
        }
    }
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
            || self.pan_vel.length_sq() > 4.0
            || self.pan_dragging
            || self.node_dragging.is_some();
        if actively_animating {
            return true;
        }
        if !self.window_focused {
            return false;
        }
        self.last_active_at.elapsed() < self.idle_grace
    }

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

        // Fixed 60 Hz physics — decoupled from render rate so vsync at any
        // frequency doesn't change how fast nodes move or how much CPU physics uses.
        const PHYS_DT: f64 = 1.0 / 60.0;
        self.physics_accumulator = (self.physics_accumulator + dt as f64).min(PHYS_DT * 5.0);
        while self.physics_accumulator >= PHYS_DT {
            self.graph.step(PHYS_DT);
            self.physics_accumulator -= PHYS_DT;
        }

        if !self.pan_dragging && self.pan_vel.length() > 0.5 {
            self.pan += self.pan_vel * dt;
            self.pan_vel *= PAN_DECAY.powf(dt / BASE_DT);
        }

        let help_target = if self.help_visible { 1.0f32 } else { 0.0 };
        self.help_anim += (help_target - self.help_anim) * (10.0 * dt).min(1.0);

        // ── View (recomputed only when the filter or graph changed) ───────────
        self.refresh_view();

        // ── Input (all mutations before painter borrows) ───────────────────────
        self.handle_keyboard(ctx);
        self.handle_pointer(ctx);
        self.refresh_faded();
        self.refresh_hov_adj();

        // ── Render ────────────────────────────────────────────────────────────
        let t = self.theme();
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
                    show_tags: self.show_tags,
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
                let t_edges  = Instant::now();
                if !no_edges  { gp.paint_edges(&painter); }
                let t_nodes  = Instant::now();
                if !no_nodes  { gp.paint_nodes(&painter); }
                let t_labels = Instant::now();
                if !no_labels && self.zoom > 0.25 { gp.paint_labels(&painter); }
                let t_help   = Instant::now();
                if self.help_anim > 0.01 { self.paint_help(&painter, graph_rect, t); }
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
                let t  = THEMES[self.theme_idx];
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


fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
