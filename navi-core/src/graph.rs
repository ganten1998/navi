use std::collections::{HashMap, HashSet, VecDeque};
use rayon::prelude::*;
use crate::db::{DanglingLink, GraphData, LinkStats, RawNode};
use crate::view::{compute_view, ViewFilter, ViewState};

// ── physics constants ─────────────────────────────────────────────────────────
pub const REPULSION: f64 = 16000.0;
pub const SPRING_K:  f64 = 0.055;
pub const SPRING_L:  f64 = 110.0;
pub const GRAVITY:   f64 = 0.020;
pub const DAMPING:   f64 = 0.90;
pub const BASE_DT:   f64 = 1.0 / 60.0;
pub const DT_CAP:    f64 = 0.05;
pub const R_MIN:     f32 = 5.0;
pub const R_MAX:     f32 = 22.0;

const LAYOUT_DUR: f32 = 1.3; // seconds for layout transition animation

#[derive(Debug, Clone)]
pub struct Node {
    pub id:          String,
    pub title:       String,
    pub file:        String,
    pub level:       i64,
    pub pos:         i64,
    pub mtime:       i64,
    pub aliases:     Vec<String>,
    pub tags:        Vec<String>,
    pub olp:         Vec<String>,
    pub parent_file: Option<usize>, // heading → its file node (containment: not an edge, not in degree)
    pub x: f64, pub y: f64,
    pub vx: f64, pub vy: f64,
    pub pinned: bool,
    pub degree: usize,              // deduped real links in the whole db
    pub radius: f32,
}

struct LayoutAnim {
    from:           Vec<(f64, f64)>,
    to:             Vec<(f64, f64)>,
    t:              f32,
    resume_physics: bool, // true = restart force-directed after landing; false = breathing mode
}

pub struct Graph {
    pub nodes:      HashMap<String, usize>,
    pub node_list:  Vec<Node>,
    pub edges:      Vec<(usize, usize)>,  // real id links: deduped unordered pairs, no self-links
    pub adj:        Vec<Vec<usize>>,      // over `edges`
    pub dangling:   Vec<DanglingLink>,
    pub link_stats: LinkStats,
    pub tag_colors: HashMap<String, [u8; 3]>,
    pub physics_on: bool,
    pub step_count: u64,
    // ── view: set by apply_view(); everything visible until then ──
    pub visible:    Vec<bool>,
    pub vis_idx:    Vec<usize>,           // visible node indices, ascending
    pub vis_edges:  Vec<(usize, usize)>,  // edges with both ends visible (node indices)
    pub vis_adj:    Vec<Vec<usize>>,      // over `vis_edges`
    vis_local:      Vec<(usize, usize)>,  // `vis_edges` as positions in `vis_idx`
    vis_layout:     Vec<(usize, usize)>,  // `vis_local` minus edges touching a view orphan (what layouts see)
    clamp_r:        f64,                  // physics radial clamp; set by placements, never by filters
    layout_anim:    Option<LayoutAnim>,
    physics_ease:   f32, // 0 → 1 ramp after layout animation so forces fade in, not snap
    settle_frames:  u32, // consecutive frames below velocity threshold; auto-pauses at limit
}

impl Graph {
    /// Build from raw nodes + id-pair edges. Edges are (re)deduped here as
    /// unordered index pairs and self-links dropped, so degree, radius and
    /// springs count each relation once whatever the caller passes.
    pub fn new(raw_nodes: Vec<RawNode>, raw_edges: Vec<(String, String)>) -> Self {
        // Initial placement: disk (phyllotaxis) so physics settles into a
        // natural filled-circle shape like Obsidian's graph view.
        let mut g = Self::build(raw_nodes, raw_edges);
        let positions = g.positions_disk();
        g.place(positions);
        g
    }

    /// `new` for a known filter: the view is applied first and only the
    /// visible subset gets the (pre-relaxed) disk layout, so a saved filter
    /// costs one layout at startup, not two. Hidden nodes get the cheap shell
    /// placement of the whole graph (no pre-relax), ready if they're revealed.
    pub fn new_with_view(raw_nodes: Vec<RawNode>, raw_edges: Vec<(String, String)>, filter: &ViewFilter) -> Self {
        let mut g = Self::build(raw_nodes, raw_edges);
        let view = compute_view(&g, filter);
        if view.counts.visible < view.counts.total {
            let deg: Vec<usize> = g.node_list.iter().map(|n| n.degree).collect();
            let all = place_disk(&deg, &g.edges);
            g.place(all);
            g.apply_view(&view);
        }
        let positions = g.positions_disk();
        g.place(positions);
        g
    }

    fn build(raw_nodes: Vec<RawNode>, raw_edges: Vec<(String, String)>) -> Self {
        let n = raw_nodes.len();
        let mut nodes:     HashMap<String, usize> = HashMap::with_capacity(n);
        let mut node_list: Vec<Node>              = Vec::with_capacity(n);

        for (i, rn) in raw_nodes.into_iter().enumerate() {
            let title = if rn.title.is_empty() { rn.id.chars().take(12).collect() } else { rn.title };
            nodes.insert(rn.id.clone(), i);
            node_list.push(Node {
                id:          rn.id,
                title,
                file:        rn.file,
                level:       rn.level,
                pos:         rn.pos,
                mtime:       rn.mtime,
                aliases:     rn.aliases,
                tags:        rn.tags,
                olp:         rn.olp,
                parent_file: rn.parent_file.as_ref().and_then(|p| nodes.get(p).copied()),
                x: 0.0, y: 0.0,
                vx: lcg_uniform(i as u64 * 2 + 3) * 4.0 - 2.0,
                vy: lcg_uniform(i as u64 * 2 + 4) * 4.0 - 2.0,
                pinned: false, degree: 0, radius: R_MIN,
            });
        }
        // A parent listed after its heading, or not passed at all: resolve
        // through one file → level-0 map (first wins, like db.rs). Headings in
        // files with no level-0 node stay None.
        if node_list.iter().any(|nd| nd.level != 0 && nd.parent_file.is_none()) {
            let mut file_node: HashMap<&str, usize> = HashMap::new();
            for (i, nd) in node_list.iter().enumerate().filter(|(_, nd)| nd.level == 0) {
                file_node.entry(nd.file.as_str()).or_insert(i);
            }
            let late: Vec<(usize, usize)> = node_list.iter().enumerate()
                .filter(|(_, nd)| nd.level != 0 && nd.parent_file.is_none())
                .filter_map(|(i, nd)| file_node.get(nd.file.as_str()).map(|&p| (i, p)))
                .collect();
            for (i, p) in late { node_list[i].parent_file = Some(p); }
        }

        let mut edges: Vec<(usize, usize)>      = Vec::new();
        let mut seen:  HashSet<(usize, usize)>  = HashSet::new();
        let mut adj:   Vec<Vec<usize>>          = vec![Vec::new(); n];
        for (src, dst) in &raw_edges {
            let (Some(&si), Some(&di)) = (nodes.get(src), nodes.get(dst)) else { continue };
            if si == di || !seen.insert((si.min(di), si.max(di))) { continue; }
            edges.push((si, di));
            node_list[si].degree += 1;
            node_list[di].degree += 1;
            adj[si].push(di);
            adj[di].push(si);
        }

        let max_deg  = node_list.iter().map(|n| n.degree).max().unwrap_or(1).max(1);
        let log_span = (max_deg as f32 + 1.0).ln_1p();
        for nd in node_list.iter_mut() {
            nd.radius = R_MIN + (R_MAX - R_MIN) * (nd.degree as f32 + 1.0).ln_1p() / log_span;
        }

        let mut all_tags: Vec<String> = node_list.iter().flat_map(|n| n.tags.iter().cloned()).collect();
        all_tags.sort(); all_tags.dedup();
        let phi = 0.618033988749895f64;
        let mut tag_colors: HashMap<String, [u8; 3]> = HashMap::new();
        for (i, tag) in all_tags.iter().enumerate() {
            let h = (i as f64 * phi) % 1.0;
            tag_colors.insert(tag.clone(), hsv_to_rgb(h, 0.62, 0.88));
        }

        let mut g = Graph {
            nodes, node_list, edges, adj, dangling: Vec::new(), link_stats: LinkStats::default(),
            tag_colors, physics_on: true, step_count: 0,
            visible: Vec::new(), vis_idx: Vec::new(), vis_edges: Vec::new(), vis_adj: Vec::new(),
            vis_local: Vec::new(), vis_layout: Vec::new(), clamp_r: base_clamp(n),
            layout_anim: None, physics_ease: 1.0, settle_frames: 0,
        };
        g.set_visible(vec![true; n], None);
        g
    }

    /// Build from a db load, keeping its dangling links and stats.
    pub fn from_data(data: GraphData) -> Self {
        let (dangling, stats) = (data.dangling, data.stats);
        let mut g = Graph::new(data.nodes, data.edges);
        g.dangling   = dangling;
        g.link_stats = stats;
        g
    }

    /// `from_data` laid out for `filter` (see `new_with_view`).
    pub fn from_data_with_view(data: GraphData, filter: &ViewFilter) -> Self {
        let (dangling, stats) = (data.dangling, data.stats);
        let mut g = Graph::new_with_view(data.nodes, data.edges, filter);
        g.dangling   = dangling;
        g.link_stats = stats;
        g
    }

    /// Put every node at `pos` (a full placement) and size the physics clamp
    /// to hold it.
    fn place(&mut self, pos: Vec<(f64, f64)>) {
        self.clamp_r = base_clamp(self.node_list.len()).max(extent(&pos) * 1.05);
        for (nd, (x, y)) in self.node_list.iter_mut().zip(pos) { nd.x = x; nd.y = y; }
    }

    /// Radius of the physics clamp: a placement sizes it, a filter change
    /// never does (shrinking it would snap visible nodes inward in one frame).
    pub fn clamp_radius(&self) -> f64 { self.clamp_r }

    // ── View ──────────────────────────────────────────────────────────────────

    /// Adopt a computed view: physics, layouts, BFS and drawing all switch to
    /// its visible subset. Hidden nodes are frozen where they are.
    pub fn apply_view(&mut self, view: &ViewState) {
        if view.visible.len() != self.node_list.len() { return; } // stale view for another graph
        let orphan = (view.orphan.len() == view.visible.len()).then_some(view.orphan.as_slice());
        self.set_visible(view.visible.clone(), orphan);
    }

    /// `orphan`: the view's orphan mask. Layouts drop edges touching a view
    /// orphan, so they put exactly the view's orphans in their orphan cluster
    /// (with links_to_dailies_count = false an orphan can still have a drawn
    /// edge). None = orphan means visible degree 0.
    fn set_visible(&mut self, visible: Vec<bool>, orphan: Option<&[bool]>) {
        let n = self.node_list.len();
        self.vis_idx = (0..n).filter(|&i| visible[i]).collect();
        let mut local = vec![usize::MAX; n];
        for (li, &i) in self.vis_idx.iter().enumerate() { local[i] = li; }
        self.vis_edges = self.edges.iter().copied().filter(|&(a, b)| visible[a] && visible[b]).collect();
        self.vis_local = self.vis_edges.iter().map(|&(a, b)| (local[a], local[b])).collect();
        self.vis_layout = match orphan {
            Some(o) => self.vis_edges.iter().zip(&self.vis_local)
                .filter(|&(&(a, b), _)| !o[a] && !o[b]).map(|(_, &l)| l).collect(),
            None    => self.vis_local.clone(),
        };
        self.vis_adj   = vec![Vec::new(); n];
        for &(a, b) in &self.vis_edges { self.vis_adj[a].push(b); self.vis_adj[b].push(a); }
        for (nd, &v) in self.node_list.iter_mut().zip(&visible) {
            if !v { nd.vx = 0.0; nd.vy = 0.0; nd.pinned = false; }
        }
        self.visible = visible;
    }

    pub fn is_visible(&self, i: usize) -> bool { self.visible.get(i).copied().unwrap_or(false) }

    /// Nodes within `hops` of `start` over VISIBLE edges (a hidden daily never
    /// bridges two notes), as a per-node mask.
    pub fn bfs_visible(&self, start: usize, hops: usize) -> Vec<bool> {
        let mut seen = vec![false; self.node_list.len()];
        let Some(s) = seen.get_mut(start) else { return seen };
        *s = true;
        let mut frontier = vec![start];
        for _ in 0..hops {
            let mut nxt = Vec::new();
            for &cur in &frontier {
                for &nb in &self.vis_adj[cur] {
                    if !seen[nb] { seen[nb] = true; nxt.push(nb); }
                }
            }
            frontier = nxt;
        }
        seen
    }

    // ── Layout transitions ────────────────────────────────────────────────────
    // Every layout runs on the visible subgraph alone (its own hub, shells,
    // islands and orphans); hidden nodes come back at their current position.

    fn layout_visible(&self, f: impl Fn(&[usize], &[(usize, usize)]) -> Vec<(f64, f64)>) -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = self.node_list.iter().map(|n| (n.x, n.y)).collect();
        let mut deg = vec![0usize; self.vis_idx.len()];
        for &(a, b) in &self.vis_layout { deg[a] += 1; deg[b] += 1; }
        for (li, p) in f(&deg, &self.vis_layout).into_iter().enumerate() { out[self.vis_idx[li]] = p; }
        out
    }

    pub fn positions_radial(&self) -> Vec<(f64, f64)> { self.layout_visible(positions_radial) }

    pub fn positions_ring(&self) -> Vec<(f64, f64)> { self.layout_visible(positions_ring) }

    pub fn positions_disk(&self) -> Vec<(f64, f64)> { self.layout_visible(positions_disk) }

    /// Column — BFS shells as vertical strips (L→R), nodes spread vertically.
    pub fn positions_column(&self) -> Vec<(f64, f64)> {
        self.layout_visible(|d, e| positions_layered(d, e, true))
    }

    /// Row — BFS shells as horizontal strips (T→B), nodes spread horizontally.
    pub fn positions_row(&self) -> Vec<(f64, f64)> {
        self.layout_visible(|d, e| positions_layered(d, e, false))
    }

    /// Top-down tree — hub at top, each BFS shell becomes a horizontal row.
    /// Nodes within each row sorted by parent position to reduce crossings.
    pub fn positions_tree(&self) -> Vec<(f64, f64)> { self.layout_visible(positions_tree) }

    /// Begin a smooth transition to `targets`. Nodes float to their new
    /// positions over LAYOUT_DUR seconds; physics resumes after.
    pub fn begin_layout_transition(&mut self, targets: Vec<(f64, f64)>, resume_physics: bool) {
        // targets hold every node (hidden ones at their current spot), so the
        // clamp covers whatever a later filter change reveals
        self.clamp_r = base_clamp(self.node_list.len()).max(extent(&targets) * 1.05);
        let from = self.node_list.iter().map(|n| (n.x, n.y)).collect();
        self.layout_anim = Some(LayoutAnim { from, to: targets, t: 0.0, resume_physics });
        self.physics_on  = true;
    }

    pub fn is_layout_animating(&self) -> bool { self.layout_anim.is_some() }

    // ── Step ─────────────────────────────────────────────────────────────────

    /// Resume full force-directed physics from the current node positions,
    /// easing forces in so nodes don't snap from their current layout.
    pub fn resume_physics(&mut self) {
        self.physics_on   = true;
        self.physics_ease = 0.0;
    }

    /// One physics tick over the VISIBLE nodes only: hidden nodes neither
    /// move nor repel, attract or count toward the radial clamp.
    pub fn step(&mut self, dt: f64) {
        let dt = dt.min(DT_CAP);
        let n  = self.node_list.len();
        if n == 0 { self.physics_on = false; return; } // empty graph: let the app idle

        // Layout animation: smoothstep lerp, no physics
        if let Some(ref mut anim) = self.layout_anim {
            self.physics_on = true; // keep step() ticking
            anim.t = (anim.t + dt as f32 / LAYOUT_DUR).min(1.0);
            let t    = anim.t;
            let ease = (t * t * (3.0 - 2.0 * t)) as f64;
            for &i in &self.vis_idx {
                let nd = &mut self.node_list[i];
                nd.x  = anim.from[i].0 + (anim.to[i].0 - anim.from[i].0) * ease;
                nd.y  = anim.from[i].1 + (anim.to[i].1 - anim.from[i].1) * ease;
                nd.vx = 0.0; nd.vy = 0.0;
            }
            if anim.t >= 1.0 {
                let resume = anim.resume_physics;
                self.layout_anim = None;
                if resume {
                    self.resume_physics(); // ease forces back in so nodes settle naturally
                } else {
                    self.physics_on = false; // breathing mode — holds layout positions
                }
            }
            return;
        }

        if !self.physics_on {
            // Breathing is handled purely visually in the painter — no position mutation here.
            self.step_count = self.step_count.wrapping_add(1);
            return;
        }

        let m = self.vis_idx.len();
        if m == 0 { self.physics_on = false; return; } // nothing on screen to simulate

        // Full force-directed physics — ease forces in after a layout to avoid snapping
        if self.physics_ease < 1.0 {
            self.physics_ease = (self.physics_ease + dt as f32 / 3.5).min(1.0);
        }
        let ease = self.physics_ease as f64;

        // Compact arrays over the visible subset: local index li ↔ node vis_idx[li]
        let idx = &self.vis_idx;
        let pts: Vec<(f64, f64)> = idx.iter().map(|&i| (self.node_list[i].x, self.node_list[i].y)).collect();
        let mut fx = vec![0.0f64; m];
        let mut fy = vec![0.0f64; m];

        {
            let bh = bh_build(&pts);
            // Each node queries the read-only tree independently — embarrassingly parallel.
            let rep_forces: Vec<(f64, f64)> = pts.par_iter().enumerate()
                .map_with(Vec::with_capacity(64), |stk, (i, &(x, y))| bh_force(&bh, x, y, i, stk))
                .collect();
            for (i, (rfx, rfy)) in rep_forces.into_iter().enumerate() {
                fx[i] += rfx; fy[i] += rfy;
            }
        }

        for &(si, di) in &self.vis_local {
            let dx   = pts[di].0 - pts[si].0;
            let dy   = pts[di].1 - pts[si].1;
            let dist = dx.hypot(dy).max(1e-4);
            let f    = SPRING_K * (dist - SPRING_L) / dist;
            fx[si] += dx * f; fy[si] += dy * f;
            fx[di] -= dx * f; fy[di] -= dy * f;
        }

        self.step_count = self.step_count.wrapping_add(1);
        let t    = self.step_count as f64 * 0.006;
        const PHI: f64 = 1.618033988749895;
        for li in 0..m {
            let phase = li as f64 * PHI;
            fx[li] += (t + phase).sin() * 0.35;
            fy[li] += (t * 1.272 + phase * 0.849).cos() * 0.35;
        }

        // Scale ALL forces (including gravity) by ease so nothing snaps after layout animation
        let g = GRAVITY * ease;
        for li in 0..m { fx[li] *= ease; fy[li] *= ease; }

        let damp  = DAMPING.powf(dt / BASE_DT);
        let max_r = self.clamp_r;
        const MAX_V: f64 = 650.0;
        let mut max_v = 0.0f64;
        for (li, &i) in idx.iter().enumerate() {
            let nd = &mut self.node_list[i];
            if nd.pinned { nd.vx = 0.0; nd.vy = 0.0; continue; }
            nd.vx = (nd.vx + (fx[li] - nd.x * g) * dt) * damp;
            nd.vy = (nd.vy + (fy[li] - nd.y * g) * dt) * damp;
            // Cap velocity before position update so no single step can escape
            let spd = nd.vx.hypot(nd.vy);
            if spd > MAX_V { nd.vx = nd.vx / spd * MAX_V; nd.vy = nd.vy / spd * MAX_V; }
            nd.x += nd.vx * dt;
            nd.y += nd.vy * dt;
            // Hard radial clamp — any node that slips past loses its velocity
            let d = nd.x.hypot(nd.y);
            if d > max_r {
                let s = max_r / d;
                nd.x *= s; nd.y *= s;
                nd.vx *= 0.1; nd.vy *= 0.1;
            }
            max_v = max_v.max(nd.vx.hypot(nd.vy));
        }

        // Auto-pause: if all nodes are near-still for ~1.5 s, switch to breathing mode
        if max_v < 1.5 && self.physics_ease >= 1.0 {
            self.settle_frames += 1;
            if self.settle_frames > 90 { self.physics_on = false; self.settle_frames = 0; }
        } else {
            self.settle_frames = 0;
        }
    }

    pub fn transplant_positions(&mut self, old: &Graph) {
        for nd in &mut self.node_list {
            if let Some(&oi) = old.nodes.get(&nd.id) {
                let on = &old.node_list[oi];
                nd.x = on.x; nd.y = on.y;
                nd.vx = on.vx; nd.vy = on.vy;
            }
        }
        let pos: Vec<(f64, f64)> = self.node_list.iter().map(|n| (n.x, n.y)).collect();
        self.clamp_r = self.clamp_r.max(old.clamp_r).max(extent(&pos) * 1.05);
    }
}

// ── Layout position calculators ───────────────────────────────────────────────
// Pure functions over a compact graph: `deg[i]` per node, `edges` in the same
// index space, n = deg.len(). The graph is split into
//   * the main component (holds the highest-degree hub; laid out at origin),
//   * islands (other components with ≥ 2 nodes; each laid out on its own,
//     then set beside the main one), and
//   * orphans (degree 0 — the same definition the view uses).

const UNSEEN: usize = usize::MAX;

struct Parts {
    adj:     Vec<Vec<usize>>,
    comps:   Vec<usize>, // hub of each component with an edge; main first, then by size desc
    orphans: Vec<usize>,
}

fn adjacency(n: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); n];
    for &(s, d) in edges { adj[s].push(d); adj[d].push(s); }
    adj
}

/// Highest degree, last index on ties (what `max_by_key` over 0..n picked before).
fn hub_of(members: impl Iterator<Item = usize>, deg: &[usize]) -> usize {
    members.max_by_key(|&i| (deg[i], i)).unwrap_or(0)
}

fn split_components(deg: &[usize], edges: &[(usize, usize)]) -> Parts {
    let n   = deg.len();
    let adj = adjacency(n, edges);
    let mut comp_of = vec![UNSEEN; n];
    let mut comps: Vec<(usize, usize, usize)> = Vec::new(); // (hub, size, first index)
    for s in 0..n {
        if comp_of[s] != UNSEEN || adj[s].is_empty() { continue; }
        let id = comps.len();
        comp_of[s] = id;
        let mut members = vec![s];
        let mut k = 0;
        while k < members.len() {
            let cur = members[k]; k += 1;
            for &nb in &adj[cur] {
                if comp_of[nb] == UNSEEN { comp_of[nb] = id; members.push(nb); }
            }
        }
        comps.push((hub_of(members.iter().copied(), deg), members.len(), s));
    }
    let main = if comps.is_empty() { UNSEEN } else { comp_of[hub_of(0..n, deg)] };
    let mut order: Vec<usize> = (0..comps.len()).collect();
    order.sort_by_key(|&c| (c != main, std::cmp::Reverse(comps[c].1), comps[c].2));
    Parts {
        comps:   order.into_iter().map(|c| comps[c].0).collect(),
        orphans: (0..n).filter(|&i| adj[i].is_empty()).collect(),
        adj,
    }
}

/// BFS shells from `hub`. `shell` is shared scratch (UNSEEN = not reached);
/// components are disjoint, so one array serves every component.
fn bfs_shells(adj: &[Vec<usize>], hub: usize, shell: &mut [usize]) -> Vec<Vec<usize>> {
    shell[hub] = 0;
    let mut shells   = vec![vec![hub]];
    let mut frontier = vec![hub];
    while !frontier.is_empty() {
        let sh = shells.len();
        let mut next = Vec::new();
        for &cur in &frontier {
            for &nb in &adj[cur] {
                if shell[nb] == UNSEEN { shell[nb] = sh; next.push(nb); }
            }
        }
        if !next.is_empty() { shells.push(next.clone()); }
        frontier = next;
    }
    shells
}

fn jitter(ni: usize, salt: u64, amp: f64) -> f64 {
    lcg_uniform((ni as u64).wrapping_mul(6364136223846793005).wrapping_add(salt)) * amp - amp / 2.0
}

/// Concentric rings around the component's hub at the origin; each ring's
/// nodes sorted by parent angle so adjacent shells don't cross.
fn place_rings(adj: &[Vec<usize>], shells: &[Vec<usize>], shell: &[usize], ring_gap: f64, jit: f64, pos: &mut [(f64, f64)]) {
    pos[shells[0][0]] = (0.0, 0.0);
    for (sh_idx, sh_nodes) in shells.iter().enumerate().skip(1) {
        let r     = sh_idx as f64 * ring_gap;
        let count = sh_nodes.len();
        let mut sorted: Vec<(usize, f64)> = sh_nodes.iter().map(|&ni| {
            let pa = adj[ni].iter()
                .filter(|&&nb| shell[nb] < sh_idx)
                .map(|&nb| pos[nb].1.atan2(pos[nb].0))
                .next().unwrap_or(0.0);
            (ni, pa)
        }).collect();
        sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        for (p, &(ni, _)) in sorted.iter().enumerate() {
            let angle = std::f64::consts::TAU * p as f64 / count as f64;
            pos[ni] = (r * angle.cos() + jitter(ni, 1, jit), r * angle.sin() + jitter(ni, 2, jit));
        }
    }
}

fn translate(pos: &mut [(f64, f64)], members: &[usize], dx: f64, dy: f64) {
    for &i in members { pos[i].0 += dx; pos[i].1 += dy; }
}

fn bbox(pos: &[(f64, f64)], members: &[usize]) -> (f64, f64, f64, f64) {
    members.iter().fold((f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY),
        |(x0, x1, y0, y1), &i| (x0.min(pos[i].0), x1.max(pos[i].0), y0.min(pos[i].1), y1.max(pos[i].1)))
}

fn extent(pos: &[(f64, f64)]) -> f64 { pos.iter().map(|p| p.0.hypot(p.1)).fold(0.0, f64::max) }

fn base_clamp(n: usize) -> f64 { (n as f64 * 15.0).max(350.0) }

/// Islands (each laid out around the origin) packed in rings just outside
/// the main component. Each island is re-centred on its centroid and turned
/// so its long axis runs along the ring (a chain lies tangent instead of
/// sticking out), and sits as close in as its own thickness allows; a ring
/// takes islands, in size order, until its circumference is used up, then
/// the next ring starts outside it. The outer radius grows with √(islands),
/// not with their count, so even hundreds of small islands start compact.
fn arrange_around(pos: &mut [(f64, f64)], comps: &[Vec<usize>]) {
    if comps.len() < 2 { return; }
    let r_main  = comps[0].iter().map(|&i| pos[i].0.hypot(pos[i].1)).fold(0.0_f64, f64::max);
    let islands = &comps[1..];
    // (half length along the long axis, half thickness across it)
    let mut half: Vec<(f64, f64)> = Vec::with_capacity(islands.len());
    for m in islands {
        let c = m.len() as f64;
        let (sx, sy) = m.iter().fold((0.0, 0.0), |(sx, sy), &i| (sx + pos[i].0, sy + pos[i].1));
        translate(pos, m, -sx / c, -sy / c);
        let (xx, yy, xy) = m.iter().fold((0.0, 0.0, 0.0), |(xx, yy, xy), &i| {
            let (x, y) = pos[i];
            (xx + x * x, yy + y * y, xy + x * y)
        });
        rotate(pos, m, -0.5 * (2.0 * xy).atan2(xx - yy)); // long axis → x
        half.push(m.iter().fold((0.0_f64, 0.0_f64), |(hx, hy), &i| (hx.max(pos[i].0.abs()), hy.max(pos[i].1.abs()))));
    }

    let gap = SPRING_L;
    let tau = std::f64::consts::TAU;
    let mut inner = r_main + gap; // nothing in this ring comes closer to the origin than this
    let mut k = 0;
    while k < islands.len() {
        let start = k;
        let mut span = 0.0; // angle used
        while k < islands.len() {
            let (hx, hy) = half[k];
            let da = (2.0 * hx + gap) / (inner + hy);
            if k > start && span + da > tau { break; }
            span += da; k += 1;
        }
        // spread the ring's islands around the whole circle (each keeps at
        // least its own angle)
        let scale = tau / span.max(1e-9);
        let (mut at, mut outer) = (0.0, inner);
        for (j, m) in islands.iter().enumerate().take(k).skip(start) {
            let (hx, hy) = half[j];
            let r  = inner + hy;
            let da = (2.0 * hx + gap) / r;
            let a  = (at + da / 2.0) * scale - std::f64::consts::FRAC_PI_2;
            rotate(pos, m, a + std::f64::consts::FRAC_PI_2); // x (long axis) → tangent
            translate(pos, m, r * a.cos(), r * a.sin());
            at += da;
            outer = outer.max((r + hy).hypot(hx));
        }
        inner = outer + gap;
    }
}

fn rotate(pos: &mut [(f64, f64)], members: &[usize], a: f64) {
    let (s, c) = a.sin_cos();
    for &i in members { let (x, y) = pos[i]; pos[i] = (x * c - y * s, x * s + y * c); }
}

/// Islands to the right of the main component, left to right.
/// `align_top`: tops level with the main component (trees); else centred on y = 0.
fn arrange_right(pos: &mut [(f64, f64)], comps: &[Vec<usize>], gap: f64, align_top: bool) {
    if comps.len() < 2 { return; }
    let (_, mut max_x, main_y0, _) = bbox(pos, &comps[0]);
    for m in &comps[1..] {
        let (x0, x1, y0, y1) = bbox(pos, m);
        let dx = max_x + gap - x0;
        let dy = if align_top { main_y0 - y0 } else { -(y0 + y1) / 2.0 };
        translate(pos, m, dx, dy);
        max_x = x1 + dx;
    }
}

/// Orphans for the radial/disk family: a compact arc at gravity equilibrium
/// (r ≈ perturbation/GRAVITY ≈ 25wu) in the bottom-left quadrant, visually
/// distinct from the rings. With nothing connected, a phyllotaxis disk instead.
fn place_orphans_near_hub(orphans: &[usize], has_connected: bool, pos: &mut [(f64, f64)]) {
    let dc = orphans.len();
    if dc == 0 { return; }
    if !has_connected { return place_orphans_spiral(orphans, 0.0, SPRING_L, pos); }
    let orphan_r  = 28.0_f64;
    let arc_start = std::f64::consts::PI * 1.15;
    let arc_span  = std::f64::consts::PI * 0.45;
    for (i, &ni) in orphans.iter().enumerate() {
        let t     = if dc == 1 { 0.5 } else { i as f64 / (dc - 1) as f64 };
        let angle = arc_start + arc_span * t;
        pos[ni] = (orphan_r * angle.cos() + jitter(ni, 7, 8.0), orphan_r * angle.sin() + jitter(ni, 8, 8.0));
    }
}

/// Orphans in a phyllotaxis spiral centred at (cx, 0) — no straight-line
/// alignment possible, so they can never look like they sit on a connection.
fn place_orphans_spiral(orphans: &[usize], cx: f64, spacing: f64, pos: &mut [(f64, f64)]) {
    let cluster_r = spacing * (orphans.len() as f64).sqrt().max(1.0) * 0.55;
    let golden    = std::f64::consts::TAU * (1.0 - 1.0 / 1.618033988749895);
    for (i, &ni) in orphans.iter().enumerate() {
        let r     = (i as f64 / orphans.len().max(1) as f64).sqrt() * cluster_r;
        let theta = i as f64 * golden;
        pos[ni] = (cx + r * theta.cos(), r * theta.sin());
    }
}

/// Radial shell layout: hub at origin, BFS shells as concentric rings.
/// Nodes within each ring sorted by parent angle to minimise crossings.
pub fn positions_radial(deg: &[usize], edges: &[(usize, usize)]) -> Vec<(f64, f64)> {
    let n = deg.len();
    if n == 0 { return Vec::new(); }
    let p = split_components(deg, edges);
    let mut pos   = vec![(0.0_f64, 0.0_f64); n];
    let mut shell = vec![UNSEEN; n];
    let mut comps = Vec::new();
    for &hub in &p.comps {
        let shells   = bfs_shells(&p.adj, hub, &mut shell);
        let size: usize = shells.iter().map(Vec::len).sum();
        let n_shells = shells.len().saturating_sub(1).max(1);
        let ring_gap = SPRING_L.max(80.0 + size as f64 * 4.0 / n_shells as f64);
        place_rings(&p.adj, &shells, &shell, ring_gap, 12.0, &mut pos);
        comps.push(shells.concat());
    }
    arrange_around(&mut pos, &comps);
    place_orphans_near_hub(&p.orphans, !comps.is_empty(), &mut pos);
    pos
}

/// Ring layout: all nodes on a single circle — main component BFS-ordered
/// from its hub, then each island, then the orphans.
pub fn positions_ring(deg: &[usize], edges: &[(usize, usize)]) -> Vec<(f64, f64)> {
    let n = deg.len();
    if n == 0 { return Vec::new(); }
    let p = split_components(deg, edges);
    let mut order   = Vec::with_capacity(n);
    let mut visited = vec![false; n];
    for &hub in &p.comps {
        let mut queue = VecDeque::new();
        queue.push_back(hub); visited[hub] = true;
        while let Some(cur) = queue.pop_front() {
            order.push(cur);
            let mut nbs: Vec<usize> = p.adj[cur].iter().copied().filter(|&nb| !visited[nb]).collect();
            nbs.sort_by_key(|&nb| std::cmp::Reverse(deg[nb]));
            for nb in nbs { visited[nb] = true; queue.push_back(nb); }
        }
    }
    order.extend_from_slice(&p.orphans);

    let r = (60.0_f64).max((380.0_f64).min(55.0 + n as f64 * 2.8));
    let mut positions = vec![(0.0_f64, 0.0_f64); n];
    for (k, &ni) in order.iter().enumerate() {
        let angle = std::f64::consts::TAU * k as f64 / n as f64;
        positions[ni] = (r * angle.cos() + jitter(ni, 1, 12.0), r * angle.sin() + jitter(ni, 2, 12.0));
    }
    positions
}

/// Disk layout: shell-based placement at SPRING_L ring gap.
/// Nodes start at their physics equilibrium distance so physics barely needs
/// to move them — edges never cross on spawn because each shell's nodes are
/// angle-sorted by their parent's position.  Physics then relaxes the layout
/// into a natural filled circle (Obsidian-style).
pub fn positions_disk(deg: &[usize], edges: &[(usize, usize)]) -> Vec<(f64, f64)> {
    let pos = place_disk(deg, edges);
    prerelax(pos, edges, deg.len())
}

/// The disk layout's shell placement alone, before `prerelax` (cheap: O(n + e)).
fn place_disk(deg: &[usize], edges: &[(usize, usize)]) -> Vec<(f64, f64)> {
    let n = deg.len();
    if n == 0 { return Vec::new(); }
    let p = split_components(deg, edges);
    let mut pos   = vec![(0.0_f64, 0.0_f64); n];
    let mut shell = vec![UNSEEN; n];
    let mut comps = Vec::new();
    for &hub in &p.comps {
        // Ring gap = SPRING_L so connected nodes begin exactly at their rest length
        let shells = bfs_shells(&p.adj, hub, &mut shell);
        place_rings(&p.adj, &shells, &shell, SPRING_L, 8.0, &mut pos);
        comps.push(shells.concat());
    }
    arrange_around(&mut pos, &comps);
    // Orphans near origin — at gravity equilibrium, won't drift
    place_orphans_near_hub(&p.orphans, !comps.is_empty(), &mut pos);
    pos
}

/// Generic layered layout used by both Column and Row.
/// `column_mode=true`  → connected shells advance left→right, nodes spread vertically.
/// `column_mode=false` → connected shells advance top→bottom, nodes spread horizontally.
/// Islands sit to the RIGHT of the main component; orphans in a separate
/// cluster to the right of everything — never inline with connected nodes,
/// so they cannot be mistaken for part of a chain.
pub fn positions_layered(deg: &[usize], edges: &[(usize, usize)], column_mode: bool) -> Vec<(f64, f64)> {
    let n = deg.len();
    if n == 0 { return Vec::new(); }
    let p = split_components(deg, edges);

    // main_gap ≈ SPRING_L so physics equilibrium matches the layout.
    // cross_gap larger so labels never crowd each other.
    let main_gap  = SPRING_L * 1.1;
    let cross_gap = SPRING_L * 1.8;

    let mut positions = vec![(0.0_f64, 0.0_f64); n];
    let mut shell     = vec![UNSEEN; n];
    let mut comps     = Vec::new();
    for &hub in &p.comps {
        let shells     = bfs_shells(&p.adj, hub, &mut shell);
        let total_main = (shells.len() as f64 - 1.0) * main_gap;
        for (sh_idx, sh_nodes) in shells.iter().enumerate() {
            let main  = sh_idx as f64 * main_gap - total_main / 2.0;
            let count = sh_nodes.len();
            let total_cross = (count as f64 - 1.0) * cross_gap;

            // Sort by parent's cross-axis position so edges don't cross between shells
            let mut sorted: Vec<(usize, f64)> = sh_nodes.iter().map(|&ni| {
                let cp = p.adj[ni].iter()
                    .filter(|&&nb| shell[nb] < sh_idx)
                    .map(|&nb| if column_mode { positions[nb].1 } else { positions[nb].0 })
                    .next().unwrap_or(0.0);
                (ni, cp)
            }).collect();
            sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

            for (k, &(ni, _)) in sorted.iter().enumerate() {
                let cross = k as f64 * cross_gap - total_cross / 2.0;
                positions[ni] = if column_mode { (main, cross) } else { (cross, main) };
            }
        }
        comps.push(shells.concat());
    }
    arrange_right(&mut positions, &comps, cross_gap, false);

    // Orphans always go to the RIGHT of everything connected, spread in y.
    // This guarantees no drawn edge (which stays within the connected x-range)
    // can pass through an orphan node regardless of layout mode.
    if !p.orphans.is_empty() {
        let max_conn_x = comps.iter().flatten().map(|&i| positions[i].0).fold(0.0_f64, f64::max);
        place_orphans_spiral(&p.orphans, max_conn_x + main_gap * 1.8, cross_gap, &mut positions);
    }
    positions
}

/// Proper top-down tree using subtree-width allocation (simplified Reingold–Tilford).
/// Each subtree occupies a non-overlapping horizontal band, so no edges can cross.
/// Islands become their own trees to the right (tops level); orphans right of all.
pub fn positions_tree(deg: &[usize], edges: &[(usize, usize)]) -> Vec<(f64, f64)> {
    let n = deg.len();
    if n == 0 { return Vec::new(); }
    let p = split_components(deg, edges);

    let unit    = SPRING_L * 1.8; // wide enough for labels; ~matches cross-axis rest length
    let row_gap = SPRING_L * 1.1;

    let mut positions = vec![(0.0_f64, 0.0_f64); n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut depth     = vec![0usize; n];
    let mut visited   = vec![false; n];
    let mut width     = vec![1.0_f64; n];
    let mut left_edge = vec![0.0_f64; n];
    let mut comps     = Vec::new();

    for &root in &p.comps {
        // Build spanning tree via BFS; each node gets exactly one parent
        let mut bfs_ord = Vec::new();
        let mut queue   = VecDeque::new();
        queue.push_back(root); visited[root] = true;
        while let Some(cur) = queue.pop_front() {
            bfs_ord.push(cur);
            let mut nbs: Vec<usize> = p.adj[cur].iter().copied().filter(|&nb| !visited[nb]).collect();
            nbs.sort_by_key(|&nb| std::cmp::Reverse(deg[nb]));
            for nb in nbs {
                visited[nb] = true;
                children[cur].push(nb);
                depth[nb] = depth[cur] + 1;
                queue.push_back(nb);
            }
        }

        // Bottom-up: each leaf = width 1, each internal node = sum of children widths
        for &ni in bfs_ord.iter().rev() {
            if !children[ni].is_empty() {
                width[ni] = children[ni].iter().map(|&c| width[c]).sum();
            }
        }

        let max_depth = bfs_ord.iter().map(|&i| depth[i]).max().unwrap_or(0);
        let total_h   = max_depth as f64 * row_gap;
        left_edge[root] = -(width[root] * unit) / 2.0;

        // Top-down: place each node at the centre of its subtree's horizontal band
        for &ni in &bfs_ord {
            let x = left_edge[ni] + width[ni] * unit / 2.0;
            let y = depth[ni] as f64 * row_gap - total_h / 2.0;
            positions[ni] = (x, y);

            // Distribute children left-to-right within this node's band
            let mut cursor = left_edge[ni];
            for &ci in &children[ni] {
                left_edge[ci] = cursor;
                cursor += width[ci] * unit;
            }
        }
        comps.push(bfs_ord);
    }
    arrange_right(&mut positions, &comps, unit, true);

    // Disconnected nodes go to the RIGHT of the trees, spread in y.
    // Placing them below would put them in the path of the tree's vertical edges.
    if !p.orphans.is_empty() {
        let max_tree_x = comps.iter().flatten().map(|&i| positions[i].0).fold(0.0_f64, f64::max);
        place_orphans_spiral(&p.orphans, max_tree_x + unit * 1.8, unit, &mut positions);
    }
    positions
}

// ── Barnes-Hut O(n log n) repulsion ──────────────────────────────────────────

const BH_THETA: f64 = 0.9; // opening angle; lower = more accurate, higher = faster

struct BHCell {
    x0: f64, y0: f64, x1: f64, y1: f64, // bounds
    mass: f64,
    cx: f64, cy: f64,                    // center of mass
    ch: [u32; 4],                        // children; u32::MAX = absent
    body: i32,                           // ≥0 leaf idx, -1 internal, -2 empty
}

impl BHCell {
    fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1, mass: 0.0, cx: 0.0, cy: 0.0, ch: [u32::MAX; 4], body: -2 }
    }
    fn quad(&self, x: f64, y: f64) -> usize {
        let mx = (self.x0 + self.x1) * 0.5;
        let my = (self.y0 + self.y1) * 0.5;
        (x >= mx) as usize | (((y >= my) as usize) << 1)
    }
    fn child_bounds(&self, q: usize) -> (f64, f64, f64, f64) {
        let mx = (self.x0 + self.x1) * 0.5;
        let my = (self.y0 + self.y1) * 0.5;
        match q {
            0 => (self.x0, self.y0, mx,       my      ),
            1 => (mx,      self.y0, self.x1,  my      ),
            2 => (self.x0, my,      mx,       self.y1 ),
            _ => (mx,      my,      self.x1,  self.y1 ),
        }
    }
}

fn bh_insert(pool: &mut Vec<BHCell>, start: usize, bx: f64, by: f64, bi: usize) {
    let mut ci = start;
    loop {
        match pool[ci].body {
            -2 => {
                pool[ci].body = bi as i32;
                pool[ci].mass = 1.0;
                pool[ci].cx   = bx;
                pool[ci].cy   = by;
                return;
            }
            -1 => {
                let m = pool[ci].mass;
                pool[ci].cx   = (pool[ci].cx * m + bx) / (m + 1.0);
                pool[ci].cy   = (pool[ci].cy * m + by) / (m + 1.0);
                pool[ci].mass = m + 1.0;
                let q  = pool[ci].quad(bx, by);
                let ch = pool[ci].ch[q];
                if ch == u32::MAX {
                    let (cx0, cy0, cx1, cy1) = pool[ci].child_bounds(q);
                    let new = pool.len() as u32;
                    pool.push(BHCell::new(cx0, cy0, cx1, cy1));
                    pool[ci].ch[q] = new;
                    ci = new as usize;
                } else {
                    ci = ch as usize;
                }
            }
            _ => {
                // Leaf → split into internal, then re-insert both bodies
                let ob = pool[ci].body as usize;
                let (ox, oy) = (pool[ci].cx, pool[ci].cy);
                if (ox - bx).abs() + (oy - by).abs() < 1e-9 {
                    pool[ci].mass += 1.0; // coincident: just accumulate mass
                    return;
                }
                pool[ci].body = -1;
                pool[ci].ch   = [u32::MAX; 4];
                pool[ci].mass = 0.0;
                bh_insert(pool, ci, ox, oy, ob);
                bh_insert(pool, ci, bx, by, bi);
                return;
            }
        }
    }
}

fn bh_force(pool: &[BHCell], bx: f64, by: f64, self_bi: usize, stack: &mut Vec<usize>) -> (f64, f64) {
    let t2 = BH_THETA * BH_THETA;
    stack.clear();
    stack.push(0);
    let (mut fx, mut fy) = (0.0f64, 0.0f64);
    while let Some(ci) = stack.pop() {
        let c = &pool[ci];
        if c.body == -2 || c.body == self_bi as i32 { continue; }
        let dx = c.cx - bx;
        let dy = c.cy - by;
        let d2 = dx * dx + dy * dy;
        if d2 < 0.01 { continue; }
        let size = (c.x1 - c.x0).max(c.y1 - c.y0);
        if c.body >= 0 || size * size < t2 * d2 {
            let d = d2.sqrt();
            let f = REPULSION * c.mass / (d2 * d);
            fx -= dx * f;
            fy -= dy * f;
        } else {
            for &ch in &c.ch { if ch != u32::MAX { stack.push(ch as usize); } }
        }
    }
    (fx, fy)
}

fn bh_build(pts: &[(f64, f64)]) -> Vec<BHCell> {
    let n = pts.len();
    if n == 0 { return vec![]; }
    let (mut x0, mut x1, mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for &(x, y) in pts {
        x0 = x0.min(x); x1 = x1.max(x);
        y0 = y0.min(y); y1 = y1.max(y);
    }
    let pad = 100.0;
    let mut pool = Vec::with_capacity(n * 4);
    pool.push(BHCell::new(x0 - pad, y0 - pad, x1 + pad, y1 + pad));
    for (i, &(x, y)) in pts.iter().enumerate() { bh_insert(&mut pool, 0, x, y, i); }
    pool
}


/// Run simplified physics on `pos` until near-equilibrium, then return the
/// settled coordinates.  Operates on a pure copy — the Graph is not mutated.
fn prerelax(
    mut pos: Vec<(f64, f64)>,
    edges:   &[(usize, usize)],
    n:       usize,
) -> Vec<(f64, f64)> {
    if n == 0 { return pos; }
    let steps = (2_000_000usize / (n * n).max(1)).clamp(30, 240);
    let mut vel: Vec<(f64, f64)> = vec![(0.0, 0.0); n];
    let dt    = 1.0 / 60.0_f64;
    let damp  = DAMPING.powf(dt / BASE_DT);
    // never tighter than the placement itself: islands packed outside the
    // main component must not be flattened onto the rim
    let max_r = base_clamp(n).max(extent(&pos) * 1.05);
    const MV: f64 = 650.0;

    for _ in 0..steps {
        let mut fx = vec![0.0f64; n];
        let mut fy = vec![0.0f64; n];

        {
            let bh = bh_build(&pos);
            let rep_forces: Vec<(f64, f64)> = pos.par_iter().enumerate()
                .map_with(Vec::with_capacity(64), |stk, (i, &(x, y))| bh_force(&bh, x, y, i, stk))
                .collect();
            for (i, (rfx, rfy)) in rep_forces.into_iter().enumerate() {
                fx[i] += rfx; fy[i] += rfy;
            }
        }
        for &(si, di) in edges {
            let dx   = pos[di].0 - pos[si].0;
            let dy   = pos[di].1 - pos[si].1;
            let dist = dx.hypot(dy).max(1e-4);
            let f    = SPRING_K * (dist - SPRING_L) / dist;
            fx[si] += dx * f; fy[si] += dy * f;
            fx[di] -= dx * f; fy[di] -= dy * f;
        }
        for i in 0..n {
            vel[i].0 = (vel[i].0 + (fx[i] - pos[i].0 * GRAVITY) * dt) * damp;
            vel[i].1 = (vel[i].1 + (fy[i] - pos[i].1 * GRAVITY) * dt) * damp;
            let spd = vel[i].0.hypot(vel[i].1);
            if spd > MV { vel[i].0 = vel[i].0 / spd * MV; vel[i].1 = vel[i].1 / spd * MV; }
            pos[i].0 += vel[i].0 * dt;
            pos[i].1 += vel[i].1 * dt;
            let d = pos[i].0.hypot(pos[i].1);
            if d > max_r { let s = max_r / d; pos[i].0 *= s; pos[i].1 *= s; vel[i] = (0.0, 0.0); }
        }
    }
    pos
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn lcg_uniform(seed: u64) -> f64 {
    let x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (x >> 11) as f64 / (1u64 << 53) as f64
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [u8; 3] {
    let i = (h * 6.0) as u32;
    let f = h * 6.0 - i as f64;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i % 6 { 0=>(v,t,p), 1=>(q,v,p), 2=>(p,v,t), 3=>(p,q,v), 4=>(t,p,v), _=>(v,p,q) };
    [(r*255.0) as u8, (g*255.0) as u8, (b*255.0) as u8]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{compute_view, DailiesMode, OrphansMode, ViewFilter};

    fn node(id: &str, title: &str, file: &str) -> RawNode {
        RawNode { id: id.into(), title: title.into(), file: file.into(), ..Default::default() }
    }

    fn e(a: &str, b: &str) -> (String, String) { (a.into(), b.into()) }

    /// main: hub—a, hub—b, a—b ; island: c—d ; orphan: o ; daily d0 linked to a and to x
    fn fixture() -> Graph {
        Graph::new(vec![
            node("hub", "Hub", "/n/hub.org"), node("a", "A", "/n/a.org"), node("b", "B", "/n/b.org"),
            node("c", "C", "/n/c.org"), node("d", "D", "/n/d.org"), node("o", "O", "/n/o.org"),
            node("d0", "2026-01-01", "/n/daily/2026-01-01.org"), node("x", "X", "/n/x.org"),
        ], vec![e("hub", "a"), e("hub", "b"), e("a", "b"), e("c", "d"), e("d0", "a"), e("d0", "x")])
    }

    fn hide_dailies(g: &mut Graph) {
        let v = compute_view(g, &ViewFilter { dailies: DailiesMode::Hide, ..ViewFilter::default() });
        g.apply_view(&v);
    }

    fn snapshot(g: &Graph) -> Vec<(f64, f64, f64, f64)> {
        g.node_list.iter().map(|n| (n.x, n.y, n.vx, n.vy)).collect()
    }

    #[test]
    fn edges_deduped_and_degree_counts_each_relation_once() {
        let g1 = Graph::new(vec![node("a", "A", "/a"), node("b", "B", "/b")], vec![e("a", "b")]);
        let g2 = Graph::new(vec![node("a", "A", "/a"), node("b", "B", "/b")],
                            vec![e("a", "b"), e("b", "a"), e("a", "b"), e("a", "a"), e("a", "zz")]);
        assert_eq!(g2.edges, vec![(0, 1)]);
        assert_eq!(g2.adj, vec![vec![1], vec![0]]);
        assert_eq!(g1.node_list[0].degree, g2.node_list[0].degree);
        assert_eq!(g1.node_list[0].radius, g2.node_list[0].radius);
    }

    #[test]
    fn containment_resolves_to_index_and_is_not_an_edge() {
        let mut h = node("h", "Heading", "/n/a.org");
        h.level = 1;
        h.parent_file = Some("f".into());
        let mut late = node("h2", "Other heading", "/n/b.org");
        late.level = 2; // parent listed after it, no parent_file from the db
        let g = Graph::new(vec![node("f", "File", "/n/a.org"), h, late, node("fb", "B", "/n/b.org")], vec![]);
        assert_eq!(g.node_list[1].parent_file, Some(0));
        assert_eq!(g.node_list[2].parent_file, Some(3));
        assert_eq!(g.node_list[0].parent_file, None);
        assert!(g.edges.is_empty());
        assert!(g.node_list.iter().all(|n| n.degree == 0));

        // file with two level-0 nodes: the first wins (as in db.rs); a file
        // with no level-0 node leaves its headings unparented
        let head = |id: &str, file: &str| { let mut h = node(id, id, file); h.level = 1; h };
        let g = Graph::new(vec![head("h", "/c.org"), node("f1", "F1", "/c.org"), node("f2", "F2", "/c.org"),
                                head("lone", "/nofile.org")], vec![]);
        assert_eq!(g.node_list[0].parent_file, Some(1));
        assert_eq!(g.node_list[3].parent_file, None);
    }

    #[test]
    fn layouts_never_move_hidden_nodes() {
        let mut g = fixture();
        hide_dailies(&mut g);
        let d0 = g.nodes["d0"];
        assert!(!g.is_visible(d0));
        // park the hidden node somewhere no layout would put it
        g.node_list[d0].x = 12345.0; g.node_list[d0].y = -6789.0;
        let layouts: [(&str, Vec<(f64, f64)>); 6] = [
            ("disk", g.positions_disk()), ("radial", g.positions_radial()), ("ring", g.positions_ring()),
            ("column", g.positions_column()), ("row", g.positions_row()), ("tree", g.positions_tree()),
        ];
        for (name, pos) in &layouts {
            assert_eq!(pos.len(), g.node_list.len());
            assert_eq!(pos[d0], (12345.0, -6789.0), "{name} moved a hidden node");
        }
        // an animated transition leaves it alone too
        let targets = g.positions_tree();
        g.begin_layout_transition(targets, true);
        for _ in 0..200 { g.step(1.0 / 60.0); }
        assert!(!g.is_layout_animating());
        assert_eq!((g.node_list[d0].x, g.node_list[d0].y), (12345.0, -6789.0));
    }

    #[test]
    fn physics_never_moves_hidden_nodes() {
        let mut g = fixture();
        hide_dailies(&mut g);
        let d0 = g.nodes["d0"];
        g.node_list[d0].vx = 500.0; // would fly if it were simulated
        g.apply_view(&compute_view(&g, &ViewFilter { dailies: DailiesMode::Hide, ..ViewFilter::default() }));
        let before = snapshot(&g);
        g.physics_on = true;
        for _ in 0..30 { g.step(1.0 / 60.0); }
        let after = snapshot(&g);
        assert_eq!((after[d0].0, after[d0].1), (before[d0].0, before[d0].1));
        assert_eq!((after[d0].2, after[d0].3), (0.0, 0.0), "hiding zeroes velocity");
        assert!((0..g.node_list.len()).any(|i| i != d0 && after[i] != before[i]), "visible nodes still simulate");
    }

    /// Hidden nodes don't repel, pull or count: a graph with hidden extras
    /// steps exactly like the graph of its visible nodes alone.
    #[test]
    fn hidden_nodes_exert_no_force_and_are_not_counted() {
        let vis = vec![node("a", "A", "/a"), node("b", "B", "/b"), node("c", "C", "/c")];
        let ve  = vec![e("a", "b"), e("b", "c")];
        let mut with_hidden = vis.clone();
        with_hidden.push(node("h1", "2026-01-01", "/h1"));
        with_hidden.push(node("h2", "H2", "/daily/h2"));
        let mut he = ve.clone();
        he.push(e("h1", "a")); he.push(e("h2", "c")); he.push(e("h1", "h2"));

        let mut small = Graph::new(vis, ve);
        let mut big   = Graph::new(with_hidden, he);
        hide_dailies(&mut big);
        assert_eq!(big.vis_idx, vec![0, 1, 2]);

        // same layout for the visible part
        let (ls, lb) = (small.positions_disk(), big.positions_disk());
        assert_eq!(&lb[..3], &ls[..]);
        let (ls, lb) = (small.positions_tree(), big.positions_tree());
        assert_eq!(&lb[..3], &ls[..]);

        // same physics from the same start
        for i in 0..3 {
            let s = small.node_list[i].clone();
            let b = &mut big.node_list[i];
            b.x = s.x; b.y = s.y; b.vx = s.vx; b.vy = s.vy;
        }
        small.physics_on = true; big.physics_on = true;
        for _ in 0..20 { small.step(1.0 / 60.0); big.step(1.0 / 60.0); }
        for i in 0..3 {
            assert_eq!((big.node_list[i].x, big.node_list[i].y), (small.node_list[i].x, small.node_list[i].y));
        }
    }

    #[test]
    fn layout_orphans_use_visible_degree() {
        let mut g = fixture();
        let id = |g: &Graph, s: &str| g.nodes[s];
        let col = g.positions_column();
        let (hub, a, b, c, d, o) = (id(&g, "hub"), id(&g, "a"), id(&g, "b"), id(&g, "c"), id(&g, "d"), id(&g, "o"));
        let x = id(&g, "x");
        let main_max_x = [hub, a, b, x, id(&g, "d0")].iter().map(|&i| col[i].0).fold(f64::MIN, f64::max);
        // island c—d is its own component to the right of the main one, not in the orphan cluster
        assert!(col[c].0 > main_max_x && col[d].0 > main_max_x);
        assert!(col[o].0 > col[c].0.max(col[d].0), "orphans right of every component");

        // Hide dailies: x's only link was to the daily, so x is now laid out as an orphan (A4)
        hide_dailies(&mut g);
        let col = g.positions_column();
        let island_max = col[c].0.max(col[d].0);
        assert!(col[x].0 > island_max, "x is visible-degree 0 → orphan cluster");
        assert!(col[o].0 > island_max);
    }

    /// With links_to_dailies_count = false an orphan can still have a drawn
    /// edge; layouts must still put exactly the view's orphans in the orphan
    /// cluster (A4), not treat x — dd as an island.
    #[test]
    fn layouts_follow_the_view_orphan_mask() {
        let mut g = Graph::new(vec![
            node("hub", "Hub", "/n/hub.org"), node("a", "A", "/n/a.org"), node("b", "B", "/n/b.org"),
            node("x", "X", "/n/x.org"), node("dd", "2026-01-02", "/n/dd.org"),
            node("y", "Y", "/n/y.org"), node("d1", "2026-01-01", "/n/d1.org"),
        ], vec![e("hub", "a"), e("hub", "b"), e("a", "b"), e("x", "dd"), e("y", "d1"), e("d1", "a")]);
        let id = |g: &Graph, s: &str| g.nodes[s];
        let filter = ViewFilter { links_to_dailies_count: false, ..ViewFilter::default() };
        let v = compute_view(&g, &filter);
        g.apply_view(&v);
        let orphans: Vec<&str> = ["hub", "a", "b", "x", "dd", "y", "d1"].into_iter().filter(|s| v.orphan[id(&g, s)]).collect();
        assert_eq!(orphans, vec!["x", "dd", "y"]);
        assert_eq!(v.visible_degree[id(&g, "x")], 1, "x is still drawn joined to dd");
        for (name, pos) in [("column", g.positions_column()), ("row", g.positions_row()), ("tree", g.positions_tree())] {
            let conn_max = ["hub", "a", "b", "d1"].iter().map(|s| pos[id(&g, s)].0).fold(f64::MIN, f64::max);
            for s in &orphans { assert!(pos[id(&g, s)].0 > conn_max, "{name}: view orphan {s} not in the orphan cluster"); }
        }
        let radial = g.positions_radial();
        for s in &orphans { let p = radial[id(&g, s)]; assert!(p.0.hypot(p.1) < 40.0, "radial: {s} not with the orphans"); }

        // default rule: x — dd is a real island again
        let v = compute_view(&g, &ViewFilter::default());
        g.apply_view(&v);
        let col = g.positions_column();
        let main_max = ["hub", "a", "b", "d1", "y"].iter().map(|s| col[id(&g, s)].0).fold(f64::MIN, f64::max);
        assert!(col[id(&g, "x")].0 > main_max && col[id(&g, "dd")].0 > main_max);
        assert!(!v.orphan[id(&g, "x")]);
    }

    fn star_with_islands(leaves: usize, islands: &[usize], orphans: usize) -> (Vec<RawNode>, Vec<(String, String)>) {
        let mut nodes = vec![node("hub", "Hub", "/n/hub.org")];
        let mut edges = Vec::new();
        for i in 0..leaves { let s = format!("l{i}"); nodes.push(node(&s, &s, "/n/l.org")); edges.push(e("hub", &s)); }
        for (k, &len) in islands.iter().enumerate() {
            for j in 0..len {
                let s = format!("i{k}_{j}");
                nodes.push(node(&s, &s, "/n/i.org"));
                if j > 0 { edges.push(e(&s, &format!("i{k}_{}", j - 1))); }
            }
        }
        for i in 0..orphans { let s = format!("o{i}"); nodes.push(node(&s, &s, "/n/o.org")); }
        (nodes, edges)
    }

    /// Island packing grows with √islands, never overlaps the main
    /// component, and keeps islands apart.
    #[test]
    fn islands_pack_compactly_without_overlap() {
        let extent_for = |k: usize| {
            let (nodes, edges) = star_with_islands(8, &vec![2; k], 0);
            let g = Graph::new(nodes, edges);
            let pos = g.positions_radial();
            let main_r = (0..9).map(|i| pos[i].0.hypot(pos[i].1)).fold(0.0, f64::max);
            let isl: Vec<(usize, (f64, f64))> = (9..pos.len()).map(|i| ((i - 9) / 2, pos[i])).collect();
            for &(k1, p) in &isl {
                assert!(p.0.hypot(p.1) > main_r + SPRING_L * 0.9, "island inside the main component");
                for &(k2, q) in &isl {
                    if k1 != k2 { assert!((p.0 - q.0).hypot(p.1 - q.1) > SPRING_L * 0.5, "islands {k1} and {k2} overlap"); }
                }
            }
            extent(&pos)
        };
        let (e50, e200) = (extent_for(50), extent_for(200));
        // the old single ring grew linearly: ~k·(2·r + gap)/τ ≈ 10 500 at k = 200
        assert!(e200 < 2.5 * e50, "extent grows faster than √k: {e50:.0} → {e200:.0}");
        assert!(e200 < 3000.0, "200 pairs spread to {e200:.0}");
    }

    /// Disk: islands packed outside the main component used to land beyond
    /// the pre-relax clamp and get flattened onto it (island edges squashed
    /// to ~40 wu), then physics held them there.
    #[test]
    fn disk_islands_are_not_squashed_against_the_clamp() {
        // the owner's db shape (12-node main, 5/3/2-chains, 13 orphans), and a
        // long chain whose placement reaches far past 15·n
        for islands in [vec![5, 3, 2, 2, 2, 2, 2, 2], vec![30]] {
            let (nodes, edges) = star_with_islands(11, &islands, 13);
            let mut g = Graph::new(nodes, edges);
            for &(a, b) in &g.edges {
                let (p, q) = (&g.node_list[a], &g.node_list[b]);
                let len = (p.x - q.x).hypot(p.y - q.y);
                assert!(len > SPRING_L * 0.7, "edge {} — {} squashed to {len:.0}", p.id, q.id);
            }
            let r = g.node_list.iter().map(|n| n.x.hypot(n.y)).fold(0.0, f64::max);
            assert!(r < g.clamp_radius() * 0.99, "a node sits on the clamp rim");
            // the first physics frame moves nothing further than MAX_V allows
            let before: Vec<(f64, f64)> = g.node_list.iter().map(|n| (n.x, n.y)).collect();
            g.resume_physics();
            g.step(1.0 / 60.0);
            for (nd, b) in g.node_list.iter().zip(&before) { assert!((nd.x - b.0).hypot(nd.y - b.1) <= 650.0 / 60.0 + 1e-9); }
        }
    }

    /// A filter change must not shrink the clamp: hiding most nodes used to
    /// snap every visible node beyond the smaller radius inward in one frame.
    #[test]
    fn filter_change_never_snaps_nodes_inward() {
        let (nodes, edges) = star_with_islands(11, &[5, 3, 2, 2, 2, 2, 2, 2], 200);
        let mut g = Graph::new(nodes, edges);
        // where a full-graph layout of 232 nodes can leave the connected ones:
        // well inside the clamp for 232, well outside 15 × the 32 left visible
        let far = g.node_list.iter().filter(|n| n.degree > 0).map(|n| n.x.hypot(n.y)).fold(0.0, f64::max);
        for nd in g.node_list.iter_mut().filter(|n| n.degree > 0) { nd.x *= 1000.0 / far; nd.y *= 1000.0 / far; }
        let clamp = g.clamp_radius();
        assert!(clamp > 1000.0);
        let v = compute_view(&g, &ViewFilter { orphans: OrphansMode::Hide, ..ViewFilter::default() });
        assert!(v.counts.visible * 2 < v.counts.total, "the filter hides most nodes");
        g.apply_view(&v);
        assert_eq!(g.clamp_radius(), clamp, "a filter never resizes the clamp");
        let before: Vec<(f64, f64)> = g.node_list.iter().map(|n| (n.x, n.y)).collect();
        g.physics_on = true;
        g.step(1.0 / 60.0);
        for (nd, b) in g.node_list.iter().zip(&before) {
            let moved = (nd.x - b.0).hypot(nd.y - b.1);
            assert!(moved <= 650.0 / 60.0 + 1e-9, "{} jumped {moved:.1} wu in one frame", nd.id);
        }
    }

    #[test]
    fn empty_graph_lets_physics_idle() {
        let mut g = Graph::new(vec![], vec![]);
        g.resume_physics();
        g.step(1.0 / 60.0);
        assert!(!g.physics_on, "no nodes → physics off, so the app can drop to its idle tier");
    }

    /// A saved filter lays out the visible subset once (no full layout first):
    /// same visible positions as laying the filtered view out after the fact.
    #[test]
    fn new_with_view_lays_out_the_visible_subset_directly() {
        let (mut nodes, mut edges) = star_with_islands(6, &[3, 2], 5);
        nodes.push(node("d0", "2026-01-01", "/n/daily/d0.org"));
        edges.push(e("d0", "l0"));
        let filter = ViewFilter { dailies: DailiesMode::Hide, orphans: OrphansMode::Hide, ..ViewFilter::default() };
        let direct = Graph::new_with_view(nodes.clone(), edges.clone(), &filter);
        let mut after = Graph::new(nodes.clone(), edges.clone());
        let v = compute_view(&after, &filter);
        after.apply_view(&v);
        let relaid = after.positions_disk();
        assert_eq!(direct.vis_idx, after.vis_idx);
        for &i in &direct.vis_idx { assert_eq!((direct.node_list[i].x, direct.node_list[i].y), relaid[i]); }
        assert!(direct.node_list.iter().all(|n| n.x.is_finite() && n.y.is_finite()), "hidden nodes placed too");
        // nothing hidden → exactly Graph::new
        let plain = Graph::new_with_view(nodes.clone(), edges.clone(), &ViewFilter::default());
        let new   = Graph::new(nodes, edges);
        for (p, q) in plain.node_list.iter().zip(&new.node_list) { assert_eq!((p.x, p.y), (q.x, q.y)); }
    }

    #[test]
    fn radial_islands_sit_outside_main_and_orphans_stay_near_hub() {
        let g = fixture();
        let p = g.positions_radial();
        let id = |s: &str| g.nodes[s];
        let main_r = ["hub", "a", "b", "x", "d0"].iter().map(|s| p[id(s)].0.hypot(p[id(s)].1)).fold(0.0, f64::max);
        for s in ["c", "d"] { assert!(p[id(s)].0.hypot(p[id(s)].1) > main_r, "island node {s} overlaps main"); }
        let o = p[id("o")];
        assert!(o.0.hypot(o.1) < 40.0);
    }

    #[test]
    fn layouts_handle_empty_and_all_orphan_views() {
        let mut g = fixture();
        let only = compute_view(&g, &ViewFilter { orphans: OrphansMode::Only, ..ViewFilter::default() });
        g.apply_view(&only);
        assert_eq!(g.vis_idx, vec![g.nodes["o"]]);
        for pos in [g.positions_disk(), g.positions_radial(), g.positions_ring(), g.positions_column(), g.positions_tree()] {
            assert!(pos.iter().all(|p| p.0.is_finite() && p.1.is_finite()));
        }
        let empty = Graph::new(vec![], vec![]);
        assert!(empty.positions_disk().is_empty());
        let mut nothing = fixture();
        nothing.apply_view(&ViewState { visible: vec![false; 8], ..ViewState::default() });
        nothing.physics_on = true;
        nothing.step(1.0 / 60.0);
        assert!(!nothing.physics_on, "nothing visible → physics idles");
    }

    #[test]
    fn bfs_does_not_cross_hidden_nodes() {
        let mut g = fixture();
        let (a, x) = (g.nodes["a"], g.nodes["x"]);
        assert!(g.bfs_visible(a, 2)[x], "a — d0 — x while the daily is shown");
        hide_dailies(&mut g);
        assert!(!g.bfs_visible(a, 3)[x], "the hidden daily no longer bridges a and x");
        assert!(g.bfs_visible(a, 1)[g.nodes["hub"]]);
    }

    #[test]
    fn stale_view_is_ignored() {
        let mut g = fixture();
        g.apply_view(&ViewState { visible: vec![false; 3], ..ViewState::default() });
        assert_eq!(g.vis_idx.len(), 8);
    }
}
