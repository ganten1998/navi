//! Named views, kept per org-roam db in `~/.config/navi/views.json`.
//!
//! A view is everything that decides what the graph shows and how: manual
//! groups, the automatic grouper, the dailies / orphans filters and the
//! group pull strength. Each db (keyed by its canonical path) has an ordered
//! list of views and remembers which one was active.
//!
//! ```json
//! { "version": 1,
//!   "dbs": { "/Users/me/.emacs.d/org-roam.db": {
//!       "active": "Default",
//!       "views": [ { "name": "Default", "auto": { "kind": "tag" }, "groups": [ … ], … } ] } } }
//! ```
//!
//! Loading is tolerant, as config.json is: a bad db entry, view, group or
//! value is dropped (to its default) on its own and the rest is kept; when
//! anything was dropped, or the file isn't JSON at all, the original is
//! copied to `views.json.bad` before navi ever writes over it. Saves are
//! atomic (temp file + rename).
//!
//! Migration: the first time a db has no views, its "Default" view takes the
//! `dailies` / `orphans` / `links_to_dailies_count` an older config.json
//! carried; main.rs drops those keys from config.json only once views.json
//! holding them has been written.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::{atomic_write, parse_tolerant, Config, LegacyView};
use crate::groups::{AutoGrouper, Group, GroupSpec};
use crate::view::{DailiesMode, OrphansMode, ViewFilter};

pub const DEFAULT_PULL: f64 = 0.1;

fn yes() -> bool { true }
fn default_pull() -> f64 { DEFAULT_PULL }
fn default_name() -> String { "Default".into() }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    #[serde(default = "default_name")]
    pub name:                   String,
    #[serde(default)]
    pub groups:                 Vec<Group>,                 // manual groups, highest precedence first
    #[serde(default)]
    pub auto:                   AutoGrouper,
    #[serde(default)]
    pub auto_hidden:            Vec<String>,                // keys of hidden auto groups ("tag:x", "folder:posts")
    #[serde(default)]
    pub auto_pull:              bool,
    #[serde(default)]
    pub auto_hull:              bool,
    #[serde(default, alias = "color_groups")]
    pub colour_groups:          bool,                       // G: colours, hulls, legend and pull on
    #[serde(default)]
    pub dailies:                DailiesMode,
    #[serde(default)]
    pub orphans:                OrphansMode,
    #[serde(default = "yes")]
    pub links_to_dailies_count: bool,
    #[serde(default = "default_pull")]
    pub group_pull:             f64,                        // centroid spring strength for pulling groups
    #[serde(default, alias = "auto_colors")]
    pub auto_colours:           BTreeMap<String, u8>,       // palette slot per auto-group key (stable colours)
    #[serde(default)]
    pub panel_open:             bool,                       // the side panel (backslash key) is open in this view
}

impl Default for View {
    fn default() -> Self {
        View {
            name: default_name(), groups: Vec::new(), auto: AutoGrouper::None, auto_hidden: Vec::new(),
            auto_pull: false, auto_hull: false, colour_groups: false,
            dailies: DailiesMode::Show, orphans: OrphansMode::Show, links_to_dailies_count: true,
            group_pull: DEFAULT_PULL, auto_colours: BTreeMap::new(), panel_open: false,
        }
    }
}

impl View {
    pub fn named(name: &str) -> Self { View { name: name.into(), ..View::default() } }

    /// A view seeded from an older config.json's filter keys.
    pub fn from_legacy(name: &str, l: &LegacyView) -> Self {
        let d = View::default();
        View {
            dailies:                l.dailies.unwrap_or(d.dailies),
            orphans:                l.orphans.unwrap_or(d.orphans),
            links_to_dailies_count: l.links_to_dailies_count.unwrap_or(d.links_to_dailies_count),
            ..View::named(name)
        }
    }

    pub fn filter(&self, dailies_dirs: &[String]) -> ViewFilter {
        ViewFilter {
            dailies:                self.dailies,
            orphans:                self.orphans,
            dailies_dirs:           dailies_dirs.to_vec(),
            links_to_dailies_count: self.links_to_dailies_count,
        }
    }

    /// The grouping half, with queries compiled.
    pub fn spec(&self) -> GroupSpec {
        GroupSpec {
            auto_hidden: self.auto_hidden.iter().cloned().collect::<HashSet<String>>(),
            auto_pull:   self.auto_pull,
            auto_hull:   self.auto_hull,
            ..GroupSpec::new(self.groups.clone(), self.auto)
        }
    }

    /// Tolerant parse of one view object: bad groups and bad values drop
    /// alone. Returns the view and whether anything was dropped.
    fn parse_value(v: serde_json::Value, what: &str) -> Option<(View, bool)> {
        let serde_json::Value::Object(mut obj) = v else {
            eprintln!("navi: {what}: not an object, skipped");
            return None;
        };
        let mut dropped = false;
        let groups = match obj.remove("groups") {
            Some(serde_json::Value::Array(items)) => items.into_iter().enumerate().filter_map(|(k, g)| {
                match serde_json::from_value::<Group>(g) {
                    Ok(g)  => Some(g),
                    Err(e) => { eprintln!("navi: {what}: group {} skipped ({e})", k + 1); dropped = true; None }
                }
            }).collect(),
            Some(_) => { eprintln!("navi: {what}: \"groups\" is not a list, ignored"); dropped = true; Vec::new() }
            None    => Vec::new(),
        };
        let (mut view, d) = parse_tolerant::<View>(obj, what);
        view.groups = groups;
        if !view.group_pull.is_finite() || view.group_pull < 0.0 { view.group_pull = DEFAULT_PULL; dropped = true; }
        Some((view, dropped || d))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DbViews {
    #[serde(default)]
    pub active: String, // name of the active view
    #[serde(default)]
    pub views:  Vec<View>,
}

impl DbViews {
    pub fn active_index(&self) -> usize { self.views.iter().position(|v| v.name == self.active).unwrap_or(0) }
}

#[derive(Serialize)]
struct FileOut<'a> {
    version: u32,
    dbs:     &'a BTreeMap<String, DbViews>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewStore {
    pub path: PathBuf,
    pub dbs:  BTreeMap<String, DbViews>,
}

/// The key a db's views are stored under: its canonical path (symlinks and
/// `..` resolved), or the path as given when it can't be resolved.
pub fn db_key(path: &str) -> String {
    std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string())
}

impl ViewStore {
    pub fn default_path() -> PathBuf { Config::config_dir().join("views.json") }

    /// Load `path`; a missing file is an empty store. See the module docs for
    /// what happens to bad content.
    pub fn load(path: &Path) -> Self {
        let mut store = ViewStore { path: path.to_path_buf(), dbs: BTreeMap::new() };
        let Ok(text) = std::fs::read_to_string(path) else { return store };
        let (dbs, dropped) = Self::parse(&text);
        if dropped {
            let bad = path.with_extension("json.bad");
            if std::fs::write(&bad, &text).is_ok() {
                eprintln!("navi: views.json: the original is kept as {}", bad.display());
            }
        }
        store.dbs = dbs;
        store
    }

    /// Parse views.json text. Returns the dbs and whether anything was
    /// dropped (including the whole file, when it isn't a JSON object).
    pub fn parse(text: &str) -> (BTreeMap<String, DbViews>, bool) {
        let mut out = BTreeMap::new();
        let root = match serde_json::from_str::<serde_json::Value>(text) {
            Ok(serde_json::Value::Object(o)) => o,
            _ => { eprintln!("navi: views.json: not a JSON object, starting fresh"); return (out, true); }
        };
        let mut dropped = false;
        let dbs = match root.get("dbs") {
            Some(serde_json::Value::Object(d)) => d.clone(),
            Some(_) => { eprintln!("navi: views.json: \"dbs\" is not an object"); return (out, true); }
            None => return (out, false),
        };
        for (key, entry) in dbs {
            let serde_json::Value::Object(mut entry) = entry else {
                eprintln!("navi: views.json: entry for {key} is not an object, skipped");
                dropped = true;
                continue;
            };
            let active = match entry.remove("active") {
                Some(serde_json::Value::String(s)) => s,
                None => String::new(),
                Some(_) => { dropped = true; String::new() }
            };
            let mut views: Vec<View> = Vec::new();
            match entry.remove("views") {
                Some(serde_json::Value::Array(items)) => {
                    for (k, v) in items.into_iter().enumerate() {
                        match View::parse_value(v, &format!("views.json: {key}: view {}", k + 1)) {
                            Some((v, d)) => { dropped |= d; views.push(v); }
                            None => dropped = true,
                        }
                    }
                }
                None => {}
                Some(_) => { eprintln!("navi: views.json: {key}: \"views\" is not a list"); dropped = true; }
            }
            // names identify views (`active`); keep them unique
            let mut seen: HashSet<String> = HashSet::new();
            for v in views.iter_mut() {
                let base = v.name.clone();
                let mut k = 2;
                while !seen.insert(v.name.clone()) { v.name = format!("{base} {k}"); k += 1; }
            }
            out.insert(key, DbViews { active, views });
        }
        (out, dropped)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&FileOut { version: 1, dbs: &self.dbs }).unwrap_or_else(|_| "{}".into())
    }

    pub fn save(&self) -> std::io::Result<()> { atomic_write(&self.path, &self.to_json()) }

    /// Make sure `key` has at least one view. A db seen for the first time
    /// gets "Default", seeded from `legacy` (an older config.json's filters).
    /// True when it created one (save, then drop the legacy keys).
    pub fn ensure_db(&mut self, key: &str, legacy: Option<&LegacyView>) -> bool {
        let db = self.dbs.entry(key.to_string()).or_default();
        if !db.views.is_empty() { return false; }
        let v = match legacy { Some(l) => View::from_legacy("Default", l), None => View::named("Default") };
        db.active = v.name.clone();
        db.views.push(v);
        true
    }

    pub fn db(&self, key: &str) -> Option<&DbViews> { self.dbs.get(key) }

    pub fn views(&self, key: &str) -> &[View] { self.dbs.get(key).map_or(&[], |d| d.views.as_slice()) }

    pub fn active_index(&self, key: &str) -> usize { self.dbs.get(key).map_or(0, |d| d.active_index()) }

    /// The active view (creating "Default" if the db has none).
    pub fn active_mut(&mut self, key: &str) -> &mut View {
        self.ensure_db(key, None);
        let db = self.dbs.get_mut(key).expect("ensured");
        let i  = db.active_index();
        &mut db.views[i]
    }

    pub fn active(&mut self, key: &str) -> View { self.active_mut(key).clone() }

    // ── managing views (the side panel) ──

    /// `base`, or `base 2`, `base 3`, … : a name no view of `key` has
    /// (`except` = the index of a view being renamed, which may keep its own).
    pub fn unique_name(&self, key: &str, base: &str, except: Option<usize>) -> String {
        let base  = base.trim();
        let base  = if base.is_empty() { "View" } else { base };
        let taken = |n: &str| self.views(key).iter().enumerate().any(|(k, v)| Some(k) != except && v.name == n);
        if !taken(base) { return base.to_string(); }
        (2..).map(|k| format!("{base} {k}")).find(|n| !taken(n)).expect("unbounded")
    }

    /// Append `view` (its name made unique) and make it active. Its index.
    pub fn add_view(&mut self, key: &str, mut view: View) -> usize {
        view.name = self.unique_name(key, &view.name, None);
        let db = self.dbs.entry(key.to_string()).or_default();
        db.active = view.name.clone();
        db.views.push(view);
        db.views.len() - 1
    }

    /// A copy of view `idx` right after it, named "<name> copy", made active.
    pub fn duplicate_view(&mut self, key: &str, idx: usize) -> Option<usize> {
        let mut v = self.views(key).get(idx)?.clone();
        v.name = self.unique_name(key, &format!("{} copy", v.name), None);
        let db = self.dbs.get_mut(key)?;
        db.active = v.name.clone();
        db.views.insert(idx + 1, v);
        Some(idx + 1)
    }

    /// Rename view `idx`. An empty or taken name is refused with the reason.
    pub fn rename_view(&mut self, key: &str, idx: usize, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() { return Err("A view needs a name".into()); }
        if self.unique_name(key, name, Some(idx)) != name { return Err(format!("There is already a view called \"{name}\"")); }
        let db = self.dbs.get_mut(key).ok_or("No views for this db")?;
        let v  = db.views.get_mut(idx).ok_or("No such view")?;
        if db.active == v.name { db.active = name.to_string(); }
        v.name = name.to_string();
        Ok(())
    }

    /// Delete view `idx`; the last view of a db is never deleted. When it was
    /// active, its neighbour (the next one, else the previous) becomes active.
    pub fn delete_view(&mut self, key: &str, idx: usize) -> bool {
        let Some(db) = self.dbs.get_mut(key) else { return false };
        if db.views.len() <= 1 || idx >= db.views.len() { return false; }
        let was_active = db.active_index() == idx;
        db.views.remove(idx);
        if was_active { db.active = db.views[idx.min(db.views.len() - 1)].name.clone(); }
        true
    }

    /// Switch to view `idx` (no-op past the end). True when it changed.
    pub fn set_active(&mut self, key: &str, idx: usize) -> bool {
        let Some(db) = self.dbs.get_mut(key) else { return false };
        let Some(v)  = db.views.get(idx) else { return false };
        if db.active == v.name { return false; }
        db.active = v.name.clone();
        true
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::CommunityScope;

    /// A fresh temp dir per test (never the real ~/.config/navi).
    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("navi-views-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn round_trip_through_a_file() {
        let dir = tmp("roundtrip");
        let path = dir.join("views.json");
        let mut s = ViewStore::load(&path);
        assert!(s.dbs.is_empty(), "missing file: empty store");
        assert!(s.ensure_db("/db/a.db", None));
        assert!(!s.ensure_db("/db/a.db", None), "only once");
        let mut rust = Group::new("Rust", "tag:rust");
        rust.colour = Some([200, 10, 10]);
        rust.hull = true;
        let v = s.active_mut("/db/a.db");
        v.groups.push(rust);
        v.auto = AutoGrouper::Community { resolution: 1.5, scope: CommunityScope::Visible };
        v.orphans = OrphansMode::Only;
        v.auto_colours.insert("tag:x".into(), 3);
        s.dbs.get_mut("/db/a.db").unwrap().views.push(View::named("Work"));
        assert!(s.set_active("/db/a.db", 1));
        assert!(!s.set_active("/db/a.db", 1));
        assert!(!s.set_active("/db/a.db", 7));
        s.save().unwrap();
        let back = ViewStore::load(&path);
        assert_eq!(back, s);
        assert_eq!(back.active_index("/db/a.db"), 1);
        assert!(!dir.join("views.json.bad").exists(), "clean file: no backup");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"version\": 1") && text.contains("\"colour\": \"#c80a0a\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One bad view, group or value never costs the rest; the original is
    /// backed up before anything overwrites it.
    #[test]
    fn tolerant_load_keeps_everything_else() {
        let dir  = tmp("tolerant");
        let path = dir.join("views.json");
        let text = r##"{ "version": 1, "dbs": {
            "/a.db": { "active": "Work", "views": [
                { "name": "Default", "orphans": "sometimes", "dailies": "hidden", "group_pull": 0.1 },
                { "name": "Work", "auto": { "kind": "tag" }, "colour_groups": true,
                  "groups": [ { "name": "Ok", "query": "tag:a", "colour": "#00ff00" },
                              { "name": "BadColour", "colour": "green" },
                              { "query": "no name" },
                              { "name": "Also ok", "query": "dir:posts", "pull": true } ] },
                "not a view",
                { "name": "Weird", "auto": { "kind": "nonsense" }, "group_pull": "strong" } ] },
            "/b.db": 42,
            "/c.db": { "views": [ { "name": "Default" }, { "name": "Default" } ] } } }"##;
        std::fs::write(&path, text).unwrap();
        let s = ViewStore::load(&path);
        assert!(dir.join("views.json.bad").exists(), "dropped content is backed up");
        assert_eq!(std::fs::read_to_string(dir.join("views.json.bad")).unwrap(), text);

        let a = s.db("/a.db").unwrap();
        assert_eq!(a.views.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), vec!["Default", "Work", "Weird"]);
        assert_eq!(a.active_index(), 1);
        let d = &a.views[0];
        assert_eq!((d.orphans, d.dailies, d.group_pull), (OrphansMode::Show, DailiesMode::Hide, 0.1));
        let w = &a.views[1];
        assert_eq!(w.auto, AutoGrouper::Tag);
        assert!(w.colour_groups);
        assert_eq!(w.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["Ok", "Also ok"]);
        assert_eq!(w.groups[0].colour, Some([0, 255, 0]));
        assert!(w.groups[1].pull);
        let weird = &a.views[2];
        assert_eq!((weird.auto, weird.group_pull), (AutoGrouper::None, DEFAULT_PULL));
        assert!(s.db("/b.db").is_none());
        let c = s.db("/c.db").unwrap();
        assert_eq!(c.views[1].name, "Default 2", "duplicate names made unique");

        // garbage file: empty store, original kept
        std::fs::write(&path, "{ not json").unwrap();
        let _ = std::fs::remove_file(dir.join("views.json.bad"));
        let s = ViewStore::load(&path);
        assert!(s.dbs.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("views.json.bad")).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Phase-1 config.json filters seed the first view, field by field.
    #[test]
    fn migrates_config_filters_into_the_first_view() {
        let dir  = tmp("migrate");
        let cfgp = dir.join("config.json");
        std::fs::write(&cfgp, r#"{ "db": "/x.db", "dailies": "hide", "orphans": "only", "dailies_dirs": ["log"] }"#).unwrap();
        let mut cfg = Config::load_from(&cfgp);
        let legacy = cfg.legacy_view();
        assert!(legacy.is_some());

        let mut s = ViewStore::load(&dir.join("views.json"));
        assert!(s.ensure_db("/x.db", legacy.as_ref()));
        s.save().unwrap();
        cfg.clear_legacy_view();
        cfg.save_to(&cfgp).unwrap();

        let v = ViewStore::load(&dir.join("views.json")).active("/x.db");
        assert_eq!((v.dailies, v.orphans, v.links_to_dailies_count), (DailiesMode::Hide, OrphansMode::Only, true));
        assert_eq!(v.filter(&cfg.dailies_dirs).dailies_dirs, vec!["log"], "dailies_dirs stays in config.json");
        let after = Config::load_from(&cfgp);
        assert_eq!(after.legacy_view(), None, "moved, not copied");
        assert_eq!(after.db, "/x.db");
        // a db that already has views is never re-seeded
        assert!(!s.ensure_db("/x.db", Some(&LegacyView { orphans: Some(OrphansMode::Hide), ..LegacyView::default() })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manage_views() {
        let mut s = ViewStore::default();
        s.ensure_db("/a.db", None);
        assert_eq!(s.unique_name("/a.db", "Default", None), "Default 2");
        assert_eq!(s.unique_name("/a.db", "Default", Some(0)), "Default", "renaming to its own name");
        assert_eq!(s.unique_name("/a.db", "  ", None), "View");

        let w = s.add_view("/a.db", View::named("Default"));
        assert_eq!((w, s.views("/a.db")[1].name.as_str()), (1, "Default 2"));
        assert_eq!(s.active_index("/a.db"), 1, "a new view becomes active");

        s.active_mut("/a.db").orphans = OrphansMode::Only;
        assert_eq!(s.duplicate_view("/a.db", 1), Some(2));
        assert_eq!(s.views("/a.db")[2].name, "Default 2 copy");
        assert_eq!(s.views("/a.db")[2].orphans, OrphansMode::Only, "a copy keeps everything");
        assert_eq!(s.active_index("/a.db"), 2);
        assert_eq!(s.duplicate_view("/a.db", 9), None);

        assert!(s.rename_view("/a.db", 2, "Default").is_err(), "taken");
        assert!(s.rename_view("/a.db", 2, " ").is_err(), "empty");
        s.rename_view("/a.db", 2, " Writing ").unwrap();
        assert_eq!(s.views("/a.db")[2].name, "Writing");
        assert_eq!(s.db("/a.db").unwrap().active, "Writing", "renaming the active view keeps it active");
        s.rename_view("/a.db", 0, "Default").unwrap();

        // deleting the active view activates its neighbour; the last view stays
        assert!(s.delete_view("/a.db", 2));
        assert_eq!(s.db("/a.db").unwrap().active, "Default 2");
        assert!(s.delete_view("/a.db", 0));
        assert_eq!(s.active_index("/a.db"), 0);
        assert_eq!(s.views("/a.db")[0].name, "Default 2");
        assert!(!s.delete_view("/a.db", 0), "never the last one");
        assert!(!s.delete_view("/none.db", 0));
        // panel_open round-trips and defaults to closed
        let v: View = serde_json::from_str(r#"{"name": "x", "panel_open": true}"#).unwrap();
        assert!(v.panel_open && !View::default().panel_open);
    }

    #[test]
    fn spec_and_filter_from_a_view() {
        let mut v = View::named("V");
        v.groups = vec![Group::new("A", "tag:a"), Group::new("B", "tag:(")];
        v.auto = AutoGrouper::Folder { depth: 2 };
        v.auto_hidden = vec!["folder:x".into()];
        v.auto_hull = true;
        let s = v.spec();
        assert_eq!(s.compiled.len(), 2);
        assert!(s.compiled[0].is_ok() && s.compiled[1].is_err());
        assert!(s.auto_hidden.contains("folder:x") && s.auto_hull && !s.auto_pull);
        let f = v.filter(&["daily".to_string()]);
        assert_eq!(f, ViewFilter { dailies_dirs: vec!["daily".into()], ..ViewFilter::default() });
    }

    #[test]
    fn db_key_canonicalises() {
        let dir = tmp("dbkey");
        let db = dir.join("org-roam.db");
        std::fs::write(&db, "").unwrap();
        let twisted = format!("{}/../{}/org-roam.db", dir.display(), dir.file_name().unwrap().to_string_lossy());
        assert_eq!(db_key(&twisted), db_key(&db.to_string_lossy()));
        assert_eq!(db_key("/no/such/file.db"), "/no/such/file.db");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
