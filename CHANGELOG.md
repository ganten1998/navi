# Changelog

All notable changes to Navi are documented here.

## [1.2.0] - 2026-10-14

### Added

- **Navi in Emacs: `org-roam-navi`** (`org-roam-navi.el`). One
  `use-package` form, then `M-x org-roam-navi`: the first run downloads the
  right Navi for the machine from the latest release and checks it against
  `SHA256SUMS`. Started that way:
  - the graph follows point: the note or heading you're in is selected and
    the view glides to it (follows the node while physics moves it; any
    drag or scroll takes over);
  - opening a node (double-click, `Enter`) opens it in that Emacs, at the
    heading, in the window you were last in. No `server-start` or
    emacsclient needed;
  - Navi draws with the Emacs theme (`default`, `org-link`, `shadow`,
    `font-lock-keyword-face`, `warning`, `mode-line`) and follows theme
    changes. Colours that wouldn't read on the canvas are replaced, so no
    theme makes nodes or labels invisible. `T` cycles to Navi's own themes
    and back;
  - Navi quits with Emacs (stdin EOF), saving its views first.
  Works from GUI and terminal frames alike.
- `--emacs` and `--db PATH` flags, plus `--version` and `--help`. The
  protocol (JSON lines on stdin/stdout) lives in `navi-core/src/bridge.rs`.
- Navi re-reads the db by itself when it changes on disk (the db or its
  `-wal`), quietly: the status bar only reports a change in the nodes.
- **FreeBSD** release build (x86_64, cross-built with zig).
- Releases carry `SHA256SUMS`, on GitHub (ganten1998/navi) and Codeberg.

## [1.1.0] - 2026-10-12

### Added

- **Side panel** (`\` toggles it; resizable, coloured by the active theme,
  and each view remembers whether it's open). Its sections:
  - **View**: every view of this db with the active one marked; switch,
    new, duplicate, rename, and delete (asked inline first; the last view
    stays).
  - **Filters**: dailies show / hide, orphans show / hide / only,
    "links to dailies count", and live counts (visible nodes and links,
    orphans in view and in the db, isolated by filters, daily-only, hidden
    by groups, dangling links).
  - **Groups**: the manual groups in precedence order. Drag a row by its
    handle (or use the arrow buttons) to reorder. Each row has a colour
    picker, its name, its query (a parse error shows right under it), how
    many nodes match, and visible / pull / hull switches; a group can be
    deleted. "Add group" starts from the search being typed (or the last
    one searched), as a group query matching the same notes. "Group by" picks the automatic grouper with its
    setting (folder depth, community resolution, communities computed on
    all links or on what's visible), switches pull and hull for every
    automatic group, and lists the automatic groups so each can be hidden.
  - **Physics**: the group pull strength, and pause / resume.
  - **Inspector** for the selected node: title, file (relative to the
    notes root), tags, aliases, links in view and in the db, orphan status
    (no links at all, isolated by filters, or daily-only), its dangling
    links, and every group it's in, marking the one that gives its colour.

  Every edit shows on the graph at once and is saved to `views.json`. While
  a panel field has the keyboard no shortcut fires, and `Escape` leaves the
  field first; `Tab` still cycles nodes. Clicks, drags and scrolling over
  the panel or its pop-ups never pan, select or zoom the graph, and the
  graph, its fit and hit-testing use the space left of the panel. The panel
  doesn't keep the app at full frame rate: it idles like the rest.
- **Groups**: a group is a query with a colour and three switches:
  visible, pull (members drift toward their centroid; off by default) and
  hull (a faint outline behind its nodes). Manual groups are an ordered
  list where the first visible match colours a node, and a node keeps every
  group it's in (the status bar lists them for the selected node). One
  automatic grouper can run below them: folder (relative to the notes
  root), tag (first tag alphabetically), component, community (Louvain with
  a connectivity refinement: deterministic, seeded, computed in the
  background on large graphs) or file (headings with their file). Hiding a
  group hides only nodes whose every group is hidden, and it happens before
  orphans are worked out, like the dailies filter.
- **Stable group colours**, readable on every theme: manual groups store
  theirs, and every automatic group's palette slot is remembered per db,
  so a tag or cluster has one colour in every view and after a restart, and
  a new group never repaints an old one. Two groups first seen apart that
  landed on one colour get two once they show together. Components and
  communities follow their notes across recomputation by overlap matching
  (also while a large graph's communities recompute in the background,
  where the previous partition keeps showing). Hiding a component or
  community stays with that cluster: it's keyed by one of its notes, not
  its size rank, and stays hidden after another view was shown. When the
  note its key names leaves (a daily hub under Dailies Hide, a deleted
  note), the cluster that is left keeps the key, its colour and its hiding,
  and unhiding a cluster merged from two hidden ones takes one click. Ungrouped
  nodes keep the theme's node colour, and orphans are in no component or
  community.
- **Query language** for the search box and group queries: `tag:`, `dir:`,
  `path:`, `file:`, `title:` (text or `/regex/`), `alias:`, `level:`,
  `degree:`, `prop:KEY=VAL` (and `prop:KEY="two words"`), `daily`,
  `orphan`, `dangling`, `AND` / `OR` / `NOT` / `-`, parentheses and quoted
  phrases; a regex may end in `i` (`/draft/i`). Plain words still search
  titles and aliases, and so does text that only looks like syntax
  (`Rust - Ownership`, `Path: x`; in the search box bare `daily` also finds
  "Daily Review", while in a group it is the predicate alone). Search text that doesn't parse
  (`(draft`) is matched as plain text with the error and its column in the
  status bar; a group query that doesn't parse matches nothing and shows
  why, and so does a group with an empty query ("Add group" with nothing
  searched never takes the whole graph). A closing quote must end its
  term (`title:"a"b` is an error, not two terms). `path:` takes absolute paths, and an absolute `dir:` ignores the
  case of the notes root (`C:` / `c:`). While typing, the status bar counts
  matches.
- **Views**: named sets of groups and filters per db in
  `~/.config/navi/views.json`; `1` to `9` and `[` / `]` switch between them,
  and the last one used is remembered. Loading keeps everything that
  parses (a bad entry is dropped alone, bytes that aren't UTF-8 are
  replaced, and the original file is copied to `views.json.bad` first); a
  views.json that can't be read at all is never written over. Keys navi
  doesn't know (a newer navi's, or your own) are kept on save. Saves are
  atomic, write through a symlinked file, keep what another navi window
  saved for another db, and a failed save is retried instead of dropped. A db that
  doesn't exist yet keys the same as it will once it does. Editing
  views.json by hand while navi runs works: the change shows within a
  second, and navi's next save keeps every setting it didn't change too.
  The notes root is remembered per db, so a folder that empties never
  re-keys the folder groups or re-aims `dir:`.
- `Shift-G` cycles the automatic grouper; a legend (top left) lists the
  groups with how many of their nodes are on screen while groups are shown.

- **Orphan filter is tri-state**: `O` cycles shown → hidden → only. The
  status bar shows the mode and counts when it isn't "shown"
  (`Orphans:Hidden 13 (2 isolated by filters)`); the `H` panel shows orphan
  and unresolved-link counts. With `links_to_dailies_count: false`, notes
  linked only to dailies count as orphans ("daily-only"); a daily linked to
  a connected note does not.
- **Filters persist** in the view (see Changed), with a new view key,
  `links_to_dailies_count`, and a new `config.json` key, `dailies_dirs`
  (which directory names mark a daily). The modes read back in any case
  (and as `"shown"` / `"hidden"`), and a bad value in `config.json` resets
  only its own key instead of the whole file (the original is kept as
  `config.json.bad`). A `config.json` that can't be read or isn't JSON (a
  typo) is no longer replaced with defaults at startup.
- **Unresolved links are counted**: `fuzzy` links and `id:` links to a
  missing node used to vanish silently; they're now kept as dangling links
  and counted (help panel, startup log).
- Heading nodes record their file node (containment, for future grouping;
  not drawn, not a link).

### Changed

- `G` toggles the group display (colours, hulls, pull, legend) instead of
  colouring by each node's first tag in database order with colours handed
  out by index. With nothing grouped yet, the first `G` turns on the tag
  grouper, so it still does what it used to.
- The `dailies`, `orphans` and `links_to_dailies_count` settings moved from
  `config.json` into the active view. Existing values carry over on the
  first run, and leave `config.json` only after `views.json` holding them
  is written; `views.json` keeps them (`legacy_filters`) for every db
  opened later too. `config.json` is now saved atomically too, and through a
  symlink (a dotfiles repo's copy stays the one in use).
- Group pull is weak by default (`group_pull: 0.1`) so links between groups
  stay readable; it pauses like the rest of the physics once things settle.
  Changing it while physics runs shows at once, and in the Column and Tree
  layouts a group change never starts physics. Hiding or showing a group
  re-forms the layout, as the dailies and orphan filters do.
- macOS: needs macOS 14 or later (the frame pacer's display link is a
  macOS 14 API; on older systems the app would quit at launch). The
  release zip is named after its architecture (`Navi-<version>-arm64.zip`),
  carries no Dropbox or Finder metadata, and is built with LTO (the release
  profile now sits at the workspace root, where cargo reads it). The
  Linux/Windows packaging script uses the same build directory, so it can't
  package an old binary. Linux tarballs no longer carry macOS `._` files,
  the Windows step finds Homebrew's llvm (and says so up front when it's
  missing), an ad-hoc macOS build never replaces the release zip, and the
  script points uploads at the Codeberg releases page.
- `views.json` is written shortly after the last edit (typing a query
  doesn't write once per key) and again on exit; every write is atomic.
- Picking a grouper in the panel's "Group by" turns the group display on,
  as `Shift-G` does. The `H` controls overlay is drawn over the whole
  window and narrows to fit it, so the side panel never cuts it off.
- Off-screen hulls cost nothing: they are culled before they are built,
  and zooming re-pads the cached outline instead of rebuilding it.
  Community detection below 500 notes stays inline (well under a
  millisecond), larger graphs go to the background worker, a request
  queued there is dropped once nothing asks for communities, and the last
  few partitions are kept, so going back to a resolution is instant.
- macOS: `ARCH=universal` builds again with rustup's toolchain (its proc
  macros are left unstripped, which dyld refused to load), and an
  unnotarized package (`NAVI_ALLOW_UNNOTARIZED=1`) is named
  `-unnotarized.zip`, never replacing the release zip.
- The missing-db message says what works: set `"db"` and restart navi, or
  press `W` once the db exists at the path shown. The README no longer
  claims the old `org-roam-graph` config path is read.

### Fixed

- `Escape` quits with the panel closed: a rename or delete prompt left open
  when the panel closed used to take the first `Escape`.
- **Orphans are judged on what's on screen**: hiding dailies left notes
  whose only links were to dailies sitting isolated, and the orphan filter
  didn't hide them. Orphan status now comes from the filtered graph, and
  layouts place the same nodes in their orphan cluster.
- **Duplicate and self-links**: the same link written twice (or both ways)
  counted twice toward degree, node size and spring force, and was drawn
  twice; a self-link added 2 to degree. Links are now deduped as unordered
  pairs and self-links dropped.
- **Hidden nodes left the simulation**: they still repelled visible nodes,
  stretched "reset view" and layout fits, and Tab could select them. Physics,
  every layout, fitting, Tab cycling and hit-testing now use visible nodes
  only; hidden nodes stay frozen where they were, and a selection that gets
  hidden is cleared.
- **Disconnected islands** in Column/Tree/Disk/Radial layouts are laid out as
  their own components beside the main one instead of being lumped in with
  the orphans. Disk/Radial pack them in rings just outside the main
  component (radius grows with √islands), and the physics clamp is sized
  from the placement, so islands aren't squashed against the rim and a
  filter key never snaps nodes inward.
- Daily detection matches `dailies_dirs` as whole directory components only
  (never the file name or a substring), on `/` and `\` paths alike, and only
  below the notes root, so a `Journal` folder above the notes isn't a match.
- Node order is deterministic (file, position, id); it used to follow
  HashMap order, so layouts differed run to run. Filters and search/local
  fading are computed only when they change instead of every frame. A saved
  filter no longer lays the graph out twice at startup, and an empty graph
  (no db yet) lets the app idle instead of rendering at full rate.

## [1.0.0] - 2026-05-20, re-released 2026-07-28

The 2026-07-28 re-release (same version, tag recut) makes the app fully
cross-platform and fixes several bugs found while porting:

### Re-release: Added

- **Linux support (X11 + Wayland, one binary)**: winit's Linux backends are now
  enabled; pacing via blocking `SwapInterval::Wait(1)` swaps on X11 / a
  refresh-rate timer on Wayland (blocking swaps can freeze there when a
  compositor withholds frame callbacks). Sets `WM_CLASS` / `app_id` to `navi`
  for i3/sway rules; pre-built x86_64 + aarch64 glibc binaries with a
  `navi.desktop` file ship with the release.
- **Windows support**: talks to the Emacs server via its native TCP
  `server-file` (with the `%APPDATA%` home resolution stock Emacs uses), finds
  `emacsclientw.exe` across official/Chocolatey/Scoop/MSYS2 installs, builds as
  a GUI-subsystem app (no console window; parent console re-attached for
  debug flags; panics surface in a dialog), db auto-detection probes
  `%APPDATA%`, and a pre-built x64 exe ships with the release.
- **`borderless` config option**: implemented (it existed in the config schema
  but was never applied); default is a normal decorated window, which tiling
  WMs (AeroSpace, i3) can actually manage.
- **Window icon on X11/Windows**, per-OS font fallback chains (CJK via `.ttc`
  collections on Windows/Linux), HiDPI initial-size clamp, and a friendly
  in-window message (instead of a silent exit) when no org-roam db is found.

### Re-release: Fixed

- **Startup race that froze the app**: the CADisplayLink activation could be
  requested before the background thread published the link; the resume was
  dropped and no frame was ever painted. Also added a fallback wake-up so a
  stalled pacer can never freeze the UI again.
- **App never activated on macOS**: the process never registered as a regular
  activated app, so the window got no keyboard focus and AX-based tools
  (AeroSpace, yabai) didn't even list it.
- **Search input was dead**: the key handler swallowed the `Text` events that
  follow each keypress, so `/` seeded the query with a slash and typed
  characters never registered.
- **CJK crash**: selecting a node with a long multi-byte title panicked on a
  byte-sliced truncation.
- **Startup fit under tiling WMs**: the initial zoom/center assumed a
  1400×870 viewport; now re-fits to the real first-frame size, and `R` uses
  the live viewport too.
- Lisp string unescaping for titles/files/aliases (quoted tags are now
  stripped like everything else), sqlite `busy_timeout` so a load racing
  `org-roam-db-sync` doesn't abort, GL config scoring (prefer no-MSAA + sRGB),
  `Q`/`Escape` exits through the event loop instead of `process::exit` (clean
  GL teardown), emacsclient children are reaped (no zombies), and `~` in the
  configured `emacsclient` path expands.

## Original release notes, 2026-05-20

Full ground-up rewrite from Python (pygame + ModernGL) to Rust (egui + glow). Same conceptual feature set, dramatically better performance and packaging. The previous Python release is superseded and unsupported.

### Added

- **Rust workspace**: `navi-core` (pure library: config, db loader, graph, physics) + `navi` (egui UI binary).
- **Native event loop**: direct `winit` + `glutin` + `egui_glow`, replacing eframe. Gives Navi full control over the paint cadence.
- **Vsync-aligned 240 Hz pacer (macOS)**: dedicated background thread owns a `CADisplayLink` pinned to the panel's max refresh rate via `CAFrameRateRange`. Each vsync sends a `UserEvent::Vsync` to the main loop via `EventLoopProxy`; mouse/keyboard events never trigger redraws. Result: locked 4.17 ms frame interval on a 240 Hz display with no missed vsyncs in steady state.
- **Idle grace window**: after ~10 s of inactivity (configurable via `NAVI_IDLE_GRACE_SECS`) the display link pauses and the OS drops the panel tier; the next input or focus event resumes within one frame.
- **Paint coalescing**: duplicate redraw triggers (e.g. a Vsync arriving microseconds after a focus-driven redraw) within half a vsync interval are dropped, preventing compositor-slot contention that produced 8–14 ms worst-case frames.
- **`NAVI_FPS_LOG=1`**: stderr probe printing fps, avg/worst frame ms, deque size, and display-link metadata once per second.
- **`NAVI_PROF=1`**: per-layer paint timing (grid / edges / nodes / labels / help).
- **Graph rendering via egui's tessellator + glow**: single-pass painter with grid, edges, nodes, labels, particle effects, and help overlay.
- **Themes**: Obsidian / Forest / Ocean / Ember / Mono, cycled with `T`.
- **Layouts**: force-directed (default) + alternates cycled with `V`.
- **Search**: `/` to search nodes by title or alias.
- **Local-graph mode**: `L` cycles 1 → 2 → 3 hops → off.
- **Filters**: `D` (daily notes) and `O` (orphans).
- **Tag colouring**: `G`; reads org-roam's `tags` table, golden-ratio hue spacing.
- **Age heatmap**: `A`; visualises file mtime in 6 stages.
- **Headline-level nodes**: open jumps to heading position via `goto-char`.
- **emacsclient discovery**: Homebrew, MacPorts, `/usr/local/bin`, `/usr/bin`, `~/.local/bin`, `~/.nix-profile/bin`, NixOS, Snap, `/Applications/Emacs.app`.
- **Emacs socket discovery**: `$EMACS_SERVER_SOCKET`, `$XDG_RUNTIME_DIR/emacs/`, `$TMPDIR/emacs{uid}/`, `/tmp`, `/private/tmp`, and `/var/folders/.../T/emacs{uid}/` (fixes GUI vs Terminal `TMPDIR` mismatch on macOS).
- **DB auto-detection**: vanilla Emacs, XDG, Doom 2.x / 3.x, Spacemacs.

### Removed

- Python launcher (`navi`, `navi.py`, `org-roam-graph-window`).
- pygame / ModernGL / Pillow / numpy dependencies.
- PyInstaller `.app` bundle build (`build/build-macos.sh`, `dist/Navi.app`). To be replaced by a Cargo-driven `.app` build in a later release.
- Borderless mode + AeroSpace integration. Will return as a follow-up once the `winit` macOS path supports the same NSWindow tweaks.
- `--check` preflight CLI flag. Will return as `cargo run -- --check`.
- Particle effects on the GPU compute path. Currently absent from the Rust port.

### Notes

- This release reuses the `v1.0.0` tag. The previous Python `v1.0.0` was deleted.
- Linux compiles but does not yet have a vsync source wired up; the macOS-only `CADisplayLink` path is gated behind `cfg(target_os = "macos")`. Tracking issue to follow.
- Windows is not supported.
