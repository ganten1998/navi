use egui::{Align2, Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, epaint::FontId};
use navi_core::{groups::{centroid, convex_hull, padded_hull}, Graph};
use std::cell::{Ref, RefCell};
use crate::theme::Theme;

// ── Age ───────────────────────────────────────────────────────────────────────

pub fn age_days(mtime: i64, now_secs: f64) -> f64 {
    ((now_secs - mtime as f64) / 86400.0).max(0.0)
}

pub fn age_stage(mtime: i64, now_secs: f64) -> usize {
    const BREAKS: &[f64] = &[7.0, 30.0, 90.0, 270.0, 540.0];
    let d = age_days(mtime, now_secs);
    for (i, &t) in BREAKS.iter().enumerate() { if d <= t { return i; } }
    BREAKS.len()
}


// Continuous age parameter: 0.0 = just created, 1.0 = fully settled.
// Exponential approach with ~180-day half-life so notes feel fresh for weeks,
// visibly settled over months, deeply integrated past a year.
fn compute_age_t(mtime: i64, now_secs: f64) -> f32 {
    let days = age_days(mtime, now_secs) as f32;
    1.0 - (-days / 180.0).exp()
}

// ── Color helpers ─────────────────────────────────────────────────────────────

fn lerp_col(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t).round() as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t).round() as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t).round() as u8,
    )
}

// C: Temperature drift.
// Fresh end: 18% blend toward the theme's warm accent (node_sel — amber or cyan
//   complement), so the colour reads as "active/present" in the palette's own language.
// Settled end: 40% blend toward the background, so old nodes visibly sink into
//   the canvas — clearly darker, clearly cooler, clearly integrated.
fn age_color(base: Color32, bg: Color32, warm_accent: Color32, t: f32) -> Color32 {
    if t < 0.35 {
        // Fresh zone: warm-accent tint fades back to true colour
        let u = t / 0.35;
        lerp_col(lerp_col(base, warm_accent, 0.18), base, u)
    } else {
        // Settled zone: true colour drifts toward background (max 40%)
        lerp_col(base, bg, 0.40 * ((t - 0.35) / 0.65))
    }
}

fn brighten(c: Color32, amount: f32) -> Color32 {
    Color32::from_rgb(
        (c.r() as f32 + (255.0 - c.r() as f32) * amount) as u8,
        (c.g() as f32 + (255.0 - c.g() as f32) * amount) as u8,
        (c.b() as f32 + (255.0 - c.b() as f32) * amount) as u8,
    )
}

fn dim(c: Color32, amt: u8) -> Color32 {
    Color32::from_rgb(
        c.r().saturating_sub(amt),
        c.g().saturating_sub(amt),
        c.b().saturating_sub(amt),
    )
}

// ── Painter ───────────────────────────────────────────────────────────────────

/// A group whose hull is drawn: its colour, label and member nodes (the
/// painter re-checks visibility and positions each frame).
pub struct HullDraw {
    pub colour:  Color32,
    pub label:   String,
    pub members: Vec<usize>,
    geom:        RefCell<Option<HullGeom>>, // the last shape, while nothing moved
}

/// A hull's shape in world space and where its label goes.
pub struct HullGeom {
    key:          u64,              // fingerprint of the visible members' positions
    core:         Vec<(f64, f64)>,  // their convex hull, unpadded
    centre:       (f64, f64),
    pad:          f64,              // what `poly` is padded by (it follows the zoom)
    pub poly:     Vec<(f64, f64)>,
    pub label_at: (f64, f64),
    pub shown:    usize,            // visible members
}

fn mix(h: u64, v: u64) -> u64 { (h ^ v).wrapping_mul(0x0100_0000_01b3) }

impl HullDraw {
    pub fn new(colour: Color32, label: String, members: Vec<usize>) -> Self {
        HullDraw { colour, label, members, geom: RefCell::new(None) }
    }

    /// The padded hull of the visible members (None: none visible, or the
    /// hull would lie outside `view`, a world-space (min, max) box, checked
    /// on the members' bounds before anything is built). The members' own
    /// hull is rebuilt only when one moved, appeared or left; a zoom (a new
    /// `pad`) only re-pads it. A resting graph costs one pass over the
    /// members. One member: the label sits above its circle, not under it.
    pub fn geometry(&self, g: &Graph, pad: f64, view: Option<((f64, f64), (f64, f64))>) -> Option<Ref<'_, HullGeom>> {
        let mut key = 0xcbf2_9ce4_8422_2325;
        let mut shown = 0;
        let (mut lo, mut hi) = ((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY));
        for &i in self.members.iter().filter(|&&i| g.is_visible(i)) {
            let nd = &g.node_list[i];
            key = mix(mix(mix(key, i as u64), nd.x.to_bits()), nd.y.to_bits());
            lo = (lo.0.min(nd.x), lo.1.min(nd.y));
            hi = (hi.0.max(nd.x), hi.1.max(nd.y));
            shown += 1;
        }
        if shown == 0 { return None; }
        if let Some((vlo, vhi)) = view {
            let off = hi.0 + pad < vlo.0 || lo.0 - pad > vhi.0 || hi.1 + pad < vlo.1 || lo.1 - pad > vhi.1;
            if off { return None; }
        }
        let mut cell = self.geom.borrow_mut();
        if !matches!(cell.as_ref(), Some(h) if h.key == key) {
            let pts: Vec<(f64, f64)> = self.members.iter().copied().filter(|&i| g.is_visible(i))
                .map(|i| (g.node_list[i].x, g.node_list[i].y)).collect();
            let centre = centroid(&pts).unwrap_or_default();
            *cell = Some(HullGeom { key, core: convex_hull(&pts), centre, pad: f64::NAN, poly: Vec::new(), label_at: centre, shown });
        }
        let h = cell.as_mut().expect("just built");
        if h.pad.to_bits() != pad.to_bits() {
            h.poly     = padded_hull(&h.core, pad, 16);
            h.label_at = if h.shown >= 2 { h.centre } else { (h.centre.0, h.centre.1 - pad) };
            h.pad      = pad;
        }
        drop(cell);
        Some(Ref::map(self.geom.borrow(), |h| h.as_ref().expect("just built")))
    }
}

pub struct GraphPainter<'a> {
    pub graph:      &'a Graph,
    pub theme:      Theme,
    pub pan:        Vec2,
    pub zoom:       f32,
    pub hovered:    Option<usize>,   // node indices; visibility comes from graph.visible
    pub selected:   Option<usize>,
    pub faded:      &'a [bool],      // search / local-graph fade, per node
    pub hov_adj:    &'a [bool],      // hovered node + its visible neighbours, per node
    pub node_colours: &'a [Option<Color32>], // group colour per node (None: the theme's node colour)
    pub hulls:      &'a [HullDraw],
    pub show_age:   bool,
    pub now_secs:   f64,
    pub graph_rect: Rect,
}

impl<'a> GraphPainter<'a> {
    pub fn w2s(&self, wx: f64, wy: f64) -> Pos2 {
        let cx = self.graph_rect.left() + self.graph_rect.width()  / 2.0;
        let cy = self.graph_rect.top()  + self.graph_rect.height() / 2.0;
        Pos2::new(cx + (wx as f32) * self.zoom + self.pan.x,
                  cy + (wy as f32) * self.zoom + self.pan.y)
    }

    pub fn s2w(&self, sx: f32, sy: f32) -> (f64, f64) {
        let cx = self.graph_rect.left() + self.graph_rect.width()  / 2.0;
        let cy = self.graph_rect.top()  + self.graph_rect.height() / 2.0;
        (((sx - cx - self.pan.x) / self.zoom) as f64,
         ((sy - cy - self.pan.y) / self.zoom) as f64)
    }

    fn node_base_colors(&self, idx: usize) -> (Color32, Color32) {
        let t = &self.theme;
        if let Some(Some(c)) = self.node_colours.get(idx) {
            return (*c, brighten(*c, 0.22));
        }
        (t.node, t.node_rim)
    }

    fn node_colors(&self, idx: usize, is_sel: bool, is_hov: bool, is_fade: bool) -> (Color32, Color32) {
        let t = &self.theme;
        if is_hov && !is_sel { return (t.node_rim, t.node_hov); }
        let (body, rim) = self.node_base_colors(idx);
        if is_fade { return (dim(rim, 70), dim(body, 70)); }
        (rim, body)
    }

    pub fn paint_grid(&self, painter: &Painter) {
        let t    = &self.theme;
        let rect = self.graph_rect;
        let col  = Color32::from_rgba_unmultiplied(t.grid.r(), t.grid.g(), t.grid.b(), 200);
        let sp   = (12.0 * self.zoom).max(3.0);
        let cx   = rect.left() + rect.width()  / 2.0 + self.pan.x;
        let cy   = rect.top()  + rect.height() / 2.0 + self.pan.y;
        let ox   = ((cx % sp) + sp) % sp;
        let oy   = ((cy % sp) + sp) % sp;
        let s    = Stroke::new(0.5, col);
        let mut x = rect.left() + ox - sp;
        while x <= rect.right() + sp {
            painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], s);
            x += sp;
        }
        let mut y = rect.top() + oy - sp;
        while y <= rect.bottom() + sp {
            painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], s);
            y += sp;
        }
    }

    /// Faint padded convex hulls behind the nodes, one per group with its
    /// hull on, over the group's visible members; the label sits at the
    /// members' centroid (above a lone member). The padding covers node
    /// radius plus the breathing wobble (4 px on screen).
    pub fn paint_hulls(&self, painter: &Painter) {
        let r   = self.graph_rect.expand(40.0);
        let pad = (navi_core::graph::R_MAX as f64 + 10.0) + 6.0 / self.zoom.max(0.01) as f64;
        // off-screen hulls are culled on their members' bounds, before any is built
        let (a, b) = (self.s2w(r.left(), r.top()), self.s2w(r.right(), r.bottom()));
        let view = ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)));
        for h in self.hulls {
            let Some(geo) = h.geometry(self.graph, pad, Some(view)) else { continue };
            let poly: Vec<Pos2> = geo.poly.iter().map(|&(x, y)| self.w2s(x, y)).collect();
            let bb = Rect::from_points(&poly);
            if !bb.intersects(r) { continue; }
            let c = h.colour;
            painter.add(Shape::convex_polygon(poly,
                Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 20),
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 70))));
            if self.zoom > 0.12 {
                let align = if geo.shown >= 2 { Align2::CENTER_CENTER } else { Align2::CENTER_BOTTOM };
                painter.text(self.w2s(geo.label_at.0, geo.label_at.1), align, &h.label, FontId::proportional(15.0),
                    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 110));
            }
        }
    }

    pub fn paint_edges(&self, painter: &Painter) {
        let t        = &self.theme;
        let edge_col = Color32::from_rgba_unmultiplied(t.edge.r(),    t.edge.g(),    t.edge.b(),    190);
        let edge_hi  = Color32::from_rgba_unmultiplied(t.edge_hi.r(), t.edge_hi.g(), t.edge_hi.b(), 255);
        let fade = |i: usize| self.faded.get(i).copied().unwrap_or(false);
        let adj  = |i: usize| self.hov_adj.get(i).copied().unwrap_or(false);
        for &(si, di) in &self.graph.vis_edges {
            let a = &self.graph.node_list[si];
            let b = &self.graph.node_list[di];
            if fade(si) && fade(di) { continue; }
            let p1 = self.w2s(a.x, a.y);
            let p2 = self.w2s(b.x, b.y);
            let r  = self.graph_rect;
            if p1.x < r.left()-10.0   && p2.x < r.left()-10.0   { continue; }
            if p1.x > r.right()+10.0  && p2.x > r.right()+10.0  { continue; }
            if p1.y < r.top()-10.0    && p2.y < r.top()-10.0    { continue; }
            if p1.y > r.bottom()+10.0 && p2.y > r.bottom()+10.0 { continue; }
            // hov_adj holds the hovered node itself, so this covers edges at it too
            let hov = adj(si) && adj(di);
            painter.line_segment([p1, p2], Stroke::new(1.0, if hov { edge_hi } else { edge_col }));
        }
    }

    pub fn paint_nodes(&self, painter: &Painter) {
        for &node_idx in &self.graph.vis_idx {
            let nd = &self.graph.node_list[node_idx];
            let sc = self.w2s(nd.x, nd.y);
            // Visual breathing — purely cosmetic screen-space offset, actual world
            // position (used for hit detection, edges, physics) is unchanged.
            let phase = node_idx as f32 * 1.618033988749895_f32;
            let bt    = self.now_secs as f32;
            let dsc   = sc + Vec2::new(
                (bt * 0.32 + phase).sin()            * 4.0,
                (bt * 0.27 + phase * 1.272).cos()    * 4.0,
            );
            let pr = (nd.radius * self.zoom).max(2.0);
            let r  = self.graph_rect;
            if sc.x + pr*4.0 < r.left()-10.0   || sc.x - pr*4.0 > r.right()+10.0  { continue; }
            if sc.y + pr*4.0 < r.top()-10.0    || sc.y - pr*4.0 > r.bottom()+10.0 { continue; }

            let is_sel  = Some(node_idx) == self.selected;
            let is_hov  = Some(node_idx) == self.hovered;
            let is_fade = self.faded.get(node_idx).copied().unwrap_or(false)
                || (self.hovered.is_some() && !self.hov_adj.get(node_idx).copied().unwrap_or(false) && !is_sel && !is_hov);

            let (rim_col, body_col) = self.node_colors(node_idx, is_sel, is_hov, is_fade);

            // Age parameter — bypassed for selected/hovered so they always read true colour
            let age_t = if !is_sel && !is_hov && self.show_age && nd.mtime > 0 {
                compute_age_t(nd.mtime, self.now_secs)
            } else {
                0.0_f32
            };

            // C: temperature drift — warm accent tint when fresh, sinks toward bg when settled
            let wa   = self.theme.node_sel; // warm accent (amber, cyan, etc. per theme)
            let body = age_color(body_col, self.theme.bg, wa, age_t);
            let rim  = age_color(rim_col,  self.theme.bg, wa, age_t);

            // Selection ring — tight accent just outside the node
            if is_sel {
                let sc = self.theme.node_selr;
                painter.circle_stroke(dsc, pr * 1.28, Stroke::new(1.8,
                    Color32::from_rgba_unmultiplied(sc.r(), sc.g(), sc.b(), 195)));
            }

            // Glow fades with age so settled nodes don't radiate
            let glow_base = if is_fade { 3u8 } else { 9 };
            let glow_a    = (glow_base as f32 * (1.0 - age_t * 0.75)) as u8;
            painter.circle_filled(dsc, pr * 1.18,
                Color32::from_rgba_unmultiplied(rim.r(), rim.g(), rim.b(), glow_a));

            painter.circle_filled(dsc, pr, body);

            // D: rim softness — crisp when fresh, dissolves as node settles
            let rim_a = (255.0 * (1.0 - age_t).powf(0.55)) as u8;
            let rim_w = (pr * 0.25).max(1.0).min(3.0) * (1.0 - age_t * 0.40).max(0.0);
            if rim_a > 4 && rim_w > 0.3 {
                painter.circle_stroke(dsc, pr, Stroke::new(rim_w,
                    Color32::from_rgba_unmultiplied(rim.r(), rim.g(), rim.b(), rim_a)));
            }

            // F: highlight depth — present surface when fresh, flattens with age
            if pr >= 5.0 {
                let hl_a = (28.0 * (1.0 - age_t).powf(0.45)) as u8;
                if hl_a > 2 {
                    painter.circle_filled(
                        dsc + Vec2::new(-pr * 0.27, -pr * 0.27),
                        (pr * 0.28).max(1.0),
                        Color32::from_rgba_unmultiplied(255, 255, 255, hl_a),
                    );
                }
            }

            // Inner ring for headline nodes
            if nd.level > 0 && pr >= 5.0 {
                painter.circle_stroke(dsc, pr * 0.33, Stroke::new(1.0, rim));
            }
        }
    }

    pub fn paint_labels(&self, painter: &Painter) {
        let t = &self.theme;
        if self.zoom < 0.25 { return; }
        let alpha_base = (((self.zoom - 0.25) / 0.35) * 255.0).min(255.0) as u8;
        let font = FontId::proportional(13.0);
        // Adaptive truncation: long titles overlap at low zoom
        let max_chars: usize = if self.zoom < 0.55 { 8 }
                               else if self.zoom < 1.0 { 15 }
                               else { usize::MAX };
        for &node_idx in &self.graph.vis_idx {
            let nd = &self.graph.node_list[node_idx];
            let sc = self.w2s(nd.x, nd.y);
            // Match breathing offset so label stays under its node
            let phase = node_idx as f32 * 1.618033988749895_f32;
            let bt    = self.now_secs as f32;
            let dsc   = sc + Vec2::new(
                (bt * 0.32 + phase).sin()         * 4.0,
                (bt * 0.27 + phase * 1.272).cos() * 4.0,
            );
            let r = self.graph_rect;
            if dsc.x < r.left()-200.0 || dsc.x > r.right()+200.0 { continue; }
            if dsc.y < r.top()-20.0   || dsc.y > r.bottom()+20.0  { continue; }
            let is_hi   = Some(node_idx) == self.hovered || Some(node_idx) == self.selected;
            let is_fade = self.faded.get(node_idx).copied().unwrap_or(false)
                || (self.hovered.is_some() && !self.hov_adj.get(node_idx).copied().unwrap_or(false) && !is_hi);
            let alpha = if is_fade { (alpha_base / 5).max(0) } else { alpha_base };
            if alpha < 5 { continue; }
            let base_col = if is_hi { t.label_hov } else { t.label };
            let col  = Color32::from_rgba_unmultiplied(base_col.r(), base_col.g(), base_col.b(), alpha);
            let pr   = (nd.radius * self.zoom).max(2.0);
            let title = if nd.title.chars().count() > max_chars {
                nd.title.chars().take(max_chars).collect::<String>() + "…"
            } else {
                nd.title.clone()
            };
            painter.text(Pos2::new(dsc.x, dsc.y + pr + 8.0),
                egui::Align2::CENTER_TOP, &title, font.clone(), col);
        }
    }

    pub fn node_at(&self, sx: f32, sy: f32) -> Option<usize> {
        let mut best_d = f32::INFINITY;
        let mut best: Option<usize> = None;
        for &i in &self.graph.vis_idx {
            let nd = &self.graph.node_list[i];
            let sc = self.w2s(nd.x, nd.y);
            let d  = (sc - Pos2::new(sx, sy)).length();
            let hr = (nd.radius * self.zoom).max(9.0);
            if d <= hr && d < best_d { best_d = d; best = Some(i); }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::{compute_view, RawNode, ViewFilter};

    fn graph() -> Graph {
        let node = |id: &str| RawNode { id: id.into(), title: id.into(), file: format!("/n/{id}.org"), ..Default::default() };
        let mut g = Graph::new(vec![node("a"), node("b"), node("c")], vec![("a".into(), "b".into())]);
        let v = compute_view(&g, &ViewFilter::default());
        g.apply_view(&v);
        g
    }

    /// Nothing moved: the hull is not rebuilt. A member moved or the zoom
    /// (padding) changed: it is.
    #[test]
    fn hull_geometry_is_cached_until_something_moves() {
        let mut g = graph();
        let h = HullDraw::new(Color32::RED, "ab".into(), vec![0, 1]);
        let first = h.geometry(&g, 30.0, None).unwrap().poly.as_ptr();
        assert_eq!(h.geometry(&g, 30.0, None).unwrap().poly.as_ptr(), first, "same positions: the cached shape");
        g.node_list[1].x += 5.0;
        let moved = h.geometry(&g, 30.0, None).unwrap().poly.clone();
        assert!(moved.iter().any(|p| p.0 > g.node_list[1].x), "rebuilt around the new position");
        let padded = h.geometry(&g, 60.0, None).unwrap().poly.clone();
        assert_ne!(padded, moved, "a new padding rebuilds");
        assert!(HullDraw::new(Color32::RED, "none".into(), vec![]).geometry(&g, 30.0, None).is_none());
    }

    /// A hull off screen is culled on its members' bounds before anything
    /// is built; a zoom (a new padding) re-pads the cached core, the same
    /// shape a full rebuild gives.
    #[test]
    fn off_screen_hulls_are_never_built() {
        let g = graph();
        let h = HullDraw::new(Color32::RED, "ab".into(), vec![0, 1]);
        let (a, b) = (&g.node_list[0], &g.node_list[1]);
        let far = ((a.x.max(b.x) + 1000.0, 0.0), (a.x.max(b.x) + 2000.0, 10.0));
        assert!(h.geometry(&g, 30.0, Some(far)).is_none());
        assert!(h.geom.borrow().is_none(), "nothing was built");
        let near = ((a.x.min(b.x) - 1.0, a.y.min(b.y) - 1.0), (a.x.max(b.x) + 1.0, a.y.max(b.y) + 1.0));
        assert!(h.geometry(&g, 30.0, Some(near)).is_some());
        let just_out = ((a.x.max(b.x) + 29.0, a.y), (a.x.max(b.x) + 40.0, a.y + 1.0));
        assert!(h.geometry(&g, 30.0, Some(just_out)).is_some(), "the padding counts");
        let zoomed = h.geometry(&g, 55.0, None).unwrap().poly.clone();
        assert_eq!(zoomed, padded_hull(&[(a.x, a.y), (b.x, b.y)], 55.0, 16));
    }

    /// A one-member group's label sits above its circle, not under the node.
    #[test]
    fn a_lone_members_label_sits_above_it() {
        let g = graph();
        let h = HullDraw::new(Color32::RED, "#solo".into(), vec![2]);
        let geo = h.geometry(&g, 30.0, None).unwrap();
        let nd = &g.node_list[2];
        assert_eq!(geo.shown, 1);
        assert_eq!(geo.label_at, (nd.x, nd.y - 30.0));
        let two = HullDraw::new(Color32::RED, "pair".into(), vec![0, 1]);
        let (a, b) = (&g.node_list[0], &g.node_list[1]);
        assert_eq!(two.geometry(&g, 30.0, None).unwrap().label_at, ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0));
    }
}
