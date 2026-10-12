use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::view::{default_dailies_dirs, DailiesMode, OrphansMode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub db: String,
    #[serde(default)]
    pub emacsclient: String,
    #[serde(default = "default_server")]
    pub server_name: String,
    #[serde(default = "default_true")]
    pub show_fps: bool,
    #[serde(default)]
    pub borderless: bool,
    #[serde(default = "default_dailies_dirs")]
    pub dailies_dirs: Vec<String>,           // directory names that mark a daily note
    // ── legacy view filters: moved into views.json (views.rs). Read so the
    // first run can seed a db's first view from them, then dropped. ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dailies: Option<DailiesMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphans: Option<OrphansMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links_to_dailies_count: Option<bool>,
    // config.json exists but couldn't be read or isn't a JSON object (a
    // hand edit's typo): this session runs on defaults and never writes it
    #[serde(skip)]
    pub keep_file:              bool,
}

/// The view filters an older config.json carried (views.json keeps them as
/// `legacy_filters`, the seed of every db's first view).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dailies:                Option<DailiesMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphans:                Option<OrphansMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links_to_dailies_count: Option<bool>,
}

fn default_server() -> String { "server".into() }
fn default_true() -> bool { true }

impl Default for Config {
    fn default() -> Self {
        Config {
            db: String::new(),
            emacsclient: String::new(),
            server_name: "server".into(),
            show_fps: true,
            borderless: false,
            dailies_dirs: default_dailies_dirs(),
            dailies: None,
            orphans: None,
            links_to_dailies_count: None,
            keep_file: false,
        }
    }
}

impl Config {
    /// The pre-views filter keys, when the file still has any.
    pub fn legacy_view(&self) -> Option<LegacyView> {
        let l = LegacyView { dailies: self.dailies, orphans: self.orphans, links_to_dailies_count: self.links_to_dailies_count };
        (l != LegacyView::default()).then_some(l)
    }

    /// Forget the legacy keys (once a view holds them).
    pub fn clear_legacy_view(&mut self) {
        self.dailies = None;
        self.orphans = None;
        self.links_to_dailies_count = None;
    }

    pub fn config_dir() -> PathBuf {
        dirs::home_dir().unwrap_or_default().join(".config").join("navi")
    }

    pub fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".config")
            .join("navi")
            .join("config.json")
    }

    pub fn load() -> Self { Self::load_from(&Self::config_path()) }

    /// Load config.json; a missing file is the defaults. Startup saves what
    /// it loaded, so nothing is lost without a copy: a file that can't be
    /// read or isn't a JSON object (a typo) is never written over this
    /// session (`keep_file`); one whose bad values were dropped, or that
    /// isn't UTF-8, is copied to config.json.bad first.
    pub fn load_from(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
            Err(e) => {
                eprintln!("navi: couldn't read {}: {e}; using defaults, not writing it", path.display());
                return Config { keep_file: true, ..Config::default() };
            }
        };
        let text  = String::from_utf8_lossy(&bytes);
        let lossy = matches!(text, std::borrow::Cow::Owned(_));
        let Some((cfg, dropped)) = Self::parse_report(&text) else {
            eprintln!("navi: {} is not a JSON object; using defaults, not writing it", path.display());
            return Config { keep_file: true, ..Config::default() };
        };
        if dropped || lossy {
            let bad = path.with_extension("json.bad");
            if std::fs::write(&bad, &bytes).is_ok() { eprintln!("navi: config.json: the original is kept as {}", bad.display()); }
        }
        cfg
    }

    /// Parse config.json. One bad value must not cost every other setting
    /// (startup saves what it loaded): a key whose value doesn't parse falls
    /// back to its default alone, the rest are kept.
    pub fn parse(s: &str) -> Self { Self::parse_report(s).map_or_else(Config::default, |(c, _)| c) }

    /// `parse`, plus whether a value was dropped; None when `s` isn't a JSON
    /// object at all.
    fn parse_report(s: &str) -> Option<(Self, bool)> {
        match serde_json::from_str::<serde_json::Value>(s) {
            Ok(serde_json::Value::Object(user)) => Some(parse_tolerant(user, "config.json")),
            _ => None,
        }
    }

    pub fn save(&self) {
        if let Err(e) = self.save_to(&Self::config_path()) { eprintln!("navi: config.json not saved: {e}"); }
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if self.keep_file {
            return Err(std::io::Error::other("not written: it couldn't be read or parsed when navi started"));
        }
        let s = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        atomic_write(path, &s)
    }
}

/// Deserialize `user` as a `T`, keeping every key whose value parses and
/// dropping (to its default) each one that doesn't, with a note on stderr.
/// Returns the value and whether anything was dropped.
pub fn parse_tolerant<T: DeserializeOwned + Default>(user: serde_json::Map<String, serde_json::Value>, what: &str) -> (T, bool) {
    if let Ok(t) = serde_json::from_value(serde_json::Value::Object(user.clone())) { return (t, false); }
    let mut kept    = serde_json::Map::new();
    let mut dropped = false;
    for (k, v) in user {
        let mut trial = kept.clone();
        trial.insert(k.clone(), v.clone());
        if serde_json::from_value::<T>(serde_json::Value::Object(trial)).is_ok() {
            kept.insert(k, v);
        } else {
            eprintln!("navi: {what}: ignoring bad value for \"{k}\" (using the default)");
            dropped = true;
        }
    }
    (serde_json::from_value(serde_json::Value::Object(kept)).unwrap_or_default(), dropped)
}

/// `path` with symlinks followed to the file they name (a dangling link
/// too): a save writes THROUGH a link (config.json kept in a dotfiles repo)
/// instead of replacing the link with a plain file.
fn link_target(path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    for _ in 0..40 { // a loop of links: give up, write at the last one
        let is_link = std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink());
        let Some(t) = is_link.then(|| std::fs::read_link(&p).ok()).flatten() else { break };
        p = if t.is_absolute() { t } else { p.parent().unwrap_or(Path::new("")).join(t) };
    }
    p
}

/// Write `data` to `path` all at once: a temp file in the same directory,
/// flushed, then renamed over the target. A crash mid-save leaves the old
/// file, never half of the new one. A symlinked `path` writes its target.
pub fn atomic_write(path: &Path, data: &str) -> std::io::Result<()> {
    let path = &link_target(path);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "navi".into());
    let tmp  = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    };
    let r = write();
    if r.is_err() { let _ = std::fs::remove_file(&tmp); }
    r
}

// ── DB path detection ─────────────────────────────────────────────────────────

static DB_CANDIDATES: &[&str] = &[
    "~/.emacs.d/org-roam.db",
    "~/.config/emacs/org-roam.db",
    "~/.config/doom/.local/etc/org-roam.db",
    "~/.config/doom/org-roam.db",
    "~/.doom.d/.local/etc/org-roam.db",
    "~/.doom.d/org-roam.db",
    "~/.spacemacs.d/org-roam.db",
];

pub fn detect_db() -> String {
    if let Ok(v) = std::env::var("ORG_ROAM_DB") {
        if std::path::Path::new(&v).exists() {
            return v;
        }
    }
    // XDG_DATA_HOME
    let xdg = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", dirs::home_dir().unwrap_or_default().display()));
    let xdg_db = format!("{}/emacs/org-roam.db", xdg);
    if std::path::Path::new(&xdg_db).exists() {
        return xdg_db;
    }
    for cand in DB_CANDIDATES {
        let expanded = expand_tilde(cand);
        if std::path::Path::new(&expanded).exists() {
            return expanded;
        }
    }
    // Windows: Emacs resolves `~` as HOME, then APPDATA (the default when
    // nothing is configured — stock installs put .emacs.d under Roaming, which
    // dirs::home_dir() (= USERPROFILE) never sees).
    #[cfg(windows)]
    for key in &["HOME", "APPDATA", "USERPROFILE"] {
        if let Ok(root) = std::env::var(key) {
            if root.is_empty() { continue; }
            for sub in &[".emacs.d", ".config/emacs"] {
                let p = std::path::Path::new(&root).join(sub).join("org-roam.db");
                if p.exists() {
                    return p.to_string_lossy().into_owned();
                }
            }
        }
    }
    expand_tilde(DB_CANDIDATES[0])
}

pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = dirs::home_dir().unwrap_or_default();
        return format!("{}/{}", home.display(), rest);
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_json_still_loads_with_view_defaults() {
        let old = r#"{ "db": "~/.emacs.d/org-roam.db", "emacsclient": "/x/emacsclient",
                       "server_name": "server", "show_fps": true, "borderless": false }"#;
        let c: Config = serde_json::from_str(old).unwrap();
        assert_eq!(c.db, "~/.emacs.d/org-roam.db");
        assert_eq!(c.legacy_view(), None);
        assert_eq!(c.dailies_dirs, default_dailies_dirs());
    }

    /// A hand edit with a bad value (e.g. the UI's own word for a mode, or a
    /// typo) resets that key alone, not the whole file.
    #[test]
    fn a_bad_value_resets_only_its_own_key() {
        let s = r#"{ "db": "/x.db", "emacsclient": "/e", "server_name": "work", "borderless": true,
                     "show_fps": false, "dailies_dirs": ["log"], "orphans": "sometimes", "dailies": "Hidden",
                     "links_to_dailies_count": "no" }"#;
        let c = Config::parse(s);
        assert_eq!((c.db.as_str(), c.emacsclient.as_str(), c.server_name.as_str()), ("/x.db", "/e", "work"));
        assert!(c.borderless && !c.show_fps);
        assert_eq!(c.dailies_dirs, vec!["log"]);
        assert_eq!(c.dailies, Some(DailiesMode::Hide), "UI word accepted");
        assert_eq!(c.orphans, None, "bad value → default");
        assert_eq!(c.links_to_dailies_count, None, "bad type → default");
        assert_eq!(Config::parse(r#"{"orphans": "hidden"}"#).orphans, Some(OrphansMode::Hide));
        assert_eq!(Config::parse("not json").server_name, "server");
    }

    /// Phase-1 view keys are read (to seed views.json) and dropped once
    /// cleared; a config without them never writes them.
    #[test]
    fn legacy_view_fields_round_trip_then_clear() {
        let mut c = Config::parse(r#"{"dailies": "hide", "orphans": "only", "links_to_dailies_count": false, "dailies_dirs": ["log"]}"#);
        assert_eq!(c.legacy_view(), Some(LegacyView {
            dailies: Some(DailiesMode::Hide), orphans: Some(OrphansMode::Only), links_to_dailies_count: Some(false) }));
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"orphans\":\"only\"") && s.contains("\"dailies\":\"hide\""), "kept until consumed");
        c.clear_legacy_view();
        let s = serde_json::to_string(&c).unwrap();
        assert!(!s.contains("orphans") && !s.contains("\"dailies\"") && !s.contains("links_to"), "{s}");
        assert!(s.contains("dailies_dirs"));
        assert!(!serde_json::to_string(&Config::default()).unwrap().contains("orphans"));
    }

    #[test]
    fn atomic_save_and_load_in_a_temp_dir() {
        let dir = std::env::temp_dir().join(format!("navi-cfg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub").join("config.json");
        let c = Config { db: "/x.db".into(), borderless: true, ..Config::default() };
        c.save_to(&path).unwrap();
        let back = Config::load_from(&path);
        assert_eq!((back.db.as_str(), back.borderless), ("/x.db", true));
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-")).collect();
        assert!(leftovers.is_empty(), "temp file renamed away");
        assert_eq!(Config::load_from(&dir.join("missing.json")).server_name, "server");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A typo (trailing comma) runs on defaults and never overwrites the
    /// file; a dropped bad value is backed up before the save.
    #[test]
    fn a_broken_config_json_is_never_lost() {
        let dir = std::env::temp_dir().join(format!("navi-cfg-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let typo = "{ \"db\": \"/my/roam.db\", \"borderless\": true, \"dailies\": \"hide\", }";
        std::fs::write(&path, typo).unwrap();
        let c = Config::load_from(&path);
        assert!(c.keep_file && c.db.is_empty());
        assert!(c.save_to(&path).is_err(), "refused");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), typo, "untouched");
        // a bad value: the rest loads, the original is kept as .bad, saving works
        let bad_value = r#"{ "db": "/my/roam.db", "orphans": "sometimes" }"#;
        std::fs::write(&path, bad_value).unwrap();
        let c = Config::load_from(&path);
        assert!(!c.keep_file && c.db == "/my/roam.db");
        assert_eq!(std::fs::read_to_string(dir.join("config.json.bad")).unwrap(), bad_value);
        c.save_to(&path).unwrap();
        assert!(Config::load_from(&path).db == "/my/roam.db");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// config.json symlinked into a dotfiles repo: a save writes the target
    /// and the link stays a link (relative and dangling links too).
    #[cfg(unix)]
    #[test]
    fn saving_writes_through_a_symlink() {
        let dir = std::env::temp_dir().join(format!("navi-cfg-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dot")).unwrap();
        let (real, link) = (dir.join("dot").join("config.json"), dir.join("config.json"));
        std::fs::write(&real, "{}").unwrap();
        std::os::unix::fs::symlink("dot/config.json", &link).unwrap();
        let c = Config { db: "/x.db".into(), ..Config::load_from(&link) };
        c.save_to(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "still a link");
        assert_eq!(Config::load_from(&real).db, "/x.db", "the target was written");
        // dangling: the target is created
        std::fs::remove_file(&real).unwrap();
        c.save_to(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(Config::load_from(&real).db, "/x.db");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
