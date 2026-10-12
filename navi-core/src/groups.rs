//! Groups: a predicate (a query, or an automatic grouper) kept apart from how
//! it is shown. Every group has a colour; each also has `visible`, `pull`
//! (soft attraction to the group's centroid) and `hull` (a faint padded
//! outline behind its nodes).
//!
//! Precedence: manual groups are an ordered list and rank above the one
//! active automatic grouper. A node keeps EVERY group it is in (`members`);
//! the first of them that is visible supplies its colour (`primary`). A node
//! is hidden only when it is in at least one group and every one of its
//! groups is hidden (Foam's rule); nodes in no group are never hidden by
//! group visibility. Hulls and pull use full membership.
//!
//! Pipeline (see view.rs): groups are evaluated on the view WITHOUT group
//! hiding (dailies filter applied, orphans shown), then the nodes whose every
//! group is hidden are removed as an attribute filter before orphan
//! classification, then the orphan filter runs. Membership therefore never
//! depends on which groups are hidden.
//!
//! Colours are stable: manual groups store theirs; Folder / Tag / File groups
//! take a slot from a hash of their key, remembered per key (a new tag never
//! repaints an old one); Component / Community groups keep their slot across
//! recomputation by greedy max-Jaccard matching against the previous
//! partition. Ungrouped nodes and orphans use the theme's own node colour.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use crate::community;
use crate::graph::Graph;
use crate::query::{EvalCtx, Query, QueryError};
use crate::view::{compute_view, compute_view_hiding, dir_components, notes_root_depth, OrphansMode, ViewFilter, ViewState};

pub type GroupId = usize; // index into GroupResolution::groups

// ── Palette ───────────────────────────────────────────────────────────────────

pub const PALETTE_LEN: usize = 12;

// Hues interleaved so neighbouring slots differ strongly (the lowest free
// slots are what a fresh partition gets). Open Color shades: light (4-5) for
// dark backgrounds, deep (8-9) for light ones. Every entry keeps ≥ 3:1
// contrast against every theme background (tested in navi/src/theme.rs).
static PALETTE_DARK: [[u8; 3]; PALETTE_LEN] = [
    [0xff, 0x6b, 0x6b], // red
    [0x38, 0xd9, 0xa9], // teal
    [0xff, 0xd4, 0x3b], // yellow
    [0x97, 0x75, 0xfa], // violet
    [0x69, 0xdb, 0x7c], // green
    [0xf7, 0x83, 0xac], // pink
    [0x4d, 0xab, 0xf7], // blue
    [0xff, 0xa9, 0x4d], // orange
    [0xc0, 0xeb, 0x75], // lime
    [0x91, 0xa7, 0xff], // indigo
    [0x66, 0xd9, 0xe8], // cyan
    [0xe5, 0x99, 0xf7], // grape
];
static PALETTE_LIGHT: [[u8; 3]; PALETTE_LEN] = [
    [0xc9, 0x2a, 0x2a],
    [0x08, 0x7f, 0x5b],
    [0xa6, 0x5f, 0x00],
    [0x67, 0x41, 0xd9],
    [0x2b, 0x8a, 0x3e],
    [0xc2, 0x25, 0x5c],
    [0x18, 0x64, 0xab],
    [0xd9, 0x48, 0x0f],
    [0x5c, 0x94, 0x0d],
    [0x36, 0x4f, 0xc7],
    [0x0b, 0x72, 0x85],
    [0x9c, 0x36, 0xb5],
];

fn lin(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// WCAG relative luminance.
pub fn luminance(c: [u8; 3]) -> f64 { 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]) }

/// WCAG contrast ratio (1 … 21).
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The palette that reads on `bg`: the light set on dark backgrounds, the
/// deep set on light ones.
pub fn palette(bg: [u8; 3]) -> &'static [[u8; 3]; PALETTE_LEN] {
    if luminance(bg) > 0.35 { &PALETTE_LIGHT } else { &PALETTE_DARK }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupColour {
    Rgb([u8; 3]), // a manual group's own colour
    Slot(u8),     // a palette slot, resolved against the theme
}

impl GroupColour {
    pub fn rgb(self, bg: [u8; 3]) -> [u8; 3] {
        match self {
            GroupColour::Rgb(c)  => c,
            GroupColour::Slot(s) => palette(bg)[s as usize % PALETTE_LEN],
        }
    }
}

/// FNV-1a 64: stable across runs, platforms and Rust versions (unlike
/// `DefaultHasher`).
pub fn stable_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(0x0100_0000_01b3); }
    h
}

pub fn hash_slot(key: &str) -> u8 { (stable_hash(key) % PALETTE_LEN as u64) as u8 }

/// "#rrggbb" ↔ [r, g, b]
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() { return None; }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

pub fn to_hex(c: [u8; 3]) -> String { format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]) }

/// A colour for a new manual group: the first palette entry (for `bg`) no
/// existing group uses, starting from the name's hash slot; when all are
/// taken, the hash slot itself. Stored on the group, so it never shifts.
pub fn new_group_colour(groups: &[Group], name: &str, bg: [u8; 3]) -> [u8; 3] {
    let pal  = palette(bg);
    let used: Vec<[u8; 3]> = groups.iter()
        .map(|g| g.colour.unwrap_or_else(|| GroupColour::Slot(hash_slot(&format!("manual:{}", g.name))).rgb(bg))).collect();
    let h = hash_slot(&format!("manual:{name}")) as usize;
    (0..PALETTE_LEN).map(|d| pal[(h + d) % PALETTE_LEN]).find(|c| !used.contains(c)).unwrap_or(pal[h])
}

// ── Colour memory (keyed auto groups) ─────────────────────────────────────────

/// Slot per auto-group key. A known key keeps its slot; new keys (in sorted
/// order, so a fresh start is deterministic) take their hash slot, or the
/// next slot no other group of the current set uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColourMemory {
    pub slots: BTreeMap<String, u8>,
}

const MEMORY_CAP: usize = 512;

impl ColourMemory {
    pub fn assign(&mut self, keys: &[String]) -> Vec<u8> {
        let mut used = [false; PALETTE_LEN];
        for k in keys { if let Some(&s) = self.slots.get(k) { used[s as usize % PALETTE_LEN] = true; } }
        let mut fresh: Vec<&String> = keys.iter().filter(|k| !self.slots.contains_key(*k)).collect();
        fresh.sort(); fresh.dedup();
        for k in fresh {
            let h = hash_slot(k) as usize;
            let s = (0..PALETTE_LEN).map(|d| (h + d) % PALETTE_LEN).find(|&s| !used[s]).unwrap_or(h);
            used[s] = true;
            self.slots.insert(k.clone(), s as u8);
        }
        if self.slots.len() > MEMORY_CAP {
            let keep: HashSet<&String> = keys.iter().collect();
            self.slots.retain(|k, _| keep.contains(k));
        }
        keys.iter().map(|k| self.slots[k] % PALETTE_LEN as u8).collect()
    }
}

/// Slots for a new partition, matched to the previous one: pairs of (new
/// group, old group) sharing members are taken greedily by Jaccard overlap,
/// highest first; a matched group inherits the old slot; the rest take the
/// lowest slots left (cycling past PALETTE_LEN). `prev` = (slot, member ids).
pub fn match_partition(prev: &[(u8, HashSet<String>)], new: &[Vec<String>]) -> Vec<u8> {
    let mut owner: HashMap<&str, usize> = HashMap::new();
    for (j, (_, m)) in prev.iter().enumerate() { for id in m { owner.insert(id.as_str(), j); } }
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, mem) in new.iter().enumerate() {
        let mut inter: BTreeMap<usize, usize> = BTreeMap::new();
        for id in mem { if let Some(&j) = owner.get(id.as_str()) { *inter.entry(j).or_insert(0) += 1; } }
        for (j, c) in inter {
            let union = mem.len() + prev[j].1.len() - c;
            pairs.push((c as f64 / union.max(1) as f64, i, j));
        }
    }
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut slot: Vec<Option<u8>> = vec![None; new.len()];
    let mut prev_taken = vec![false; prev.len()];
    let mut used: HashSet<u8> = HashSet::new();
    for (_, i, j) in pairs {
        if slot[i].is_some() || prev_taken[j] || used.contains(&prev[j].0) { continue; }
        slot[i] = Some(prev[j].0);
        prev_taken[j] = true;
        used.insert(prev[j].0);
    }
    let mut next = 0u8;
    (0..new.len()).map(|i| slot[i].unwrap_or_else(|| {
        while used.contains(&next) && (used.len() < PALETTE_LEN) { next = (next + 1) % PALETTE_LEN as u8; }
        let s = if used.len() < PALETTE_LEN { next } else { (i % PALETTE_LEN) as u8 };
        used.insert(s);
        s
    })).collect()
}

// ── Specs (what views.json stores) ────────────────────────────────────────────

fn yes() -> bool { true }

fn de_colour<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 3]>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    match s {
        None => Ok(None),
        Some(s) => parse_hex(&s).map(Some).ok_or_else(|| serde::de::Error::custom(format!("bad colour {s:?} (want \"#rrggbb\")"))),
    }
}

fn ser_colour<S: Serializer>(c: &Option<[u8; 3]>, s: S) -> Result<S::Ok, S::Error> {
    match c { Some(c) => s.serialize_str(&to_hex(*c)), None => s.serialize_none() }
}

/// A manual group: a named query with its own colour and display toggles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub name:    String,
    #[serde(default)]
    pub query:   String,
    #[serde(default, alias = "color", deserialize_with = "de_colour", serialize_with = "ser_colour",
            skip_serializing_if = "Option::is_none")]
    pub colour:  Option<[u8; 3]>, // None: a palette slot from the name's hash
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub pull:    bool,
    #[serde(default)]
    pub hull:    bool,
}

impl Group {
    pub fn new(name: &str, query: &str) -> Self {
        Group { name: name.into(), query: query.into(), colour: None, visible: true, pull: false, hull: false }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommunityScope {
    #[default]
    All,     // the unfiltered real-link graph: communities don't move when a filter changes
    Visible, // the graph left by the dailies filter (before group hiding)
}

fn one() -> usize { 1 }
fn unit() -> f64 { 1.0 }

/// The automatic grouper; at most one is active, ranked below manual groups.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AutoGrouper {
    #[default]
    None,
    Folder {
        #[serde(default = "one")]
        depth: usize, // directory levels below the notes root
    },
    Tag,
    Component,
    Community {
        #[serde(default = "unit")]
        resolution: f64,
        #[serde(default)]
        scope:      CommunityScope,
    },
    File,
}

impl AutoGrouper {
    pub fn label(&self) -> String {
        match self {
            AutoGrouper::None                   => "None".into(),
            AutoGrouper::Folder { depth: 1 }    => "Folder".into(),
            AutoGrouper::Folder { depth }       => format!("Folder (depth {depth})"),
            AutoGrouper::Tag                    => "Tag".into(),
            AutoGrouper::Component              => "Component".into(),
            AutoGrouper::Community { .. }       => "Community".into(),
            AutoGrouper::File                   => "File".into(),
        }
    }

    /// None → Folder → Tag → Component → Community → File → None (Shift+G).
    pub fn cycled(&self) -> AutoGrouper {
        match self {
            AutoGrouper::None             => AutoGrouper::Folder { depth: 1 },
            AutoGrouper::Folder { .. }    => AutoGrouper::Tag,
            AutoGrouper::Tag              => AutoGrouper::Component,
            AutoGrouper::Component        => AutoGrouper::Community { resolution: 1.0, scope: CommunityScope::All },
            AutoGrouper::Community { .. } => AutoGrouper::File,
            AutoGrouper::File             => AutoGrouper::None,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            AutoGrouper::None => "none", AutoGrouper::Folder { .. } => "folder", AutoGrouper::Tag => "tag",
            AutoGrouper::Component => "component", AutoGrouper::Community { .. } => "community", AutoGrouper::File => "file",
        }
    }
}

/// Everything grouping needs from a view, with the manual queries compiled
/// once (rebuild it when the view changes, not per frame).
#[derive(Debug, Clone, Default)]
pub struct GroupSpec {
    pub manual:      Vec<Group>,
    pub compiled:    Vec<Result<Query, QueryError>>,
    pub auto:        AutoGrouper,
    pub auto_hidden: HashSet<String>, // keys of hidden auto groups
    pub auto_pull:   bool,
    pub auto_hull:   bool,
}

impl GroupSpec {
    pub fn new(manual: Vec<Group>, auto: AutoGrouper) -> Self {
        let compiled = manual.iter().map(|g| Query::parse(&g.query)).collect();
        GroupSpec { manual, compiled, auto, ..GroupSpec::default() }
    }

    pub fn is_empty(&self) -> bool { self.manual.is_empty() && self.auto == AutoGrouper::None }
}

// ── Resolution ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct GroupInfo {
    pub key:     String,              // manual: "manual:<name>"; auto: "<kind>:<what>"
    pub label:   String,
    pub colour:  GroupColour,
    pub manual:  bool,
    pub visible: bool,
    pub pull:    bool,
    pub hull:    bool,
    pub members: usize,               // nodes in the group that pass the dailies filter
    pub shown:   usize,               // members on screen after every filter
    pub error:   Option<QueryError>,  // manual group whose query doesn't parse (matches nothing)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupResolution {
    pub groups:  Vec<GroupInfo>,          // manual groups in order, then the auto grouper's
    pub primary: Vec<Option<GroupId>>,    // per node: the group that supplies its colour
    pub members: Vec<Vec<GroupId>>,       // per node: every group, in precedence order
    pub hidden:  Vec<bool>,               // per node: in ≥ 1 group and every one hidden
}

impl GroupResolution {
    /// Member node indices of every group, restricted to `visible` nodes.
    pub fn member_lists(&self, visible: &[bool]) -> Vec<Vec<usize>> {
        let mut out = vec![Vec::new(); self.groups.len()];
        for (i, gs) in self.members.iter().enumerate() {
            if !visible.get(i).copied().unwrap_or(false) { continue; }
            for &g in gs { out[g].push(i); }
        }
        out
    }

    /// The groups that pull, as member lists (visible members only; the
    /// physics re-checks visibility every step).
    pub fn pull_lists(&self, visible: &[bool]) -> Vec<Vec<usize>> {
        self.member_lists(visible).into_iter().zip(&self.groups)
            .filter(|(m, g)| g.pull && g.visible && m.len() >= 2).map(|(m, _)| m).collect()
    }

    fn count_shown(&mut self, visible: &[bool]) {
        for g in self.groups.iter_mut() { g.shown = 0; }
        for (i, gs) in self.members.iter().enumerate() {
            if visible.get(i).copied().unwrap_or(false) { for &g in gs { self.groups[g].shown += 1; } }
        }
    }
}

/// What an automatic grouper produced.
#[derive(Debug, Clone, Default)]
struct AutoSet {
    keys:    Vec<String>,
    labels:  Vec<String>,
    members: Vec<Vec<usize>>, // per node: auto group indices, primary first
}

impl AutoSet {
    fn with_nodes(n: usize) -> Self { AutoSet { members: vec![Vec::new(); n], ..AutoSet::default() } }

    fn group_members(&self) -> Vec<Vec<usize>> {
        let mut out = vec![Vec::new(); self.keys.len()];
        for (i, gs) in self.members.iter().enumerate() { for &g in gs { out[g].push(i); } }
        out
    }
}

fn hub_title(g: &Graph, members: &[usize]) -> String {
    members.iter().copied().max_by_key(|&i| (g.node_list[i].degree, std::cmp::Reverse(i)))
        .map(|i| g.node_list[i].title.clone()).unwrap_or_default()
}

/// Folder groups: the first `depth` directory components below the notes
/// root. Files directly in the root stay ungrouped.
fn auto_folder(g: &Graph, eligible: &[bool], depth: usize) -> AutoSet {
    let n = g.node_list.len();
    let root = notes_root_depth(g.node_list.iter().map(|nd| nd.file.as_str()));
    let mut set = AutoSet::with_nodes(n);
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut keys: Vec<(String, String)> = Vec::new();
    let mut of = vec![None; n];
    for i in (0..n).filter(|&i| eligible[i]) {
        let comps: Vec<&str> = dir_components(&g.node_list[i].file).skip(root).filter(|c| !c.is_empty()).take(depth.max(1)).collect();
        if comps.is_empty() { continue; }
        let path = comps.join("/");
        let k = *index.entry(path.clone()).or_insert_with(|| { keys.push((format!("folder:{path}"), format!("{path}/"))); keys.len() - 1 });
        of[i] = Some(k);
    }
    // order groups by key so they don't depend on node order
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by(|&a, &b| keys[a].0.cmp(&keys[b].0));
    let mut rank = vec![0; keys.len()];
    for (r, &k) in order.iter().enumerate() { rank[k] = r; }
    for &k in &order { set.keys.push(keys[k].0.clone()); set.labels.push(keys[k].1.clone()); }
    for (m, k) in set.members.iter_mut().zip(&of) { if let Some(k) = k { m.push(rank[*k]); } }
    set
}

/// Tag groups: every tag is a group; a node's primary tag is its first in a
/// STABLE order (alphabetical ignoring case, then exact), never db row order.
fn auto_tag(g: &Graph, eligible: &[bool]) -> AutoSet {
    let n = g.node_list.len();
    let tag_order = |a: &String, b: &String| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b));
    let mut tags: Vec<String> = (0..n).filter(|&i| eligible[i]).flat_map(|i| g.node_list[i].tags.iter().cloned()).collect();
    tags.sort_by(tag_order); tags.dedup();
    let index: HashMap<&str, usize> = tags.iter().enumerate().map(|(k, t)| (t.as_str(), k)).collect();
    let mut set = AutoSet::with_nodes(n);
    for i in (0..n).filter(|&i| eligible[i]) {
        let mut mine: Vec<usize> = g.node_list[i].tags.iter().filter_map(|t| index.get(t.as_str()).copied()).collect();
        mine.sort(); mine.dedup(); // index order IS the stable tag order
        set.members[i] = mine;
    }
    set.keys   = tags.iter().map(|t| format!("tag:{t}")).collect();
    set.labels = tags.iter().map(|t| format!("#{t}")).collect();
    set
}

/// One group per partition class with ≥ 2 eligible members, largest first.
fn from_partition(g: &Graph, eligible: &[bool], class: &[Option<usize>], kind: &str) -> AutoSet {
    let n = g.node_list.len();
    let mut by: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in (0..n).filter(|&i| eligible[i]) { if let Some(c) = class.get(i).copied().flatten() { by.entry(c).or_default().push(i); } }
    let mut parts: Vec<Vec<usize>> = by.into_values().filter(|m| m.len() >= 2).collect();
    parts.sort_by_key(|m| (std::cmp::Reverse(m.len()), m[0]));
    let mut set = AutoSet::with_nodes(n);
    for (k, m) in parts.iter().enumerate() {
        set.keys.push(format!("{kind}:{}", k + 1));
        set.labels.push(hub_title(g, m));
        for &i in m { set.members[i].push(k); }
    }
    set
}

/// Connected components of the pre-hiding view, orphans excluded.
fn auto_component(g: &Graph, pre: &ViewState) -> AutoSet {
    let n = g.node_list.len();
    let ok = |i: usize| pre.visible[i] && !pre.orphan[i];
    let mut comp = vec![None; n];
    let mut k = 0;
    for s in 0..n {
        if !ok(s) || comp[s].is_some() { continue; }
        comp[s] = Some(k);
        let mut stack = vec![s];
        while let Some(cur) = stack.pop() {
            for &nb in &g.adj[cur] { if ok(nb) && comp[nb].is_none() { comp[nb] = Some(k); stack.push(nb); } }
        }
        k += 1;
    }
    from_partition(g, &pre.visible, &comp, "component")
}

/// File groups: a file node with its headings (containment via
/// `parent_file`). Files without headings stay ungrouped.
fn auto_file(g: &Graph, eligible: &[bool]) -> AutoSet {
    let n = g.node_list.len();
    let class: Vec<Option<usize>> = (0..n).map(|i| match g.node_list[i].parent_file {
        Some(p) => Some(p),
        None if g.node_list[i].level == 0 => Some(i),
        None => None,
    }).collect();
    let mut set = from_partition(g, eligible, &class, "file");
    // key by the file path, label by the file node's title
    let groups = set.group_members();
    for (k, m) in groups.iter().enumerate() {
        let f = class[m[0]].unwrap_or(m[0]);
        set.keys[k]   = format!("file:{}", g.node_list[f].file);
        set.labels[k] = g.node_list[f].title.clone();
    }
    set
}

// ── Community cache (off the UI thread) ───────────────────────────────────────

/// A community job's input: its key, node count and edges.
#[derive(Debug, Clone)]
pub struct CommunityJob {
    pub key:        u64,
    pub n:          usize,
    pub edges:      Vec<(usize, usize)>,
    pub resolution: f64,
}

impl CommunityJob {
    fn run(&self) -> Vec<Option<usize>> {
        let c = community::communities(self.n, &self.edges, self.resolution, community::DEFAULT_SEED);
        c.into_iter().map(Some).collect()
    }
}

/// Latest-wins community computation. Graphs under `sync_below` nodes are
/// computed inline (a few ms); bigger ones run on a worker thread, at most
/// one at a time, and a request made while one runs replaces any earlier
/// queued request (the debounce). Until the new partition lands, the last
/// one (same node count) keeps showing.
pub struct CommunityCache {
    pub sync_below: usize,
    done:           Option<(u64, Arc<Vec<Option<usize>>>)>,
    running:        Option<(u64, Receiver<Vec<Option<usize>>>)>,
    queued:         Option<CommunityJob>,
}

impl Default for CommunityCache {
    fn default() -> Self { CommunityCache { sync_below: 3000, done: None, running: None, queued: None } }
}

impl CommunityCache {
    fn spawn(&mut self, job: CommunityJob) {
        let (tx, rx) = channel();
        let key = job.key;
        std::thread::spawn(move || { let _ = tx.send(job.run()); });
        self.running = Some((key, rx));
    }

    fn get(&mut self, job: CommunityJob) -> Option<Arc<Vec<Option<usize>>>> {
        if let Some((k, p)) = &self.done { if *k == job.key { return Some(p.clone()); } }
        let n = job.n;
        if n < self.sync_below {
            let p = Arc::new(job.run());
            self.done = Some((job.key, p.clone()));
            return Some(p);
        }
        match &self.running {
            Some((k, _)) if *k == job.key => {}
            Some(_) => self.queued = Some(job),
            None    => self.spawn(job),
        }
        self.done.as_ref().filter(|(_, p)| p.len() == n).map(|(_, p)| p.clone())
    }

    /// Collect a finished job (and start the queued one). True when a new
    /// partition landed: recompute the view.
    pub fn poll(&mut self) -> bool {
        let Some((key, rx)) = &self.running else { return false };
        match rx.try_recv() {
            Ok(p) => {
                self.done    = Some((*key, Arc::new(p)));
                self.running = None;
                if let Some(job) = self.queued.take() { self.spawn(job); }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(_) => { self.running = None; false } // worker died: next request retries
        }
    }

    pub fn pending(&self) -> bool { self.running.is_some() }
}

fn edges_key(n: usize, edges: &[(usize, usize)], resolution: f64, scope: CommunityScope) -> u64 {
    let mut h = stable_hash(&format!("{n}/{}/{scope:?}", resolution.to_bits()));
    for &(a, b) in edges { h = (h ^ ((a as u64) << 32 | b as u64)).wrapping_mul(0x0100_0000_01b3); }
    h
}

// ── Grouping (stateful: colour memory, previous partitions, communities) ──────

#[derive(Default)]
pub struct Grouping {
    pub memory:    ColourMemory,
    prev:          HashMap<&'static str, Vec<(u8, HashSet<String>)>>,
    pub community: CommunityCache,
}

impl Grouping {
    /// The grouped view: evaluate groups on the view without group hiding,
    /// hide nodes whose every group is hidden, classify orphans, filter.
    pub fn compute(&mut self, g: &Graph, filter: &ViewFilter, spec: &GroupSpec) -> (ViewState, GroupResolution) {
        let pre = compute_view(g, &ViewFilter { orphans: OrphansMode::Show, ..filter.clone() });
        let mut res = self.resolve(g, &pre, spec);
        let view = if res.hidden.iter().any(|&h| h) || filter.orphans != OrphansMode::Show {
            compute_view_hiding(g, filter, &res.hidden)
        } else {
            pre
        };
        res.count_shown(&view.visible);
        (view, res)
    }

    /// Membership + colours on `pre` (the view without group hiding).
    pub fn resolve(&mut self, g: &Graph, pre: &ViewState, spec: &GroupSpec) -> GroupResolution {
        let n = g.node_list.len();
        if pre.visible.len() != n { return GroupResolution::default(); }
        let eligible = &pre.visible;
        let cx = EvalCtx::from_view(g, pre);

        // manual groups (a spec built by hand may not have compiled them)
        let compiled_here: Vec<Result<Query, QueryError>>;
        let compiled = if spec.compiled.len() == spec.manual.len() { &spec.compiled } else {
            compiled_here = spec.manual.iter().map(|g| Query::parse(&g.query)).collect();
            &compiled_here
        };
        let mut groups: Vec<GroupInfo> = Vec::new();
        let mut members: Vec<Vec<GroupId>> = vec![Vec::new(); n];
        for (gi, (grp, q)) in spec.manual.iter().zip(compiled).enumerate() {
            let colour = grp.colour.map_or(GroupColour::Slot(hash_slot(&format!("manual:{}", grp.name))), GroupColour::Rgb);
            groups.push(GroupInfo {
                key: format!("manual:{}", grp.name), label: grp.name.clone(), colour, manual: true,
                visible: grp.visible, pull: grp.pull, hull: grp.hull, members: 0, shown: 0,
                error: q.as_ref().err().cloned(),
            });
            if let Ok(q) = q {
                for i in (0..n).filter(|&i| eligible[i]) { if q.matches(&cx, i) { members[i].push(gi); } }
            }
        }

        // the automatic grouper, ranked below
        let auto = match spec.auto {
            AutoGrouper::None              => AutoSet::with_nodes(n),
            AutoGrouper::Folder { depth }  => auto_folder(g, eligible, depth),
            AutoGrouper::Tag               => auto_tag(g, eligible),
            AutoGrouper::Component         => auto_component(g, pre),
            AutoGrouper::File              => auto_file(g, eligible),
            AutoGrouper::Community { resolution, scope } => {
                let edges: Vec<(usize, usize)> = match scope {
                    CommunityScope::All     => g.edges.clone(),
                    CommunityScope::Visible => g.edges.iter().copied().filter(|&(a, b)| eligible[a] && eligible[b]).collect(),
                };
                let key = edges_key(n, &edges, resolution, scope);
                match self.community.get(CommunityJob { key, n, edges, resolution }) {
                    Some(p) => from_partition(g, eligible, &p, "community"),
                    None    => AutoSet::with_nodes(n), // first run still computing
                }
            }
        };
        let slots: Vec<u8> = match spec.auto {
            AutoGrouper::Component | AutoGrouper::Community { .. } => {
                let ids: Vec<Vec<String>> = auto.group_members().iter()
                    .map(|m| m.iter().map(|&i| g.node_list[i].id.clone()).collect()).collect();
                let kind = spec.auto.kind();
                let prev = self.prev.get(kind).map(|v| v.as_slice()).unwrap_or(&[]);
                let slots = match_partition(prev, &ids);
                self.prev.insert(kind, slots.iter().copied().zip(ids.into_iter().map(|v| v.into_iter().collect())).collect());
                slots
            }
            _ => self.memory.assign(&auto.keys),
        };
        let base = groups.len();
        for (k, key) in auto.keys.iter().enumerate() {
            groups.push(GroupInfo {
                key: key.clone(), label: auto.labels[k].clone(), colour: GroupColour::Slot(slots[k]), manual: false,
                visible: !spec.auto_hidden.contains(key), pull: spec.auto_pull, hull: spec.auto_hull,
                members: 0, shown: 0, error: None,
            });
        }
        for (i, am) in auto.members.iter().enumerate() { members[i].extend(am.iter().map(|&k| base + k)); }

        let mut primary = vec![None; n];
        let mut hidden  = vec![false; n];
        for i in 0..n {
            for &gid in &members[i] { groups[gid].members += 1; }
            primary[i] = members[i].iter().copied().find(|&gid| groups[gid].visible);
            hidden[i]  = !members[i].is_empty() && primary[i].is_none();
        }
        GroupResolution { groups, primary, members, hidden }
    }
}

// ── Hulls ─────────────────────────────────────────────────────────────────────

/// Convex hull (Andrew's monotone chain), counter-clockwise, no repeated
/// end point. Fewer than 3 distinct points come back as they are.
pub fn convex_hull(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut p: Vec<(f64, f64)> = points.iter().copied().filter(|q| q.0.is_finite() && q.1.is_finite()).collect();
    p.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    p.dedup();
    if p.len() < 3 { return p; }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut lower: Vec<(f64, f64)> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 { lower.pop(); }
        lower.push(q);
    }
    let mut upper: Vec<(f64, f64)> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 { upper.pop(); }
        upper.push(q);
    }
    lower.pop(); upper.pop();
    lower.extend(upper);
    lower
}

/// A rounded hull `pad` outside every point: the hull of a `segments`-gon
/// around each vertex of the points' own hull. One point gives a circle,
/// two a capsule. Convex, counter-clockwise.
pub fn padded_hull(points: &[(f64, f64)], pad: f64, segments: usize) -> Vec<(f64, f64)> {
    let core = convex_hull(points);
    if core.is_empty() { return core; }
    let seg = segments.max(6);
    let mut ring = Vec::with_capacity(core.len() * seg);
    for &(x, y) in &core {
        for s in 0..seg {
            let a = std::f64::consts::TAU * s as f64 / seg as f64;
            ring.push((x + pad * a.cos(), y + pad * a.sin()));
        }
    }
    convex_hull(&ring)
}

pub fn centroid(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    if points.is_empty() { return None; }
    let c = points.len() as f64;
    let (x, y) = points.iter().fold((0.0, 0.0), |(x, y), p| (x + p.0, y + p.1));
    Some((x / c, y / c))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::RawNode;
    use crate::view::DailiesMode;

    fn node(id: &str, title: &str, file: &str, tags: &[&str]) -> RawNode {
        RawNode { id: id.into(), title: title.into(), file: file.into(),
                  tags: tags.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }
    fn e(a: &str, b: &str) -> (String, String) { (a.into(), b.into()) }

    /// root /n. a—b—c (posts/), d—e (projects/), f alone (root), daily g—a.
    /// tags: a [rust, blog], b [blog], d [rust], e [Zed], f []
    fn fixture() -> Graph {
        Graph::new(vec![
            node("a", "Alpha", "/n/posts/a.org", &["rust", "blog"]),
            node("b", "Beta", "/n/posts/b.org", &["blog"]),
            node("c", "Gamma", "/n/posts/2026/c.org", &[]),
            node("d", "Delta", "/n/projects/d.org", &["rust"]),
            node("e", "Epsilon", "/n/projects/e.org", &["Zed"]),
            node("f", "Phi", "/n/f.org", &[]),
            node("g", "2026-01-01", "/n/daily/g.org", &[]),
        ], vec![e("a", "b"), e("b", "c"), e("d", "e"), e("g", "a")])
    }

    fn id(g: &Graph, s: &str) -> usize { g.nodes[s] }

    fn run(g: &Graph, spec: &GroupSpec, filter: &ViewFilter) -> (ViewState, GroupResolution) {
        Grouping::default().compute(g, filter, spec)
    }

    fn labels(res: &GroupResolution, g: &Graph, s: &str) -> Vec<String> {
        res.members[id(g, s)].iter().map(|&k| res.groups[k].label.clone()).collect()
    }

    #[test]
    fn manual_groups_precedence_and_multiple_membership() {
        let g = fixture();
        let spec = GroupSpec::new(vec![Group::new("Blog", "tag:blog"), Group::new("Rust", "tag:rust")], AutoGrouper::None);
        let (v, res) = run(&g, &spec, &ViewFilter::default());
        let (a, b, d, f) = (id(&g, "a"), id(&g, "b"), id(&g, "d"), id(&g, "f"));
        assert_eq!(res.members[a], vec![0, 1], "a is in both, in list order");
        assert_eq!(res.primary[a], Some(0), "first match wins the colour");
        assert_eq!(res.primary[d], Some(1));
        assert_eq!(res.primary[f], None, "ungrouped: the theme's neutral colour");
        assert_eq!(res.groups[0].members, 2);
        assert_eq!(res.groups[1].members, 2);
        assert_eq!(res.groups[0].shown, 2);
        assert!(v.visible.iter().all(|&x| x));
        // swapping the order swaps who colours `a`, not membership
        let spec = GroupSpec::new(vec![Group::new("Rust", "tag:rust"), Group::new("Blog", "tag:blog")], AutoGrouper::None);
        let (_, res) = run(&g, &spec, &ViewFilter::default());
        assert_eq!(res.groups[res.primary[a].unwrap()].label, "Rust");
        assert_eq!(res.members[b], vec![1]);
    }

    /// Foam: hidden only when EVERY group of the node is hidden; ungrouped
    /// nodes never; the colour comes from the first visible group.
    #[test]
    fn hiding_follows_the_foam_rule() {
        let g = fixture();
        let mut blog = Group::new("Blog", "tag:blog");
        blog.visible = false;
        let spec = GroupSpec::new(vec![blog, Group::new("Rust", "tag:rust")], AutoGrouper::None);
        let (v, res) = run(&g, &spec, &ViewFilter::default());
        let (a, b, f) = (id(&g, "a"), id(&g, "b"), id(&g, "f"));
        assert!(v.visible[a], "a is also in Rust, which is visible");
        assert_eq!(res.primary[a], Some(1), "colour from the first VISIBLE group");
        assert!(!v.visible[b] && res.hidden[b], "b's only group is hidden");
        assert!(v.visible[f], "ungrouped is never hidden by groups");
        assert_eq!(v.counts.hidden_by_groups, 1);
        assert_eq!(res.groups[0].members, 2);
        assert_eq!(res.groups[0].shown, 1, "a still shows");
        // hidden groups filter BEFORE orphan classification: c lost its only link (to b)
        let c = id(&g, "c");
        assert!(v.orphan[c]);
        let (v, _) = run(&g, &spec, &ViewFilter { orphans: OrphansMode::Hide, ..ViewFilter::default() });
        assert!(!v.visible[c] && !v.visible[f]);
        // membership is evaluated before hiding: hiding Blog doesn't make
        // `orphan` true for b's neighbours inside a group query
        let mut hidden_blog = Group::new("Blog", "tag:blog");
        hidden_blog.visible = false;
        let lonely = GroupSpec::new(vec![Group::new("Lonely", "orphan"), hidden_blog], AutoGrouper::None);
        let (_, res) = run(&g, &lonely, &ViewFilter::default());
        assert!(!res.members[c].contains(&0), "c is not an orphan before group hiding");
        assert!(res.members[f].contains(&0));
    }

    #[test]
    fn auto_groups_rank_below_manual_and_can_be_hidden() {
        let g = fixture();
        let mut spec = GroupSpec::new(vec![Group::new("Beta", "title:beta")], AutoGrouper::Folder { depth: 1 });
        let (_, res) = run(&g, &spec, &ViewFilter::default());
        let b = id(&g, "b");
        assert_eq!(labels(&res, &g, "b"), vec!["Beta", "posts/"]);
        assert_eq!(res.primary[b], Some(0));
        assert_eq!(res.groups.iter().map(|x| x.label.as_str()).collect::<Vec<_>>(), vec!["Beta", "daily/", "posts/", "projects/"]);
        spec.auto_hidden.insert("folder:projects".into());
        let (v, _) = run(&g, &spec, &ViewFilter::default());
        assert!(!v.visible[id(&g, "d")] && !v.visible[id(&g, "e")]);
        assert!(v.visible[id(&g, "f")], "root-level file: ungrouped, never hidden");
    }

    #[test]
    fn folder_grouper_is_relative_to_the_notes_root() {
        let g = fixture();
        let (_, res) = run(&g, &GroupSpec::new(vec![], AutoGrouper::Folder { depth: 2 }), &ViewFilter::default());
        assert_eq!(labels(&res, &g, "c"), vec!["posts/2026/"]);
        assert_eq!(labels(&res, &g, "a"), vec!["posts/"]);
        assert!(res.members[id(&g, "f")].is_empty());
        // hidden dailies are not grouped (and don't make a daily/ group)
        let (_, res) = run(&g, &GroupSpec::new(vec![], AutoGrouper::Folder { depth: 1 }),
                           &ViewFilter { dailies: DailiesMode::Hide, ..ViewFilter::default() });
        assert!(res.groups.iter().all(|x| x.label != "daily/"));
    }

    #[test]
    fn tag_grouper_uses_a_stable_first_tag() {
        let g = fixture();
        let (_, res) = run(&g, &GroupSpec::new(vec![], AutoGrouper::Tag), &ViewFilter::default());
        assert_eq!(res.groups.iter().map(|x| x.label.as_str()).collect::<Vec<_>>(), vec!["#blog", "#rust", "#Zed"]);
        let a = id(&g, "a");
        assert_eq!(labels(&res, &g, "a"), vec!["#blog", "#rust"], "every tag, alphabetical");
        assert_eq!(res.groups[res.primary[a].unwrap()].label, "#blog", "first alphabetically, not db order (rust came first)");
        assert!(res.members[id(&g, "f")].is_empty());
    }

    #[test]
    fn component_grouper_excludes_orphans_and_sorts_by_size() {
        let g = fixture();
        let (_, res) = run(&g, &GroupSpec::new(vec![], AutoGrouper::Component), &ViewFilter::default());
        assert_eq!(res.groups.len(), 2);
        assert_eq!(res.groups[0].members, 4, "a b c g");
        assert_eq!(res.groups[0].label, "Alpha", "labelled by its best-linked node");
        assert_eq!(res.groups[1].members, 2);
        assert!(res.members[id(&g, "f")].is_empty(), "orphan: no component");
        // hiding dailies splits nothing here but drops g from the component
        let (_, res) = run(&g, &GroupSpec::new(vec![], AutoGrouper::Component),
                           &ViewFilter { dailies: DailiesMode::Hide, ..ViewFilter::default() });
        assert_eq!(res.groups[0].members, 3);
    }

    #[test]
    fn file_grouper_clusters_headings_with_their_file() {
        let mut h1 = node("h1", "Intro", "/n/posts/a.org", &[]); h1.level = 1;
        let mut h2 = node("h2", "Details", "/n/posts/a.org", &[]); h2.level = 2;
        let g = Graph::new(vec![node("a", "Alpha", "/n/posts/a.org", &[]), h1, h2, node("b", "Beta", "/n/b.org", &[])], vec![]);
        let (_, res) = run(&g, &GroupSpec::new(vec![], AutoGrouper::File), &ViewFilter::default());
        assert_eq!(res.groups.len(), 1, "a file without headings stays ungrouped");
        assert_eq!(res.groups[0].label, "Alpha");
        assert_eq!(res.groups[0].key, "file:/n/posts/a.org");
        assert_eq!(res.groups[0].members, 3);
        assert!(res.members[3].is_empty());
    }

    #[test]
    fn community_grouper_is_deterministic() {
        // two triangles joined by one edge + an isolated node
        let g = Graph::new(vec![
            node("a", "A", "/n/a.org", &[]), node("b", "B", "/n/b.org", &[]), node("c", "C", "/n/c.org", &[]),
            node("x", "X", "/n/x.org", &[]), node("y", "Y", "/n/y.org", &[]), node("z", "Z", "/n/z.org", &[]),
            node("o", "O", "/n/o.org", &[]),
        ], vec![e("a", "b"), e("b", "c"), e("c", "a"), e("x", "y"), e("y", "z"), e("z", "x"), e("c", "x")]);
        let spec = GroupSpec::new(vec![], AutoGrouper::Community { resolution: 1.0, scope: CommunityScope::All });
        let (_, r1) = run(&g, &spec, &ViewFilter::default());
        let (_, r2) = run(&g, &spec, &ViewFilter::default());
        assert_eq!(r1, r2);
        assert_eq!(r1.groups.len(), 2);
        assert_eq!(r1.members[0], r1.members[1]);
        assert_ne!(r1.members[0], r1.members[3]);
        assert!(r1.members[6].is_empty(), "a singleton community is no group");
    }

    #[test]
    fn community_cache_runs_off_thread_above_the_threshold() {
        let g = fixture();
        let spec = GroupSpec::new(vec![], AutoGrouper::Community { resolution: 1.0, scope: CommunityScope::All });
        let mut grouping = Grouping::default();
        grouping.community.sync_below = 0; // force the worker
        let (_, res) = grouping.compute(&g, &ViewFilter::default(), &spec);
        assert!(res.groups.is_empty(), "first request: nothing yet");
        assert!(grouping.community.pending());
        let t = std::time::Instant::now();
        while !grouping.community.poll() {
            assert!(t.elapsed().as_secs() < 10, "worker never finished");
            std::thread::yield_now();
        }
        let (_, res) = grouping.compute(&g, &ViewFilter::default(), &spec);
        assert!(!res.groups.is_empty());
        assert!(!grouping.community.pending(), "same key: served from cache");
        // a changed key while a job runs is queued; only the latest runs next
        let s2 = GroupSpec::new(vec![], AutoGrouper::Community { resolution: 0.5, scope: CommunityScope::All });
        let s3 = GroupSpec::new(vec![], AutoGrouper::Community { resolution: 2.0, scope: CommunityScope::All });
        let (_, r) = grouping.compute(&g, &ViewFilter::default(), &s2);
        assert!(!r.groups.is_empty(), "the stale partition shows while the new one runs");
        grouping.compute(&g, &ViewFilter::default(), &s3);
        let mut landed = 0;
        let t = std::time::Instant::now();
        while grouping.community.pending() || landed == 0 {
            if grouping.community.poll() { landed += 1; }
            assert!(t.elapsed().as_secs() < 10);
            std::thread::yield_now();
        }
        assert!(landed <= 2);
    }

    /// A new tag (or folder) never repaints the existing ones.
    #[test]
    fn colours_are_stable_when_a_tag_is_added() {
        let g = fixture();
        let spec = GroupSpec::new(vec![], AutoGrouper::Tag);
        let mut grouping = Grouping::default();
        let (_, before) = grouping.compute(&g, &ViewFilter::default(), &spec);
        let colours = |r: &GroupResolution| r.groups.iter().map(|x| (x.key.clone(), x.colour)).collect::<HashMap<_, _>>();
        let c0 = colours(&before);
        let mut distinct: Vec<GroupColour> = c0.values().copied().collect(); distinct.sort_by_key(|c| format!("{c:?}")); distinct.dedup();
        assert_eq!(distinct.len(), c0.len(), "no collisions in a small set");

        // a reload where `f` gained a tag that sorts first and hashes anywhere
        let mut nodes: Vec<RawNode> = g.node_list.iter().map(|n| node(&n.id, &n.title, &n.file, &n.tags.iter().map(|s| s.as_str()).collect::<Vec<_>>())).collect();
        for t in ["aardvark", "abacus", "zz-top", "blog2", "rust-lang", "x"] { nodes[5].tags.push(t.into()); }
        let g2 = Graph::new(nodes, vec![]);
        let (_, after) = grouping.compute(&g2, &ViewFilter::default(), &spec);
        let c1 = colours(&after);
        for (k, c) in &c0 { assert_eq!(c1[k], *c, "{k} changed colour"); }

        // a fresh session is deterministic too
        let (_, again) = Grouping::default().compute(&g, &ViewFilter::default(), &spec);
        assert_eq!(colours(&again), c0);
    }

    #[test]
    fn colour_memory_assigns_distinct_slots_and_remembers() {
        let mut m = ColourMemory::default();
        let keys: Vec<String> = (0..PALETTE_LEN).map(|i| format!("tag:t{i}")).collect();
        let s = m.assign(&keys);
        let mut d = s.clone(); d.sort(); d.dedup();
        assert_eq!(d.len(), PALETTE_LEN, "a full palette before any repeat");
        assert_eq!(m.assign(&keys), s);
        let mut more = keys.clone(); more.insert(0, "tag:new".into());
        assert_eq!(&m.assign(&more)[1..], &s[..]);
    }

    /// Communities / components keep their colours across recomputation by
    /// Jaccard matching, even when renumbered.
    #[test]
    fn partition_colours_follow_overlap() {
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<HashSet<String>>();
        let vecs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        let prev = vec![(0u8, set(&["a", "b", "c"])), (5u8, set(&["x", "y"])), (7u8, set(&["p", "q"]))];
        // reordered, x/y grew, a/b/c lost c, and a brand-new group
        let new = vec![vecs(&["x", "y", "z", "w"]), vecs(&["a", "b"]), vecs(&["m", "n"]), vecs(&["c", "p"])];
        let s = match_partition(&prev, &new);
        assert_eq!(s[0], 5);
        assert_eq!(s[1], 0);
        assert_eq!(s[3], 7, "p's group wins c+p");
        assert!(![0, 5, 7].contains(&s[2]), "new group takes a free slot");
        assert_eq!(s[2], 1, "the lowest free one");
        // a split: the larger overlap keeps the colour
        let s = match_partition(&prev, &[vecs(&["a"]), vecs(&["b", "c"])]);
        assert_eq!(s[1], 0);
        assert_ne!(s[0], 0);
        // nothing before: slots in order
        assert_eq!(match_partition(&[], &[vecs(&["a"]), vecs(&["b"])]), vec![0, 1]);
        // more groups than the palette: cycles instead of panicking
        let many: Vec<Vec<String>> = (0..30).map(|i| vec![format!("n{i}")]).collect();
        assert_eq!(match_partition(&[], &many).len(), 30);
    }

    #[test]
    fn component_colours_survive_recomputation() {
        let g = fixture();
        let spec = GroupSpec::new(vec![], AutoGrouper::Component);
        let mut grouping = Grouping::default();
        let (_, r1) = grouping.compute(&g, &ViewFilter::default(), &spec);
        // hiding dailies changes the big component (g leaves) but not its colour
        let (_, r2) = grouping.compute(&g, &ViewFilter { dailies: DailiesMode::Hide, ..ViewFilter::default() }, &spec);
        let a = id(&g, "a");
        let d = id(&g, "d");
        assert_eq!(r1.groups[r1.primary[a].unwrap()].colour, r2.groups[r2.primary[a].unwrap()].colour);
        assert_eq!(r1.groups[r1.primary[d].unwrap()].colour, r2.groups[r2.primary[d].unwrap()].colour);
    }

    #[test]
    fn bad_manual_query_matches_nothing_and_reports() {
        let g = fixture();
        let spec = GroupSpec::new(vec![Group::new("Broken", "tag:("), Group::new("Ok", "tag:rust")], AutoGrouper::None);
        let (_, res) = run(&g, &spec, &ViewFilter::default());
        assert!(res.groups[0].error.is_some());
        assert_eq!(res.groups[0].members, 0);
        assert_eq!(res.groups[1].members, 2);
    }

    #[test]
    fn pull_and_member_lists_use_visible_members() {
        let g = fixture();
        let mut rust = Group::new("Rust", "tag:rust");
        rust.pull = true;
        let spec = GroupSpec::new(vec![rust, Group::new("Blog", "tag:blog")], AutoGrouper::None);
        let (v, res) = run(&g, &spec, &ViewFilter::default());
        let pulls = res.pull_lists(&v.visible);
        assert_eq!(pulls, vec![vec![id(&g, "a"), id(&g, "d")]]);
        let mut vis = v.visible.clone();
        vis[id(&g, "d")] = false;
        assert!(res.pull_lists(&vis).is_empty(), "one visible member: nothing to pull toward");
        assert_eq!(res.member_lists(&vis)[1], vec![id(&g, "a"), id(&g, "b")]);
    }

    #[test]
    fn group_pull_draws_members_together_and_still_settles() {
        let g0 = fixture();
        let a = id(&g0, "a");
        let d = id(&g0, "d");
        let dist = |g: &Graph| { let (p, q) = (&g.node_list[a], &g.node_list[d]); (p.x - q.x).hypot(p.y - q.y) };
        let mut free = fixture();
        let mut pulled = fixture();
        pulled.set_group_pull(vec![vec![a, d]], 0.5);
        free.resume_physics();
        for _ in 0..900 { free.step(1.0 / 60.0); pulled.step(1.0 / 60.0); }
        assert!(dist(&pulled) < dist(&free) * 0.8, "pulled {:.0} vs free {:.0}", dist(&pulled), dist(&free));
        // auto-pause still happens with the pull on
        let mut k = 0;
        while pulled.physics_on && k < 6000 { pulled.step(1.0 / 60.0); k += 1; }
        assert!(!pulled.physics_on, "physics never settled with group pull");
        // hidden members neither move nor attract
        let mut h = fixture();
        let mut hide = vec![false; h.node_list.len()];
        hide[d] = true;
        let v = compute_view_hiding(&h, &ViewFilter::default(), &hide);
        h.apply_view(&v);
        let before = (h.node_list[d].x, h.node_list[d].y);
        h.set_group_pull(vec![vec![a, d]], 0.2);
        for _ in 0..60 { h.step(1.0 / 60.0); }
        assert_eq!((h.node_list[d].x, h.node_list[d].y), before);
        // switching the pull on wakes physics; setting the same pull again doesn't
        let mut s = fixture();
        s.physics_on = false;
        s.set_group_pull(vec![vec![a, d]], 0.05);
        assert!(s.physics_on);
        s.physics_on = false;
        s.set_group_pull(vec![vec![a, d]], 0.05);
        assert!(!s.physics_on);
        s.set_group_pull(vec![vec![a]], f64::NAN); // garbage in: off, no panic
        assert!(!s.group_pull_active());
    }

    #[test]
    fn hulls() {
        let sq = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 0.5), (1.0, 1.0)];
        let h = convex_hull(&sq);
        assert_eq!(h.len(), 4, "{h:?}");
        assert!(!h.contains(&(0.5, 0.5)));
        assert_eq!(convex_hull(&[(1.0, 1.0)]), vec![(1.0, 1.0)]);
        assert!(convex_hull(&[]).is_empty());
        assert_eq!(convex_hull(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]).len(), 2, "collinear");
        let p = padded_hull(&[(0.0, 0.0)], 10.0, 12);
        assert_eq!(p.len(), 12);
        assert!(p.iter().all(|q| (q.0.hypot(q.1) - 10.0).abs() < 1e-9));
        let cap = padded_hull(&[(0.0, 0.0), (100.0, 0.0)], 10.0, 12);
        let xs: Vec<f64> = cap.iter().map(|q| q.0).collect();
        assert!(xs.iter().cloned().fold(f64::MAX, f64::min) <= -9.99 && xs.iter().cloned().fold(f64::MIN, f64::max) >= 109.99);
        // every input point lies inside the padded hull
        let pts: Vec<(f64, f64)> = (0..50).map(|i| ((i * 37 % 101) as f64, (i * 53 % 89) as f64)).collect();
        let hull = padded_hull(&pts, 15.0, 12);
        for &q in &pts {
            for k in 0..hull.len() {
                let (a, b) = (hull[k], hull[(k + 1) % hull.len()]);
                assert!((b.0 - a.0) * (q.1 - a.1) - (b.1 - a.1) * (q.0 - a.0) >= -1e-9, "point outside the hull");
            }
        }
        assert_eq!(centroid(&[(0.0, 0.0), (2.0, 4.0)]), Some((1.0, 2.0)));
        assert_eq!(centroid(&[]), None);
    }

    #[test]
    fn palette_reads_on_dark_and_light_backgrounds() {
        for bg in [[0x10, 0x1e, 0x2e], [0, 0, 0], [26, 8, 16], [0x20, 0x20, 0x20]] {
            for c in palette(bg) { assert!(contrast(*c, bg) >= 3.0, "{c:?} on {bg:?}: {:.2}", contrast(*c, bg)); }
        }
        for bg in [[255, 255, 255], [0xf8, 0xf4, 0xec], [0xe8, 0xe8, 0xe8]] {
            assert!(std::ptr::eq(palette(bg), &PALETTE_LIGHT));
            for c in palette(bg) { assert!(contrast(*c, bg) >= 3.0, "{c:?} on {bg:?}: {:.2}", contrast(*c, bg)); }
        }
        assert_eq!(parse_hex("#FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("ff8000"), None);
        assert_eq!(parse_hex("#ff80"), None);
        assert_eq!(parse_hex("#ffé000"), None);
        assert_eq!(to_hex([255, 128, 0]), "#ff8000");
        assert_eq!(stable_hash("tag:rust"), stable_hash("tag:rust"));
        assert_eq!(stable_hash(""), 0xcbf2_9ce4_8422_2325, "FNV-1a offset basis: the hash never changes");
    }

    #[test]
    fn new_groups_get_distinct_stored_colours() {
        let bg = [0x10, 0x1e, 0x2e];
        let mut gs: Vec<Group> = Vec::new();
        for k in 0..PALETTE_LEN {
            let name = format!("Group {k}");
            let mut g = Group::new(&name, "");
            g.colour = Some(new_group_colour(&gs, &name, bg));
            gs.push(g);
        }
        let mut c: Vec<[u8; 3]> = gs.iter().map(|g| g.colour.unwrap()).collect();
        c.sort(); c.dedup();
        assert_eq!(c.len(), PALETTE_LEN, "the whole palette before any repeat");
        assert!(c.iter().all(|x| palette(bg).contains(x)));
        // full palette: falls back to the hash slot, no panic
        assert!(palette(bg).contains(&new_group_colour(&gs, "One more", bg)));
        // a group without a stored colour still counts as using its hash colour
        let implicit = vec![Group::new("Rust", "tag:rust")];
        let theirs = GroupColour::Slot(hash_slot("manual:Rust")).rgb(bg);
        assert_ne!(new_group_colour(&implicit, "Rust", bg), theirs);
    }

    #[test]
    fn spec_serde() {
        let g: Group = serde_json::from_str(r##"{"name": "Rust", "query": "tag:rust", "color": "#ff0000", "pull": true}"##).unwrap();
        assert_eq!(g.colour, Some([255, 0, 0]));
        assert!(g.visible && g.pull && !g.hull);
        let s = serde_json::to_string(&g).unwrap();
        assert!(s.contains("\"colour\":\"#ff0000\""), "{s}");
        assert!(serde_json::from_str::<Group>(r#"{"name": "x", "colour": "red"}"#).is_err());
        let a: AutoGrouper = serde_json::from_str(r#"{"kind": "folder"}"#).unwrap();
        assert_eq!(a, AutoGrouper::Folder { depth: 1 });
        let a: AutoGrouper = serde_json::from_str(r#"{"kind": "community", "scope": "visible"}"#).unwrap();
        assert_eq!(a, AutoGrouper::Community { resolution: 1.0, scope: CommunityScope::Visible });
        assert_eq!(serde_json::to_string(&AutoGrouper::Tag).unwrap(), r#"{"kind":"tag"}"#);
        let mut k = AutoGrouper::None;
        for _ in 0..6 { k = k.cycled(); }
        assert_eq!(k, AutoGrouper::None, "Shift+G cycles through all six");
    }
}
