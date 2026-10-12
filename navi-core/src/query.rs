//! The query language shared by the search box, filters and groups.
//!
//! ```text
//! query  := or
//! or     := and ( "OR" and )*
//! and    := unary ( "AND"? unary )*          implicit AND between terms
//! unary  := ( "NOT" | "-" ) unary | atom
//! atom   := "(" query ")" | term
//! term   := key ":" value | bare
//! value  := plain | "quoted text" | /regex/
//! ```
//!
//! Keys: `tag:` (exact, any case), `dir:` (path-component prefix below the
//! notes root), `path:` (substring of the full path), `file:` (substring of
//! the file name), `title:`, `alias:` (substring, or `/regex/`), `level:` and
//! `degree:` (`N`, `=N`, `>N`, `<N`, `>=N`, `<=N`), `prop:KEY` / `prop:KEY=VAL`,
//! `is:daily` / `is:orphan` / `is:dangling`. A bare word, a "quoted phrase"
//! or a bare `/regex/` matches title or alias, case-insensitively, so plain
//! search text behaves as it always did. Bare `daily` / `orphan` / `dangling`
//! are the predicate (same as `is:`); in the search box (`Query::search`)
//! they also match the word in a title or alias, so searching `daily` still
//! finds "Daily Review" while a group `daily` holds only daily notes. Also
//! bare text: a word whose prefix before `:` is not a key
//! (`http://x`, `Rust:`), a key followed by a space (`Path: x`), and a lone
//! `-` (`Book - Chapter`). A value starting with `/` is a regex only when its
//! closing `/` ends the term (`path:/n/posts` is a path). Operators are upper
//! case; `and` / `or` / `not` are ordinary words. Text matching and regexes
//! ignore case (`(?-i)` inside a regex turns that off).
//!
//! Parsing never panics: every error carries the character column it was
//! found at. The search box uses `Query::search`, which falls back to the old
//! whole-text substring match when the text doesn't parse; `group_source`
//! turns search text into a group query that matches the same nodes.

use regex::{Regex, RegexBuilder};
use std::fmt;

use crate::graph::Graph;
use crate::view::{dir_components, notes_root_depth, ViewState};

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryError {
    pub pos: usize,  // 0-based character offset into the query
    pub msg: String,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "col {}: {}", self.pos + 1, self.msg)
    }
}

impl std::error::Error for QueryError {}

fn err<T>(pos: usize, msg: impl Into<String>) -> Result<T, QueryError> {
    Err(QueryError { pos, msg: msg.into() })
}

// ── AST ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Text {
    Sub(String),   // lowercase substring
    Exact(String), // lowercase whole value
    Re(Regex),     // case-insensitive unless the pattern says otherwise
}

impl Text {
    pub fn is_match(&self, hay: &str) -> bool {
        match self {
            Text::Sub(s)   => s.is_empty() || hay.to_lowercase().contains(s.as_str()),
            Text::Exact(s) => hay.to_lowercase() == *s,
            Text::Re(r)    => r.is_match(hay),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp { Eq, Lt, Gt, Le, Ge }

impl Cmp {
    fn test(self, a: i64, b: i64) -> bool {
        match self { Cmp::Eq => a == b, Cmp::Lt => a < b, Cmp::Gt => a > b, Cmp::Le => a <= b, Cmp::Ge => a >= b }
    }
}

#[derive(Debug, Clone)]
pub enum Pred {
    Bare(Text),                    // title or any alias
    Tag(Text),
    Dir(Vec<String>),              // components below the notes root (or absolute, root stripped)
    Path(Text),
    File(Text),
    Title(Text),
    Alias(Text),
    Level(Cmp, i64),
    Degree(Cmp, i64),              // deduped real links in the whole db
    Prop(String, Option<Text>),    // key (any case), value
    Daily,
    Orphan,                        // in-view orphan (see EvalCtx)
    Dangling,                      // has unresolved links
}

#[derive(Debug, Clone)]
pub enum Expr {
    All,
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Pred(Pred),
}

/// A parsed query. `Query::parse("")` is the empty query, which matches
/// every node.
#[derive(Debug, Clone)]
pub struct Query {
    pub src: String,
    expr:    Expr,
}

const KEYS: &[&str] = &["tag", "dir", "path", "file", "title", "alias", "level", "degree", "prop", "is"];

// ── Lexer ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Plain(String),
    Quoted(String),
    Regex(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Open,
    Close,
    And,
    Or,
    Not,
    Minus,
    Term { key: Option<String>, val: Val, vpos: usize },
}

fn is_sep(c: char) -> bool { c.is_whitespace() || c == '(' || c == ')' }

/// Read a "quoted" or /regex/ body starting at the opening delimiter `cs[i]`.
/// `\` escapes the delimiter (and a backslash, in quotes); in a regex any
/// other escape is kept for the regex engine.
fn read_delimited(cs: &[char], i: usize, what: &str) -> Result<(String, usize), QueryError> {
    let d = cs[i];
    let mut out = String::new();
    let mut j = i + 1;
    while j < cs.len() {
        let c = cs[j];
        if c == '\\' && j + 1 < cs.len() {
            let n = cs[j + 1];
            if n == d || (d == '"' && n == '\\') { out.push(n); } else { out.push(c); out.push(n); }
            j += 2;
            continue;
        }
        if c == d { return Ok((out, j + 1)); }
        out.push(c);
        j += 1;
    }
    err(i, format!("unterminated {what}"))
}

/// A `/regex/` at `cs[i]`, only when its closing `/` ends the term (end of
/// input, space or a paren); otherwise (`/n/posts`) None: it is a word. An
/// unterminated one is an error, or a word too when `lenient` (path-like
/// values, where `/tmp` is a path).
fn whole_regex(cs: &[char], i: usize, lenient: bool) -> Result<Option<(String, usize)>, QueryError> {
    match read_delimited(cs, i, "regex (missing closing /)") {
        Ok((s, j)) if j >= cs.len() || is_sep(cs[j]) => Ok(Some((s, j))),
        Ok(_) => Ok(None),
        Err(_) if lenient => Ok(None),
        Err(e) => Err(e),
    }
}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, QueryError> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() { i += 1; continue; }
        let start = i;
        match c {
            '(' => { out.push((Tok::Open, i)); i += 1; continue; }
            ')' => { out.push((Tok::Close, i)); i += 1; continue; }
            // a lone '-' ("Book - Chapter") is a word; "-x" negates x
            '-' if i + 1 < cs.len() && !cs[i + 1].is_whitespace() && cs[i + 1] != ')' => { out.push((Tok::Minus, i)); i += 1; continue; }
            '"' => {
                let (s, j) = read_delimited(&cs, i, "quote")?;
                out.push((Tok::Term { key: None, val: Val::Quoted(s), vpos: i }, start));
                i = j; continue;
            }
            '/' => {
                if let Some((s, j)) = whole_regex(&cs, i, false)? {
                    out.push((Tok::Term { key: None, val: Val::Regex(s), vpos: i }, start));
                    i = j; continue;
                }
            }
            _ => {}
        }
        // key:value ?
        let mut k = i;
        while k < cs.len() && cs[k].is_ascii_alphabetic() { k += 1; }
        if k > i && k < cs.len() && cs[k] == ':' {
            let key: String = cs[i..k].iter().collect::<String>().to_ascii_lowercase();
            let v = k + 1;
            // "Path: x" is text with a colon in it, not an empty key
            if KEYS.contains(&key.as_str()) && !(v < cs.len() && cs[v].is_whitespace()) {
                if v >= cs.len() || is_sep(cs[v]) {
                    return err(v, format!("'{key}:' needs a value"));
                }
                let path_like = key == "path" || key == "file";
                let regex = if cs[v] == '/' && key != "dir" { whole_regex(&cs, v, path_like)? } else { None };
                let (val, j) = match (cs[v], regex) {
                    (_, Some((s, j))) => (Val::Regex(s), j),
                    ('"', None) => { let (s, j) = read_delimited(&cs, v, "quote")?; (Val::Quoted(s), j) }
                    _ => {
                        let mut j = v;
                        while j < cs.len() && !is_sep(cs[j]) { j += 1; }
                        (Val::Plain(cs[v..j].iter().collect()), j)
                    }
                };
                out.push((Tok::Term { key: Some(key), val, vpos: v }, start));
                i = j;
                continue;
            }
        }
        // plain word
        let mut j = i;
        while j < cs.len() && !is_sep(cs[j]) { j += 1; }
        let w: String = cs[i..j].iter().collect();
        let tok = match w.as_str() {
            "AND" => Tok::And,
            "OR"  => Tok::Or,
            "NOT" => Tok::Not,
            _     => Tok::Term { key: None, val: Val::Plain(w), vpos: i },
        };
        out.push((tok, start));
        i = j;
    }
    Ok(out)
}

// ── Parser ────────────────────────────────────────────────────────────────────

struct Parser {
    toks:   Vec<(Tok, usize)>,
    at:     usize,
    end:    usize, // char length of the source (error position at end of input)
    search: bool,  // the search box: bare predicate words match titles too
}

impl Parser {
    fn peek(&self) -> Option<&Tok> { self.toks.get(self.at).map(|t| &t.0) }
    fn pos(&self) -> usize { self.toks.get(self.at).map_or(self.end, |t| t.1) }

    fn or(&mut self) -> Result<Expr, QueryError> {
        let mut terms = vec![self.and()?];
        while self.peek() == Some(&Tok::Or) {
            let p = self.pos();
            self.at += 1;
            if matches!(self.peek(), None | Some(Tok::Close) | Some(Tok::Or) | Some(Tok::And)) {
                return err(p, "OR needs a term on both sides");
            }
            terms.push(self.and()?);
        }
        Ok(if terms.len() == 1 { terms.pop().unwrap() } else { Expr::Or(terms) })
    }

    fn and(&mut self) -> Result<Expr, QueryError> {
        let mut terms = vec![self.unary()?];
        loop {
            match self.peek() {
                None | Some(Tok::Close) | Some(Tok::Or) => break,
                Some(Tok::And) => {
                    let p = self.pos();
                    self.at += 1;
                    if matches!(self.peek(), None | Some(Tok::Close) | Some(Tok::Or) | Some(Tok::And)) {
                        return err(p, "AND needs a term on both sides");
                    }
                }
                _ => {}
            }
            terms.push(self.unary()?);
        }
        Ok(if terms.len() == 1 { terms.pop().unwrap() } else { Expr::And(terms) })
    }

    fn unary(&mut self) -> Result<Expr, QueryError> {
        match self.peek() {
            Some(Tok::Not) | Some(Tok::Minus) => {
                let p = self.pos();
                let word = if self.peek() == Some(&Tok::Not) { "NOT" } else { "-" };
                self.at += 1;
                if matches!(self.peek(), None | Some(Tok::Close) | Some(Tok::Or) | Some(Tok::And)) {
                    return err(p, format!("{word} needs a term after it"));
                }
                Ok(Expr::Not(Box::new(self.unary()?)))
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<Expr, QueryError> {
        let p = self.pos();
        let Some((tok, _)) = self.toks.get(self.at).cloned() else { return err(p, "expected a term") };
        self.at += 1;
        match tok {
            Tok::Open => {
                if self.peek() == Some(&Tok::Close) { return err(p, "empty parentheses"); }
                let e = self.or()?;
                if self.peek() != Some(&Tok::Close) { return err(p, "missing ')' for this '('"); }
                self.at += 1;
                Ok(e)
            }
            Tok::Close => err(p, "unexpected ')'"),
            Tok::And | Tok::Or => err(p, "AND / OR need a term on both sides"),
            Tok::Not | Tok::Minus => unreachable!("unary() consumes negation"),
            Tok::Term { key, val, vpos } => term(key.as_deref(), val, vpos, self.search),
        }
    }
}

fn regex(src: &str, pos: usize) -> Result<Regex, QueryError> {
    RegexBuilder::new(src).case_insensitive(true).size_limit(1 << 20).build().or_else(|e| {
        let detail = e.to_string();
        let last = detail.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        err(pos, format!("bad regex: {}", last.trim_start_matches("error: ")))
    })
}

fn text(val: Val, vpos: usize, exact: bool) -> Result<Text, QueryError> {
    Ok(match val {
        Val::Plain(s) | Val::Quoted(s) if exact => Text::Exact(s.to_lowercase()),
        Val::Plain(s) | Val::Quoted(s)          => Text::Sub(s.to_lowercase()),
        Val::Regex(r)                           => Text::Re(regex(&r, vpos)?),
    })
}

fn number(key: &str, val: Val, vpos: usize) -> Result<(Cmp, i64), QueryError> {
    let s = match val {
        Val::Plain(s) | Val::Quoted(s) => s,
        Val::Regex(_) => return err(vpos, format!("'{key}:' takes a number, not a regex")),
    };
    let s = s.trim();
    let (cmp, rest) = if let Some(r) = s.strip_prefix(">=") { (Cmp::Ge, r) }
        else if let Some(r) = s.strip_prefix("<=") { (Cmp::Le, r) }
        else if let Some(r) = s.strip_prefix('>') { (Cmp::Gt, r) }
        else if let Some(r) = s.strip_prefix('<') { (Cmp::Lt, r) }
        else if let Some(r) = s.strip_prefix('=') { (Cmp::Eq, r) }
        else { (Cmp::Eq, s) };
    match rest.trim().parse::<i64>() {
        Ok(n) => Ok((cmp, n)),
        Err(_) => err(vpos, format!("'{key}:' takes a number like 0, >0, <3 or >=2")),
    }
}

fn path_parts(s: &str) -> Vec<String> {
    s.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").map(|c| c.to_string()).collect()
}

fn term(key: Option<&str>, val: Val, vpos: usize, search: bool) -> Result<Expr, QueryError> {
    let Some(key) = key else {
        // bare daily / orphan / dangling: the predicate; in the search box
        // also the word in a title (searching "daily" still finds "Daily
        // Review"), never in a group (a hidden `daily` group hides dailies)
        if let Val::Plain(w) = &val {
            let w = w.to_lowercase();
            let pred = match w.as_str() { "daily" => Some(Pred::Daily), "orphan" => Some(Pred::Orphan), "dangling" => Some(Pred::Dangling), _ => None };
            match pred {
                Some(p) if search => return Ok(Expr::Or(vec![Expr::Pred(p), Expr::Pred(Pred::Bare(Text::Sub(w)))])),
                Some(p) => return Ok(Expr::Pred(p)),
                None => {}
            }
        }
        return Ok(Expr::Pred(Pred::Bare(text(val, vpos, false)?)));
    };
    Ok(Expr::Pred(match key {
        "tag"   => Pred::Tag(text(val, vpos, true)?),
        "path"  => Pred::Path(text(val, vpos, false)?),
        "file"  => Pred::File(text(val, vpos, false)?),
        "title" => Pred::Title(text(val, vpos, false)?),
        "alias" => Pred::Alias(text(val, vpos, false)?),
        "dir"   => match val {
            Val::Plain(s) | Val::Quoted(s) => Pred::Dir(path_parts(&crate::config::expand_tilde(&s))),
            Val::Regex(_) => return err(vpos, "'dir:' takes a folder path, not a regex"),
        },
        "level"  => { let (c, n) = number(key, val, vpos)?; Pred::Level(c, n) }
        "degree" => { let (c, n) = number(key, val, vpos)?; Pred::Degree(c, n) }
        "prop"   => {
            let s = match val {
                Val::Plain(s) | Val::Quoted(s) => s,
                Val::Regex(_) => return err(vpos, "'prop:' takes KEY or KEY=VALUE"),
            };
            let (k, v) = match s.split_once('=') { Some((k, v)) => (k, Some(v)), None => (s.as_str(), None) };
            if k.trim().is_empty() { return err(vpos, "'prop:' needs a property name before '='"); }
            Pred::Prop(k.trim().to_lowercase(), v.map(|v| Text::Exact(v.to_lowercase())))
        }
        "is" => match val {
            Val::Plain(s) | Val::Quoted(s) => match s.to_lowercase().as_str() {
                "daily"    => Pred::Daily,
                "orphan"   => Pred::Orphan,
                "dangling" => Pred::Dangling,
                _ => return err(vpos, "'is:' takes daily, orphan or dangling"),
            },
            Val::Regex(_) => return err(vpos, "'is:' takes daily, orphan or dangling"),
        },
        _ => unreachable!("lexer only emits known keys"),
    }))
}

// ── Evaluation ────────────────────────────────────────────────────────────────

/// What a query is evaluated against: the graph plus the per-node facts the
/// view pipeline computed. `orphan` is "isolated in this view": for the
/// search box that is the final view; for groups it is the view BEFORE group
/// hiding (see view.rs), so membership never depends on group visibility.
pub struct EvalCtx<'a> {
    pub graph:  &'a Graph,
    pub daily:  &'a [bool],
    pub orphan: &'a [bool],
    root:       Vec<String>, // notes-root directory components
}

impl<'a> EvalCtx<'a> {
    pub fn new(graph: &'a Graph, daily: &'a [bool], orphan: &'a [bool]) -> Self {
        let depth = notes_root_depth(graph.node_list.iter().map(|n| n.file.as_str()));
        let root  = graph.node_list.first()
            .map(|n| dir_components(&n.file).take(depth).filter(|c| !c.is_empty()).map(|c| c.to_string()).collect())
            .unwrap_or_default();
        EvalCtx { graph, daily, orphan, root }
    }

    pub fn from_view(graph: &'a Graph, view: &'a ViewState) -> Self { Self::new(graph, &view.daily, &view.orphan) }

    /// Directory components of a node's file below the notes root.
    fn rel_dirs<'f>(&self, file: &'f str) -> Vec<&'f str> {
        let all: Vec<&str> = dir_components(file).filter(|c| !c.is_empty()).collect();
        let skip = if all.len() >= self.root.len() && all.iter().zip(&self.root).all(|(a, b)| a == b) { self.root.len() } else { 0 };
        all[skip..].to_vec()
    }
}

fn eq_ci(a: &str, b: &str) -> bool { a.to_lowercase() == b.to_lowercase() }

impl Pred {
    fn eval(&self, cx: &EvalCtx, i: usize) -> bool {
        let nd = &cx.graph.node_list[i];
        match self {
            Pred::Bare(t)  => t.is_match(&nd.title) || nd.aliases.iter().any(|a| t.is_match(a)),
            Pred::Title(t) => t.is_match(&nd.title),
            Pred::Alias(t) => nd.aliases.iter().any(|a| t.is_match(a)),
            Pred::Tag(t)   => nd.tags.iter().any(|g| t.is_match(g)),
            Pred::Path(t)  => t.is_match(&nd.file),
            Pred::File(t)  => t.is_match(nd.file.rsplit(['/', '\\']).next().unwrap_or("")),
            Pred::Dir(want) => {
                // absolute (root included) or relative to the notes root
                // the root compares in any case: a drive letter (c: / C:) or a
                // typed ~/Notes against a db that stored ~/notes
                let want: &[String] = if want.len() >= cx.root.len() && !cx.root.is_empty()
                    && want.iter().zip(&cx.root).all(|(a, b)| eq_ci(a, b)) { &want[cx.root.len()..] } else { want };
                let have = cx.rel_dirs(&nd.file);
                want.len() <= have.len() && want.iter().zip(&have).all(|(w, h)| eq_ci(w, h))
            }
            Pred::Level(c, n)  => c.test(nd.level, *n),
            Pred::Degree(c, n) => c.test(nd.degree as i64, *n),
            Pred::Prop(k, v) => nd.properties.iter().any(|(pk, pv)| {
                pk.to_lowercase() == *k && match v { Some(t) => t.is_match(pv), None => true }
            }),
            Pred::Daily    => cx.daily.get(i).copied().unwrap_or(false),
            Pred::Orphan   => cx.orphan.get(i).copied().unwrap_or(false),
            Pred::Dangling => nd.dangling > 0,
        }
    }
}

impl Expr {
    fn eval(&self, cx: &EvalCtx, i: usize) -> bool {
        match self {
            Expr::All     => true,
            Expr::And(v)  => v.iter().all(|e| e.eval(cx, i)),
            Expr::Or(v)   => v.iter().any(|e| e.eval(cx, i)),
            Expr::Not(e)  => !e.eval(cx, i),
            Expr::Pred(p) => p.eval(cx, i),
        }
    }

    fn first_bare(&self) -> Option<String> {
        match self {
            Expr::Pred(Pred::Bare(Text::Sub(s))) => Some(s.clone()),
            Expr::And(v) | Expr::Or(v) => v.iter().find_map(|e| e.first_bare()),
            _ => None,
        }
    }
}

impl Query {
    /// Groups and filters: the language exactly.
    pub fn parse(src: &str) -> Result<Query, QueryError> { Self::parse_as(src, false) }

    fn parse_as(src: &str, search: bool) -> Result<Query, QueryError> {
        let toks = lex(src)?;
        let end  = src.chars().count();
        if toks.is_empty() { return Ok(Query { src: src.to_string(), expr: Expr::All }); }
        let mut p = Parser { toks, at: 0, end, search };
        let expr = p.or()?;
        if p.at < p.toks.len() {
            let pos = p.pos();
            return err(pos, if p.peek() == Some(&Tok::Close) { "unexpected ')'" } else { "unexpected term" });
        }
        Ok(Query { src: src.to_string(), expr })
    }

    /// The search box: `parse` (bare `daily` / `orphan` / `dangling` also
    /// match the word in a title or alias), or when the text doesn't parse,
    /// the old search (title or alias contains the whole text, any case)
    /// plus the error, to show as a hint. Plain search text never stops
    /// working.
    pub fn search(src: &str) -> (Query, Option<QueryError>) {
        match Query::parse_as(src, true) {
            Ok(q)  => (q, None),
            Err(e) => (Query { src: src.to_string(), expr: Expr::Pred(Pred::Bare(Text::Sub(src.to_lowercase()))) }, Some(e)),
        }
    }

    /// A group query that matches what `search(src)` matches ("Add group"
    /// from the search box): the text itself when it means the same as a
    /// group query; the whole text quoted when the search fell back to plain
    /// text; bare `daily` / `orphan` / `dangling` widened to the title match
    /// the search gives them.
    pub fn group_source(src: &str) -> String {
        let src = src.trim();
        let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let Ok(toks) = lex(src) else { return quote(src) };
        if Query::parse(src).is_err() { return quote(src); }
        // rebuild only when a bare predicate word is in it
        let widen = |t: &Tok| matches!(t, Tok::Term { key: None, val: Val::Plain(w), .. }
                                        if matches!(w.to_lowercase().as_str(), "daily" | "orphan" | "dangling"));
        if !toks.iter().any(|(t, _)| widen(t)) { return src.to_string(); }
        let cs: Vec<char> = src.chars().collect();
        let mut out = String::new();
        let mut last = 0;
        for (t, at) in &toks {
            let Tok::Term { key: None, val: Val::Plain(w), .. } = t else { continue };
            if !widen(t) { continue; }
            out.extend(&cs[last..*at]);
            out.push_str(&format!("({w} OR \"{w}\")"));
            last = at + w.chars().count();
        }
        out.extend(&cs[last..]);
        out
    }

    /// Matches every node (empty or blank source).
    pub fn is_empty(&self) -> bool { matches!(self.expr, Expr::All) }

    pub fn matches(&self, cx: &EvalCtx, i: usize) -> bool { self.expr.eval(cx, i) }

    /// One flag per node.
    pub fn eval_all(&self, cx: &EvalCtx) -> Vec<bool> {
        (0..cx.graph.node_list.len()).map(|i| self.expr.eval(cx, i)).collect()
    }

    /// The first bare (non-negated) search word, lowercase: what search
    /// ranks "title starts with" on.
    pub fn first_bare(&self) -> Option<String> { self.expr.first_bare() }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::RawNode;
    use crate::view::{compute_view, ViewFilter};

    fn node(id: &str, title: &str, file: &str, tags: &[&str]) -> RawNode {
        RawNode { id: id.into(), title: title.into(), file: file.into(),
                  tags: tags.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    /// root /n ; posts/ and posts/deep/ ; a heading ; a daily ; a db orphan
    fn fixture() -> Graph {
        let mut h = node("h", "Ownership", "/n/rust.org", &["rust"]);
        h.level = 2;
        let mut r = node("r", "Rust Notes", "/n/rust.org", &["Rust", "lang"]);
        r.aliases = vec!["Crab Book".into()];
        r.properties = vec![("STATUS".into(), "Draft".into()), ("CATEGORY".into(), "lang".into())];
        let mut g = Graph::new(vec![
            r, h,
            node("p", "First Post", "/n/posts/first.org", &["blog"]),
            node("q", "Deep Post", "/n/posts/deep/q.org", &[]),
            node("x", "Postscript", "/n/postsx/x.org", &[]),
            node("d", "2026-09-01", "/n/2026-09-01.org", &[]),
            node("o", "Lonely (draft)", "/n/o.org", &["blog"]),
        ], vec![("r".into(), "p".into()), ("r".into(), "q".into()), ("p".into(), "d".into()), ("r".into(), "h".into())]);
        g.set_dangling(vec![crate::db::DanglingLink { source: "o".into(), dest: "Nowhere".into(), link_type: "fuzzy".into() }],
                       Default::default());
        g
    }

    fn ids(g: &Graph, q: &str) -> Vec<String> {
        let v  = compute_view(g, &ViewFilter::default());
        let cx = EvalCtx::from_view(g, &v);
        let q  = Query::parse(q).unwrap_or_else(|e| panic!("{q:?}: {e}"));
        let m  = q.eval_all(&cx);
        (0..m.len()).filter(|&i| m[i]).map(|i| g.node_list[i].id.clone()).collect()
    }

    fn perr(q: &str) -> QueryError { Query::parse(q).expect_err(q) }

    /// What the search box matches (`Query::search`).
    fn sids(g: &Graph, q: &str) -> Vec<String> {
        let v  = compute_view(g, &ViewFilter::default());
        let cx = EvalCtx::from_view(g, &v);
        let m  = Query::search(q).0.eval_all(&cx);
        (0..m.len()).filter(|&i| m[i]).map(|i| g.node_list[i].id.clone()).collect()
    }

    /// Groups use the predicate alone: a `daily` group never takes "Daily
    /// Review" (and hiding it never hides that note); `-orphan` never drops
    /// a linked "Orphan Black". The search box still finds both by title.
    #[test]
    fn bare_predicate_words_are_exact_in_groups() {
        let g = Graph::new(vec![
            node("c", "Daily Review", "/n/c.org", &["x"]),
            node("d", "2026-09-01", "/n/daily/2026-09-01.org", &[]),
            node("f", "Orphan Black", "/n/f.org", &["x"]),
            node("o", "Alone", "/n/o.org", &["x"]),
        ], vec![("c".into(), "d".into()), ("f".into(), "c".into())]);
        assert_eq!(ids(&g, "daily"), vec!["d"]);
        assert_eq!(ids(&g, "orphan"), vec!["o"]);
        assert_eq!(ids(&g, "tag:x -orphan"), vec!["c", "f"], "a linked note titled Orphan stays");
        assert_eq!(sids(&g, "daily"), vec!["c", "d"]);
        assert_eq!(sids(&g, "orphan"), vec!["f", "o"]);
        assert_eq!(sids(&g, "tag:x -orphan"), vec!["c"]);
    }

    /// "Add group" from the search box: the group matches what the search
    /// showed, also for text that only matched as plain text.
    #[test]
    fn group_source_matches_what_the_search_matched() {
        let g = Graph::new(vec![
            node("a", "Lonely (draft", "/n/a.org", &[]),
            node("b", "\"rust\" notes", "/n/b.org", &[]),
            node("c", "/tmp files", "/n/c.org", &[]),
            node("d", "Daily Review", "/n/d.org", &["x"]),
            node("e", "2026-09-01", "/n/daily/2026-09-01.org", &[]),
            node("f", "back\\slash \"q", "/n/f.org", &[]),
        ], vec![("d".into(), "e".into())]);
        for src in ["lonely (draft", "\"rust", "/tmp", "daily", "tag:x OR daily", "-daily", "back\\slash \"q", " rust ", "", "orphan dangling"] {
            let group = Query::group_source(src);
            assert!(Query::parse(&group).is_ok(), "{src:?} -> {group:?}");
            assert_eq!(ids(&g, &group), sids(&g, src), "{src:?} -> {group:?}");
        }
        assert_eq!(Query::group_source("tag:x OR dir:posts"), "tag:x OR dir:posts", "text that parses is kept");
        assert_eq!(Query::group_source("lonely (draft"), "\"lonely (draft\"");
        assert_eq!(Query::group_source("Daily"), "(Daily OR \"Daily\")");
    }

    #[test]
    fn bare_words_match_title_or_alias_like_the_old_search() {
        let g = fixture();
        assert_eq!(ids(&g, ""), vec!["r", "h", "p", "q", "x", "d", "o"], "empty matches all");
        assert_eq!(ids(&g, "   "), ids(&g, ""));
        assert_eq!(ids(&g, "post"), vec!["p", "q", "x"], "case-insensitive substring");
        assert_eq!(ids(&g, "CRAB"), vec!["r"], "aliases count");
        assert_eq!(ids(&g, "deep post"), vec!["q"], "words are ANDed");
        assert_eq!(ids(&g, "\"deep post\""), vec!["q"], "quoted phrase");
        assert_eq!(ids(&g, "\"t d\""), Vec::<String>::new());
        assert_eq!(ids(&g, "/^(first|deep) /"), vec!["p", "q"], "bare regex");
        assert_eq!(ids(&g, "and"), Vec::<String>::new(), "lower-case operators are words");
        assert_eq!(ids(&g, "http://x"), Vec::<String>::new(), "unknown key: bare text, no error");
        assert!(Query::parse("Rust: ownership").is_ok());
        assert_eq!(Query::parse("Deep post").unwrap().first_bare().as_deref(), Some("deep"));
        assert!(Query::parse("tag:x").unwrap().first_bare().is_none());
        assert!(Query::parse("").unwrap().is_empty());
    }

    #[test]
    fn field_predicates() {
        let g = fixture();
        assert_eq!(ids(&g, "tag:rust"), vec!["r", "h"], "exact, any case");
        assert_eq!(ids(&g, "tag:ru"), Vec::<String>::new(), "tag is not a substring match");
        assert_eq!(ids(&g, "tag:/^ru/"), vec!["r", "h"]);
        assert_eq!(ids(&g, "title:post"), vec!["p", "q", "x"]);
        assert_eq!(ids(&g, "title:/^post/"), vec!["x"]);
        assert_eq!(ids(&g, "alias:crab"), vec!["r"]);
        assert_eq!(ids(&g, "title:crab"), Vec::<String>::new(), "title: ignores aliases");
        assert_eq!(ids(&g, "path:posts"), vec!["p", "q", "x"], "path: is a substring");
        assert_eq!(ids(&g, "file:first"), vec!["p"]);
        assert_eq!(ids(&g, "file:2026"), vec!["d"]);
        assert_eq!(ids(&g, "file:n"), Vec::<String>::new(), "file name only, not its directories");
        assert_eq!(ids(&g, "level:0"), vec!["r", "p", "q", "x", "d", "o"]);
        assert_eq!(ids(&g, "level:>0"), vec!["h"]);
        assert_eq!(ids(&g, "level:>=2"), vec!["h"]);
        assert_eq!(ids(&g, "degree:>1"), vec!["r", "p"]);
        assert_eq!(ids(&g, "degree:=1"), vec!["h", "q", "d"]);
        assert_eq!(ids(&g, "degree:<1"), vec!["x", "o"]);
        assert_eq!(ids(&g, "degree:<=0"), vec!["x", "o"]);
        assert_eq!(ids(&g, "prop:status"), vec!["r"]);
        assert_eq!(ids(&g, "prop:STATUS=draft"), vec!["r"], "key and value ignore case");
        assert_eq!(ids(&g, "prop:status=final"), Vec::<String>::new());
        assert_eq!(ids(&g, "prop:\"category=lang\""), vec!["r"]);
        assert_eq!(ids(&g, "daily"), vec!["d"]);
        assert_eq!(ids(&g, "is:daily"), vec!["d"]);
        assert_eq!(ids(&g, "orphan"), vec!["x", "o"]);
        assert_eq!(ids(&g, "dangling"), vec!["o"]);
        assert_eq!(ids(&g, "is:dangling"), vec!["o"]);
        assert_eq!(ids(&g, "\"daily\""), Vec::<String>::new(), "quoted: a title search, not the predicate");
    }

    /// dir: is a whole-component prefix below the notes root, never a substring.
    #[test]
    fn dir_is_a_component_prefix_relative_to_the_notes_root() {
        let g = fixture();
        assert_eq!(ids(&g, "dir:posts"), vec!["p", "q"], "postsx/ is not under posts/");
        assert_eq!(ids(&g, "dir:posts/deep"), vec!["q"]);
        assert_eq!(ids(&g, "dir:POSTS/"), vec!["p", "q"], "case-insensitive, trailing slash ok");
        assert_eq!(ids(&g, "dir:post"), Vec::<String>::new(), "no substring");
        assert_eq!(ids(&g, "dir:/n/posts"), vec!["p", "q"], "absolute path: root stripped");
        assert_eq!(ids(&g, r"dir:posts\deep"), vec!["q"], "either separator");
        assert_eq!(ids(&g, "dir:."), ids(&g, ""), "the root itself holds everything");
        assert_eq!(ids(&g, "-dir:posts level:0"), vec!["r", "x", "d", "o"]);
    }

    #[test]
    fn boolean_operators_and_precedence() {
        let g = fixture();
        assert_eq!(ids(&g, "tag:blog OR tag:lang"), vec!["r", "p", "o"]);
        assert_eq!(ids(&g, "tag:blog AND post"), vec!["p"]);
        assert_eq!(ids(&g, "tag:blog post"), vec!["p"], "implicit AND");
        // AND binds tighter than OR: (tag:rust AND level:>0) OR daily
        assert_eq!(ids(&g, "tag:rust level:>0 OR daily"), vec!["h", "d"]);
        assert_eq!(ids(&g, "tag:rust (level:>0 OR daily)"), vec!["h"]);
        assert_eq!(ids(&g, "NOT tag:blog"), vec!["r", "h", "q", "x", "d"]);
        assert_eq!(ids(&g, "-tag:blog"), ids(&g, "NOT tag:blog"));
        assert_eq!(ids(&g, "-(tag:blog OR tag:rust)"), vec!["q", "x", "d"]);
        assert_eq!(ids(&g, "NOT NOT daily"), vec!["d"]);
        assert_eq!(ids(&g, "--daily"), vec!["d"]);
        assert_eq!(ids(&g, "((daily))"), vec!["d"]);
        assert_eq!(ids(&g, "tag:blog -orphan"), vec!["p"]);
        assert_eq!(ids(&g, "(tag:blog OR tag:rust) AND NOT level:>0 AND -dangling"), vec!["r", "p"]);
        assert_eq!(ids(&g, "title:\"first post\" OR title:\"deep post\""), vec!["p", "q"]);
        assert_eq!(ids(&g, "lonely (draft)"), vec!["o"], "parens group: lonely AND draft");
    }

    #[test]
    fn errors_carry_positions_and_never_panic() {
        assert_eq!(perr("tag:"), QueryError { pos: 4, msg: "'tag:' needs a value".into() });
        assert_eq!(perr("(tag:a OR"), QueryError { pos: 7, msg: "OR needs a term on both sides".into() });
        assert_eq!(perr("x (tag:a").pos, 2, "missing ')' points at its '('");
        assert_eq!(perr("a)").pos, 1);
        assert_eq!(perr("()").msg, "empty parentheses");
        assert_eq!(perr("AND x").pos, 0);
        assert_eq!(perr("x AND").pos, 2);
        assert_eq!(perr("x OR OR y").pos, 2);
        assert_eq!(perr("NOT").msg, "NOT needs a term after it");
        assert_eq!(perr("title:\"open").pos, 6);
        assert_eq!(perr("\"open").msg, "unterminated quote");
        assert_eq!(perr("title:/ab").pos, 6);
        let e = perr("title:/(/");
        assert_eq!(e.pos, 6);
        assert!(e.msg.starts_with("bad regex"), "{}", e.msg);
        assert_eq!(perr("level:x").msg, "'level:' takes a number like 0, >0, <3 or >=2");
        assert_eq!(perr("degree:>").pos, 7);
        assert_eq!(perr("is:lonely").pos, 3);
        assert_eq!(perr("prop:=x").pos, 5);
        assert_eq!(perr("ünï tag:").pos, 8, "positions count characters, not bytes");
        assert_eq!(format!("{}", perr("tag:")), "col 5: 'tag:' needs a value");
        // nothing a user can type panics
        for s in ["(", ")", "-", "--", "\"", "/", "\\", "tag:\"\\", "((a)", "a))", "NOT (", "OR", "-(", "prop:", "x:\"", "/\\/"] {
            let _ = Query::parse(s);
        }
    }

    /// Plain search text that used to work still does: a lone '-', a key
    /// followed by a space, the predicate words, and (through
    /// `Query::search`) text that doesn't parse at all.
    #[test]
    fn plain_text_that_looks_like_syntax_still_searches() {
        let g = Graph::new(vec![
            node("a", "Rust - Ownership", "/n/a.org", &[]),
            node("b", "Path: x marks the spot", "/n/b.org", &[]),
            node("c", "Daily Review", "/n/c.org", &[]),
            node("d", "2026-09-01", "/n/daily/2026-09-01.org", &[]),
            node("e", "Lonely (draft)", "/n/e.org", &[]),
            node("f", "Orphan Black", "/n/f.org", &[]),
        ], vec![("a".into(), "b".into()), ("c".into(), "d".into())]);
        assert_eq!(ids(&g, "rust - ownership"), vec!["a"]);
        assert_eq!(ids(&g, "Book - Chapter"), Vec::<String>::new());
        assert_eq!(ids(&g, "-"), vec!["a", "d"], "a lone '-' is a word");
        assert_eq!(ids(&g, "-(daily)"), vec!["a", "b", "c", "e", "f"], "'-' before '(' still negates");
        assert_eq!(sids(&g, "-(daily)"), vec!["a", "b", "e", "f"], "the search's daily takes the title too");
        assert_eq!(ids(&g, "Path: x"), vec!["b"], "a key and a space: text");
        assert_eq!(ids(&g, "tag: x"), Vec::<String>::new());
        assert!(Query::parse("File: x").is_ok() && Query::parse("Is: x").is_ok());
        assert_eq!(sids(&g, "daily"), vec!["c", "d"], "search: the daily note AND the title");
        assert_eq!(sids(&g, "is:daily"), vec!["d"], "is: is the predicate alone");
        assert_eq!(sids(&g, "orphan"), vec!["e", "f"]);
        assert_eq!(Query::search("Daily").0.first_bare().as_deref(), Some("daily"));
        // text that doesn't parse: the old whole-text search, and the error
        for src in ["(draft", "lonely (draft", "\"rust"] {
            let (q, e) = Query::search(src);
            assert!(e.is_some(), "{src}");
            let v  = compute_view(&g, &ViewFilter::default());
            let cx = EvalCtx::from_view(&g, &v);
            let hit: Vec<usize> = (0..g.node_list.len()).filter(|&i| q.matches(&cx, i)).collect();
            assert_eq!(hit.len(), usize::from(src != "\"rust"), "{src}: {hit:?}");
        }
        let (q, e) = Query::search("tag:x");
        assert!(e.is_none() && q.src == "tag:x");
        // still errors where the text can only be a mistake
        assert!(Query::parse("tag:").is_err() && Query::parse("tag:(").is_err());
    }

    /// path: takes a path: a leading '/' is a regex only when the regex is
    /// the whole value.
    #[test]
    fn absolute_paths_in_path_and_file() {
        let g = fixture();
        assert_eq!(ids(&g, "path:/n/posts"), vec!["p", "q", "x"], "a substring of the path, like path:posts");
        assert_eq!(ids(&g, "path:/n/posts/"), vec!["p", "q"]);
        assert_eq!(ids(&g, "path:/n/posts/first.org"), vec!["p"]);
        assert_eq!(ids(&g, "path:/tmp"), Vec::<String>::new(), "unterminated: still a path, no error");
        assert_eq!(ids(&g, "path:/deep/"), vec!["q"], "a whole /regex/ is still one");
        assert_eq!(ids(&g, "path:/posts/ level:0"), vec!["p", "q", "x"]);
        assert_eq!(ids(&g, "title:/^post/"), vec!["x"]);
        assert_eq!(ids(&g, "/n/posts"), Vec::<String>::new(), "bare: a word, not regex + word");
    }

    /// The notes root in an absolute dir: compares in any case (drive
    /// letters, a typed ~/Notes).
    #[test]
    fn absolute_dir_ignores_case_in_the_root() {
        let g = Graph::new(vec![node("a", "A", "c:/Users/x/notes/a.org", &[]), node("p", "P", "c:/Users/x/notes/posts/p.org", &[])], vec![]);
        assert_eq!(ids(&g, "dir:c:/Users/x/notes/posts"), vec!["p"]);
        assert_eq!(ids(&g, r#"dir:"C:\Users\x\notes\posts""#), vec!["p"]);
        assert_eq!(ids(&g, "dir:C:/users/X/Notes/Posts"), vec!["p"]);
        assert_eq!(ids(&g, "dir:C:/Users/x/notes"), vec!["a", "p"]);
    }

    #[test]
    fn escapes_inside_quotes_and_regexes() {
        let g = fixture();
        assert_eq!(ids(&g, r"title:/\(draft\)/"), vec!["o"], "regex escapes reach the engine");
        assert!(Query::parse(r#"title:"say \"hi\"""#).is_ok());
        assert!(Query::parse(r"title:/a\/b/").is_ok(), "escaped slash inside a regex");
    }
}
