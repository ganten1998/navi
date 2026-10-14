use egui::Color32;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub bg:        Color32,
    pub grid:      Color32,
    pub edge:      Color32,
    pub edge_hi:   Color32,
    pub node:      Color32,
    pub node_rim:  Color32,
    pub node_hov:  Color32,
    pub node_sel:  Color32,
    pub node_selr: Color32,
    pub label:     Color32,
    pub label_hov: Color32,
    pub bar_bg:    Color32,
    pub bar_line:  Color32,
    pub bar_text:  Color32,
}

impl Theme {
    pub fn glow(&self) -> Color32 {
        let c = self.node;
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 28)
    }
}

// ── Navi palette ──────────────────────────────────────────────────────────────
// Source: macro photo of a black butterfly on vivid blue salvia flowers with a
// glowing cyan bokeh orb.  All five Navi variants share the same background
// architecture, bar, labels, and amber complement anchor.  Only nodes, edges,
// and highlights differ — each drawn from a different family in the palette.

// Shared background infrastructure (same across all Navi variants)
//   bg       #101e2e  Deep Night Blue    (btop main_bg)
//   grid     #192e48  Dark Cerulean      (btop meter_bg — one luminance step up)
//   bar_bg   #0a1c34  Oxford Blue        (btop inactive_bg)
//   bar_line #265080  Payne's Grey blue  (btop div_line)
//   bar_text #a8c8d8  Powder Blue        (btop graph_text)
//   label    #7898b8  Cadet Grey         (btop inactive_fg)
//   label_hov#e0f0ff  Alice Blue         (btop main_fg)
//   node_sel #f09030  Amber              (complement anchor — the one warm hue)

pub const THEMES: &[Theme] = &[
    // ── 0 · Salvia ────────────────────────────────────────────────────────────
    // Periwinkle nodes / Electric Cyan ghost-light rim.
    // The salvia flowers (electric periwinkle #5578ff) meet the bokeh orb
    // (#40e8ff) — the signature Navi look.
    Theme {
        name: "Salvia",
        bg:        Color32::from_rgb( 16,  30,  46),  // #101e2e Deep Night Blue
        grid:      Color32::from_rgb( 25,  46,  72),  // #192e48 Dark Cerulean
        edge:      Color32::from_rgb( 38,  80, 128),  // #265080 Payne's Grey blue
        edge_hi:   Color32::from_rgb( 64, 232, 255),  // #40e8ff Electric Cyan
        node:      Color32::from_rgb( 85, 120, 255),  // #5578ff Electric Periwinkle
        node_rim:  Color32::from_rgb( 64, 232, 255),  // #40e8ff Electric Cyan
        node_hov:  Color32::from_rgb(  0, 216, 248),  // #00d8f8 Vivid Cerulean
        node_sel:  Color32::from_rgb(240, 144,  48),  // #f09030 Amber
        node_selr: Color32::from_rgb(156, 242, 255),  // #9cf2ff soft cyan — rim brightened
        label:     Color32::from_rgb(120, 152, 184),  // #7898b8 Cadet Grey
        label_hov: Color32::from_rgb(224, 240, 255),  // #e0f0ff Alice Blue
        bar_bg:    Color32::from_rgb( 10,  28,  52),  // #0a1c34 Oxford Blue
        bar_line:  Color32::from_rgb( 38,  80, 128),  // #265080 Payne's Grey blue
        bar_text:  Color32::from_rgb(168, 200, 216),  // #a8c8d8 Powder Blue
    },
    // ── 1 · Bokeh ─────────────────────────────────────────────────────────────
    // Pure cyan family — the glowing bokeh orb made into nodes.
    // Pacific Blue nodes (#00b8d8) ringed in Electric Cyan (#40e8ff).
    // Edges pull from Deep Teal (#023040), the darkest cyan bg in the palette.
    Theme {
        name: "Bokeh",
        bg:        Color32::from_rgb( 16,  30,  46),  // #101e2e
        grid:      Color32::from_rgb( 25,  46,  72),  // #192e48
        edge:      Color32::from_rgb( 26,  96, 112),  // #1a6070 mid teal
        edge_hi:   Color32::from_rgb( 64, 232, 255),  // #40e8ff Electric Cyan
        node:      Color32::from_rgb(  0, 184, 216),  // #00b8d8 Pacific Blue
        node_rim:  Color32::from_rgb( 64, 232, 255),  // #40e8ff Electric Cyan
        node_hov:  Color32::from_rgb(  0, 216, 248),  // #00d8f8 Vivid Cerulean
        node_sel:  Color32::from_rgb(240, 144,  48),  // #f09030 Amber
        node_selr: Color32::from_rgb(156, 242, 255),  // #9cf2ff soft cyan — rim brightened
        label:     Color32::from_rgb(120, 152, 184),  // #7898b8 Cadet Grey
        label_hov: Color32::from_rgb(224, 240, 255),  // #e0f0ff Alice Blue
        bar_bg:    Color32::from_rgb( 10,  28,  52),  // #0a1c34 Oxford Blue
        bar_line:  Color32::from_rgb( 38,  80, 128),  // #265080 Payne's Grey blue
        bar_text:  Color32::from_rgb(168, 200, 216),  // #a8c8d8 Powder Blue
    },
    // ── 2 · Stem ──────────────────────────────────────────────────────────────
    // The cool stem greens beneath the salvia flowers.
    // Caribbean Green nodes (#00e898) / Aquamarine rim (#00f0b0).
    // Edges from Hunter Green (#052818) — darkest green bg in the palette.
    Theme {
        name: "Stem",
        bg:        Color32::from_rgb( 16,  30,  46),  // #101e2e
        grid:      Color32::from_rgb( 25,  46,  72),  // #192e48
        edge:      Color32::from_rgb( 20, 100,  56),  // #146438 mid green
        edge_hi:   Color32::from_rgb(  0, 240, 176),  // #00f0b0 Aquamarine
        node:      Color32::from_rgb(  0, 232, 152),  // #00e898 Caribbean Green
        node_rim:  Color32::from_rgb(  0, 240, 176),  // #00f0b0 Aquamarine
        node_hov:  Color32::from_rgb( 64, 216, 112),  // #40d870 Malachite
        node_sel:  Color32::from_rgb(240, 144,  48),  // #f09030 Amber
        node_selr: Color32::from_rgb(128, 248, 216),  // #80f8d8 soft aqua — rim brightened
        label:     Color32::from_rgb(120, 152, 184),  // #7898b8 Cadet Grey
        label_hov: Color32::from_rgb(224, 240, 255),  // #e0f0ff Alice Blue
        bar_bg:    Color32::from_rgb( 10,  28,  52),  // #0a1c34 Oxford Blue
        bar_line:  Color32::from_rgb( 38,  80, 128),  // #265080 Payne's Grey blue
        bar_text:  Color32::from_rgb(168, 200, 216),  // #a8c8d8 Powder Blue
    },
    // ── 3 · Petal ─────────────────────────────────────────────────────────────
    // The iridescent wing of the black butterfly — violet and magenta.
    // Amethyst nodes (#a868d8) / Electric Violet rim (#c040f8).
    // Edges from Violet Black (#1e0a48) — darkest magenta bg in the palette.
    Theme {
        name: "Petal",
        bg:        Color32::from_rgb( 16,  30,  46),  // #101e2e
        grid:      Color32::from_rgb( 25,  46,  72),  // #192e48
        edge:      Color32::from_rgb( 72,  24, 144),  // #481890 Dark Indigo
        edge_hi:   Color32::from_rgb(224,  48, 255),  // #e030ff Electric Magenta
        node:      Color32::from_rgb(168, 104, 216),  // #a868d8 Amethyst
        node_rim:  Color32::from_rgb(192,  64, 248),  // #c040f8 Electric Violet
        node_hov:  Color32::from_rgb(224,  48, 255),  // #e030ff Electric Magenta
        node_sel:  Color32::from_rgb(240, 144,  48),  // #f09030 Amber
        node_selr: Color32::from_rgb(223, 159, 251),  // #df9ffb soft lavender — rim brightened
        label:     Color32::from_rgb(120, 152, 184),  // #7898b8 Cadet Grey
        label_hov: Color32::from_rgb(224, 240, 255),  // #e0f0ff Alice Blue
        bar_bg:    Color32::from_rgb( 10,  28,  52),  // #0a1c34 Oxford Blue
        bar_line:  Color32::from_rgb( 38,  80, 128),  // #265080 Payne's Grey blue
        bar_text:  Color32::from_rgb(168, 200, 216),  // #a8c8d8 Powder Blue
    },
    // ── 4 · Azure ─────────────────────────────────────────────────────────────
    // The deeper blue-cooler family — calmer than Salvia, still fully Navi.
    // Cobalt Blue nodes (#3d7fff) / Azure Blue rim (#1a9aff).
    // Edges from Midnight Indigo (#082060).
    Theme {
        name: "Azure",
        bg:        Color32::from_rgb( 16,  30,  46),  // #101e2e
        grid:      Color32::from_rgb( 25,  46,  72),  // #192e48
        edge:      Color32::from_rgb( 24,  56, 160),  // #1838a0 Ultramarine
        edge_hi:   Color32::from_rgb( 26, 154, 255),  // #1a9aff Azure Blue
        node:      Color32::from_rgb( 61, 127, 255),  // #3d7fff Cobalt Blue
        node_rim:  Color32::from_rgb( 26, 154, 255),  // #1a9aff Azure Blue
        node_hov:  Color32::from_rgb( 64, 232, 255),  // #40e8ff Electric Cyan
        node_sel:  Color32::from_rgb(240, 144,  48),  // #f09030 Amber
        node_selr: Color32::from_rgb(140, 204, 255),  // #8cccff soft sky blue — rim brightened
        label:     Color32::from_rgb(120, 152, 184),  // #7898b8 Cadet Grey
        label_hov: Color32::from_rgb(224, 240, 255),  // #e0f0ff Alice Blue
        bar_bg:    Color32::from_rgb( 10,  28,  52),  // #0a1c34 Oxford Blue
        bar_line:  Color32::from_rgb( 38,  80, 128),  // #265080 Payne's Grey blue
        bar_text:  Color32::from_rgb(168, 200, 216),  // #a8c8d8 Powder Blue
    },
    // ── 5 · Ruby ──────────────────────────────────────────────────────────────
    // Fully independent background — wine-dark near-black (#1a0810) so the
    // crimson nodes feel emitted rather than placed.  Cool-shifted ruby crimson
    // (192, 24, 64) follows the palette rule: reds bent toward blue.
    // Rim: Flamingo (#f85888).  Connections: Claret (#6a1018).
    // Selection flips the complement — Electric Cyan (#40e8ff) glows against
    // warm red the same way amber glows against cool blue in the other variants.
    Theme {
        name: "Ruby",
        bg:        Color32::from_rgb( 26,   8,  16),  // #1a0810 wine-dark near-black
        grid:      Color32::from_rgb( 56,  16,  28),  // #38101c dark maroon
        edge:      Color32::from_rgb(106,  16,  24),  // #6a1018 Claret
        edge_hi:   Color32::from_rgb(255,  96,  96),  // #ff6060 Bittersweet
        node:      Color32::from_rgb(192,  24,  64),  // deep ruby crimson
        node_rim:  Color32::from_rgb(248,  88, 136),  // #f85888 Flamingo
        node_hov:  Color32::from_rgb(255,  96,  96),  // #ff6060 Bittersweet
        node_sel:  Color32::from_rgb( 64, 232, 255),  // #40e8ff Electric Cyan — cool complement
        node_selr: Color32::from_rgb(251, 168, 192),  // #fba8c0 soft rose — rim brightened
        label:     Color32::from_rgb(184, 136, 152),  // warm rose-grey
        label_hov: Color32::from_rgb(240, 220, 228),  // warm near-white
        bar_bg:    Color32::from_rgb( 16,   4,   8),  // #100408 near-black
        bar_line:  Color32::from_rgb(106,  16,  24),  // #6a1018 Claret
        bar_text:  Color32::from_rgb(200, 120, 120),  // #c87878 Dusty Rose
    },
    // ── Legacy themes ─────────────────────────────────────────────────────────
    Theme {
        name: "Obsidian",
        bg:        Color32::from_rgb( 13,  13,  20),
        grid:      Color32::from_rgb( 48,  48,  75),
        edge:      Color32::from_rgb( 60,  60, 100),
        edge_hi:   Color32::from_rgb(160, 110, 255),
        node:      Color32::from_rgb(124,  77, 255),
        node_rim:  Color32::from_rgb(175, 140, 255),
        node_hov:  Color32::from_rgb(185, 155, 255),
        node_sel:  Color32::from_rgb(255, 200,  40),
        node_selr: Color32::from_rgb(212, 192, 255),  // #d4c0ff soft pale purple — rim brightened
        label:     Color32::from_rgb(185, 180, 215),
        label_hov: Color32::from_rgb(255, 255, 255),
        bar_bg:    Color32::from_rgb( 18,  18,  30),
        bar_line:  Color32::from_rgb( 48,  48,  75),
        bar_text:  Color32::from_rgb(140, 135, 175),
    },
    Theme {
        name: "Forest",
        bg:        Color32::from_rgb(  8,  18,  10),
        grid:      Color32::from_rgb( 32,  60,  36),
        edge:      Color32::from_rgb( 40,  80,  50),
        edge_hi:   Color32::from_rgb( 80, 220, 100),
        node:      Color32::from_rgb( 50, 180,  80),
        node_rim:  Color32::from_rgb(100, 220, 130),
        node_hov:  Color32::from_rgb( 80, 200, 110),
        node_sel:  Color32::from_rgb(255, 200,  40),
        node_selr: Color32::from_rgb(176, 238, 192),  // #b0eec0 soft mint — rim brightened
        label:     Color32::from_rgb(170, 210, 175),
        label_hov: Color32::from_rgb(220, 255, 220),
        bar_bg:    Color32::from_rgb( 12,  24,  14),
        bar_line:  Color32::from_rgb( 30,  60,  35),
        bar_text:  Color32::from_rgb(120, 175, 130),
    },
    Theme {
        name: "Ocean",
        bg:        Color32::from_rgb(  8,  14,  28),
        grid:      Color32::from_rgb( 28,  50,  88),
        edge:      Color32::from_rgb( 30,  60, 110),
        edge_hi:   Color32::from_rgb( 60, 150, 255),
        node:      Color32::from_rgb( 40, 130, 220),
        node_rim:  Color32::from_rgb( 80, 170, 250),
        node_hov:  Color32::from_rgb( 60, 155, 240),
        node_sel:  Color32::from_rgb(255, 200,  40),
        node_selr: Color32::from_rgb(168, 212, 252),  // #a8d4fc soft periwinkle — rim brightened
        label:     Color32::from_rgb(160, 190, 225),
        label_hov: Color32::from_rgb(200, 230, 255),
        bar_bg:    Color32::from_rgb( 12,  20,  38),
        bar_line:  Color32::from_rgb( 28,  50,  90),
        bar_text:  Color32::from_rgb(100, 145, 195),
    },
    Theme {
        name: "Ember",
        bg:        Color32::from_rgb( 20,  10,   8),
        grid:      Color32::from_rgb( 68,  34,  26),
        edge:      Color32::from_rgb(100,  40,  20),
        edge_hi:   Color32::from_rgb(255, 100,  50),
        node:      Color32::from_rgb(220,  80,  40),
        node_rim:  Color32::from_rgb(250, 130,  80),
        node_hov:  Color32::from_rgb(240, 105,  60),
        node_sel:  Color32::from_rgb(255, 240,  40),
        node_selr: Color32::from_rgb(252, 192, 168),  // #fcc0a8 soft peach — rim brightened
        label:     Color32::from_rgb(215, 185, 170),
        label_hov: Color32::from_rgb(255, 240, 230),
        bar_bg:    Color32::from_rgb( 28,  14,  10),
        bar_line:  Color32::from_rgb( 60,  30,  20),
        bar_text:  Color32::from_rgb(175, 135, 120),
    },
    Theme {
        name: "Mono",
        bg:        Color32::from_rgb( 12,  12,  12),
        grid:      Color32::from_rgb( 50,  50,  50),
        edge:      Color32::from_rgb( 80,  80,  80),
        edge_hi:   Color32::from_rgb(200, 200, 200),
        node:      Color32::from_rgb(170, 170, 170),
        node_rim:  Color32::from_rgb(210, 210, 210),
        node_hov:  Color32::from_rgb(200, 200, 200),
        node_sel:  Color32::from_rgb(255, 200,  40),
        node_selr: Color32::from_rgb(232, 232, 232),  // #e8e8e8 soft silver-white — rim brightened
        label:     Color32::from_rgb(175, 175, 175),
        label_hov: Color32::from_rgb(240, 240, 240),
        bar_bg:    Color32::from_rgb( 18,  18,  18),
        bar_line:  Color32::from_rgb( 45,  45,  45),
        bar_text:  Color32::from_rgb(130, 130, 130),
    },
];

/// The theme navi draws with when Emacs sends its colours (`--emacs` mode):
/// the canvas is the `default` background, nodes wear the colour links have
/// in your notes, the status bar is the mode line. A colour that wouldn't
/// read on the canvas (or that Emacs didn't specify) is mixed from the
/// foreground instead, so no Emacs theme can make the graph invisible.
pub fn from_emacs(p: &navi_core::bridge::Palette) -> Theme {
    use navi_core::groups::{contrast, luminance};
    let bg = p.bg.unwrap_or([16, 30, 46]);
    let light = luminance(bg) > 0.35;
    let fg = p.fg.filter(|&c| contrast(c, bg) >= 4.5).unwrap_or(if light { [20, 20, 20] } else { [230, 230, 230] });
    let mix = |a: [u8; 3], b: [u8; 3], t: f32| -> [u8; 3] {
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
        [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
    };
    // last resort: walk toward black or white (whichever `on` is further
    // from) until the colour reads
    let ensure = |c: [u8; 3], on: [u8; 3], min: f64| -> [u8; 3] {
        let pole = if luminance(on) > 0.18 { [0, 0, 0] } else { [255, 255, 255] };
        (0..=10).map(|k| mix(c, pole, k as f32 / 10.0)).find(|&m| contrast(m, on) >= min).unwrap_or(pole)
    };
    // `want` if it reads on the canvas at `min`, else the fallback
    let pick = |want: Option<[u8; 3]>, min: f64, fallback: [u8; 3]| {
        want.filter(|&c| contrast(c, bg) >= min).unwrap_or_else(|| ensure(fallback, bg, min))
    };
    let accent = pick(p.accent, 3.0, mix(bg, fg, 0.75));
    let node   = pick(p.link, 3.0, accent);
    let warm   = pick(p.warm, 3.0, if light { [200, 90, 0] } else { [240, 144, 48] });
    let dim    = pick(p.dim, 2.5, mix(bg, fg, 0.55));
    let bar_bg = p.bar_bg.unwrap_or(mix(bg, fg, 0.06));
    let bar_fg = p.bar_fg.filter(|&c| contrast(c, bar_bg) >= 3.0).unwrap_or_else(|| ensure(mix(bar_bg, fg, 0.8), bar_bg, 3.0));
    let c = |v: [u8; 3]| Color32::from_rgb(v[0], v[1], v[2]);
    Theme {
        name: "Emacs",
        bg:        c(bg),
        grid:      c(mix(bg, fg, 0.07)),
        edge:      c(mix(bg, accent, 0.4)),
        edge_hi:   c(accent),
        node:      c(node),
        node_rim:  c(mix(node, fg, 0.45)),
        node_hov:  c(mix(accent, fg, 0.25)),
        node_sel:  c(warm),
        node_selr: c(mix(node, fg, 0.7)),
        label:     c(dim),
        label_hov: c(fg),
        bar_bg:    c(bar_bg),
        bar_line:  c(mix(bar_bg, fg, 0.2)),
        bar_text:  c(bar_fg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::groups::{contrast, palette, GroupColour, PALETTE_LEN};

    /// Every group colour reads on every theme: ≥ 3:1 against the canvas
    /// (WCAG's bar for graphics). All themes are dark today; a light one
    /// would get the deep palette (checked in navi-core).
    #[test]
    fn group_palette_is_readable_on_every_theme() {
        for t in THEMES {
            let bg = [t.bg.r(), t.bg.g(), t.bg.b()];
            for s in 0..PALETTE_LEN as u8 {
                let c = GroupColour::Slot(s).rgb(bg);
                assert!(contrast(c, bg) >= 3.0, "{}: slot {s} {c:?} contrast {:.2}", t.name, contrast(c, bg));
                assert_eq!(c, palette(bg)[s as usize]);
            }
        }
    }

    fn rgb(c: Color32) -> [u8; 3] { [c.r(), c.g(), c.b()] }

    #[test]
    fn emacs_theme_takes_the_faces_it_can_read() {
        use navi_core::bridge::Palette;
        let p = Palette {
            bg: Some([16, 30, 46]), fg: Some([224, 240, 255]), link: Some([85, 120, 255]),
            accent: Some([64, 232, 255]), warm: Some([240, 144, 48]), dim: Some([120, 152, 184]),
            bar_bg: Some([10, 28, 52]), bar_fg: Some([168, 200, 216]),
        };
        let t = from_emacs(&p);
        assert_eq!(rgb(t.bg), [16, 30, 46]);
        assert_eq!(rgb(t.node), [85, 120, 255]);
        assert_eq!(rgb(t.edge_hi), [64, 232, 255]);
        assert_eq!(rgb(t.node_sel), [240, 144, 48]);
        assert_eq!(rgb(t.bar_bg), [10, 28, 52]);
    }

    /// Whatever Emacs sends (nothing, a link colour equal to the background,
    /// a light theme), nodes, labels and the status text stay readable.
    #[test]
    fn emacs_theme_is_always_readable() {
        use navi_core::bridge::Palette;
        use navi_core::groups::contrast;
        let cases = [
            Palette::default(),
            Palette { bg: Some([255, 255, 255]), fg: Some([0, 0, 0]), ..Default::default() },
            Palette { bg: Some([250, 250, 245]), fg: Some([60, 60, 60]), link: Some([250, 250, 240]), accent: Some([255, 255, 0]), ..Default::default() },
            Palette { bg: Some([0, 0, 0]), fg: Some([5, 5, 5]), link: Some([0, 0, 10]), bar_bg: Some([0, 0, 0]), bar_fg: Some([0, 0, 0]), ..Default::default() },
            // stock Emacs: a light grey mode line, here on a dark canvas with no mode-line foreground
            Palette { bg: Some([30, 30, 30]), fg: Some([220, 220, 220]), bar_bg: Some([191, 191, 191]), ..Default::default() },
        ];
        for p in cases {
            let t = from_emacs(&p);
            let bg = rgb(t.bg);
            for (what, c, min) in [("node", t.node, 3.0), ("selected", t.node_sel, 3.0), ("label", t.label, 2.5), ("label_hov", t.label_hov, 4.5), ("edge_hi", t.edge_hi, 3.0)] {
                assert!(contrast(rgb(c), bg) >= min, "{p:?}: {what} {:?} on {bg:?}", rgb(c));
            }
            assert!(contrast(rgb(t.bar_text), rgb(t.bar_bg)) >= 3.0, "{p:?}: status text");
        }
    }
}
