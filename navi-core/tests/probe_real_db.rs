//! Sanity probe against a real org-roam db, opened READ-ONLY.
//! `cargo test -p navi-core --test probe_real_db -- --ignored --nocapture`
//! (path: $ORG_ROAM_DB, else ~/.emacs.d/org-roam.db).

use navi_core::groups::{AutoGrouper, CommunityScope, GroupSpec, Grouping};
use navi_core::view::{notes_root_depth, relative_to_root};
use navi_core::{compute_view, load_graph, DailiesMode, EvalCtx, Graph, OrphanKind, OrphansMode, Query, ViewFilter};

fn db_path() -> String {
    std::env::var("ORG_ROAM_DB").unwrap_or_else(|_| navi_core::config::expand_tilde("~/.emacs.d/org-roam.db"))
}

#[test]
#[ignore = "reads the owner's real org-roam.db"]
fn probe_real_db() {
    let path = db_path();
    let data = load_graph(&path).expect("open db read-only");
    let s = data.stats;
    println!("db: {path}");
    println!("nodes {}  headings {}  edges {}  dangling {}  (rows {}  dup {}  self {}  external {}  stray {})",
        data.nodes.len(), data.nodes.iter().filter(|n| n.level > 0).count(), data.edges.len(),
        data.dangling.len(), s.rows, s.duplicates, s.self_links, s.external, s.stray_sources);
    for d in &data.dangling { println!("  dangling {:>5}  {} -> {:?}", d.link_type, d.source, d.dest); }
    println!("tagged {}  with properties {}", data.nodes.iter().filter(|n| !n.tags.is_empty()).count(),
        data.nodes.iter().filter(|n| !n.properties.is_empty()).count());

    let g = Graph::from_data(data);
    for (dailies, orphans) in [(DailiesMode::Show, OrphansMode::Show), (DailiesMode::Hide, OrphansMode::Show),
                               (DailiesMode::Hide, OrphansMode::Hide), (DailiesMode::Show, OrphansMode::Only)] {
        let v = compute_view(&g, &ViewFilter { dailies, orphans, ..ViewFilter::default() });
        println!("{dailies:?}/{orphans:?}: {:?}", v.counts);
        for i in 0..v.visible.len() {
            if v.visible[i] && orphans == OrphansMode::Hide { assert!(v.visible_degree[i] > 0); }
        }
        // the inspector's per-node orphan wording adds up to the counts
        let kinds: Vec<OrphanKind> = (0..v.visible.len()).map(|i| v.orphan_kind(i)).collect();
        let n = |k: OrphanKind| kinds.iter().filter(|&&x| x == k).count();
        assert_eq!(n(OrphanKind::IsolatedByFilters), v.counts.isolated_by_filters);
        assert_eq!(n(OrphanKind::DailyOnly), v.counts.daily_only);
        assert_eq!(v.counts.orphans_in_view, kinds.len() - n(OrphanKind::Linked));
    }
    // file paths as the inspector shows them: relative to the notes root
    let root = notes_root_depth(g.node_list.iter().map(|nd| nd.file.as_str()));
    let mut rel: Vec<String> = g.node_list.iter().map(|nd| relative_to_root(&nd.file, root)).collect();
    rel.sort();
    let nested = rel.iter().filter(|r| r.contains('/')).count();
    println!("relative paths: {} flat, {nested} in folders; e.g. {:?}", rel.len() - nested, rel.iter().find(|r| r.contains('/')));
    assert!(rel.iter().all(|r| !r.starts_with('/')));
}

/// Group counts per automatic grouper on the real db, plus a few queries.
#[test]
#[ignore = "reads the owner's real org-roam.db"]
fn probe_real_db_groups() {
    let g = Graph::from_data(load_graph(&db_path()).expect("open db read-only"));
    let filter = ViewFilter::default();
    let mut grouping = Grouping::default();
    for auto in [AutoGrouper::Folder { depth: 1 }, AutoGrouper::Tag, AutoGrouper::Component,
                 AutoGrouper::Community { resolution: 1.0, scope: CommunityScope::All }, AutoGrouper::File] {
        let t = std::time::Instant::now();
        let (v, res) = grouping.compute(&g, &filter, &GroupSpec::new(vec![], auto));
        let grouped = res.primary.iter().filter(|p| p.is_some()).count();
        println!("{:<10} {} groups, {grouped}/{} nodes grouped ({:?})", auto.label(), res.groups.len(), v.counts.visible, t.elapsed());
        for gi in &res.groups { println!("    {:<40} {:>3} members  {:?}", gi.label, gi.members, gi.colour); }
        assert!(res.groups.windows(2).all(|w| w[0].key != w[1].key));
    }
    let v  = compute_view(&g, &filter);
    let cx = EvalCtx::from_view(&g, &v);
    for q in ["orphan", "dangling", "dir:posts", "-dir:posts level:0", "degree:>3", "prop:category", "tag:rust OR tag:tangent"] {
        let n = Query::parse(q).unwrap().eval_all(&cx).iter().filter(|&&b| b).count();
        println!("query {q:<28} {n}");
    }
}
