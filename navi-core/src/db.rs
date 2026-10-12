use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct RawNode {
    pub id:          String,
    pub title:       String,
    pub file:        String,
    pub level:       i64,
    pub pos:         i64,
    pub mtime:       i64,
    pub aliases:     Vec<String>,
    pub tags:        Vec<String>,
    pub olp:         Vec<String>,    // outline path of a heading node (empty for file nodes)
    pub parent_file: Option<String>, // heading node → id of its file's level-0 node (containment, NOT a link)
}

/// A link that names something which is not a node: an `id:` link whose
/// target id is missing from `nodes`, or a `fuzzy` (unresolved `[[title]]`)
/// link. Recorded so the UI can count them instead of silently dropping them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DanglingLink {
    pub source:    String, // node id the link lives in
    pub dest:      String, // raw (unquoted) target text: an id or a title/description
    pub link_type: String, // "id" (target missing) or "fuzzy"
}

/// Bookkeeping from link classification — nothing here reaches the graph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    pub rows:          usize, // every row read from `links`
    pub duplicates:    usize, // id links repeating an already-seen unordered pair
    pub self_links:    usize, // id links from a node to itself (dropped)
    pub external:      usize, // https/file/cite/… — not node links, kept out of the graph
    pub stray_sources: usize, // links whose source is not a node (can't be drawn or counted)
}

/// Everything the graph needs from the db, already cleaned:
/// `edges` are real id links (both endpoints exist), deduped as UNORDERED
/// pairs in first-seen order, self-links removed.
#[derive(Debug, Clone, Default)]
pub struct GraphData {
    pub nodes:    Vec<RawNode>,
    pub edges:    Vec<(String, String)>,
    pub dangling: Vec<DanglingLink>,
    pub stats:    LinkStats,
}

/// One row of `links` after unquoting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLink {
    pub source:    String,
    pub dest:      String,
    pub link_type: String,
}

pub fn load_graph(db_path: &str) -> rusqlite::Result<GraphData> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    // org-roam-db-sync may be writing while we read; don't fail the whole
    // load (or startup) on a transient SQLITE_BUSY.
    let _ = conn.busy_timeout(std::time::Duration::from_millis(1500));
    load_graph_from(&conn)
}

/// The loader proper, on any open connection (tests feed an in-memory db).
pub fn load_graph_from(conn: &Connection) -> rusqlite::Result<GraphData> {
    let mut nodes: HashMap<String, RawNode> = HashMap::new();

    // Try with files JOIN first; mtime may be Emacs `(HIGH LOW ...)` string
    let rows_result = conn.prepare(
        "SELECT n.id, n.title, n.file, n.level, n.pos, f.mtime
         FROM nodes n LEFT JOIN files f ON n.file = f.file",
    );

    match rows_result {
        Ok(mut stmt) => {
            let iter = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    row.get::<_, Option<String>>(5)?,   // may be "(HIGH LOW ...)" or integer
                ))
            })?;
            for row in iter.flatten() {
                let (id, title, file, level, pos, mtime_raw) = row;
                let id = unquote(&id);
                let mtime = parse_mtime(mtime_raw.as_deref());
                nodes.insert(id.clone(), RawNode {
                    id, title: unquote(&title), file: unquote(&file), level, pos, mtime,
                    ..RawNode::default()
                });
            }
        }
        Err(_) => {
            let mut stmt = conn.prepare(
                "SELECT id, COALESCE(title,''), COALESCE(file,''), COALESCE(level,0), COALESCE(pos,0) FROM nodes",
            )?;
            let iter = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?;
            for row in iter.flatten() {
                let (id, title, file, level, pos) = row;
                let id = unquote(&id);
                nodes.insert(id.clone(), RawNode {
                    id, title: unquote(&title), file: unquote(&file), level, pos,
                    ..RawNode::default()
                });
            }
        }
    }

    // Outline paths — optional column, separate query so its absence can't
    // knock the main node query onto the mtime-less fallback.
    if let Ok(mut stmt) = conn.prepare("SELECT id, olp FROM nodes") {
        if let Ok(iter) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        }) {
            for (id, olp) in iter.flatten() {
                if let (Some(n), Some(olp)) = (nodes.get_mut(&unquote(&id)), olp) {
                    n.olp = parse_lisp_strings(&olp);
                }
            }
        }
    }

    // Links — with their type when the schema has one (pre-v2 dbs don't:
    // every row is then taken to be an id link).
    let mut raw_links: Vec<RawLink> = Vec::new();
    match conn.prepare("SELECT source, dest, type FROM links") {
        Ok(mut stmt) => {
            if let Ok(iter) = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                ))
            }) {
                for (s, d, t) in iter.flatten() {
                    raw_links.push(RawLink { source: unquote(&s), dest: unquote(&d), link_type: unquote(&t) });
                }
            }
        }
        Err(_) => {
            if let Ok(mut stmt) = conn.prepare("SELECT source, dest FROM links") {
                if let Ok(iter) = stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                }) {
                    for (s, d) in iter.flatten() {
                        raw_links.push(RawLink { source: unquote(&s), dest: unquote(&d), link_type: "id".into() });
                    }
                }
            }
        }
    }

    // Aliases
    if let Ok(mut stmt) = conn.prepare("SELECT node_id, alias FROM aliases") {
        let iter = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        });
        if let Ok(iter) = iter {
            for row in iter.flatten() {
                if let Some(n) = nodes.get_mut(&unquote(&row.0)) {
                    let alias = unquote(&row.1);
                    if !alias.is_empty() {
                        n.aliases.push(alias);
                    }
                }
            }
        }
    }

    // Tags
    if let Ok(mut stmt) = conn.prepare("SELECT node_id, tag FROM tags") {
        let iter = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        });
        if let Ok(iter) = iter {
            for row in iter.flatten() {
                if let Some(n) = nodes.get_mut(&unquote(&row.0)) {
                    let tag = unquote(&row.1);
                    if !tag.is_empty() {
                        n.tags.push(tag);
                    }
                }
            }
        }
    }

    // Deterministic order (file, then position in it, then id): node
    // indices — and with them layout jitter seeds — no longer depend on
    // HashMap iteration order, and a file's headings sit next to it.
    let mut node_list: Vec<RawNode> = nodes.into_values().collect();
    node_list.sort_by(|a, b| a.file.cmp(&b.file).then(a.pos.cmp(&b.pos)).then(a.id.cmp(&b.id)));
    assign_parent_files(&mut node_list);

    let ids: HashSet<&str> = node_list.iter().map(|n| n.id.as_str()).collect();
    let (edges, dangling, stats) = classify_links(&ids, raw_links);
    Ok(GraphData { nodes: node_list, edges, dangling, stats })
}

// ── Link classification ───────────────────────────────────────────────────────

/// Sort raw link rows into graph edges, dangling links and stats.
///  * `id` with both endpoints a node → edge, deduped as an UNORDERED pair
///    (a→b and b→a are one edge), self-links dropped and counted.
///  * `id` whose dest is not a node, or `fuzzy` → dangling (source must be a node).
///  * anything else (https, file, cite, …) → `external`, never in the graph.
pub fn classify_links(
    ids:   &HashSet<&str>,
    links: Vec<RawLink>,
) -> (Vec<(String, String)>, Vec<DanglingLink>, LinkStats) {
    let mut stats    = LinkStats { rows: links.len(), ..LinkStats::default() };
    let mut seen:      HashSet<(String, String)> = HashSet::new();
    let mut seen_dang: HashSet<DanglingLink>     = HashSet::new();
    let mut edges    = Vec::new();
    let mut dangling = Vec::new();

    for l in links {
        let is_id = l.link_type == "id";
        if !is_id && l.link_type != "fuzzy" { stats.external += 1; continue; }
        if !ids.contains(l.source.as_str()) { stats.stray_sources += 1; continue; }
        if is_id && ids.contains(l.dest.as_str()) {
            if l.source == l.dest { stats.self_links += 1; continue; }
            let key = if l.source <= l.dest { (l.source.clone(), l.dest.clone()) }
                      else                  { (l.dest.clone(), l.source.clone()) };
            if seen.insert(key) { edges.push((l.source, l.dest)); }
            else                { stats.duplicates += 1; }
        } else {
            let d = DanglingLink { source: l.source, dest: l.dest, link_type: l.link_type };
            if seen_dang.insert(d.clone()) { dangling.push(d); }
        }
    }
    (edges, dangling, stats)
}

/// Containment: each heading node (level > 0) gets the id of the level-0 node
/// of the same file. A file with no file-level node leaves its headings `None`.
pub fn assign_parent_files(nodes: &mut [RawNode]) {
    let mut file_node: HashMap<String, String> = HashMap::new();
    for n in nodes.iter().filter(|n| n.level == 0) {
        file_node.entry(n.file.clone()).or_insert_with(|| n.id.clone());
    }
    for n in nodes.iter_mut() {
        n.parent_file = if n.level > 0 { file_node.get(&n.file).cloned() } else { None };
    }
}

// ── Lisp value helpers ────────────────────────────────────────────────────────

/// org-roam stores strings as Lisp-printed literals: wrapped in one pair of
/// double quotes, with `\"` and `\\` escapes inside. Strip exactly one quote
/// pair (a title legitimately ending in `"` keeps it) and unescape; strings
/// stored without quotes pass through untouched.
pub fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|t| t.strip_suffix('"')) else {
        return s.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(n) => out.push(n),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Every string literal in a printed Lisp list: `("A" "B \"c\"")` → [A, B "c"].
/// `nil`, empty, or anything without quotes → empty.
pub fn parse_lisp_strings(s: &str) -> Vec<String> {
    let mut out   = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '"' { continue; }
        let mut cur = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => if let Some(n) = chars.next() { cur.push(n) },
                '"'  => break,
                _    => cur.push(c),
            }
        }
        out.push(cur);
    }
    out
}

/// Parse Emacs mtime which may be:
///   - A plain integer string "1779015285"
///   - Emacs internal time "(HIGH LOW MICROSEC PICOSEC)" → HIGH*65536 + LOW
///   - nil / empty → 0
fn parse_mtime(raw: Option<&str>) -> i64 {
    let s = match raw { Some(s) if !s.is_empty() => s, _ => return 0 };
    // Try plain integer first
    if let Ok(n) = s.trim().parse::<i64>() { return n; }
    // Emacs list: strip parens, split on whitespace, take HIGH and LOW
    let inner = s.trim().trim_start_matches('(').trim_end_matches(')');
    let parts: Vec<&str> = inner.split_whitespace().collect();
    if parts.len() >= 2 {
        let high: i64 = parts[0].parse().unwrap_or(0);
        let low:  i64 = parts[1].parse().unwrap_or(0);
        return high * 65536 + low;
    }
    0
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// org-roam v2 schema (as `org-roam-db--table-schemata` creates it).
    pub(crate) const SCHEMA: &str = "
        CREATE TABLE files (file UNIQUE PRIMARY KEY, title, hash NOT NULL, atime NOT NULL, mtime NOT NULL);
        CREATE TABLE nodes (id NOT NULL PRIMARY KEY, file NOT NULL, level NOT NULL, pos NOT NULL, todo, priority,
                            scheduled text, deadline text, title, properties, olp);
        CREATE TABLE aliases (node_id NOT NULL, alias);
        CREATE TABLE tags (node_id NOT NULL, tag);
        CREATE TABLE links (pos NOT NULL, source NOT NULL, dest NOT NULL, type NOT NULL, properties NOT NULL);
    ";

    fn q(s: &str) -> String { format!("\"{s}\"") }

    /// In-memory db with Lisp-quoted strings, like a real org-roam.db.
    pub(crate) fn fixture(
        nodes: &[(&str, &str, &str, i64)],      // id, title, file, level
        links: &[(&str, &str, &str)],           // source, dest, type
    ) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        let mut files: Vec<&str> = nodes.iter().map(|n| n.2).collect();
        files.sort(); files.dedup();
        for f in files {
            c.execute("INSERT INTO files VALUES (?1, NULL, 'h', '(0 0 0 0)', '(26000 1000 0 0)')", [q(f)]).unwrap();
        }
        for (i, (id, title, file, level)) in nodes.iter().enumerate() {
            let olp = if *level > 0 { Some("(\"Parent\" \"Sub \\\"x\\\"\")".to_string()) } else { None };
            c.execute(
                "INSERT INTO nodes (id, file, level, pos, title, olp) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![q(id), q(file), level, (i as i64) * 10 + 1, q(title), olp],
            ).unwrap();
        }
        for (i, (s, d, t)) in links.iter().enumerate() {
            c.execute("INSERT INTO links VALUES (?1, ?2, ?3, ?4, '(:outline nil)')",
                rusqlite::params![i as i64, q(s), q(d), q(t)]).unwrap();
        }
        c
    }

    fn rl(s: &str, d: &str, t: &str) -> RawLink {
        RawLink { source: s.into(), dest: d.into(), link_type: t.into() }
    }

    #[test]
    fn unquote_strips_one_pair_and_unescapes() {
        assert_eq!(unquote("\"id\""), "id");
        assert_eq!(unquote("\"fuzzy\""), "fuzzy");
        assert_eq!(unquote("\"say \\\"hi\\\"\""), "say \"hi\"");
        assert_eq!(unquote("\"a\\\\b\""), "a\\b");
        assert_eq!(unquote("plain"), "plain");
        assert_eq!(unquote("\"\""), "");
        assert_eq!(unquote("\"unterminated"), "\"unterminated");
    }

    #[test]
    fn lisp_string_lists() {
        assert_eq!(parse_lisp_strings("(\"A\" \"B \\\"c\\\"\")"), vec!["A", "B \"c\""]);
        assert!(parse_lisp_strings("nil").is_empty());
        assert!(parse_lisp_strings("").is_empty());
    }

    #[test]
    fn link_typing_keeps_external_out() {
        let ids: HashSet<&str> = ["a", "b"].into_iter().collect();
        let (e, d, s) = classify_links(&ids, vec![
            rl("a", "b", "id"),
            rl("a", "//example.com", "https"),
            rl("a", "/tmp/x.org", "file"),
            rl("a", "smith2020", "cite"),
        ]);
        assert_eq!(e, vec![("a".to_string(), "b".to_string())]);
        assert!(d.is_empty());
        assert_eq!(s.external, 3);
        assert_eq!(s.rows, 4);
    }

    #[test]
    fn dedupe_unordered_pairs_and_drop_self_links() {
        let ids: HashSet<&str> = ["a", "b", "c"].into_iter().collect();
        let (e, _, s) = classify_links(&ids, vec![
            rl("a", "b", "id"), rl("a", "b", "id"), rl("b", "a", "id"),
            rl("c", "c", "id"), rl("b", "c", "id"),
        ]);
        assert_eq!(e, vec![("a".into(), "b".into()), ("b".into(), "c".into())]);
        assert_eq!(s.duplicates, 2);
        assert_eq!(s.self_links, 1);
    }

    #[test]
    fn dangling_classification() {
        let ids: HashSet<&str> = ["a", "b"].into_iter().collect();
        let (e, d, s) = classify_links(&ids, vec![
            rl("a", "gone-id", "id"),
            rl("a", "Some Title", "fuzzy"),
            rl("a", "Some Title", "fuzzy"),   // identical dangling link counted once
            rl("ghost", "b", "id"),           // source not a node: unusable
            rl("ghost", "Whatever", "fuzzy"),
        ]);
        assert!(e.is_empty());
        assert_eq!(d, vec![
            DanglingLink { source: "a".into(), dest: "gone-id".into(),    link_type: "id".into() },
            DanglingLink { source: "a".into(), dest: "Some Title".into(), link_type: "fuzzy".into() },
        ]);
        assert_eq!(s.stray_sources, 2);
    }

    #[test]
    fn containment_points_headings_at_their_file_node() {
        let mut ns = vec![
            RawNode { id: "f".into(),  file: "/n/a.org".into(), level: 0, ..Default::default() },
            RawNode { id: "h1".into(), file: "/n/a.org".into(), level: 1, ..Default::default() },
            RawNode { id: "h2".into(), file: "/n/a.org".into(), level: 3, ..Default::default() },
            RawNode { id: "x".into(),  file: "/n/b.org".into(), level: 2, ..Default::default() },
        ];
        assign_parent_files(&mut ns);
        assert_eq!(ns[0].parent_file, None);
        assert_eq!(ns[1].parent_file.as_deref(), Some("f"));
        assert_eq!(ns[2].parent_file.as_deref(), Some("f"));
        assert_eq!(ns[3].parent_file, None, "file without a file node has no parent");
    }

    #[test]
    fn loads_v2_fixture_end_to_end() {
        let c = fixture(
            &[("A", "Alpha", "/n/a.org", 0), ("A1", "Alpha head", "/n/a.org", 1),
              ("B", "Beta", "/n/b.org", 0), ("C", "Gamma \\\"q\\\"", "/n/c.org", 0)],
            &[("A", "B", "id"), ("B", "A", "id"), ("A", "A", "id"),
              ("A", "missing", "id"), ("B", "Nowhere", "fuzzy"), ("C", "//x.org", "https")],
        );
        c.execute("INSERT INTO tags VALUES ('\"A\"', '\"rust\"')", []).unwrap();
        c.execute("INSERT INTO aliases VALUES ('\"B\"', '\"Bee\"')", []).unwrap();
        let g = load_graph_from(&c).unwrap();

        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "A1", "B", "C"], "sorted by file then pos, ids unquoted");
        assert_eq!(g.edges, vec![("A".to_string(), "B".to_string())]);
        assert_eq!(g.stats.duplicates, 1);
        assert_eq!(g.stats.self_links, 1);
        assert_eq!(g.stats.external, 1);
        assert_eq!(g.dangling.len(), 2);
        let a  = &g.nodes[0];
        let a1 = &g.nodes[1];
        assert_eq!(a.tags, vec!["rust"]);
        assert_eq!(g.nodes[2].aliases, vec!["Bee"]);
        assert_eq!(g.nodes[3].title, "Gamma \"q\"");
        assert_eq!(a.file, "/n/a.org");
        assert!(a.mtime > 0, "mtime from the files join");
        assert_eq!(a1.parent_file.as_deref(), Some("A"));
        assert_eq!(a1.olp, vec!["Parent", "Sub \"x\""]);
    }

    #[test]
    fn old_schema_fallbacks_still_load() {
        // No files table, no olp, links without a type column.
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("
            CREATE TABLE nodes (id, file, level, pos, title);
            CREATE TABLE links (source, dest);
            INSERT INTO nodes VALUES ('\"a\"', '\"/n/a.org\"', 0, 1, '\"A\"');
            INSERT INTO nodes VALUES ('\"b\"', '\"/n/b.org\"', 0, 1, '\"B\"');
            INSERT INTO links VALUES ('\"a\"', '\"b\"');
            INSERT INTO links VALUES ('\"a\"', '\"zz\"');
        ").unwrap();
        let g = load_graph_from(&c).unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges, vec![("a".to_string(), "b".to_string())]);
        assert_eq!(g.dangling.len(), 1, "untyped link to a missing id is dangling");
        assert_eq!(g.nodes[0].mtime, 0);
    }
}
