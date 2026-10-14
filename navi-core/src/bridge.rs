//! The `--emacs` protocol between navi and `org-roam-navi.el`.
//!
//! Emacs starts navi as a subprocess (`navi --emacs --db PATH`) and the two
//! talk in JSON lines: commands on navi's stdin, events on its stdout. A pipe
//! needs no Emacs server, no emacsclient path and no socket discovery, works
//! the same on every OS, and closes by itself when either side goes: navi
//! reads EOF and quits when Emacs exits or kills it.
//!
//! Commands (Emacs to navi), one JSON object per line:
//!
//! | `op`     | Fields                        | Effect |
//! |----------|-------------------------------|--------|
//! | `show`   | `id`, `file` (either or both) | Select that node and glide the view to it; never takes focus |
//! | `raise`  |                               | Bring the window to the front |
//! | `reload` |                               | Re-read the db |
//! | `theme`  | colours, `"#rrggbb"`          | Draw with Emacs's colours (see [`Palette`]) |
//! | `quit`   |                               | Save and exit |
//!
//! Events (navi to Emacs): `hello` (`version`, `protocol`) once at start,
//! `open` (`id`, `file`, `pos`, `level`) when a node is opened, `bye` on exit.

use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use crate::graph::{Graph, Node};

/// Bumped when a command or event changes incompatibly; org-roam-navi.el
/// refuses a navi that speaks an older one and says how to update.
pub const PROTOCOL: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Show { id: Option<String>, file: Option<String> },
    Raise,
    Reload,
    Theme(Palette),
    Quit,
}

/// Colours taken from the Emacs faces of the frame navi was started from.
/// Every one is optional: a face Emacs leaves unspecified (a tty frame with
/// no background set, say) falls back to a colour derived from the others.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Palette {
    /// `default` background and foreground.
    pub bg: Option<[u8; 3]>,
    pub fg: Option<[u8; 3]>,
    /// `shadow` foreground: quiet text (labels at rest).
    pub dim: Option<[u8; 3]>,
    /// `org-link` foreground: the colour links (and so nodes) already have in your notes.
    pub link: Option<[u8; 3]>,
    /// `font-lock-keyword-face` foreground: highlighted edges, rims, hover.
    pub accent: Option<[u8; 3]>,
    /// `warning` foreground: the selected node, one hue apart from the rest.
    pub warm: Option<[u8; 3]>,
    /// `mode-line` background and foreground: the status bar and side panel.
    pub bar_bg: Option<[u8; 3]>,
    pub bar_fg: Option<[u8; 3]>,
}

#[derive(Deserialize)]
struct Wire {
    op: String,
    #[serde(default)] id: Option<String>,
    #[serde(default)] file: Option<String>,
    #[serde(default)] bg: Option<String>,
    #[serde(default)] fg: Option<String>,
    #[serde(default)] dim: Option<String>,
    #[serde(default)] link: Option<String>,
    #[serde(default)] accent: Option<String>,
    #[serde(default)] warm: Option<String>,
    #[serde(default)] bar_bg: Option<String>,
    #[serde(default)] bar_fg: Option<String>,
}

/// One line from Emacs. Blank strings count as absent, so the elisp side can
/// send `""` for "no node here" without a special case.
pub fn parse_command(line: &str) -> Result<Command, String> {
    let w: Wire = serde_json::from_str(line.trim()).map_err(|e| format!("bad command: {e}"))?;
    let some = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
    Ok(match w.op.as_str() {
        "show" => {
            let (id, file) = (some(w.id), some(w.file));
            if id.is_none() && file.is_none() { return Err("show needs an id or a file".into()); }
            Command::Show { id, file }
        }
        "raise" => Command::Raise,
        "reload" => Command::Reload,
        "quit" => Command::Quit,
        "theme" => {
            let c = |s: Option<String>| s.as_deref().and_then(parse_colour);
            Command::Theme(Palette {
                bg: c(w.bg), fg: c(w.fg), dim: c(w.dim), link: c(w.link), accent: c(w.accent),
                warm: c(w.warm), bar_bg: c(w.bar_bg), bar_fg: c(w.bar_fg),
            })
        }
        other => return Err(format!("unknown op {other:?}")),
    })
}

/// `#rgb`, `#rrggbb` or Emacs's 16-bit `#rrrrggggbbbb`; anything else
/// (`unspecified-bg`, a colour name) is `None`.
pub fn parse_colour(s: &str) -> Option<[u8; 3]> {
    let hex = s.trim().strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    let n = match hex.len() { 3 => 1, 6 => 2, 9 => 3, 12 => 4, _ => return None };
    let mut out = [0u8; 3];
    for (k, slot) in out.iter_mut().enumerate() {
        let part = &hex[k * n..(k + 1) * n];
        let v = u32::from_str_radix(part, 16).ok()?;
        let max = (1u32 << (4 * n as u32)) - 1;
        *slot = ((v * 255 + max / 2) / max) as u8;
    }
    Some(out)
}

pub fn hello_event(version: &str) -> String {
    json!({ "event": "hello", "version": version, "protocol": PROTOCOL }).to_string()
}

pub fn open_event(node: &Node) -> String {
    json!({ "event": "open", "id": node.id, "file": node.file, "pos": node.pos, "level": node.level }).to_string()
}

pub fn bye_event() -> String {
    json!({ "event": "bye" }).to_string()
}

/// The node a `show` names: by id, else the file node of that file (paths
/// compared as given, then canonicalised, since Emacs may visit a note
/// through a symlink the db doesn't use).
pub fn resolve_show(graph: &Graph, id: Option<&str>, file: Option<&str>) -> Option<usize> {
    if let Some(i) = id.and_then(|id| graph.nodes.get(id)) { return Some(*i); }
    let file = file?;
    let file_node = |pred: &dyn Fn(&str) -> bool| {
        graph.node_list.iter().position(|n| n.level == 0 && pred(&n.file))
    };
    if let Some(i) = file_node(&|f| f == file) { return Some(i); }
    let want = std::fs::canonicalize(file).ok()?;
    file_node(&|f| std::fs::canonicalize(Path::new(f)).map_or(false, |c| c == want))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{GraphData, RawNode};

    fn node(id: &str, file: &str, level: i64) -> RawNode {
        RawNode { id: id.into(), title: id.into(), file: file.into(), level, pos: if level > 0 { 40 } else { 1 }, ..Default::default() }
    }

    fn graph() -> Graph {
        Graph::from_data(GraphData { nodes: vec![node("a", "/n/a.org", 0), node("a-h", "/n/a.org", 1), node("b", "/n/b.org", 0)], ..Default::default() })
    }

    #[test]
    fn parses_every_op() {
        assert_eq!(parse_command(r#"{"op":"raise"}"#), Ok(Command::Raise));
        assert_eq!(parse_command(r#"{"op":"reload"}"#), Ok(Command::Reload));
        assert_eq!(parse_command("  {\"op\":\"quit\"}\r\n"), Ok(Command::Quit));
        assert_eq!(
            parse_command(r#"{"op":"show","id":"x","file":""}"#),
            Ok(Command::Show { id: Some("x".into()), file: None })
        );
        assert!(parse_command(r#"{"op":"show","id":" "}"#).is_err());
        assert!(parse_command(r#"{"op":"dance"}"#).is_err());
        assert!(parse_command("not json").is_err());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // a newer org-roam-navi.el may send more than this navi knows
        assert_eq!(parse_command(r#"{"op":"raise","frame":"F1"}"#), Ok(Command::Raise));
    }

    #[test]
    fn theme_keeps_what_parses() {
        let Ok(Command::Theme(p)) = parse_command(r##"{"op":"theme","bg":"#101e2e","fg":"unspecified-fg","link":"#fff"}"##) else { panic!() };
        assert_eq!(p.bg, Some([0x10, 0x1e, 0x2e]));
        assert_eq!(p.fg, None);
        assert_eq!(p.link, Some([255, 255, 255]));
        assert_eq!(p.accent, None);
    }

    #[test]
    fn colours_in_every_width() {
        assert_eq!(parse_colour("#abc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_colour("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_colour("#ffff80000000"), Some([255, 128, 0]));
        assert_eq!(parse_colour("#000000000"), Some([0, 0, 0]));
        assert_eq!(parse_colour("red"), None);
        assert_eq!(parse_colour("#12345"), None);
        assert_eq!(parse_colour("#gg0000"), None);
    }

    #[test]
    fn events_are_single_lines() {
        let g = graph();
        let open = open_event(&g.node_list[g.nodes["a-h"]]);
        assert!(!open.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&open).unwrap();
        assert_eq!(v["event"], "open");
        assert_eq!(v["id"], "a-h");
        assert_eq!(v["file"], "/n/a.org");
        assert_eq!(v["pos"], 40);
        assert_eq!(v["level"], 1);
        let hello: serde_json::Value = serde_json::from_str(&hello_event("9.9.9")).unwrap();
        assert_eq!(hello["protocol"], PROTOCOL);
        assert_eq!(hello["version"], "9.9.9");
    }

    #[test]
    fn show_resolves_id_then_file_node() {
        let g = graph();
        assert_eq!(resolve_show(&g, Some("a-h"), None), Some(g.nodes["a-h"]));
        // an id navi doesn't know yet (not saved, db not synced): the file's own node
        assert_eq!(resolve_show(&g, Some("new"), Some("/n/a.org")), Some(g.nodes["a"]));
        assert_eq!(resolve_show(&g, None, Some("/n/b.org")), Some(g.nodes["b"]));
        assert_eq!(resolve_show(&g, Some("new"), Some("/n/zzz.org")), None);
        assert_eq!(resolve_show(&g, None, None), None);
    }
}
