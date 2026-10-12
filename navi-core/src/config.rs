use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::view::{default_dailies_dirs, DailiesMode, OrphansMode, ViewFilter};

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
    // ── view filters (D / O persist here) ──
    #[serde(default)]
    pub dailies: DailiesMode,                // "show" | "hide"
    #[serde(default)]
    pub orphans: OrphansMode,                // "show" | "hide" | "only"
    #[serde(default = "default_dailies_dirs")]
    pub dailies_dirs: Vec<String>,           // directory names that mark a daily note
    #[serde(default = "default_true")]
    pub links_to_dailies_count: bool,        // false: a link to a daily doesn't save a note from orphanhood
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
            dailies: DailiesMode::Show,
            orphans: OrphansMode::Show,
            dailies_dirs: default_dailies_dirs(),
            links_to_dailies_count: true,
        }
    }
}

impl Config {
    pub fn view_filter(&self) -> ViewFilter {
        ViewFilter {
            dailies:                self.dailies,
            orphans:                self.orphans,
            dailies_dirs:           self.dailies_dirs.clone(),
            links_to_dailies_count: self.links_to_dailies_count,
        }
    }

    pub fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".config")
            .join("navi")
            .join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(s) = std::fs::read_to_string(&path) {
                return Self::parse(&s);
            }
        }
        Config::default()
    }

    /// Parse config.json. One bad value must not cost every other setting
    /// (startup saves what it loaded): a key whose value doesn't parse falls
    /// back to its default alone, the rest are kept.
    pub fn parse(s: &str) -> Self {
        if let Ok(c) = serde_json::from_str(s) { return c; }
        let Ok(serde_json::Value::Object(user)) = serde_json::from_str::<serde_json::Value>(s) else {
            return Config::default();
        };
        let mut kept = serde_json::Map::new();
        for (k, v) in user {
            let mut trial = kept.clone();
            trial.insert(k.clone(), v.clone());
            if serde_json::from_value::<Config>(serde_json::Value::Object(trial)).is_ok() {
                kept.insert(k, v);
            } else {
                eprintln!("navi: config.json: ignoring bad value for \"{k}\" (using the default)");
            }
        }
        serde_json::from_value(serde_json::Value::Object(kept)).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, s);
        }
    }
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
        assert_eq!(c.view_filter(), ViewFilter::default());
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
        assert_eq!(c.dailies, DailiesMode::Hide, "UI word accepted");
        assert_eq!(c.orphans, OrphansMode::Show, "bad value → default");
        assert!(c.links_to_dailies_count, "bad type → default");
        assert_eq!(Config::parse(r#"{"orphans": "hidden"}"#).orphans, OrphansMode::Hide);
        assert_eq!(Config::parse("not json").server_name, "server");
    }

    #[test]
    fn view_fields_round_trip() {
        let c = Config { dailies: DailiesMode::Hide, orphans: OrphansMode::Only,
                         dailies_dirs: vec!["log".into()], links_to_dailies_count: false, ..Config::default() };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"orphans\":\"only\"") && s.contains("\"dailies\":\"hide\""));
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back.view_filter(), c.view_filter());
    }
}
