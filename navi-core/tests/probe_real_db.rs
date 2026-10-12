//! Sanity probe against a real org-roam db, opened READ-ONLY.
//! `cargo test -p navi-core --test probe_real_db -- --ignored --nocapture`
//! (path: $ORG_ROAM_DB, else ~/.emacs.d/org-roam.db).

use navi_core::{compute_view, load_graph, DailiesMode, Graph, OrphansMode, ViewFilter};

#[test]
#[ignore = "reads the owner's real org-roam.db"]
fn probe_real_db() {
    let path = std::env::var("ORG_ROAM_DB").unwrap_or_else(|_| navi_core::config::expand_tilde("~/.emacs.d/org-roam.db"));
    let data = load_graph(&path).expect("open db read-only");
    let s = data.stats;
    println!("db: {path}");
    println!("nodes {}  headings {}  edges {}  dangling {}  (rows {}  dup {}  self {}  external {}  stray {})",
        data.nodes.len(), data.nodes.iter().filter(|n| n.level > 0).count(), data.edges.len(),
        data.dangling.len(), s.rows, s.duplicates, s.self_links, s.external, s.stray_sources);
    for d in &data.dangling { println!("  dangling {:>5}  {} -> {:?}", d.link_type, d.source, d.dest); }
    println!("tagged {}", data.nodes.iter().filter(|n| !n.tags.is_empty()).count());

    let g = Graph::from_data(data);
    for (dailies, orphans) in [(DailiesMode::Show, OrphansMode::Show), (DailiesMode::Hide, OrphansMode::Show),
                               (DailiesMode::Hide, OrphansMode::Hide), (DailiesMode::Show, OrphansMode::Only)] {
        let v = compute_view(&g, &ViewFilter { dailies, orphans, ..ViewFilter::default() });
        println!("{dailies:?}/{orphans:?}: {:?}", v.counts);
        for i in 0..v.visible.len() {
            if v.visible[i] && orphans == OrphansMode::Hide { assert!(v.visible_degree[i] > 0); }
        }
    }
}
