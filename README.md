# Navi

An interactive graph viewer for [org-roam](https://www.orgroam.com/), running as a native desktop window. Use it from Emacs with the small `org-roam-navi` package (the graph follows the note you're in, and opens nodes in that Emacs), or on its own: Navi reads your `org-roam.db` directly and opens nodes in your running Emacs via `emacsclient`.

Written in Rust on top of [egui](https://github.com/emilk/egui) + [glow](https://github.com/grovesNL/glow) (OpenGL). On macOS it ships a hand-rolled vsync-aligned event loop that locks the renderer to the panel's native refresh rate (60, 120, 240 Hz) with sub-millisecond frame time variance, then drops to ~30 fps when idle to save power on laptops.

---

## Status

- **macOS**: primary target, fully supported. Tested on Apple Silicon (M-series) at 60 / 120 / 240 Hz. A pre-built `Navi.app` ships with each release.
- **Linux**: supported, X11 and Wayland backends both built in (one binary; X11 is picked automatically under i3 and other X11 WMs). Vsync pacing via blocking `SwapInterval::Wait(1)` swaps on X11, and a refresh-rate timer on Wayland. Sets `WM_CLASS` / `app_id` to `navi` for window-manager rules. Pre-built x86_64 and aarch64 glibc binaries ship with each release.
- **FreeBSD**: supported (x86_64, FreeBSD 14 or later, X11). Pre-built binaries ship with each release. They are cross-built, and the same code paths as Linux/X11.
- **Windows**: supported. Talks to the Emacs server over its native TCP `server-file` mechanism, finds `emacsclientw.exe` across the official installer / Chocolatey / Scoop / MSYS2 layouts, and builds as a GUI-subsystem app (no console window). A pre-built x64 exe ships with each release.

---

## In Emacs

One form in your init and Navi is part of Emacs:

```elisp
(use-package org-roam-navi
  :vc (:url "https://codeberg.org/ganten1998/navi")
  :bind ("C-c n g" . org-roam-navi))
```

Then `C-c n g` (or `M-x org-roam-navi`). The first time, it offers to download the right Navi for your machine from the [Releases](https://github.com/ganten1998/navi/releases) page (checked against the release's `SHA256SUMS`), so there is nothing else to install. After that:

- **The graph follows you.** The note or heading at point is selected and the view glides to it. Turn on local-graph mode (`L` in Navi) and you always see the neighbourhood of what you're writing.
- **Nodes open in this Emacs.** Double-click a node (or `Enter`) and it opens in the window you were last in, at the heading. No `server-start`, no emacsclient path, no sockets: Navi talks to the Emacs that started it over a pipe.
- **It wears your theme.** Navi draws with your Emacs colours (canvas from `default`, nodes in your `org-link` colour, the status bar from the mode line) and follows `load-theme`. `T` still cycles Navi's own themes.
- **It stays current.** Navi re-reads the database whenever org-roam writes it.
- **It goes when Emacs goes.**

Running `org-roam-navi` again brings the window to the front. Works the same from GUI frames and terminal frames (`emacsclient -t`), on macOS, Linux, Windows and FreeBSD.

| Command | |
|---|---|
| `org-roam-navi` | Open the graph, or raise it |
| `org-roam-navi-quit` | Close it |
| `org-roam-navi-install` | Download the latest Navi (replaces the downloaded one) |
| `org-roam-navi-update` | The same, only when a newer release exists |

| Option | Default | |
|---|---|---|
| `org-roam-navi-program` | `nil` | Navi to run. `nil`: the downloaded copy, then `Navi.app` in `/Applications` on macOS |
| `org-roam-navi-follow` | `t` | Follow the node at point |
| `org-roam-navi-follow-delay` | `0.25` | Idle seconds before following |
| `org-roam-navi-sync-theme` | `t` | Draw with the Emacs theme |
| `org-roam-navi-display-function` | `pop-to-buffer-same-window` | How an opened note is shown |
| `org-roam-navi-install-directory` | `~/.emacs.d/org-roam-navi/` | Where the download goes |
| `org-roam-navi-release-urls` | GitHub, then Codeberg | Release APIs to download from, in order (a release without `SHA256SUMS` is skipped) |

`org-roam-navi-open-hook` runs in a note's buffer after the graph opens it. Navi's stderr goes to the `*navi-log*` buffer.

<details>
<summary>Emacs 29, straight, elpaca, Doom</summary>

```elisp
;; Emacs 29 (package-vc)
(package-vc-install '(org-roam-navi :url "https://codeberg.org/ganten1998/navi"))

;; straight.el
(straight-use-package '(org-roam-navi :type git :repo "https://codeberg.org/ganten1998/navi" :files ("org-roam-navi.el")))

;; elpaca
(elpaca (org-roam-navi :repo "https://codeberg.org/ganten1998/navi" :files ("org-roam-navi.el")))

;; Doom: packages.el
(package! org-roam-navi :recipe (:host nil :repo "https://codeberg.org/ganten1998/navi" :files ("org-roam-navi.el")))
```

Or copy `org-roam-navi.el` onto your `load-path` and `(require 'org-roam-navi)`.
</details>

Under the hood, `org-roam-navi` starts `navi --emacs --db <org-roam-db-location>` and the two exchange JSON lines (protocol in `navi-core/src/bridge.rs`). Navi also runs on its own, as below; config.json is shared, and `--db` doesn't change it.

---

## Quick start

### Pre-built binaries

All from the [Releases](https://github.com/ganten1998/navi/releases) page on GitHub (the source lives on [Codeberg](https://codeberg.org/ganten1998/navi), mirrored to GitHub):

- **macOS 14 or later (Apple Silicon and Intel)**: `Navi-<version>-universal.zip`: unzip, double-click `Navi.app`. The app is signed with a Developer ID and notarized by Apple, so it opens like any downloaded app.
- **Linux**: `navi-<version>-linux-x86_64.tar.gz` / `navi-<version>-linux-aarch64.tar.gz`: unpack, run `./navi`. Needs glibc ≥ 2.32 (musl-based distros need a source build) and the usual X11/GL runtime libs (`libX11`, `libxkbcommon`, `libxkbcommon-x11`, Mesa/libGL, present on any desktop install; on a minimal Void install: `xbps-install libxkbcommon mesa-dri`). The tarball includes `navi.desktop` + icon for launcher integration (`cp navi ~/.local/bin/ && cp navi.desktop ~/.local/share/applications/ && cp icon.png ~/.local/share/icons/hicolor/1024x1024/apps/navi.png`).
- **FreeBSD (x86_64)**: `navi-<version>-freebsd-x86_64.tar.gz`: unpack, run `./navi`. Needs X11 and Mesa: `pkg install libX11 libxkbcommon libXcursor libXrandr libXi mesa-libs mesa-dri` (all present on a desktop install).
- **Windows (x64)**: `navi-<version>-windows-x86_64.zip`: unzip, run `navi.exe`. Self-contained: no Visual C++ redistributable needed.

### From source

```bash
git clone https://codeberg.org/ganten1998/navi.git
cd navi
cargo build --release
./target/release/navi
```

To build the `.app` bundle yourself:

```bash
./scripts/build-macos.sh                # this Mac's arch (Navi-<version>-arm64.zip on Apple Silicon)
ARCH=universal ./scripts/build-macos.sh # arm64 + x86_64 universal
open dist/Navi.app
```

On first run, Navi auto-detects your `org-roam.db` and creates `~/.config/navi/config.json`. Subsequent launches start in well under a second.

---

## Requirements

- **Rust 1.75+** (stable), install via [rustup](https://rustup.rs/) (source builds only)
- **org-roam v2** database (`nodes`, `files`, `links`, `tags`, `aliases`)
- To open nodes when Navi runs on its own: **emacsclient** + a running Emacs server (`(server-start)` in your init). On Windows the server runs in TCP mode automatically; nothing extra to configure. Started from Emacs by `org-roam-navi`, Navi needs neither.
- A working OpenGL (or GLES) context, built into macOS, standard on Linux (Mesa) and Windows

---

## Configuration

Config file: `~/.config/navi/config.json`. Created on first run with auto-detected defaults.

```json
{
  "db": "~/.emacs.d/org-roam.db",
  "emacsclient": "/opt/homebrew/bin/emacsclient",
  "server_name": "server",
  "show_fps": true,
  "borderless": false,
  "dailies_dirs": ["daily", "dailies", "journal", "journals"]
}
```

| Key | Description |
|---|---|
| `db` | Path to `org-roam.db`. Auto-detected from common Emacs / Doom / Spacemacs / XDG locations on first run. |
| `emacsclient` | Path to `emacsclient`. Bare names are resolved against `PATH` plus the common per-OS install locations (Homebrew, MacPorts, `/Applications/Emacs.app`, `~/.nix-profile`, Snap, and on Windows the official installer, Chocolatey, Scoop, and MSYS2 layouts, preferring `emacsclientw.exe` there). |
| `server_name` | Emacs server name (default `server`). |
| `show_fps` | Show FPS counter in the status bar (`F` toggles at runtime). |
| `borderless` | Draw the window without titlebar/decorations (default `false`). Leave off under tiling WMs: a decorated, resizable window is what lets AeroSpace / i3 tile it. |
| `dailies_dirs` | Directory names that mark a daily note (default `daily`, `dailies`, `journal`, `journals`). Matched as whole path components, case-insensitively, `/` or `\` separated, so `dailyish/` and `daily.org` don't match. Only directories *below* the notes root (the deepest folder every note shares) count, so notes kept under e.g. `~/Journal/roam/` aren't all dailies. A `YYYY-MM-DD` title is always a daily. |

A bad value in any key falls back to that key's default alone; the rest of the file is kept, and the original is first copied to `config.json.bad`. A config.json that can't be read or isn't JSON at all (a trailing comma after a hand edit) is never written over: navi runs on defaults for that session and leaves the file for you to fix. Saves are atomic (a temp file renamed into place), and a config.json that is a symlink (into a dotfiles repo, say) is written through: the link stays a link.

The filters `dailies`, `orphans` and `links_to_dailies_count` used to live here. They now belong to a [view](#views-and-groups): on the first run they move to the top of `views.json` (`legacy_filters`), and the `Default` view of every db that has no views yet takes them, the one you open now and any you open later (another `db`, `ORG_ROAM_DB`, or the right path after a first run on a wrong one). Only once `views.json` holding them is written are the keys dropped from config.json.

### DB auto-detection order

| Path | Setup |
|---|---|
| `$ORG_ROAM_DB` | env override |
| `$XDG_DATA_HOME/emacs/org-roam.db` | XDG-strict Linux |
| `~/.emacs.d/org-roam.db` | vanilla Emacs |
| `~/.config/emacs/org-roam.db` | XDG-style Emacs |
| `~/.config/doom/.local/etc/org-roam.db` | Doom 3.x |
| `~/.config/doom/org-roam.db` | Doom 3.x fallback |
| `~/.doom.d/.local/etc/org-roam.db` | Doom 2.x |
| `~/.doom.d/org-roam.db` | Doom 2.x fallback |
| `~/.spacemacs.d/org-roam.db` | Spacemacs |

---

## Views and groups

A **view** is a named set of filters and groups. Views are stored per org-roam db (keyed by its canonical path) in `~/.config/navi/views.json`, and navi remembers which view was last active for each db. `1` to `9` switch to a view by position, `[` and `]` step through them. Everything below can be edited in the [side panel](#side-panel); editing `views.json` by hand works too, also while navi runs: navi picks the change up within a second, and when it saves, a setting you changed by hand is kept unless navi changed that same setting meanwhile.

A **group** is a query plus how it's shown: a colour (always), and per group whether it is `visible`, whether it `pull`s its members together (a soft spring toward the group's centroid; off by default) and whether it draws a `hull` (a faint padded outline behind its nodes, labelled at their centre).

- **Manual groups** are an ordered list. A node can be in several; the first *visible* group it's in gives it its colour, and the status bar shows every group of the selected node, marking the one that colours it.
- One **automatic grouper** can be active, ranked below the manual groups (`Shift+G` cycles it):

| Grouper | Groups |
|---|---|
| `folder` | Directory below the notes root (the deepest folder every note shares), `depth` levels deep. Notes directly in the root stay ungrouped. |
| `tag` | One group per tag. A node with several tags is in all of them and coloured by the first alphabetically (never by database order). |
| `component` | Connected pieces of the graph left by the dailies filter, largest first. Orphans are not grouped. |
| `community` | Densely linked clusters (Louvain modularity with a connectivity refinement, so every community is connected; deterministic, seeded). `resolution` above 1 makes more, smaller communities. Computed on every real link by default (`"scope": "all"`), or on what the dailies filter leaves (`"scope": "visible"`). Large graphs compute in the background. Orphans of the view are not grouped, even when a hidden daily links them. |
| `file` | A file node with its heading nodes. Files without headings stay ungrouped. |

- **Hiding** a group hides only the nodes whose *every* group is hidden; nodes in no group are never hidden by groups. Hiding happens before orphans are worked out, like the dailies filter: a note whose only links went into a hidden group becomes an orphan ("isolated by filters"), and `O` treats it like any other.
- **Colours stay put.** Manual groups store their colour. Every automatic group's colour is remembered per db (`auto_colours`), so a tag or a cluster has the same colour in every view and after a restart, and a new group never repaints an old one. Folder, tag and file groups start from a palette colour picked by a hash of their name; two groups that land on the same colour get different ones as soon as they show together. Components and communities are named after one of their notes and follow it: the group a recomputation overlaps most with the old one keeps the old name and colour. Ungrouped nodes keep the theme's own node colour. The palette is readable on every theme.
- `G` turns the group display on and off: colours, hulls, pull and the legend (top left, with how many nodes of each group are on screen). Pressing `G` in a view with no groups starts the `tag` grouper, like the old tag colouring. Group visibility still filters while the display is off.

```json
{
  "version": 1,
  "dbs": {
    "/Users/me/.emacs.d/org-roam.db": {
      "active": "Default",
      "views": [
        {
          "name": "Default",
          "colour_groups": true,
          "groups": [
            { "name": "Rust", "query": "tag:rust OR title:cargo", "colour": "#ff8a3d", "hull": true },
            { "name": "Blog", "query": "dir:posts", "pull": true },
            { "name": "Loose ends", "query": "dangling OR orphan", "visible": false }
          ],
          "auto": { "kind": "community", "resolution": 1.0, "scope": "all" },
          "auto_hidden": [],
          "auto_pull": false,
          "auto_hull": false,
          "dailies": "show",
          "orphans": "show",
          "links_to_dailies_count": true,
          "group_pull": 0.1
        },
        { "name": "Writing", "auto": { "kind": "folder", "depth": 1 }, "colour_groups": true, "dailies": "hide" }
      ]
    }
  }
}
```

| View key | Description |
|---|---|
| `name` | Shown when switching (`View  Writing  (2/2)`) and in the status bar when a db has more than one view. |
| `groups` | Manual groups, highest precedence first: `name`, `query` (see below), `colour` (`"#rrggbb"`; left out, a palette colour from the name), `visible` (default `true`), `pull`, `hull` (default `false`). A group whose query doesn't parse matches nothing and the status bar says why; so does a group with an empty query. |
| `auto` | The automatic grouper: `{"kind": "none" \| "folder" \| "tag" \| "component" \| "community" \| "file"}`, plus `depth` (folder, default 1), `resolution` and `scope` (community). |
| `auto_hidden` | Keys of hidden automatic groups, like `"tag:draft"` or `"folder:archive"`. A component or community is keyed by one of its notes (`"community:<node id>"`), so it stays hidden when the clusters are recomputed and reordered, and when that note itself leaves (a daily hub under Dailies Hide, a deleted note), the cluster that is left keeps the key. |
| `auto_pull`, `auto_hull` | Pull / hull for every automatic group. |
| `colour_groups` | The `G` toggle. |
| `dailies` | `"show"` or `"hide"` (`D` toggles and saves it). Read back in any case; `"shown"` / `"hidden"` work too. |
| `orphans` | `"show"`, `"hide"` or `"only"` (`O` cycles and saves it). An orphan is a node with no link to another node *in the current view*. Hiding dailies (or a group) can isolate a note whose only links went there; the status bar counts those as "isolated by filters". |
| `links_to_dailies_count` | When `false`, a link to a daily doesn't save a note from being an orphan: a note linked only to dailies counts as one. A daily stays connected if it links to a connected note (otherwise it's an orphan too, so `O` never leaves one drawn alone). The status bar calls these "daily-only". Only matters while dailies are shown; default `true`. |
| `group_pull` | Strength of `pull` (default `0.1`, a slow drift; `0.5` makes tight clusters; at most `1.0`). |

Next to a db's `views`, navi writes `auto_colours`: the palette slot every automatic group got, shared by all of that db's views, so colours survive restarts. It also writes `notes_root`, the notes root it last saw: the root never moves deeper than that while every note is still under it, so when a top-level folder empties (or the last note directly in the root goes), folder groups keep their keys and `dir:` keeps its meaning.

Loading is forgiving: a bad view, group or value is dropped on its own and everything else is kept, and navi first copies the original file to `views.json.bad` so nothing you wrote is lost. Keys navi doesn't know (your own notes, or settings from a newer navi) are kept and written back as they are in the file; a file from a newer navi is also copied to `views.json.bad` first. Writes are atomic, and happen shortly after the last change (and on exit), so typing in the panel doesn't write once per key. `panel_open` (written by navi) remembers whether the side panel is open in that view.

### Side panel

`\` opens and closes a panel on the right. It's resizable, takes its colours from the theme, and each view remembers whether it's open. Changes show on the graph immediately and are saved to `views.json`.

| Section | What's there |
|---|---|
| View | Every view of this db, the active one marked; click one to switch to it. New, Duplicate, Rename, Delete (asks first; the last view can't be deleted). |
| Filters | Dailies show / hide, orphans show / hide / only, "links to dailies count", and live counts: visible nodes and links, orphans in view (and in the db), isolated by filters, daily-only, hidden by groups, dangling links. |
| Groups | "Show groups" (`G`). The manual groups, first = highest precedence: drag a row by its handle (the dots on the left) or use the arrow buttons to reorder. Each row: colour (click to pick), name, how many nodes match, the query (an error and its column show underneath), Visible / Pull / Hull, and delete. "Add group" starts from the search you're typing, or the last one you ran. "Group by" chooses the automatic grouper and its setting (folder depth, community resolution, communities computed on all links or only the visible graph), Pull / Hull for all automatic groups, and a list of them with a checkbox each to hide it. |
| Physics | Group pull strength (0.1 drifts, 0.5 clusters tightly) and pause / resume. |
| Inspector | The selected node: title, file relative to the notes root, tags, aliases, links in view and in the db, orphan status (no links at all, isolated by filters, or daily-only), its dangling links, and every group it's in with the one that gives its colour marked. "Open in Emacs". |

While a text field in the panel has the keyboard, no shortcut fires (typing `d` in a query doesn't hide dailies); `Escape` leaves the field first, and a click on the graph does too. `Tab` keeps cycling nodes (and leaves the field). Rename puts the keyboard in the name field; `Escape` cancels a rename or a delete prompt. Clicks, drags and scrolling over the panel stay in the panel: they never pan, select or zoom the graph, and the graph is laid out, fitted and hit-tested in the space left of it.

### Query language

The search box (`/`) and group queries share one language. Terms next to each other must all match (AND).

| Query | Matches |
|---|---|
| `rust`, `"rust book"` | Title or alias contains the word / phrase (any case). This is what plain search text does. |
| `/^20\d\d/` | Title or alias matches the regex (any case; `(?-i)` turns case back on) |
| `title:TEXT`, `title:/re/` | Title only |
| `alias:TEXT` | An alias |
| `tag:X`, `tag:/re/` | Has tag `X` (whole tag, any case) |
| `dir:PATH` | File is in folder `PATH` or below it, relative to the notes root (`dir:posts` matches `posts/a.org` and `posts/2026/b.org`, never `postscript/`). Whole folder names only; `/` or `\`; an absolute path works too. |
| `path:TEXT` | Full file path contains `TEXT` (an absolute path works: `path:/Users/me/notes/posts`) |
| `file:TEXT` | File name contains `TEXT` |
| `level:0`, `level:>0` | File nodes / heading nodes (`=N`, `>N`, `<N`, `>=N`, `<=N`) |
| `degree:>N` | More than N links (`=N`, `<N`, ...; counts every real link in the db) |
| `prop:KEY`, `prop:KEY=VALUE` | Has an org property (any case; `prop:KEY="two words"`; `prop:"KEY=two words"` works too) |
| `is:daily` | A daily note |
| `is:orphan` | No link to anything in the current view (for groups: before group hiding) |
| `is:dangling` | Has a link to a missing node or an unresolved `[[title]]` |
| `daily`, `orphan`, `dangling` | The same. In the search box also a title or alias containing the word (so searching `daily` still finds "Daily Review"); in a group, only the predicate (a hidden `daily` group never hides "Daily Review") |
| `A OR B`, `A AND B`, `NOT A`, `-A` | Either, both, neither. `AND` binds tighter than `OR`; `-` works on a term or a group: `-(tag:a OR tag:b)` |
| `( ... )` | Grouping |

Operators are upper case (`and`, `or`, `not` are ordinary words). Plain text stays plain text: a word whose part before `:` isn't one of the keys above (`http://x`, `Rust:`), a key followed by a space (`Path: x`), and a lone `-` (`Rust - Ownership`). A value starting with `/` is a regex only when the closing `/` ends it, so `path:/n/posts` is a path and `title:/^draft/` a regex; a trailing `i` (`title:/^draft/i`) is accepted, as matching ignores case anyway. A closing quote ends its term: `title:"a"b` is an error, not two terms. While the search doesn't parse (`(draft`), it matches as plain text, the old way, and the status bar says why and where (`col 1: missing ')' for this '('`); a group query that doesn't parse matches nothing and shows the error. "Add group" turns the search into a group query that matches the same notes (quoted, when the search only matched as plain text). `Enter` jumps to the first visible match, preferring a title that starts with the first search word.

---

## Opening nodes in Emacs

Double-click a node (or select it and press `Enter` / `Space`). File nodes open the file; **headline nodes jump to the heading** via `goto-char`.

Started by [`org-roam-navi`](#in-emacs), Navi hands the node to that Emacs over its pipe and nothing below applies. On its own, it uses emacsclient:

GUI apps get a minimal `PATH`, so Navi resolves `emacsclient` to an absolute path itself and locates the running server:

- Unix (socket): `$EMACS_SOCKET_NAME` / `$EMACS_SERVER_SOCKET`, `$XDG_RUNTIME_DIR/emacs/`, `$TMPDIR/emacs{uid}/`, `/tmp`, `/private/tmp` (covers the macOS GUI-vs-Terminal `TMPDIR` mismatch)
- Unix (TCP mode): `$EMACS_SERVER_FILE`, `~/.emacs.d/server/`, `$XDG_CONFIG_HOME/emacs/server/`
- Windows (always TCP): `$EMACS_SERVER_FILE`, then `.emacs.d/server/` and `.config/emacs/server/` under `%HOME%`, `%APPDATA%`, `%USERPROFILE%`, passed via `--server-file`

If open fails, an error appears in the status bar. Make sure Emacs has `(server-start)` in its init.

---

## Controls

| Input | Action |
|---|---|
| Drag background | Pan view |
| Swipe + release | Kinetic pan (momentum) |
| Drag node | Move node |
| Scroll / trackpad | Zoom toward cursor |
| Click node | Select, highlights connections (the status bar lists its groups; the panel's inspector shows the rest) |
| Double-click node | Open in Emacs |
| `Tab` / `Shift-Tab` | Cycle visible nodes |
| `Enter` / `Space` | Open selected node |
| `T` | Cycle colour theme (started from Emacs, the Emacs theme is one of them) |
| `G` | Group display on / off (colours, hulls, pull, legend) |
| `Shift-G` | Cycle the automatic grouper: none → folder → tag → component → community → file |
| `1` to `9` | Switch to view 1 to 9 |
| `\` | Open / close the [side panel](#side-panel) |
| `[` / `]` | Previous / next view |
| `A` | Toggle age / weathering heatmap |
| `D` | Show / hide daily notes (saved in the view) |
| `O` | Cycle orphans: shown → hidden → only (saved in the view) |
| `L` | Cycle local-graph mode (1 → 2 → 3 hops → off) |
| `V` | Cycle layout algorithm |
| `/` | Search with the [query language](#query-language); plain words search titles and aliases |
| `W` | Reload graph from database (Navi also reloads by itself when the db changes on disk) |
| `F` | Toggle FPS counter |
| `P` | Pause / resume physics |
| `R` | Reset view |
| `H` | Hold to show the controls overlay (plus orphan and unresolved-link counts) |
| `Q` / `Escape` | Quit |

---

## Frame pacing

Mouse / keyboard events do **not** trigger paints on any platform: a per-platform vsync source is the sole paint signal, which eliminates the multi-paint-per-frame burn that input-driven loops produce. After ~10 s of inactivity (configurable with `NAVI_IDLE_GRACE_SECS`) the app drops to a ~30 fps idle tier; the next input or focus event restores full speed within a frame.

- **macOS**: a dedicated background thread owns a `CADisplayLink` pinned to the panel's max refresh rate via `CAFrameRateRange`; each vsync sends a `UserEvent::Vsync` to the main thread. `swap_buffers` uses `SwapInterval::DontWait` because macOS GL caps swap-vsync at ~108 Hz on ProMotion panels regardless of tier; the display link does the pacing.
- **Linux/X11 & Windows**: vsync-blocking swaps (`SwapInterval::Wait(1)`): each paint re-arms the next `request_redraw`, and the blocking swap throttles the loop to the monitor's refresh rate.
- **Linux/Wayland**: swaps never block (compositors stop frame callbacks for hidden surfaces, which would freeze a blocking swap); a `WaitUntil` timer paces at the monitor's reported refresh rate instead. When the window is fully occluded (macOS occlusion state, X11 visibility events) painting stops entirely.

Result on a 240 Hz display (macOS): 4.17 ms average frame interval, worst-case 5–7 ms (no missed vsyncs in steady state), and the application sits near 0 % CPU when idle.

### Command line

| Flag | |
|---|---|
| `--db PATH` | Read this org-roam.db, for this run only (config.json keeps its own) |
| `--emacs` | Started by org-roam-navi.el: commands on stdin, events on stdout, quit at EOF |
| `-V`, `--version` | Print the version |

### Diagnostics

| Env var | Effect |
|---|---|
| `NAVI_FPS_LOG=1` | Print fps + frame stats + display-link metadata to stderr every ~1 s |
| `NAVI_PROF=1` | Print per-layer paint timing (grid/edges/nodes/labels/help) every ~1 s |
| `NAVI_IDLE_GRACE_SECS=N` | Override the 10 s active-after-idle grace window |
| `NAVI_NO_GRID`, `NAVI_NO_EDGES`, `NAVI_NO_NODES`, `NAVI_NO_LABELS` | Disable individual paint layers (perf debugging) |
| `NAVI_EVENT_DEBUG=1` | Print every window event except cursor moves (platform debugging) |
| `NAVI_INPUT_DEBUG=1` | Print per-frame pointer/hover/hit-test state |
| `WINIT_X11_SCALE_FACTOR=1` | (winit built-in) Override X11 HiDPI scale if EDID-derived scaling misfires |

---

## Project layout

```
Cargo.toml                Workspace root
navi-core/                Pure-Rust library: config, org-roam DB loader, view pipeline, groups, graph + physics
  src/lib.rs
  src/config.rs           Config load/save (tolerant, atomic), db detection, path expansion
  src/db.rs               org-roam.db loader: link typing, dedupe, dangling links, containment, properties
  src/emacs.rs            emacsclient + socket discovery
  src/bridge.rs           The --emacs protocol (JSON lines with org-roam-navi.el)
  src/view.rs             Dailies / group / orphan filters → visible nodes + edges, orphan counts
  src/query.rs            The query language: lexer, parser, evaluator
  src/groups.rs           Manual + automatic groups, precedence, hiding, stable colours, hulls
  src/community.rs        Deterministic community detection (Louvain + connectivity refinement)
  src/views.rs            Named views per db in views.json, migration from config.json
  src/graph.rs            Force-directed physics (with group pull) + layouts over the visible subset
  tests/probe_real_db.rs  #[ignore]d read-only probes of your real org-roam.db (filters, groups)
navi/                     Binary: UI, rendering, event loop
  src/main.rs             winit + glutin event loop, paint pacer
  src/app.rs              egui app: input, layout, views, search, status bar, legend
  src/app/panel.rs        Side panel: views, filters, groups, physics, inspector
  src/app/ui_tests.rs     Headless frames through the app: panel keyboard / pointer rules
  src/painter.rs          GraphPainter: grid, group hulls, edges, nodes, labels
  src/macos_display.rs    macOS CADisplayLink + tier control
  src/theme.rs            Colour themes
scripts/build-macos.sh    Cargo build + Navi.app, Developer ID signing, notarization, zip for release
scripts/build-cross.sh    Linux + FreeBSD (zig) and Windows (cargo-xwin, needs Homebrew llvm) release builds, SHA256SUMS
org-roam-navi.el          The Emacs package: start, follow, theme, open, download (at the root so package-vc finds it)
emacs/test/               ERT tests (a fake navi speaks the protocol)
assets/icon.png           App icon (bundle icns is generated from this)
assets/navi.desktop       Linux desktop entry (shipped in the Linux tarball)
```

---

## Building

```bash
# Release (recommended for use)
cargo build --release

# Dev (slower at runtime, faster compile, includes debuginfo)
cargo build

# macOS .app bundle + release zip
./scripts/build-macos.sh
```

Tests: `cargo test --workspace` for Navi, and for the Emacs package (org-roam must be loadable):

```bash
emacs -Q --batch --eval '(package-initialize)' -L . -l emacs/test/org-roam-navi-test.el -f ert-run-tests-batch-and-exit
```

The release profile in the workspace has `lto = true` and `opt-level = 3`. Expect a 30–60 s clean release build on a modern laptop.

---

## License

Source release. Add a `LICENSE` file if you intend to distribute binaries.
