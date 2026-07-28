use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct RawNode {
    pub id: String,
    pub title: String,
    pub file: String,
    pub level: i64,
    pub pos: i64,
    pub mtime: i64,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
}

pub fn load_graph(db_path: &str) -> rusqlite::Result<(Vec<RawNode>, Vec<(String, String)>)> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    // org-roam-db-sync may be writing while we read; don't fail the whole
    // load (or startup) on a transient SQLITE_BUSY.
    let _ = conn.busy_timeout(std::time::Duration::from_millis(1500));

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
                let title = unquote(&title);
                let file = unquote(&file);
                let mtime = parse_mtime(mtime_raw.as_deref());
                nodes.insert(id.clone(),
                    RawNode { id, title, file, level, pos, mtime, aliases: vec![], tags: vec![] });
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
                let title = unquote(&title);
                let file = unquote(&file);
                nodes.insert(id.clone(),
                    RawNode { id, title, file, level, pos, mtime: 0, aliases: vec![], tags: vec![] });
            }
        }
    }

    // Edges
    let mut edges: Vec<(String, String)> = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT source, dest FROM links") {
        let iter = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        });
        if let Ok(iter) = iter {
            for row in iter.flatten() {
                if nodes.contains_key(&row.0) && nodes.contains_key(&row.1) {
                    edges.push(row);
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
                if let Some(n) = nodes.get_mut(&row.0) {
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
                if let Some(n) = nodes.get_mut(&row.0) {
                    let tag = unquote(&row.1);
                    if !tag.is_empty() {
                        n.tags.push(tag);
                    }
                }
            }
        }
    }

    let node_list: Vec<RawNode> = nodes.into_values().collect();
    Ok((node_list, edges))
}

/// org-roam stores strings as Lisp-printed literals: wrapped in one pair of
/// double quotes, with `\"` and `\\` escapes inside. Strip exactly one quote
/// pair (a title legitimately ending in `"` keeps it) and unescape; strings
/// stored without quotes pass through untouched.
fn unquote(s: &str) -> String {
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
