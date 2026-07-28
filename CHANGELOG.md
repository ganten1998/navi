# Changelog

All notable changes to Navi are documented here.

## [1.0.0] — 2026-05-20, re-released 2026-07-28

The 2026-07-28 re-release (same version, tag recut) makes the app fully
cross-platform and fixes several bugs found while porting:

### Re-release — Added

- **Linux support (X11 + Wayland, one binary)** — winit's Linux backends are now
  enabled; pacing via blocking `SwapInterval::Wait(1)` swaps on X11 / a
  refresh-rate timer on Wayland (blocking swaps can freeze there when a
  compositor withholds frame callbacks). Sets `WM_CLASS` / `app_id` to `navi`
  for i3/sway rules; pre-built x86_64 + aarch64 glibc binaries with a
  `navi.desktop` file ship with the release.
- **Windows support** — talks to the Emacs server via its native TCP
  `server-file` (with the `%APPDATA%` home resolution stock Emacs uses), finds
  `emacsclientw.exe` across official/Chocolatey/Scoop/MSYS2 installs, builds as
  a GUI-subsystem app (no console window; parent console re-attached for
  debug flags; panics surface in a dialog), db auto-detection probes
  `%APPDATA%`, and a pre-built x64 exe ships with the release.
- **`borderless` config option** — implemented (it existed in the config schema
  but was never applied); default is a normal decorated window, which tiling
  WMs (AeroSpace, i3) can actually manage.
- **Window icon on X11/Windows**, per-OS font fallback chains (CJK via `.ttc`
  collections on Windows/Linux), HiDPI initial-size clamp, and a friendly
  in-window message (instead of a silent exit) when no org-roam db is found.

### Re-release — Fixed

- **Startup race that froze the app** — the CADisplayLink activation could be
  requested before the background thread published the link; the resume was
  dropped and no frame was ever painted. Also added a fallback wake-up so a
  stalled pacer can never freeze the UI again.
- **App never activated on macOS** — the process never registered as a regular
  activated app, so the window got no keyboard focus and AX-based tools
  (AeroSpace, yabai) didn't even list it.
- **Search input was dead** — the key handler swallowed the `Text` events that
  follow each keypress, so `/` seeded the query with a slash and typed
  characters never registered.
- **CJK crash** — selecting a node with a long multi-byte title panicked on a
  byte-sliced truncation.
- **Startup fit under tiling WMs** — the initial zoom/center assumed a
  1400×870 viewport; now re-fits to the real first-frame size, and `R` uses
  the live viewport too.
- Lisp string unescaping for titles/files/aliases (quoted tags are now
  stripped like everything else), sqlite `busy_timeout` so a load racing
  `org-roam-db-sync` doesn't abort, GL config scoring (prefer no-MSAA + sRGB),
  `Q`/`Escape` exits through the event loop instead of `process::exit` (clean
  GL teardown), emacsclient children are reaped (no zombies), and `~` in the
  configured `emacsclient` path expands.

## Original release notes — 2026-05-20

Full ground-up rewrite from Python (pygame + ModernGL) to Rust (egui + glow). Same conceptual feature set, dramatically better performance and packaging. The previous Python release is superseded and unsupported.

### Added

- **Rust workspace** — `navi-core` (pure library: config, db loader, graph, physics) + `navi` (egui UI binary).
- **Native event loop** — direct `winit` + `glutin` + `egui_glow`, replacing eframe. Gives Navi full control over the paint cadence.
- **Vsync-aligned 240 Hz pacer (macOS)** — dedicated background thread owns a `CADisplayLink` pinned to the panel's max refresh rate via `CAFrameRateRange`. Each vsync sends a `UserEvent::Vsync` to the main loop via `EventLoopProxy`; mouse/keyboard events never trigger redraws. Result: locked 4.17 ms frame interval on a 240 Hz display with no missed vsyncs in steady state.
- **Idle grace window** — after ~10 s of inactivity (configurable via `NAVI_IDLE_GRACE_SECS`) the display link pauses and the OS drops the panel tier; the next input or focus event resumes within one frame.
- **Paint coalescing** — duplicate redraw triggers (e.g. a Vsync arriving microseconds after a focus-driven redraw) within half a vsync interval are dropped, preventing compositor-slot contention that produced 8–14 ms worst-case frames.
- **`NAVI_FPS_LOG=1`** — stderr probe printing fps, avg/worst frame ms, deque size, and display-link metadata once per second.
- **`NAVI_PROF=1`** — per-layer paint timing (grid / edges / nodes / labels / help).
- **Graph rendering via egui's tessellator + glow** — single-pass painter with grid, edges, nodes, labels, particle effects, and help overlay.
- **Themes** — Obsidian / Forest / Ocean / Ember / Mono, cycled with `T`.
- **Layouts** — force-directed (default) + alternates cycled with `V`.
- **Search** — `/` to search nodes by title or alias.
- **Local-graph mode** — `L` cycles 1 → 2 → 3 hops → off.
- **Filters** — `D` (daily notes) and `O` (orphans).
- **Tag colouring** — `G`; reads org-roam's `tags` table, golden-ratio hue spacing.
- **Age heatmap** — `A`; visualises file mtime in 6 stages.
- **Headline-level nodes** — open jumps to heading position via `goto-char`.
- **emacsclient discovery** — Homebrew, MacPorts, `/usr/local/bin`, `/usr/bin`, `~/.local/bin`, `~/.nix-profile/bin`, NixOS, Snap, `/Applications/Emacs.app`.
- **Emacs socket discovery** — `$EMACS_SERVER_SOCKET`, `$XDG_RUNTIME_DIR/emacs/`, `$TMPDIR/emacs{uid}/`, `/tmp`, `/private/tmp`, and `/var/folders/.../T/emacs{uid}/` (fixes GUI vs Terminal `TMPDIR` mismatch on macOS).
- **DB auto-detection** — vanilla Emacs, XDG, Doom 2.x / 3.x, Spacemacs.

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
