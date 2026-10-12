//! The view pipeline: which nodes and edges are on screen, and which nodes
//! count as orphans. One pure function, `compute_view`, that every consumer
//! (painter, physics, layouts, fit, Tab, hit-testing, status bar) reads.
//!
//! Order of operations:
//!   1. attribute filters (dailies, then   → `pass[i]`
//!      hidden groups: `hidden` mask)
//!   2. effective edges                    → both endpoints pass, and — when
//!      `links_to_dailies_count` is false — neither endpoint is a daily
//!   3. orphan classification              → `orphan[i]` = passes and has no
//!      effective edge ("isolated in this view")
//!   4. orphan filter (Show / Hide / Only) → `visible[i]`
//!
//! Step 4 is NOT iterated to a fixpoint: every visible non-orphan keeps a
//! visible edge to another non-orphan when the orphans are removed, so hiding
//! orphans can't create new ones.
//!
//! `links_to_dailies_count` (default true) only matters while dailies are
//! SHOWN: with dailies hidden, edges to them already fail step 2. When false,
//! an edge to a daily gives the NOTE at its other end no credit, so a note
//! linked only to dailies is an orphan. A daily, in turn, is connected when it
//! links to a connected note (one extra pass, not a fixpoint: note status
//! never depends on dailies), so dailies stay put under Orphans=Hide unless
//! everything they link to is itself an orphan; that keeps the guarantee
//! above (no daily is left drawn alone). Such rule-made orphans can still
//! have drawn edges (`visible_edges` holds every real edge between visible
//! nodes); layouts take the view's orphan mask, not their edges, so they sit
//! in the orphan cluster like any other orphan.
//!
//! Groups (groups.rs) run the pipeline twice: once WITHOUT group hiding and
//! with orphans shown (`compute_view`), which is the world group queries,
//! `orphan` and the Component grouper see; then again with the nodes whose
//! every group is hidden passed as `hidden` (`compute_view_hiding`). Hiding a
//! group is therefore an attribute filter like hiding dailies: it comes
//! BEFORE orphan classification, so a note whose only links were into a
//! hidden group is an orphan in the final view ("isolated by filters"), and
//! no group's membership depends on another group's visibility.
//!
//! Dailies dirs are matched below the NOTES ROOT (the deepest directory every
//! node's file shares, ≈ org-roam-directory), so a `~/Journal/roam/` checkout
//! doesn't make every note a daily.

use regex::Regex;
use serde::{de, Deserialize, Deserializer, Serialize};
use std::sync::OnceLock;

use crate::graph::Graph;

// Saved lowercase ("show" / "hide" / "only"); read back leniently, any case,
// with the words the UI shows ("shown" / "hidden") as aliases.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DailiesMode { #[default] Show, Hide }

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrphansMode { #[default] Show, Hide, Only }

fn mode_word<'de, D: Deserializer<'de>>(d: D, expected: &'static [&'static str]) -> Result<&'static str, D::Error> {
    let s = String::deserialize(d)?;
    match s.trim().to_ascii_lowercase().as_str() {
        "show" | "shown" => Ok("show"),
        "hide" | "hidden" => Ok("hide"),
        "only" if expected.contains(&"only") => Ok("only"),
        _ => Err(de::Error::unknown_variant(&s, expected)),
    }
}

impl<'de> Deserialize<'de> for DailiesMode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(if mode_word(d, &["show", "hide"])? == "hide" { Self::Hide } else { Self::Show })
    }
}

impl<'de> Deserialize<'de> for OrphansMode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match mode_word(d, &["show", "hide", "only"])? { "hide" => Self::Hide, "only" => Self::Only, _ => Self::Show })
    }
}

impl DailiesMode {
    pub fn toggled(self) -> Self { match self { Self::Show => Self::Hide, Self::Hide => Self::Show } }
}

impl OrphansMode {
    /// Show → Hide → Only → Show (the `O` key).
    pub fn cycled(self) -> Self {
        match self { Self::Show => Self::Hide, Self::Hide => Self::Only, Self::Only => Self::Show }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Show => "Shown", Self::Hide => "Hidden", Self::Only => "Only" }
    }
}

pub fn default_dailies_dirs() -> Vec<String> {
    ["daily", "dailies", "journal", "journals"].iter().map(|s| s.to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewFilter {
    pub dailies:                DailiesMode,
    pub orphans:                OrphansMode,
    pub dailies_dirs:           Vec<String>, // whole directory components, ASCII case-insensitive
    pub links_to_dailies_count: bool,        // false: an edge to a daily never rescues from orphanhood
}

impl Default for ViewFilter {
    fn default() -> Self {
        ViewFilter {
            dailies:                DailiesMode::Show,
            orphans:                OrphansMode::Show,
            dailies_dirs:           default_dailies_dirs(),
            links_to_dailies_count: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewCounts {
    pub total:               usize,
    pub visible:             usize,
    pub visible_edges:       usize,
    pub dailies:             usize, // daily nodes in the db (shown or not)
    pub orphans_in_view:     usize, // isolated under the current non-orphan filters
    pub orphans_in_db:       usize, // no real link at all
    pub isolated_by_filters: usize, // in-view orphans whose real links all go to filtered-out nodes
    pub daily_only:          usize, // in-view orphans only by links_to_dailies_count = false
    pub dangling:            usize, // unresolved / missing-target links
    pub hidden_by_groups:    usize, // passed the dailies filter, removed because every group they're in is hidden
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewState {
    pub visible:        Vec<bool>,
    pub visible_edges:  Vec<(usize, usize)>,
    pub visible_degree: Vec<usize>, // over visible_edges
    pub orphan:         Vec<bool>,  // isolated in this view (independent of the orphan filter)
    pub db_orphan:      Vec<bool>,  // no real links in the whole db
    pub daily:          Vec<bool>,
    pub pass_degree:    Vec<usize>, // real links to nodes that pass the attribute filters (dailies, hidden groups)
    pub counts:         ViewCounts,
}

/// Why a node is (or isn't) an orphan in a view; the same split the
/// counts make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanKind {
    Linked,            // has a link in this view
    InDb,              // no real link at all
    IsolatedByFilters, // every real link goes to a node the filters removed
    DailyOnly,         // linked, but only to dailies (links_to_dailies_count = false)
}

impl OrphanKind {
    pub fn label(self) -> &'static str {
        match self {
            OrphanKind::Linked            => "Not an orphan",
            OrphanKind::InDb              => "Orphan in the db (no links at all)",
            OrphanKind::IsolatedByFilters => "Orphan in this view (isolated by filters)",
            OrphanKind::DailyOnly         => "Orphan in this view (daily-only)",
        }
    }
}

impl ViewState {
    pub fn orphan_kind(&self, i: usize) -> OrphanKind {
        if !self.orphan.get(i).copied().unwrap_or(false) { return OrphanKind::Linked; }
        if self.db_orphan.get(i).copied().unwrap_or(false) { return OrphanKind::InDb; }
        if self.pass_degree.get(i).copied().unwrap_or(0) == 0 { OrphanKind::IsolatedByFilters } else { OrphanKind::DailyOnly }
    }
}

/// `file` relative to the notes root (`root_depth` leading directories, see
/// `notes_root_depth`), always `/`-separated. Shorter paths come back whole.
pub fn relative_to_root(file: &str, root_depth: usize) -> String {
    let parts: Vec<&str> = file.split(['/', '\\']).collect();
    if root_depth >= parts.len() { return parts.join("/"); }
    parts[root_depth..].join("/")
}

fn date_title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap())
}

/// Directory components of a path (`/` and `\` both separate; file name dropped).
pub(crate) fn dir_components(file: &str) -> impl Iterator<Item = &str> {
    let mut comps: Vec<&str> = file.split(['/', '\\']).collect();
    comps.pop(); // file name
    comps.into_iter()
}

/// How many leading directory components every file shares: the notes root
/// (≈ org-roam-directory). Dailies dirs only match below it.
pub fn notes_root_depth<'a>(files: impl IntoIterator<Item = &'a str>) -> usize {
    let mut files = files.into_iter();
    let Some(first) = files.next() else { return 0 };
    let mut root: Vec<&str> = dir_components(first).collect();
    for f in files {
        let common = root.iter().zip(dir_components(f)).take_while(|(a, b)| *a == b).count();
        root.truncate(common);
        if root.is_empty() { break; }
    }
    root.len()
}

/// A daily note: date title (`YYYY-MM-DD`), or a DIRECTORY component of its
/// path, below the first `root_depth` components (see `notes_root_depth`),
/// equal to one of `dirs` (whole component, ASCII case-insensitive; `/` and
/// `\` both separate). The file name itself never matches, and neither does a
/// substring (`dailyish/`, `daily.org`).
pub fn is_daily(title: &str, file: &str, root_depth: usize, dirs: &[String]) -> bool {
    if date_title_re().is_match(title) { return true; }
    dir_components(file).skip(root_depth)
        .any(|c| !c.is_empty() && dirs.iter().any(|d| d.eq_ignore_ascii_case(c)))
}

pub fn compute_view(g: &Graph, f: &ViewFilter) -> ViewState { compute_view_hiding(g, f, &[]) }

/// `compute_view` with `hidden[i]` (when `hidden` has one entry per node)
/// removing nodes in step 1, alongside the dailies filter. Groups pass the
/// nodes whose every group is hidden.
pub fn compute_view_hiding(g: &Graph, f: &ViewFilter, hidden: &[bool]) -> ViewState {
    let n = g.node_list.len();
    let hidden = |i: usize| hidden.len() == n && hidden[i];

    // 1. attribute filters
    let root  = notes_root_depth(g.node_list.iter().map(|nd| nd.file.as_str()));
    let daily: Vec<bool> = g.node_list.iter().map(|nd| is_daily(&nd.title, &nd.file, root, &f.dailies_dirs)).collect();
    let pass:  Vec<bool> = daily.iter().enumerate().map(|(i, &d)| !(d && f.dailies == DailiesMode::Hide) && !hidden(i)).collect();

    // 2. effective edges → 3. orphan classification
    let mut pass_deg = vec![0usize; n]; // real edges to passing nodes
    let mut eff_deg  = vec![0usize; n];
    for &(a, b) in &g.edges {
        if !pass[a] || !pass[b] { continue; }
        pass_deg[a] += 1; pass_deg[b] += 1;
        if !f.links_to_dailies_count && (daily[a] || daily[b]) { continue; }
        eff_deg[a] += 1; eff_deg[b] += 1;
    }
    let mut orphan: Vec<bool> = (0..n).map(|i| pass[i] && eff_deg[i] == 0).collect();
    if !f.links_to_dailies_count {
        // a daily is connected when it links to a connected note
        let mut rescued = vec![false; n];
        for &(a, b) in &g.edges {
            if !pass[a] || !pass[b] { continue; }
            if daily[a] && !daily[b] && !orphan[b] { rescued[a] = true; }
            if daily[b] && !daily[a] && !orphan[a] { rescued[b] = true; }
        }
        for i in 0..n { if pass[i] && daily[i] { orphan[i] = !rescued[i]; } }
    }
    let db_orphan: Vec<bool> = g.node_list.iter().map(|nd| nd.degree == 0).collect();

    // 4. orphan filter
    let visible: Vec<bool> = (0..n).map(|i| pass[i] && match f.orphans {
        OrphansMode::Show => true,
        OrphansMode::Hide => !orphan[i],
        OrphansMode::Only => orphan[i],
    }).collect();

    let visible_edges: Vec<(usize, usize)> = g.edges.iter().copied()
        .filter(|&(a, b)| visible[a] && visible[b]).collect();
    let mut visible_degree = vec![0usize; n];
    for &(a, b) in &visible_edges { visible_degree[a] += 1; visible_degree[b] += 1; }

    let count = |p: &dyn Fn(usize) -> bool| (0..n).filter(|&i| p(i)).count();
    let counts = ViewCounts {
        total:               n,
        visible:             count(&|i| visible[i]),
        visible_edges:       visible_edges.len(),
        dailies:             count(&|i| daily[i]),
        orphans_in_view:     count(&|i| orphan[i]),
        orphans_in_db:       count(&|i| db_orphan[i]),
        isolated_by_filters: count(&|i| orphan[i] && !db_orphan[i] && pass_deg[i] == 0),
        daily_only:          count(&|i| orphan[i] && pass_deg[i] > 0),
        dangling:            g.dangling.len(),
        hidden_by_groups:    count(&|i| hidden(i) && !(daily[i] && f.dailies == DailiesMode::Hide)),
    };
    ViewState { visible, visible_edges, visible_degree, orphan, db_orphan, daily, pass_degree: pass_deg, counts }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::RawNode;

    fn node(id: &str, title: &str, file: &str) -> RawNode {
        RawNode { id: id.into(), title: title.into(), file: file.into(), ..Default::default() }
    }

    fn e(a: &str, b: &str) -> (String, String) { (a.into(), b.into()) }

    /// x — d (daily) ; a — b ; o (db orphan) ; d2 (daily, unlinked)
    fn fixture() -> Graph {
        Graph::new(vec![
            node("x",  "Note X",     "/n/x.org"),
            node("d",  "2026-09-01", "/n/2026-09-01.org"),
            node("a",  "A",          "/n/a.org"),
            node("b",  "B",          "/n/b.org"),
            node("o",  "Lonely",     "/n/o.org"),
            node("d2", "Standup",    "/n/daily/standup.org"),
        ], vec![e("x", "d"), e("a", "b")])
    }

    fn filt(dailies: DailiesMode, orphans: OrphansMode, ltdc: bool) -> ViewFilter {
        ViewFilter { dailies, orphans, links_to_dailies_count: ltdc, ..ViewFilter::default() }
    }

    fn idx(g: &Graph, id: &str) -> usize { g.nodes[id] }

    fn visible_ids(g: &Graph, v: &ViewState) -> Vec<String> {
        (0..v.visible.len()).filter(|&i| v.visible[i]).map(|i| g.node_list[i].id.clone()).collect()
    }

    #[test]
    fn dailies_match_whole_path_components_only() {
        let dirs = default_dailies_dirs();
        let d = |t: &str, f: &str| is_daily(t, f, 0, &dirs);
        assert!(d("2026-09-01", "/n/x.org"));
        assert!(!d("2026-09-01 notes", "/n/x.org"));
        assert!(d("x", "/home/me/org/daily/x.org"));
        assert!(d("x", "/home/me/org/Journals/x.org"), "case-insensitive");
        assert!(d("x", r"C:\Users\me\org\journal\x.org"), "windows separators");
        assert!(d("x", r"C:\Users\me/org\dailies/x.org"), "mixed separators");
        assert!(!d("x", "/home/me/org/dailyish.org"));
        assert!(!d("x", "/home/me/org/dailyish/x.org"));
        assert!(!d("x", "/home/me/org/daily.org"), "the file name never matches");
        assert!(!d("x", "/home/me/org/my-journal/x.org"));
        assert!(!d("x", ""));
        let custom = vec!["Logs".to_string()];
        assert!(is_daily("x", "/n/logs/x.org", 0, &custom));
        assert!(!is_daily("x", "/n/daily/x.org", 0, &custom));
        assert!(!is_daily("x", "/n/daily/x.org", 0, &[]));
    }

    /// An ancestor of the notes directory named like a dailies dir (a
    /// `~/Journal/roam/` or `C:\Users\me\Daily\roam\` checkout) must not
    /// make every note a daily: matching starts below the notes root.
    #[test]
    fn dailies_dirs_match_below_the_notes_root_only() {
        let dirs = default_dailies_dirs();
        let mac = ["/Users/me/Journal/roam/note.org", "/Users/me/Journal/roam/sub/b.org",
                   "/Users/me/Journal/roam/daily/2026.org"];
        let root = notes_root_depth(mac);
        assert_eq!(root, 5, "['', Users, me, Journal, roam]");
        assert!(!is_daily("Note", mac[0], root, &dirs));
        assert!(!is_daily("B", mac[1], root, &dirs));
        assert!(is_daily("Standup", mac[2], root, &dirs));
        assert!(is_daily("Note", mac[0], 0, &dirs), "unanchored, the ancestor would match");

        let win = [r"C:\Users\me\Daily\roam\a.org", r"C:\Users\me\Daily\roam\journal\j.org"];
        let root = notes_root_depth(win);
        assert!(!is_daily("A", win[0], root, &dirs));
        assert!(is_daily("J", win[1], root, &dirs));

        assert_eq!(notes_root_depth(["/a/b/x.org", "/a/c/y.org"]), 2);
        assert_eq!(notes_root_depth(["/a/x.org"]), 2, "one file: its own directory");
        assert_eq!(notes_root_depth(std::iter::empty::<&str>()), 0);
        assert_eq!(notes_root_depth(["/a/x.org", "rel.org"]), 0);

        // through compute_view: every note under ~/Journal/roam stays a note
        let g = Graph::new(vec![node("n", "Note", mac[0]), node("b", "B", mac[1]), node("s", "Standup", mac[2])], vec![]);
        let v = compute_view(&g, &filt(DailiesMode::Hide, OrphansMode::Show, true));
        assert_eq!(visible_ids(&g, &v), vec!["n", "b"]);
        assert_eq!(v.counts.dailies, 1);
    }

    #[test]
    fn show_everything_by_default() {
        let g = fixture();
        let v = compute_view(&g, &ViewFilter::default());
        assert!(v.visible.iter().all(|&b| b));
        assert_eq!(v.counts.visible, 6);
        assert_eq!(v.counts.dailies, 2);
        assert_eq!(v.counts.orphans_in_db, 2);   // o, d2
        assert_eq!(v.counts.orphans_in_view, 2);
        assert_eq!(v.counts.isolated_by_filters, 0);
        assert_eq!(v.visible_edges.len(), 2);
    }

    #[test]
    fn orphans_hide_and_only() {
        let g = fixture();
        let hide = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Hide, true));
        assert_eq!(visible_ids(&g, &hide), vec!["x", "d", "a", "b"]);
        let only = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Only, true));
        assert_eq!(visible_ids(&g, &only), vec!["o", "d2"]);
        assert!(only.visible_edges.is_empty());
        // Hide and Only partition the attribute-filtered set
        for i in 0..g.node_list.len() { assert!(hide.visible[i] ^ only.visible[i]); }
    }

    #[test]
    fn hiding_dailies_isolates_a_note_linked_only_to_a_daily() {
        let g = fixture();
        let (x, d) = (idx(&g, "x"), idx(&g, "d"));
        for ltdc in [true, false] {
            let v = compute_view(&g, &filt(DailiesMode::Hide, OrphansMode::Show, ltdc));
            assert!(!v.visible[d]);
            assert!(v.visible[x]);
            assert!(v.orphan[x] && !v.db_orphan[x], "isolated by filters (ltdc={ltdc})");
            assert_eq!(v.counts.isolated_by_filters, 1);
            assert_eq!(v.counts.orphans_in_view, 2); // x, o (d2 is filtered out, not an orphan in view)
            assert_eq!(v.visible_degree[x], 0);
            let v = compute_view(&g, &filt(DailiesMode::Hide, OrphansMode::Hide, ltdc));
            assert_eq!(visible_ids(&g, &v), vec!["a", "b"], "A1: the orphan filter sees the filtered graph");
        }
    }

    #[test]
    fn links_to_dailies_count_only_matters_while_dailies_are_shown() {
        let g = fixture();
        let (x, d, o) = (idx(&g, "x"), idx(&g, "d"), idx(&g, "o"));
        let counted = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Show, true));
        assert!(!counted.orphan[x] && !counted.orphan[d]);

        let ignored = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Show, false));
        assert!(ignored.orphan[x], "an edge to a daily doesn't rescue x");
        assert!(ignored.orphan[d], "d links only to x, itself an orphan: drawn alone otherwise");
        assert_eq!(ignored.counts.isolated_by_filters, 0, "no filter hides anything");
        assert_eq!(ignored.counts.daily_only, 2);
        assert!(ignored.orphan[o] && ignored.db_orphan[o]);
        // still drawn: classification never removes an edge between visible nodes
        assert_eq!(ignored.visible_edges.len(), 2);

        let only = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Only, false));
        assert_eq!(visible_ids(&g, &only), vec!["x", "d", "o", "d2"]);
        let hide = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Hide, false));
        assert_eq!(visible_ids(&g, &hide), vec!["a", "b"]);
    }

    /// The rule takes credit from the NOTE end only: a daily that links to a
    /// connected note stays connected (and visible under Orphans=Hide).
    #[test]
    fn a_daily_linked_to_a_real_note_is_not_an_orphan() {
        // a — b ; d (daily) — a ; d — x ; x — d3 (daily) ; d3 — d4 (daily)
        let g = Graph::new(vec![
            node("a", "A", "/n/a.org"), node("b", "B", "/n/b.org"), node("x", "X", "/n/x.org"),
            node("d", "2026-09-01", "/n/d.org"), node("d3", "2026-09-03", "/n/d3.org"),
            node("d4", "Weekly", "/n/journal/d4.org"),
        ], vec![e("a", "b"), e("d", "a"), e("d", "x"), e("x", "d3"), e("d3", "d4")]);
        let id = |s: &str| idx(&g, s);
        let v = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Show, false));
        assert!(!v.orphan[id("d")], "d links to a, a connected note");
        assert!(v.orphan[id("x")], "x links only to dailies");
        assert!(v.orphan[id("d3")] && v.orphan[id("d4")], "dailies whose notes are all orphans");
        assert_eq!(v.counts.daily_only, 3);
        assert_eq!(v.counts.isolated_by_filters, 0);

        let hide = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Hide, false));
        assert_eq!(visible_ids(&g, &hide), vec!["a", "b", "d"], "the linked daily stays");
        for i in 0..g.node_list.len() { if hide.visible[i] { assert!(hide.visible_degree[i] > 0); } }
    }

    /// The no-fixpoint guarantee: after the orphan filter, no visible node
    /// that was classified connected has lost all of its visible edges.
    #[test]
    fn hiding_orphans_never_creates_new_orphans() {
        // star around a daily hub + chains + a note linked to both a daily and a note
        let g = Graph::new(vec![
            node("h", "2026-01-01", "/n/h.org"),
            node("s1", "S1", "/n/s1.org"), node("s2", "S2", "/n/s2.org"),
            node("p", "P", "/n/p.org"), node("q", "Q", "/n/q.org"), node("r", "R", "/n/r.org"),
            node("j", "J", "/n/journal/j.org"), node("k", "K", "/n/k.org"),
            node("z", "Z", "/n/z.org"),
        ], vec![e("h", "s1"), e("h", "s2"), e("p", "q"), e("q", "r"), e("j", "k"), e("k", "p"), e("s1", "j")]);
        for dailies in [DailiesMode::Show, DailiesMode::Hide] {
            for ltdc in [true, false] {
                let v = compute_view(&g, &filt(dailies, OrphansMode::Hide, ltdc));
                for i in 0..g.node_list.len() {
                    if v.visible[i] {
                        assert!(!v.orphan[i]);
                        assert!(v.visible_degree[i] > 0, "node {i} isolated by Hide ({dailies:?}, ltdc={ltdc})");
                    }
                }
                if ltdc {
                    // with the default rule, view orphan == drawn degree 0
                    let s = compute_view(&g, &filt(dailies, OrphansMode::Show, ltdc));
                    for i in 0..g.node_list.len() {
                        if s.visible[i] { assert_eq!(s.orphan[i], s.visible_degree[i] == 0); }
                    }
                }
            }
        }
    }

    #[test]
    fn counts_and_dedupe_reach_the_view() {
        // duplicate + reversed + self link collapse to one edge (A2)
        let g = Graph::new(vec![node("a", "A", "/a.org"), node("b", "B", "/b.org"), node("c", "C", "/c.org")],
                           vec![e("a", "b"), e("b", "a"), e("a", "b"), e("c", "c")]);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.node_list[g.nodes["a"]].degree, 1);
        assert_eq!(g.node_list[g.nodes["c"]].degree, 0, "self-link adds nothing");
        let v = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Hide, true));
        assert_eq!(v.counts, ViewCounts {
            total: 3, visible: 2, visible_edges: 1, dailies: 0,
            orphans_in_view: 1, orphans_in_db: 1, isolated_by_filters: 0, daily_only: 0, dangling: 0,
            hidden_by_groups: 0,
        });
    }

    /// A5 end to end: dangling links found by the db loader reach the view
    /// counts (and so the help panel) through `Graph::from_data`.
    #[test]
    fn dangling_links_reach_the_view_counts() {
        let c = crate::db::tests::fixture(
            &[("A", "Alpha", "/n/a.org", 0), ("B", "Beta", "/n/b.org", 0)],
            &[("A", "B", "id"), ("A", "missing", "id"), ("B", "Nowhere", "fuzzy"), ("A", "//x.org", "https")],
        );
        let data = crate::db::load_graph_from(&c).unwrap();
        assert_eq!(data.dangling.len(), 2);
        let g = Graph::from_data(data);
        assert_eq!(g.dangling.len(), 2);
        let v = compute_view(&g, &ViewFilter::default());
        assert_eq!(v.counts.dangling, 2);
        assert_eq!(v.counts.visible_edges, 1, "https link stays out of the graph");
        let v = compute_view(&g, &filt(DailiesMode::Hide, OrphansMode::Only, true));
        assert_eq!(v.counts.dangling, 2, "a db-wide count, whatever the filters");
    }

    /// Group hiding is an attribute filter: it runs before orphan
    /// classification, exactly like hiding dailies.
    #[test]
    fn hidden_mask_filters_before_orphan_classification() {
        let g = fixture();
        let (x, d, a, b) = (idx(&g, "x"), idx(&g, "d"), idx(&g, "a"), idx(&g, "b"));
        let mut hidden = vec![false; g.node_list.len()];
        hidden[d] = true; // hide the daily as if by a group
        let v = compute_view_hiding(&g, &ViewFilter::default(), &hidden);
        assert!(!v.visible[d] && v.visible[x]);
        assert!(v.orphan[x], "x lost its only link to a group-hidden node");
        assert_eq!(v.counts.isolated_by_filters, 1);
        assert_eq!(v.counts.hidden_by_groups, 1);
        let v = compute_view_hiding(&g, &filt(DailiesMode::Show, OrphansMode::Hide, true), &hidden);
        assert_eq!(visible_ids(&g, &v), vec!["a", "b"]);
        // already hidden by the dailies filter: not counted twice
        let v = compute_view_hiding(&g, &filt(DailiesMode::Hide, OrphansMode::Show, true), &hidden);
        assert_eq!(v.counts.hidden_by_groups, 0);
        // a mask of the wrong length is ignored (stale), never indexed
        let v = compute_view_hiding(&g, &ViewFilter::default(), &[true]);
        assert!(v.visible[a] && v.visible[b] && v.visible[d]);
    }

    /// The inspector's orphan wording matches the counts' split.
    #[test]
    fn orphan_kind_per_node() {
        let g = fixture();
        let (x, d, a, o) = (idx(&g, "x"), idx(&g, "d"), idx(&g, "a"), idx(&g, "o"));
        let v = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Show, true));
        assert_eq!((v.orphan_kind(x), v.orphan_kind(a), v.orphan_kind(o)), (OrphanKind::Linked, OrphanKind::Linked, OrphanKind::InDb));
        let v = compute_view(&g, &filt(DailiesMode::Hide, OrphansMode::Show, true));
        assert_eq!(v.orphan_kind(x), OrphanKind::IsolatedByFilters);
        let v = compute_view(&g, &filt(DailiesMode::Show, OrphansMode::Show, false));
        assert_eq!(v.orphan_kind(x), OrphanKind::DailyOnly);
        assert_eq!(v.orphan_kind(d), OrphanKind::DailyOnly, "a daily linked only to an orphan note");
        assert_eq!(ViewState::default().orphan_kind(3), OrphanKind::Linked, "out of range: no panic");
    }

    #[test]
    fn paths_relative_to_the_notes_root() {
        let files = ["/n/posts/a.org", "/n/b.org"];
        let root = notes_root_depth(files);
        assert_eq!(relative_to_root("/n/posts/a.org", root), "posts/a.org");
        assert_eq!(relative_to_root("/n/b.org", root), "b.org");
        assert_eq!(relative_to_root(r"C:\notes\x\y.org", 2), "x/y.org");
        assert_eq!(relative_to_root("a.org", 5), "a.org");
    }

    #[test]
    fn modes_cycle_and_serialize_lowercase() {
        assert_eq!(OrphansMode::Show.cycled(), OrphansMode::Hide);
        assert_eq!(OrphansMode::Hide.cycled(), OrphansMode::Only);
        assert_eq!(OrphansMode::Only.cycled(), OrphansMode::Show);
        assert_eq!(DailiesMode::Show.toggled(), DailiesMode::Hide);
        assert_eq!(serde_json::to_string(&OrphansMode::Only).unwrap(), "\"only\"");
        assert_eq!(serde_json::from_str::<DailiesMode>("\"hide\"").unwrap(), DailiesMode::Hide);
        // the words the UI shows, any case, read back too
        assert_eq!(serde_json::from_str::<OrphansMode>("\"Hidden\"").unwrap(), OrphansMode::Hide);
        assert_eq!(serde_json::from_str::<OrphansMode>("\" ONLY \"").unwrap(), OrphansMode::Only);
        assert_eq!(serde_json::from_str::<DailiesMode>("\"Shown\"").unwrap(), DailiesMode::Show);
        assert!(serde_json::from_str::<DailiesMode>("\"only\"").is_err());
        assert!(serde_json::from_str::<OrphansMode>("\"maybe\"").is_err());
        assert!(serde_json::from_str::<OrphansMode>("3").is_err());
    }
}
