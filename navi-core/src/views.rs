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
//! value is dropped (to its default) on its own and the rest is kept; bytes
//! that aren't UTF-8 are read as U+FFFD. Keys navi doesn't know (a newer
//! navi's, or a hand edit's) are kept and written back as they were. When
//! anything was dropped or replaced, the file isn't JSON at all, or a newer
//! navi wrote it (`version` above this one's), the original is copied to
//! `views.json.bad` before navi ever writes over it. A views.json that
//! exists but can't be read at all (permissions, an I/O error) is never
//! written over in that session. Saves are atomic (temp file + rename) and
//! re-read the file first (`sync`): every db but this session's own keeps
//! what is on disk, so two navi windows on different dbs don't undo each
//! other; in this session's own db, whatever changed in the file since navi
//! last read or wrote it (a hand edit while navi runs) is taken, setting by
//! setting, unless this session changed that same setting too. Unknown
//! top-level keys are always the file's.
//!
//! Auto-group colours (`auto_colours`, a palette slot per key) belong to the
//! db, not a view, so a tag has one colour in every view; per-view maps an
//! earlier build wrote are merged into the db's on load (the active view's
//! first).
//!
//! Migration: the `dailies` / `orphans` / `links_to_dailies_count` an older
//! config.json carried move to views.json's top level (`legacy_filters`):
//! every db seen for the first time, then or later, seeds its "Default"
//! view from them. main.rs drops those keys from config.json only once
//! views.json holding them has been written.
//!
//! `notes_root` (per db) is the notes root last seen, so a folder that
//! empties never moves the root deeper (see `view::pinned_root_depth`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::config::{atomic_write, parse_tolerant, Config, LegacyView};
use crate::groups::{AutoGrouper, Group, GroupSpec};
use crate::view::{DailiesMode, OrphansMode, ViewFilter};

pub const DEFAULT_PULL: f64 = 0.1;
pub const VERSION: u64 = 1; // views.json format this navi writes

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
    #[serde(default, alias = "auto_colors", skip_serializing)]
    pub auto_colours:           BTreeMap<String, u8>,       // legacy: read, then moved into the db's (DbViews)
    #[serde(default)]
    pub panel_open:             bool,                       // the side panel (backslash key) is open in this view
    #[serde(flatten)]
    pub extra:                  BTreeMap<String, serde_json::Value>, // keys this navi doesn't know: kept as they are
}

impl Default for View {
    fn default() -> Self {
        View {
            name: default_name(), groups: Vec::new(), auto: AutoGrouper::None, auto_hidden: Vec::new(),
            auto_pull: false, auto_hull: false, colour_groups: false,
            dailies: DailiesMode::Show, orphans: OrphansMode::Show, links_to_dailies_count: true,
            group_pull: DEFAULT_PULL, auto_colours: BTreeMap::new(), panel_open: false, extra: BTreeMap::new(),
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

    /// `G` and the panel's "Show groups": the group display on or off. On,
    /// in a view with nothing to group, it starts the tag grouper (the old
    /// tag colouring).
    pub fn set_colour_groups(&mut self, on: bool) {
        self.colour_groups = on;
        if on && self.groups.is_empty() && self.auto == AutoGrouper::None { self.auto = AutoGrouper::Tag; }
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
    pub active:       String,                              // name of the active view
    #[serde(default)]
    pub views:        Vec<View>,
    #[serde(default, alias = "auto_colors", skip_serializing_if = "BTreeMap::is_empty")]
    pub auto_colours: BTreeMap<String, u8>,                // palette slot per auto-group key, every view's
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_root:   Option<String>,                      // the notes root last seen ("/Users/me/notes")
    #[serde(flatten)]
    pub extra:        BTreeMap<String, serde_json::Value>, // keys this navi doesn't know: kept as they are
}

impl DbViews {
    pub fn active_index(&self) -> usize { self.views.iter().position(|v| v.name == self.active).unwrap_or(0) }
}

#[derive(Serialize)]
struct FileOut<'a> {
    version:        u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    legacy_filters: Option<&'a LegacyView>,
    dbs:            &'a BTreeMap<String, DbViews>,
    #[serde(flatten)]
    extra:   &'a serde_json::Map<String, serde_json::Value>,
}

/// What views.json held.
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub dbs:     BTreeMap<String, DbViews>,
    pub extra:   serde_json::Map<String, serde_json::Value>, // top-level keys this navi doesn't know
    pub legacy:  Option<LegacyView>,                         // `legacy_filters`: the first-view seed
    pub dropped: bool,                                       // something was dropped: back the original up
}

#[derive(Debug, Clone, Default)]
pub struct ViewStore {
    pub path:   PathBuf,
    pub dbs:    BTreeMap<String, DbViews>,
    extra:      serde_json::Map<String, serde_json::Value>, // unknown top-level keys, written back
    legacy:     Option<LegacyView>,               // an older config.json's filters: every new db's first view
    owned:      BTreeSet<String>,                 // dbs this session edits; sync() takes every other one from disk
    base:       BTreeMap<String, DbViews>,        // the file's dbs as last read or written (what a hand edit changed)
    seen:       Option<SystemTime>,               // the file's mtime then
    read_error: Option<String>,                   // views.json exists but couldn't be read: never written over
}

fn mtime(path: &Path) -> Option<SystemTime> { std::fs::metadata(path).and_then(|m| m.modified()).ok() }

/// The key a db's views are stored under: its canonical path (symlinks and
/// `..` resolved). A db that doesn't exist yet resolves its deepest existing
/// folder and appends the rest, so it keys the same once it exists. Windows'
/// `\\?\` prefix is dropped. The path as given when nothing resolves.
pub fn db_key(path: &str) -> String {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = Path::new(path);
    loop {
        if let Ok(mut c) = std::fs::canonicalize(cur) {
            for r in rest.iter().rev() { c.push(r); }
            return strip_verbatim(&c.to_string_lossy());
        }
        match (cur.parent(), cur.file_name()) {
            (Some(up), Some(name)) => { rest.push(name.to_os_string()); cur = up; }
            _ => return path.to_string(),
        }
    }
}

/// `\\?\C:\x` → `C:\x`, `\\?\UNC\srv\x` → `\\srv\x` (what canonicalize
/// returns on Windows, versus how every other path there is spelled).
fn strip_verbatim(p: &str) -> String {
    if let Some(unc) = p.strip_prefix(r"\\?\UNC\") { return format!(r"\\{unc}"); }
    p.strip_prefix(r"\\?\").unwrap_or(p).to_string()
}

impl ViewStore {
    pub fn default_path() -> PathBuf { Config::config_dir().join("views.json") }

    /// Load `path`; a missing file is an empty store. See the module docs for
    /// what happens to bad content and to a file that can't be read.
    pub fn load(path: &Path) -> Self {
        let mut store = ViewStore { path: path.to_path_buf(), ..ViewStore::default() };
        match Self::read(path) {
            Ok(p) => if let Some(p) = p { store.dbs = p.dbs; store.extra = p.extra; store.legacy = p.legacy; },
            Err(e)  => {
                eprintln!("navi: couldn't read {}: {e}; views won't be saved this session", path.display());
                store.read_error = Some(e.to_string());
            }
        }
        store.base = store.dbs.clone();
        store.seen = mtime(path);
        store
    }

    /// Read and tolerantly parse `path`: None when it doesn't exist; an error
    /// only when it exists and can't be read. Dropped or non-UTF-8 content
    /// backs the original bytes up to `views.json.bad` first.
    fn read(path: &Path) -> std::io::Result<Option<Parsed>> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let text = String::from_utf8_lossy(&bytes);
        let lossy = matches!(text, std::borrow::Cow::Owned(_));
        if lossy { eprintln!("navi: views.json: not valid UTF-8, read with the bad bytes replaced"); }
        let parsed = Self::parse_file(&text);
        if parsed.dropped || lossy {
            let bad = path.with_extension("json.bad");
            if std::fs::write(&bad, &bytes).is_ok() {
                eprintln!("navi: views.json: the original is kept as {}", bad.display());
            }
        }
        Ok(Some(parsed))
    }

    /// Parse views.json text. Returns the dbs and whether anything was
    /// dropped (including the whole file, when it isn't a JSON object).
    pub fn parse(text: &str) -> (BTreeMap<String, DbViews>, bool) {
        let p = Self::parse_file(text);
        (p.dbs, p.dropped)
    }

    /// `parse`, plus the top-level keys navi doesn't know. A file a newer
    /// navi wrote counts as dropped (backed up): its keys are kept, but what
    /// they mean may have changed.
    pub fn parse_file(text: &str) -> Parsed {
        let mut out = BTreeMap::new();
        let mut root = match serde_json::from_str::<serde_json::Value>(text) {
            Ok(serde_json::Value::Object(o)) => o,
            _ => {
                eprintln!("navi: views.json: not a JSON object, starting fresh");
                return Parsed { dropped: true, ..Parsed::default() };
            }
        };
        let mut dropped = false;
        match root.remove("version") {
            Some(v) if v.as_u64().is_some_and(|v| v > VERSION) => {
                eprintln!("navi: views.json: written by a newer navi (version {v}); everything is kept");
                dropped = true;
            }
            _ => {}
        }
        let legacy = match root.remove("legacy_filters") {
            None => None,
            Some(v) => match serde_json::from_value::<LegacyView>(v) {
                Ok(l)  => Some(l).filter(|l| *l != LegacyView::default()),
                Err(_) => { eprintln!("navi: views.json: bad \"legacy_filters\", ignored"); dropped = true; None }
            },
        };
        let dbs = match root.remove("dbs") {
            Some(serde_json::Value::Object(d)) => d,
            Some(_) => {
                eprintln!("navi: views.json: \"dbs\" is not an object");
                return Parsed { dropped: true, extra: root, legacy, ..Parsed::default() };
            }
            None => return Parsed { dropped, extra: root, legacy, ..Parsed::default() },
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
            let colours = entry.remove("auto_colours").or_else(|| entry.remove("auto_colors"));
            let mut auto_colours: BTreeMap<String, u8> = match colours.map(serde_json::from_value) {
                None         => BTreeMap::new(),
                Some(Ok(m))  => m,
                Some(Err(_)) => { eprintln!("navi: views.json: {key}: bad \"auto_colours\", ignored"); dropped = true; BTreeMap::new() }
            };
            // an earlier build kept them per view: the db's now (the active
            // view's win, then the others in order)
            let act = views.iter().position(|v| v.name == active).unwrap_or(0);
            for i in std::iter::once(act).chain(0..views.len()) {
                if let Some(v) = views.get_mut(i) {
                    for (k, s) in std::mem::take(&mut v.auto_colours) { auto_colours.entry(k).or_insert(s); }
                }
            }
            let notes_root = match entry.remove("notes_root") {
                None => None,
                Some(serde_json::Value::String(s)) => Some(s),
                Some(_) => { eprintln!("navi: views.json: {key}: bad \"notes_root\", ignored"); dropped = true; None }
            };
            out.insert(key, DbViews { active, views, auto_colours, notes_root, extra: entry.into_iter().collect() });
        }
        Parsed { dbs: out, extra: root, legacy, dropped }
    }

    pub fn to_json(&self) -> String {
        let out = FileOut { version: VERSION, legacy_filters: self.legacy.as_ref(), dbs: &self.dbs, extra: &self.extra };
        serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".into())
    }

    /// The file changed since this session last read or wrote it.
    pub fn changed_on_disk(&self) -> bool { self.read_error.is_none() && mtime(&self.path) != self.seen }

    /// Take what changed in views.json since this session last read or wrote
    /// it (a hand edit, another navi). Every db this session doesn't edit is
    /// the file's; in one it does, a setting changed in the file and not
    /// here is taken from the file, one changed in both keeps this session's
    /// (`merge_db`). A file that lost content to an error (a typo) changes
    /// nothing here: it is backed up, and the next save writes this
    /// session's. Unknown top-level keys are the file's. True when a db this
    /// session edits changed: reload the working view.
    pub fn sync(&mut self) -> std::io::Result<bool> {
        if self.read_error.is_some() { return Ok(false); }
        let seen = mtime(&self.path);
        let Some(disk) = Self::read(&self.path)? else { self.seen = seen; return Ok(false) };
        self.seen = seen;
        let mut changed = false;
        for (k, d) in disk.dbs {
            if !self.owned.contains(&k) { self.base.insert(k.clone(), d.clone()); self.dbs.insert(k, d); continue; }
            if disk.dropped || self.base.get(&k) == Some(&d) { continue; }
            let ours = self.dbs.entry(k.clone()).or_default();
            changed |= merge_db(self.base.get(&k), ours, &d);
            self.base.insert(k, d);
        }
        self.extra = disk.extra;
        if self.legacy.is_none() { self.legacy = disk.legacy; }
        Ok(changed)
    }

    /// Write views.json, after taking what changed in it (`sync`); refused
    /// when the file couldn't be read at load. True when the sync changed a
    /// db this session edits (reload the working view).
    pub fn save(&mut self) -> std::io::Result<bool> {
        if let Some(e) = &self.read_error {
            return Err(std::io::Error::other(format!("not written: it couldn't be read when navi started ({e})")));
        }
        let changed = self.sync()?;
        atomic_write(&self.path, &self.to_json())?;
        self.base = self.dbs.clone();
        self.seen = mtime(&self.path);
        Ok(changed)
    }

    /// False when views.json couldn't be read at load: every save fails, so
    /// retrying one is pointless.
    pub fn writable(&self) -> bool { self.read_error.is_none() }

    /// The db's auto-group colours (shared by its views).
    pub fn auto_colours(&self, key: &str) -> BTreeMap<String, u8> {
        self.dbs.get(key).map(|d| d.auto_colours.clone()).unwrap_or_default()
    }

    /// Replace the db's auto-group colours. True when they changed (save).
    pub fn set_auto_colours(&mut self, key: &str, slots: &BTreeMap<String, u8>) -> bool {
        self.owned.insert(key.to_string());
        let db = self.dbs.entry(key.to_string()).or_default();
        if db.auto_colours == *slots { return false; }
        db.auto_colours = slots.clone();
        true
    }

    /// Make sure `key` has at least one view. `legacy` (an older
    /// config.json's filters) becomes the store's `legacy_filters`; a db seen
    /// for the first time, now or in any later session, gets "Default"
    /// seeded from them. True when it created one (save, then drop the
    /// legacy keys from config.json).
    pub fn ensure_db(&mut self, key: &str, legacy: Option<&LegacyView>) -> bool {
        if let Some(l) = legacy { self.legacy = Some(*l); }
        let seed = self.legacy;
        self.owned.insert(key.to_string());
        let db = self.dbs.entry(key.to_string()).or_default();
        if !db.views.is_empty() { return false; }
        let v = match seed { Some(l) => View::from_legacy("Default", &l), None => View::named("Default") };
        db.active = v.name.clone();
        db.views.push(v);
        true
    }

    /// The legacy filters every new db's first view starts from.
    pub fn legacy(&self) -> Option<LegacyView> { self.legacy }

    /// The notes root last seen for the db (`Graph::root_pin`).
    pub fn notes_root(&self, key: &str) -> Option<String> { self.dbs.get(key)?.notes_root.clone() }

    /// Remember the db's notes root. True when it changed (save).
    pub fn set_notes_root(&mut self, key: &str, root: &str) -> bool {
        self.owned.insert(key.to_string());
        let db = self.dbs.entry(key.to_string()).or_default();
        if db.notes_root.as_deref() == Some(root) { return false; }
        db.notes_root = Some(root.to_string());
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
        self.owned.insert(key.to_string());
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

// ── Merging an outside edit ──────────────────────────────────────────────────

/// Three-way merge into `ours` of what changed from `base` to `disk`: a
/// value changed only in the file is taken from it; one changed here (both
/// changed, too) stays ours. Objects merge key by key (a key added in the
/// file is added, one it removed is removed when ours is still the base's);
/// anything else (a group list, a number) is one value. True when `ours`
/// changed.
fn merge_json(base: Option<&Value>, ours: &mut Value, disk: &Value) -> bool {
    if base == Some(disk) || &*ours == disk { return false; }
    if let (Value::Object(o), Value::Object(d)) = (&mut *ours, disk) {
        let b = base.and_then(Value::as_object);
        let mut changed = false;
        for (k, dv) in d {
            let bv = b.and_then(|b| b.get(k));
            match o.get_mut(k) {
                Some(ov) => changed |= merge_json(bv, ov, dv),
                None if bv.is_none() => { o.insert(k.clone(), dv.clone()); changed = true; } // new in the file
                None => {} // removed here: stays removed
            }
        }
        for (k, bv) in b.into_iter().flatten() {
            if !d.contains_key(k) && o.get(k) == Some(bv) { o.remove(k); changed = true; }
        }
        return changed;
    }
    if Some(&*ours) == base { *ours = disk.clone(); return true; }
    false
}

/// `merge_json` for one db's entry, with its views matched by name: a view
/// added in the file is added, one it deleted (or renamed) is removed when
/// this session didn't touch it (never the last), and a view in both merges
/// setting by setting. True when `ours` changed.
fn merge_db(base: Option<&DbViews>, ours: &mut DbViews, disk: &DbViews) -> bool {
    let json  = |d: &DbViews| serde_json::to_value(d).unwrap_or(Value::Null);
    let views = |v: &mut Value| match v.as_object_mut().and_then(|m| m.remove("views")) { Some(Value::Array(a)) => a, _ => Vec::new() };
    let name  = |v: &Value| v.get("name").and_then(Value::as_str).map(str::to_string);
    let find  = |list: &[Value], n: &Option<String>| list.iter().position(|v| name(v) == *n);
    let (mut o, mut d) = (json(ours), json(disk));
    let mut b  = base.map(json);
    let mut ov = views(&mut o);
    let dv     = views(&mut d);
    let bv     = b.as_mut().map(views).unwrap_or_default();
    let mut changed = merge_json(b.as_ref(), &mut o, &d);
    for dview in &dv {
        let n = name(dview);
        let bview = find(&bv, &n).map(|i| &bv[i]);
        match find(&ov, &n) {
            Some(i) => changed |= merge_json(bview, &mut ov[i], dview),
            None if bview.is_none() => { ov.push(dview.clone()); changed = true; }
            None => {} // deleted or renamed here: that wins
        }
    }
    let gone: Vec<usize> = (0..ov.len()).filter(|&i| {
        let n = name(&ov[i]);
        find(&dv, &n).is_none() && find(&bv, &n).is_some_and(|j| bv[j] == ov[i])
    }).collect();
    if gone.len() < ov.len() { for &i in gone.iter().rev() { ov.remove(i); changed = true; } }
    if !changed { return false; }
    if let Some(m) = o.as_object_mut() { m.insert("views".into(), Value::Array(ov)); }
    match serde_json::from_value::<DbViews>(o) {
        Ok(db) => { *ours = db; true }
        Err(e) => { eprintln!("navi: views.json: couldn't merge an outside edit ({e}); keeping this session's"); false }
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
        assert!(s.set_auto_colours("/db/a.db", &BTreeMap::from([("tag:x".to_string(), 3)])));
        s.dbs.get_mut("/db/a.db").unwrap().views.push(View::named("Work"));
        assert!(s.set_active("/db/a.db", 1));
        assert!(!s.set_active("/db/a.db", 1));
        assert!(!s.set_active("/db/a.db", 7));
        s.save().unwrap();
        let back = ViewStore::load(&path);
        assert_eq!(back.dbs, s.dbs);
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

    /// Keys this navi doesn't know (a newer navi's, a hand edit's) survive a
    /// save; a file from a newer navi is backed up before it's written.
    #[test]
    fn unknown_keys_survive_a_save() {
        let dir  = tmp("unknown");
        let path = dir.join("views.json");
        let text = r#"{ "version": 2, "future": [1, 2],
            "dbs": { "/a.db": { "active": "V", "pinned": true, "views": [
                { "name": "V", "physics": { "k": 3 },
                  "groups": [ { "name": "G", "query": "tag:x", "note": "keep me" } ] } ] } } }"#;
        std::fs::write(&path, text).unwrap();
        let mut s = ViewStore::load(&path);
        assert_eq!(std::fs::read_to_string(dir.join("views.json.bad")).unwrap(), text, "newer version: backed up");
        s.active_mut("/a.db").panel_open = true;
        s.save().unwrap();
        let back: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back["future"], serde_json::json!([1, 2]));
        assert_eq!(back["dbs"]["/a.db"]["pinned"], serde_json::json!(true));
        let v = &back["dbs"]["/a.db"]["views"][0];
        assert_eq!(v["physics"]["k"], serde_json::json!(3));
        assert_eq!(v["groups"][0]["note"], serde_json::json!("keep me"));
        assert_eq!(v["panel_open"], serde_json::json!(true), "and the edit");
        // this navi's own version: no backup for unknown keys alone (they're kept)
        let _ = std::fs::remove_file(dir.join("views.json.bad"));
        std::fs::write(&path, text.replace("\"version\": 2", "\"version\": 1")).unwrap();
        ViewStore::load(&path);
        assert!(!dir.join("views.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Per-view colour maps (an earlier build) become the db's, the active
    /// view's first; they are written once, on the db.
    #[test]
    fn per_view_colours_move_to_the_db() {
        let text = r#"{ "version": 1, "dbs": { "/a.db": { "active": "B", "views": [
            { "name": "A", "auto_colours": { "tag:x": 1, "tag:y": 2 } },
            { "name": "B", "auto_colours": { "tag:x": 5 } } ] } } }"#;
        let (dbs, dropped) = ViewStore::parse(text);
        assert!(!dropped);
        let db = &dbs["/a.db"];
        assert_eq!(db.auto_colours, BTreeMap::from([("tag:x".to_string(), 5), ("tag:y".to_string(), 2)]));
        assert!(db.views.iter().all(|v| v.auto_colours.is_empty()));
        let out = serde_json::to_string(db).unwrap();
        assert_eq!(out.matches("auto_colours").count(), 1, "{out}");
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
    fn showing_groups_in_an_empty_view_starts_the_tag_grouper() {
        let mut v = View::named("V");
        v.set_colour_groups(true);
        assert!(v.colour_groups);
        assert_eq!(v.auto, AutoGrouper::Tag);
        v.set_colour_groups(false);
        assert_eq!((v.colour_groups, v.auto), (false, AutoGrouper::Tag), "off keeps the grouper");
        let mut w = View::named("W");
        w.groups.push(Group::new("Rust", "tag:rust"));
        w.set_colour_groups(true);
        assert_eq!(w.auto, AutoGrouper::None, "manual groups: nothing added");
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

    /// A db that doesn't exist yet (unsynced, unmounted) keys the same as it
    /// will once it does, even through a symlinked folder (macOS /var).
    #[test]
    fn db_key_is_the_same_before_and_after_the_db_exists() {
        let dir = tmp("dbkey-missing");
        let db  = dir.join("sub").join("org-roam.db");
        let before = db_key(&db.to_string_lossy());
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        std::fs::write(&db, "").unwrap();
        assert_eq!(before, db_key(&db.to_string_lossy()));
        assert_eq!(strip_verbatim(r"\\?\C:\Users\x\db"), r"C:\Users\x\db");
        assert_eq!(strip_verbatim(r"\\?\UNC\srv\share\db"), r"\\srv\share\db");
        assert_eq!(strip_verbatim("/Users/x/db"), "/Users/x/db");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bytes that aren't UTF-8 cost nothing but those bytes, and the original
    /// is backed up before the file is written again.
    #[test]
    fn non_utf8_views_json_is_kept_and_backed_up() {
        let dir  = tmp("latin1");
        let path = dir.join("views.json");
        let mut raw = br#"{ "version": 1, "dbs": { "/other.db": { "active": "Caf"#.to_vec();
        raw.push(0xE9);
        raw.extend_from_slice(br#"", "views": [ { "name": "Other", "groups": [ { "name": "G", "query": "tag:x" } ] } ] } } }"#);
        std::fs::write(&path, &raw).unwrap();
        let mut s = ViewStore::load(&path);
        assert_eq!(std::fs::read(dir.join("views.json.bad")).unwrap(), raw, "the original bytes are backed up");
        assert_eq!(s.views("/other.db").len(), 1, "everything else is kept");
        assert!(s.ensure_db("/new.db", None));
        s.save().unwrap();
        let back = ViewStore::load(&path);
        assert_eq!(back.views("/other.db")[0].groups[0].query, "tag:x");
        assert_eq!(back.views("/new.db").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A views.json that exists but can't be read is never written over.
    #[cfg(unix)]
    #[test]
    fn unreadable_views_json_is_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let dir  = tmp("unreadable");
        let path = dir.join("views.json");
        let text = r#"{ "version": 1, "dbs": { "/other.db": { "views": [ { "name": "Mine" } ] } } }"#;
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() { let _ = std::fs::remove_dir_all(&dir); return; } // root reads anything
        let mut s = ViewStore::load(&path);
        assert!(s.ensure_db("/new.db", None));
        assert!(s.save().is_err(), "refused");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "untouched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two navi windows (different dbs, each with its own snapshot): the
    /// later save keeps the earlier one's db.
    #[test]
    fn concurrent_sessions_keep_each_others_dbs() {
        let dir  = tmp("concurrent");
        let path = dir.join("views.json");
        let mut a = ViewStore::load(&path);
        let mut b = ViewStore::load(&path);
        a.ensure_db("/db1", None);
        a.save().unwrap();
        b.ensure_db("/db2", None);
        b.save().unwrap();
        a.add_view("/db1", View::named("A-new"));
        a.save().unwrap();
        b.active_mut("/db2").group_pull = 0.5;
        b.save().unwrap();
        let back = ViewStore::load(&path);
        assert_eq!(back.views("/db1").iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), vec!["Default", "A-new"]);
        assert_eq!(back.views("/db2")[0].group_pull, 0.5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The legacy filters stay in views.json: a db opened for the first time
    /// in a later session (another "db", or the right path after a first run
    /// on a stale one) starts from them too.
    #[test]
    fn legacy_filters_seed_every_new_db() {
        let dir  = tmp("legacy-seed");
        let cfgp = dir.join("config.json");
        let path = dir.join("views.json");
        std::fs::write(&cfgp, r#"{ "db": "/stale/org-roam.db", "dailies": "hide", "orphans": "hide" }"#).unwrap();
        let mut cfg = Config::load_from(&cfgp);
        let mut s = ViewStore::load(&path);
        assert!(s.ensure_db("/stale/org-roam.db", cfg.legacy_view().as_ref()));
        s.save().unwrap();
        cfg.clear_legacy_view();
        cfg.save_to(&cfgp).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("legacy_filters"));

        let cfg = Config::load_from(&cfgp);
        assert_eq!(cfg.legacy_view(), None, "gone from config.json");
        let mut s = ViewStore::load(&path);
        assert!(s.ensure_db("/real/org-roam.db", cfg.legacy_view().as_ref()));
        let v = s.active("/real/org-roam.db");
        assert_eq!((v.dailies, v.orphans), (DailiesMode::Hide, OrphansMode::Hide), "seeded from views.json");
        s.save().unwrap();
        assert_eq!(ViewStore::load(&path).legacy(), Some(LegacyView { dailies: Some(DailiesMode::Hide), orphans: Some(OrphansMode::Hide), links_to_dailies_count: None }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unknown top-level keys are the file's as it is now: a newer value (or
    /// a removal) made while navi runs is never written back over.
    #[test]
    fn unknown_top_level_keys_follow_the_file() {
        let dir  = tmp("extra");
        let path = dir.join("views.json");
        std::fs::write(&path, r#"{ "version": 1, "dbs": {}, "future": 1, "gone": true }"#).unwrap();
        let mut s = ViewStore::load(&path);
        std::fs::write(&path, r#"{ "version": 1, "dbs": {}, "future": 2 }"#).unwrap();
        s.ensure_db("/a.db", None);
        s.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"future\": 2") && !text.contains("gone"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hand edit of this session's own db while navi runs: a setting
    /// changed only in the file is kept, one changed here too stays this
    /// session's; views added or deleted in the file follow it.
    #[test]
    fn a_hand_edit_while_navi_runs_is_kept() {
        let dir  = tmp("handedit");
        let path = dir.join("views.json");
        let mut s = ViewStore::load(&path);
        s.ensure_db("/a.db", None);
        s.add_view("/a.db", View::named("Old"));
        s.set_active("/a.db", 0);
        assert!(!s.save().unwrap(), "nothing from outside");
        assert!(!s.changed_on_disk());

        // in Emacs: a group, a pull strength, a new view; "Old" deleted
        let mut file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let db = &mut file["dbs"]["/a.db"];
        db["views"][0]["groups"] = serde_json::json!([{ "name": "Hand", "query": "tag:x" }]);
        db["views"][0]["group_pull"] = serde_json::json!(0.4);
        db["views"][0]["orphans"] = serde_json::json!("only");
        db["views"].as_array_mut().unwrap().remove(1);
        db["views"].as_array_mut().unwrap().push(serde_json::json!({ "name": "Typed" }));
        std::thread::sleep(std::time::Duration::from_millis(20)); // a new mtime
        std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
        assert!(s.changed_on_disk());

        // meanwhile in navi: D, and the same orphans setting
        s.active_mut("/a.db").dailies = DailiesMode::Hide;
        s.active_mut("/a.db").orphans = OrphansMode::Hide;
        assert!(s.save().unwrap(), "the outside edit was taken: reload the working view");
        for back in [s.active("/a.db"), ViewStore::load(&path).active("/a.db")] {
            assert_eq!(back.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["Hand"], "the hand edit survives");
            assert_eq!(back.group_pull, 0.4);
            assert_eq!(back.dailies, DailiesMode::Hide, "and navi's own edit");
            assert_eq!(back.orphans, OrphansMode::Hide, "both changed it: navi's");
        }
        assert_eq!(s.views("/a.db").iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), vec!["Default", "Typed"]);
        assert!(!dir.join("views.json.bad").exists());
        // nothing new: polling takes nothing
        assert!(!s.changed_on_disk() && !s.sync().unwrap());

        // a typo drops content: nothing is taken from it, the original is backed up
        let text = std::fs::read_to_string(&path).unwrap().replace("\"Hand\"", "7");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, &text).unwrap();
        assert!(!s.sync().unwrap());
        assert_eq!(s.active("/a.db").groups.len(), 1, "a bad hand edit never deletes a group here");
        assert!(dir.join("views.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_root_is_kept_per_db() {
        let dir  = tmp("root");
        let path = dir.join("views.json");
        let mut s = ViewStore::load(&path);
        assert_eq!(s.notes_root("/a.db"), None);
        assert!(s.set_notes_root("/a.db", "/Users/me/notes"));
        assert!(!s.set_notes_root("/a.db", "/Users/me/notes"), "unchanged");
        s.ensure_db("/a.db", None);
        s.save().unwrap();
        assert_eq!(ViewStore::load(&path).notes_root("/a.db").as_deref(), Some("/Users/me/notes"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
