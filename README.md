# Navi

An interactive graph viewer for [org-roam](https://www.orgroam.com/), running as a native desktop window. No Emacs package required — Navi reads your `org-roam.db` directly and opens nodes in your existing Emacs process via `emacsclient`.

Written in Rust on top of [egui](https://github.com/emilk/egui) + [glow](https://github.com/grovesNL/glow) (OpenGL). On macOS it ships a hand-rolled vsync-aligned event loop that locks the renderer to the panel's native refresh rate (60, 120, 240 Hz) with sub-millisecond frame time variance, then drops to ~30 fps when idle to save power on laptops.

---

## Status

- **macOS** — primary target, fully supported. Tested on Apple Silicon (M-series) at 60 / 120 / 240 Hz. A pre-built `Navi.app` ships with each release.
- **Linux** — supported, X11 and Wayland backends both built in (one binary; X11 is picked automatically under i3 and other X11 WMs). Vsync pacing via blocking `SwapInterval::Wait(1)` swaps on X11, and a refresh-rate timer on Wayland. Sets `WM_CLASS` / `app_id` to `navi` for window-manager rules. Pre-built x86_64 and aarch64 glibc binaries ship with each release.
- **Windows** — supported. Talks to the Emacs server over its native TCP `server-file` mechanism, finds `emacsclientw.exe` across the official installer / Chocolatey / Scoop / MSYS2 layouts, and builds as a GUI-subsystem app (no console window). A pre-built x64 exe ships with each release.

---

## Quick start

### Pre-built binaries

All from the [Releases](https://codeberg.org/ganten1998/navi/releases) page:

- **macOS (Apple Silicon)** — `Navi-<version>-host.zip`: unzip, double-click `Navi.app`. The bundle is ad-hoc signed; on first launch macOS Gatekeeper may flag it — right-click → Open, or run `xattr -cr Navi.app` to clear the quarantine flag.
- **Linux** — `navi-<version>-linux-x86_64.tar.gz` / `navi-<version>-linux-aarch64.tar.gz`: unpack, run `./navi`. Needs glibc ≥ 2.32 (musl-based distros need a source build) and the usual X11/GL runtime libs (`libX11`, `libxkbcommon`, `libxkbcommon-x11`, Mesa/libGL — present on any desktop install; on a minimal Void install: `xbps-install libxkbcommon mesa-dri`). The tarball includes `navi.desktop` + icon for launcher integration (`cp navi ~/.local/bin/ && cp navi.desktop ~/.local/share/applications/ && cp icon.png ~/.local/share/icons/hicolor/1024x1024/apps/navi.png`).
- **Windows (x64)** — `navi-<version>-windows-x86_64.zip`: unzip, run `navi.exe`.

### From source

```bash
git clone https://codeberg.org/ganten1998/navi.git
cd navi
cargo build --release
./target/release/navi
```

To build the `.app` bundle yourself:

```bash
./scripts/build-macos.sh                # arm64 (host)
ARCH=universal ./scripts/build-macos.sh # arm64 + x86_64 universal
open dist/Navi.app
```

On first run, Navi auto-detects your `org-roam.db` and creates `~/.config/navi/config.json`. Subsequent launches start in well under a second.

---

## Requirements

- **Rust 1.75+** (stable) — install via [rustup](https://rustup.rs/) (source builds only)
- **org-roam v2** database (`nodes`, `files`, `links`, `tags`, `aliases`)
- **emacsclient** + a running Emacs server (`(server-start)` in your init) — for double-click-to-open. On Windows the server runs in TCP mode automatically; nothing extra to configure.
- A working OpenGL (or GLES) context — built into macOS, standard on Linux (Mesa) and Windows

---

## Configuration

Config file: `~/.config/navi/config.json`. Created on first run with auto-detected defaults.

```json
{
  "db": "~/.emacs.d/org-roam.db",
  "emacsclient": "/opt/homebrew/bin/emacsclient",
  "server_name": "server",
  "show_fps": true,
  "borderless": false
}
```

| Key | Description |
|---|---|
| `db` | Path to `org-roam.db`. Auto-detected from common Emacs / Doom / Spacemacs / XDG locations on first run. |
| `emacsclient` | Path to `emacsclient`. Bare names are resolved against `PATH` plus the common per-OS install locations (Homebrew, MacPorts, `/Applications/Emacs.app`, `~/.nix-profile`, Snap, and on Windows the official installer, Chocolatey, Scoop, and MSYS2 layouts — preferring `emacsclientw.exe` there). |
| `server_name` | Emacs server name (default `server`). |
| `show_fps` | Show FPS counter in the status bar (`F` toggles at runtime). |
| `borderless` | Draw the window without titlebar/decorations (default `false`). Leave off under tiling WMs — a decorated, resizable window is what lets AeroSpace / i3 tile it. |

The legacy config path `~/.config/org-roam-graph/config.json` is also read; the next save writes to the new path.

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

## Opening nodes in Emacs

Double-click a node (or select it and press `Enter` / `Space`). File nodes open the file; **headline nodes jump to the heading** via `goto-char`.

GUI apps get a minimal `PATH`, so Navi resolves `emacsclient` to an absolute path itself and locates the running server:

- Unix (socket): `$EMACS_SOCKET_NAME` / `$EMACS_SERVER_SOCKET`, `$XDG_RUNTIME_DIR/emacs/`, `$TMPDIR/emacs{uid}/`, `/tmp`, `/private/tmp` (covers the macOS GUI-vs-Terminal `TMPDIR` mismatch)
- Unix (TCP mode): `$EMACS_SERVER_FILE`, `~/.emacs.d/server/`, `$XDG_CONFIG_HOME/emacs/server/`
- Windows (always TCP): `$EMACS_SERVER_FILE`, then `.emacs.d/server/` and `.config/emacs/server/` under `%HOME%`, `%APPDATA%`, `%USERPROFILE%` — passed via `--server-file`

If open fails, an error appears in the status bar. Make sure Emacs has `(server-start)` in its init.

---

## Controls

| Input | Action |
|---|---|
| Drag background | Pan view |
| Swipe + release | Kinetic pan (momentum) |
| Drag node | Move node |
| Scroll / trackpad | Zoom toward cursor |
| Click node | Select — highlights connections |
| Double-click node | Open in Emacs |
| `Tab` / `Shift-Tab` | Cycle nodes |
| `Enter` / `Space` | Open selected node |
| `T` | Cycle colour theme |
| `G` | Toggle tag colouring |
| `A` | Toggle age / weathering heatmap |
| `D` | Toggle daily-notes filter |
| `O` | Toggle orphan filter |
| `L` | Cycle local-graph mode (1 → 2 → 3 hops → off) |
| `V` | Cycle layout algorithm |
| `/` | Search by title or alias |
| `W` | Reload graph from database |
| `F` | Toggle FPS counter |
| `P` | Pause / resume physics |
| `R` | Reset view |
| `H` | Hold to show controls panel |
| `Q` / `Escape` | Quit |

---

## Frame pacing

Mouse / keyboard events do **not** trigger paints on any platform — a per-platform vsync source is the sole paint signal, which eliminates the multi-paint-per-frame burn that input-driven loops produce. After ~10 s of inactivity (configurable with `NAVI_IDLE_GRACE_SECS`) the app drops to a ~30 fps idle tier; the next input or focus event restores full speed within a frame.

- **macOS** — a dedicated background thread owns a `CADisplayLink` pinned to the panel's max refresh rate via `CAFrameRateRange`; each vsync sends a `UserEvent::Vsync` to the main thread. `swap_buffers` uses `SwapInterval::DontWait` because macOS GL caps swap-vsync at ~108 Hz on ProMotion panels regardless of tier — the display link does the pacing.
- **Linux/X11 & Windows** — vsync-blocking swaps (`SwapInterval::Wait(1)`): each paint re-arms the next `request_redraw`, and the blocking swap throttles the loop to the monitor's refresh rate.
- **Linux/Wayland** — swaps never block (compositors stop frame callbacks for hidden surfaces, which would freeze a blocking swap); a `WaitUntil` timer paces at the monitor's reported refresh rate instead. When the window is fully occluded (macOS occlusion state, X11 visibility events) painting stops entirely.

Result on a 240 Hz display (macOS): 4.17 ms average frame interval, worst-case 5–7 ms (no missed vsyncs in steady state), and the application sits near 0 % CPU when idle.

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
navi-core/                Pure-Rust library: config, org-roam DB loader, graph + physics
  src/lib.rs
  src/config.rs           Config load/save, db detection, path expansion
  src/emacs.rs            emacsclient + socket discovery
  src/graph.rs            Force-directed physics, layouts, hidden/faded sets
navi/                     Binary: UI, rendering, event loop
  src/main.rs             winit + glutin event loop, paint pacer
  src/app.rs              egui app: input, layout, status bar
  src/painter.rs          GraphPainter — grid, edges, nodes, labels
  src/macos_display.rs    macOS CADisplayLink + tier control
  src/theme.rs            Colour themes
scripts/build-macos.sh    Cargo build + Navi.app + zip for release
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

The release profile in the workspace has `lto = true` and `opt-level = 3`. Expect a 30–60 s clean release build on a modern laptop.

---

## License

Source release. Add a `LICENSE` file if you intend to distribute binaries.
