// Release builds on Windows are GUI-subsystem apps so double-clicking the exe
// doesn't open a console window behind it. Debug builds keep the console for
// live stderr. Release builds re-attach to a parent console in main() below.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod painter;
mod theme;

#[cfg(target_os = "macos")]
mod macos_display;

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_winit::winit;
use glutin::context::NotCurrentGlContext;
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::prelude::GlSurface;
use raw_window_handle::HasWindowHandle;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::Window;

use navi_core::{
    config::{detect_db, expand_tilde, Config},
    load_graph,
    views::{db_key, ViewStore},
    Graph,
};

/// Cross-thread events delivered to winit's main event loop.
///
/// `Vsync` is the heart of the new pacer: a dedicated background thread runs a
/// `CADisplayLink` against its own NSRunLoop, and on each tick (4.17 ms on this
/// 240 Hz panel) it sends one of these to the main thread. We render exactly
/// once per vsync — no mouse-driven extra repaints, no main-thread sleeps.
#[derive(Debug)]
pub enum UserEvent {
    Vsync,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // GUI-subsystem builds have no std handles; if we were launched from a
    // terminal, glue stderr back to it so the NAVI_* debug flags still print.
    #[cfg(all(windows, not(debug_assertions)))]
    unsafe {
        use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    // A GUI-subsystem process that panics (GL bring-up failure, driver
    // without a usable pixel format, …) would otherwise vanish without a
    // trace — surface the message in a dialog instead.
    #[cfg(all(windows, not(debug_assertions)))]
    {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let msg = format!("Navi crashed:\n\n{info}");
            let mut wide: Vec<u16> = msg.encode_utf16().collect();
            wide.push(0);
            let title: Vec<u16> = "Navi\0".encode_utf16().collect();
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
                MessageBoxW(std::ptr::null_mut(), wide.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
            }
            default_hook(info);
        }));
    }

    let mut cfg = Config::load();
    if cfg.db.is_empty() {
        cfg.db = detect_db();
    }
    if cfg.emacsclient.is_empty() {
        cfg.emacsclient = navi_core::EmacsClient::new("", &cfg.server_name)
            .binary
            .clone();
    }
    let db_path = expand_tilde(&cfg.db);

    // Views (groups + filters) live in views.json per canonical db path. The
    // filter keys an older config.json carried move to views.json
    // (`legacy_filters`), where every db's first view starts from them, this
    // one's now and any opened later (another "db", ORG_ROAM_DB, a path fixed
    // after a failed first run); they leave config.json only once views.json
    // holding them is safely written.
    let mut views = ViewStore::load(&ViewStore::default_path());
    let key = db_key(&db_path);
    let legacy = cfg.legacy_view();
    if views.ensure_db(&key, legacy.as_ref()) || legacy.is_some() {
        match views.save() {
            Ok(_) => if legacy.is_some() { cfg.clear_legacy_view(); },
            Err(e) => eprintln!("navi: couldn't write {}: {e}", views.path.display()),
        }
    }
    let filter = views.active(&key).filter(&cfg.dailies_dirs);

    // A missing/unreadable db must not kill the app before a window exists —
    // double-clicking the binary would just silently do nothing. Open with an
    // empty graph and say where we looked; `W` re-reads the db once it exists.
    let (graph, startup_error) = match load_graph(&db_path) {
        Ok(data) => {
            let s = data.stats;
            eprintln!(
                "navi: loaded {} nodes, {} edges ({} duplicate, {} self-links dropped; {} unresolved links)",
                data.nodes.len(), data.edges.len(), s.duplicates, s.self_links, data.dangling.len(),
            );
            // laid out for the saved filter: one layout, of what's on screen
            (Graph::from_data_with_view(data, &filter), None)
        }
        Err(e) => {
            eprintln!("navi: failed to load {db_path}: {e}");
            let msg = format!(
                "No org-roam db at {db_path}. Set \"db\" in {} and restart navi, or press W once the db exists there",
                Config::config_path().display()
            );
            (Graph::new(Vec::new(), Vec::new()), Some(msg))
        }
    };
    cfg.save();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let mut app_state = AppState::new(graph, cfg, db_path, (views, key), proxy, startup_error);
    event_loop.run_app(&mut app_state)?;
    Ok(())
}

/// All runtime state owned by the winit `ApplicationHandler`.
struct AppState {
    proxy: EventLoopProxy<UserEvent>,
    graph: Option<Graph>,
    cfg: Option<Config>,
    db_path: Option<String>,
    views: Option<(ViewStore, String)>, // store + this db's key
    startup_error: Option<String>,

    // True while the OS reports the window fully hidden (macOS occlusion,
    // X11 VisibilityNotify). Windows/Wayland never emit it — stays false there.
    occluded: bool,

    // Created on `resumed` once we have an `ActiveEventLoop`.
    gl_window: Option<GlutinWindow>,
    gl: Option<Arc<glow::Context>>,
    egui_glow: Option<egui_glow::EguiGlow>,
    app: Option<app::NaviApp>,

    // Idle scheduling: when we're not active, we don't run the display link;
    // we wake on a timer instead. `next_idle_wake` tells `new_events` that the
    // resume-time fired and a redraw is due.
    next_idle_wake: Option<Instant>,

    // Stats
    frame_times: std::collections::VecDeque<f32>,
    last_frame: Instant,
    last_fps_log: Option<Instant>,

    // Set by egui's repaint callback when a widget asks to be repainted
    // sooner than the idle tier paints (an animation, a drag in the panel).
    egui_wants_frames: Arc<std::sync::atomic::AtomicBool>,

    // Wall-clock of the last actual paint. Used to coalesce back-to-back
    // redraw triggers (e.g. a Vsync user-event arriving microseconds after a
    // RedrawRequested from a focus/input event). At 240 Hz the vsync window is
    // ~4.17 ms; anything closer than half that is duplicate work that just
    // contends with the compositor for the same presentation slot.
    last_paint_at: Option<Instant>,
}

impl AppState {
    fn new(
        graph: Graph,
        cfg: Config,
        db_path: String,
        views: (ViewStore, String),
        proxy: EventLoopProxy<UserEvent>,
        startup_error: Option<String>,
    ) -> Self {
        Self {
            proxy,
            graph: Some(graph),
            cfg: Some(cfg),
            db_path: Some(db_path),
            views: Some(views),
            startup_error,
            occluded: false,
            gl_window: None,
            gl: None,
            egui_glow: None,
            app: None,
            next_idle_wake: None,
            frame_times: std::collections::VecDeque::new(),
            last_frame: Instant::now(),
            last_fps_log: None,
            last_paint_at: None,
            egui_wants_frames: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

impl ApplicationHandler<UserEvent> for AppState {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gl_window.is_some() {
            return;
        }

        let borderless = self.cfg.as_ref().map_or(false, |c| c.borderless);
        let (gl_window, gl) = unsafe { GlutinWindow::create(event_loop, borderless) };
        let gl = Arc::new(gl);

        // Explicitly register as a regular, activated app. Without this the
        // process never becomes frontmost: the window gets no keyboard focus
        // (input appears dead) and AX-based tools — AeroSpace, yabai — don't
        // even list it. Also what makes a bare `./navi` from a terminal
        // focusable without an app bundle.
        #[cfg(target_os = "macos")]
        if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
            let ns_app = NSApplication::sharedApplication(mtm);
            ns_app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            #[allow(deprecated)]
            ns_app.activateIgnoringOtherApps(true);
        }

        gl_window.window().set_visible(true);

        let egui_glow = egui_glow::EguiGlow::new(event_loop, gl.clone(), None, None, true);
        setup_fonts(&egui_glow.egui_ctx);
        // Widgets' repaint requests don't trigger paints (vsync / the idle
        // timer do); a request sooner than the idle tier only tells the app
        // to stay at full rate for the next frame (needs_full_speed bounds it).
        let wants = self.egui_wants_frames.clone();
        egui_glow.egui_ctx.set_request_repaint_callback(move |info| {
            if info.delay < Duration::from_millis(33) { wants.store(true, std::sync::atomic::Ordering::Relaxed); }
        });

        // Spawn the display link on a dedicated bg thread. From there it sends
        // `UserEvent::Vsync` to the main thread on every tick. The link is
        // started in the *paused* state — we'll resume it whenever the app
        // enters its full-speed mode.
        #[cfg(target_os = "macos")]
        macos_display::install_bg_link(gl_window.window(), self.proxy.clone());

        let graph = self.graph.take().expect("graph initialised in main()");
        let cfg = self.cfg.take().expect("cfg initialised in main()");
        let db_path = self.db_path.take().expect("db_path initialised in main()");
        let (views, key) = self.views.take().expect("views initialised in main()");
        let mut app = app::NaviApp::new(graph, cfg, db_path, views, key);
        if let Some(msg) = self.startup_error.take() {
            app.set_startup_error(msg);
        }

        self.gl_window = Some(gl_window);
        self.gl = Some(gl);
        self.egui_glow = Some(egui_glow);
        self.app = Some(app);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if std::env::var_os("NAVI_EVENT_DEBUG").is_some()
            && !matches!(event, WindowEvent::CursorMoved { .. })
        {
            eprintln!("navi-event: {:?}", event);
        }
        if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {
            event_loop.exit();
            return;
        }
        if let WindowEvent::Resized(size) = &event {
            if let Some(gw) = self.gl_window.as_ref() {
                gw.resize(*size);
            }
        }

        if matches!(event, WindowEvent::RedrawRequested) {
            self.redraw(event_loop);
            return;
        }

        // Fully-hidden tracking (macOS occlusion state, X11 VisibilityNotify;
        // never emitted on Windows/Wayland). While hidden, redraw() skips
        // painting entirely and keeps a slow poll timer instead.
        if let WindowEvent::Occluded(occluded) = &event {
            self.occluded = *occluded;
            if !*occluded {
                if let Some(gw) = self.gl_window.as_ref() {
                    gw.window().request_redraw();
                }
            }
        }

        // True iff we're currently in idle (link paused, waiting on the 33 ms
        // resume timer). In that mode we *do* want input/focus events to kick
        // an immediate redraw so the cadence transition happens within one
        // frame. In active mode we explicitly do NOT request_redraw — the
        // CADisplayLink Vsync user-events are the sole paint trigger, and any
        // extra request_redraw causes a double-paint that contends with the
        // compositor and produces a missed-vsync outlier.
        let is_idle = self.next_idle_wake.is_some();

        // Focus changes drive the idle/active cadence. Gaining focus instantly
        // restores the full-speed tier; losing focus lets the next paint drop
        // us straight into idle mode.
        if let WindowEvent::Focused(focused) = &event {
            if let Some(app) = self.app.as_mut() {
                app.set_focused(*focused);
            }
            if *focused && is_idle {
                if let Some(gw) = self.gl_window.as_ref() {
                    gw.window().request_redraw();
                }
            }
        }

        // Any user input bumps the activity timer (keeps us in full-speed mode
        // for `idle_grace`). Only kick a redraw if we're idle right now.
        let is_input = matches!(
            event,
            WindowEvent::CursorMoved { .. }
                | WindowEvent::CursorEntered { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::KeyboardInput { .. }
                | WindowEvent::ModifiersChanged(..)
                | WindowEvent::Ime(..)
                | WindowEvent::Touch(..)
                | WindowEvent::PinchGesture { .. }
                | WindowEvent::PanGesture { .. }
                | WindowEvent::RotationGesture { .. }
        );
        if is_input {
            if let Some(app) = self.app.as_mut() {
                app.touch_input();
            }
            if is_idle {
                if let Some(gw) = self.gl_window.as_ref() {
                    gw.window().request_redraw();
                }
            }
        }

        // Feed the event to egui_winit so it can update its input state. We
        // *deliberately* ignore `event_response.repaint` — the whole point of
        // this rewrite is to decouple our redraw cadence from input arrival.
        // Display vsync ticks are the only signal that triggers a paint.
        if let (Some(eg), Some(gw)) = (self.egui_glow.as_mut(), self.gl_window.as_ref()) {
            let _ = eg.on_window_event(gw.window(), &event);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Vsync => {
                // Paint *directly* — skip the request_redraw → RedrawRequested
                // round-trip through winit's queue. That extra hop costs us
                // some latency on every frame and, when we're close to the
                // vsync deadline, occasionally pushes us into the next vsync
                // window and produces a 8–12 ms outlier (visible micro-stutter).
                self.redraw(event_loop);
            }
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: winit::event::StartCause) {
        if let winit::event::StartCause::ResumeTimeReached { .. } = cause {
            if let Some(gw) = self.gl_window.as_ref() {
                gw.window().request_redraw();
            }
            self.next_idle_wake = None;
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // views.json saves are debounced: write a pending edit before going
        if let Some(app) = self.app.as_mut() {
            app.shutdown();
        }
        if let Some(eg) = self.egui_glow.as_mut() {
            eg.destroy();
        }
    }
}

impl AppState {
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();

        // Fully hidden: don't paint at all. Keep a slow wake-up so pending
        // reloads still land; the Occluded(false) handler repaints immediately.
        if self.occluded {
            #[cfg(target_os = "macos")]
            macos_display::set_active(false);
            let next = now + Duration::from_millis(250);
            self.next_idle_wake = Some(next);
            event_loop.set_control_flow(ControlFlow::WaitUntil(next));
            return;
        }

        // Coalesce duplicate redraw triggers. At 240 Hz the vsync window is
        // ~4.17 ms; a paint that lands within half of that of the previous
        // paint is a duplicate (e.g. a Vsync arrived microseconds after a
        // RedrawRequested from the same vsync, or two redraw paths fired in
        // rapid succession). Painting twice in the same compositor window
        // contends for the same presentation slot and shows up as an 8–12 ms
        // worst-case frame in the FPS log even when avg is locked at 4.17.
        if let Some(prev) = self.last_paint_at {
            if now.duration_since(prev) < Duration::from_micros(2_000) {
                // On swap-paced platforms this early-return would otherwise be
                // the death of the redraw chain (nothing else is scheduled) —
                // re-arm a short timer before bailing.
                #[cfg(not(target_os = "macos"))]
                event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(4)));
                return;
            }
        }

        // Frame timing
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;
        self.frame_times.push_back(dt);
        while self.frame_times.iter().sum::<f32>() > 1.0 && self.frame_times.len() > 2 {
            self.frame_times.pop_front();
        }

        let egui_busy = self.egui_wants_frames.swap(false, std::sync::atomic::Ordering::Relaxed);
        let needs_full_speed = match self.app.as_mut() {
            Some(a) => { a.set_egui_busy(egui_busy); a.needs_full_speed() }
            None => false,
        };

        // Toggle the display link based on app state. Active = link runs at
        // 240 Hz and pumps Vsync events; Idle = link paused, OS drops the
        // refresh tier, we use a 33 ms timer for the next wake-up.
        #[cfg(target_os = "macos")]
        macos_display::set_active(needs_full_speed);
        if !needs_full_speed {
            // 33 ms normally; stretch to 1 s once long-unfocused — platforms
            // without occlusion events (Wayland, composited X11) would
            // otherwise paint a fully covered window at 30 fps forever.
            let long_idle = self.app.as_ref().map_or(false, |a| a.long_idle());
            let next = now + Duration::from_millis(if long_idle { 1000 } else { 33 });
            self.next_idle_wake = Some(next);
            event_loop.set_control_flow(ControlFlow::WaitUntil(next));
        } else {
            // While active, we paint only when a Vsync user-event arrives. No
            // polling, no busy-looping. Clear `next_idle_wake` so the
            // input/focus handlers in window_event know we're no longer in the
            // idle path and don't fire extra request_redraw calls (which would
            // cause double-paints).
            self.next_idle_wake = None;
            // macOS: keep a 100 ms fallback deadline armed. If the display
            // link isn't delivering (e.g. still starting up), we degrade to
            // slow paints instead of freezing; while the link runs normally,
            // every Vsync paint re-arms this before it can fire, and a
            // spurious firing is swallowed by the coalescing guard above.
            #[cfg(target_os = "macos")]
            event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(100)));
            #[cfg(not(target_os = "macos"))]
            event_loop.set_control_flow(ControlFlow::Wait);
        }

        // Run the egui frame.
        let gw = self.gl_window.as_mut().expect("gl_window");
        let eg = self.egui_glow.as_mut().expect("egui_glow");
        let app = self.app.as_mut().expect("app");

        eg.run(gw.window(), |ctx| {
            app.update(ctx);
        });

        // Q/Escape: exit through the event loop so exiting() destroys the GL
        // state properly (drivers on Linux dislike a raw process::exit).
        if app.take_quit_request() {
            event_loop.exit();
            return;
        }

        // Paint.
        let theme_bg = app.bg_color();
        unsafe {
            use glow::HasContext as _;
            let gl = self.gl.as_ref().expect("gl");
            gl.clear_color(theme_bg[0], theme_bg[1], theme_bg[2], 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        eg.paint(gw.window());
        #[cfg(not(target_os = "macos"))]
        let swap_started = Instant::now();
        let _ = gw.swap_buffers();
        self.last_paint_at = Some(now);

        // Non-macOS pacer: there's no CADisplayLink-style bg thread sending
        // Vsync user-events, so re-arm the next frame ourselves while active.
        //   X11 / Windows: `swap_buffers` above normally blocked until vblank
        //   (`Wait(1)`), so request_redraw isn't a busy loop — it's paced at
        //   the display's refresh rate. But swaps can stop blocking (window
        //   unmapped on an i3 workspace switch, minimized on Windows, driver
        //   with vsync forced off) — if this swap returned instantly, fall
        //   back to timer pacing so we neither stall nor spin.
        //   Wayland: the swap must never block (compositors stop frame
        //   callbacks for hidden surfaces, which would freeze us), so the
        //   swap interval is DontWait there and the timer is the only pacer.
        #[cfg(not(target_os = "macos"))]
        if needs_full_speed {
            let swap_blocked = swap_started.elapsed() >= Duration::from_millis(1);
            if gw.paces_via_swap && swap_blocked {
                gw.window().request_redraw();
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(now + gw.frame_interval()));
            }
        }

        // Diagnostics
        if std::env::var_os("NAVI_FPS_LOG").is_some() {
            let due = self
                .last_fps_log
                .map_or(true, |t| t.elapsed() >= Duration::from_secs(1));
            if due && self.frame_times.len() >= 2 {
                let n = self.frame_times.len() as f32;
                let total: f32 = self.frame_times.iter().sum();
                let avg_ms = total / n * 1000.0;
                let max_ms = self
                    .frame_times
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max)
                    * 1000.0;
                let fps = n / total;
                #[cfg(target_os = "macos")]
                let link_info = {
                    let ticks = macos_display::tick_count();
                    let interval_us = macos_display::vsync_interval()
                        .map(|d| d.as_micros() as u64)
                        .unwrap_or(0);
                    let hz = if interval_us > 0 {
                        1_000_000.0 / interval_us as f64
                    } else {
                        0.0
                    };
                    format!("  link_total_ticks={ticks}  link_interval={interval_us}us ({hz:.0}Hz)")
                };
                #[cfg(not(target_os = "macos"))]
                let link_info = String::new();
                eprintln!(
                    "navi: fps={:.1}  avg={:.2}ms  worst={:.2}ms  frames={}{}",
                    fps,
                    avg_ms,
                    max_ms,
                    self.frame_times.len(),
                    link_info,
                );
                self.last_fps_log = Some(now);
            }
        }
    }
}

// ─── Glutin context plumbing ───────────────────────────────────────────────────

struct GlutinWindow {
    window: Window,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_display: glutin::display::Display,
    gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
    /// True where `swap_buffers` blocks until vblank and thereby paces the
    /// render loop (X11, Windows). False on macOS (display link paces) and
    /// Wayland (timer paces).
    paces_via_swap: bool,
}

impl GlutinWindow {
    /// Construct the window + GL context. Adapted from egui_glow's pure_glow example.
    unsafe fn create(event_loop: &ActiveEventLoop, borderless: bool) -> (Self, glow::Context) {
        // Clamp the default size to the monitor so HiDPI X11 setups (where
        // winit may infer a 2–3× scale from EDID) don't open bigger than the
        // screen.
        let mut initial_size = winit::dpi::LogicalSize::<f64>::new(1400.0, 900.0);
        if let Some(mon) = event_loop.primary_monitor() {
            let logical: winit::dpi::LogicalSize<f64> = mon.size().to_logical(mon.scale_factor());
            if logical.width > 0.0 && logical.height > 0.0 {
                initial_size.width = initial_size.width.min(logical.width * 0.92);
                initial_size.height = initial_size.height.min(logical.height * 0.92);
            }
        }

        #[allow(unused_mut)]
        let mut window_attrs = winit::window::WindowAttributes::default()
            .with_resizable(true)
            .with_inner_size(initial_size)
            .with_min_inner_size(winit::dpi::LogicalSize::new(400.0, 300.0))
            .with_title("Navi")
            // Normal decorated window by default; `"borderless": true` in
            // config.json opts back into the chromeless look. Tiling WMs
            // (AeroSpace, i3) manage the decorated window much better.
            .with_decorations(!borderless)
            .with_visible(false);

        // WM_CLASS on X11 / app_id on Wayland (both read the same attribute) —
        // gives i3/sway users a stable handle: `for_window [class="navi"] …`
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use winit::platform::x11::WindowAttributesExtX11;
            window_attrs = window_attrs.with_name("navi", "navi");
        }

        // Taskbar/titlebar icon — honored on Windows and X11; macOS uses the
        // app bundle's icns and Wayland the .desktop file, both no-ops here.
        #[cfg(not(target_os = "macos"))]
        {
            if let Some(icon) = load_window_icon() {
                window_attrs = window_attrs.with_window_icon(Some(icon));
            }
        }

        let cfg_template = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);

        let (mut window_opt, gl_config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(window_attrs.clone()))
            .build(event_loop, cfg_template, |it| {
                // Drivers list many configs; the first isn't always sane
                // (X11/Mesa may lead with multisampled ones). egui does its own
                // AA, so prefer no MSAA, then sRGB capability.
                use glutin::config::GlConfig as _;
                it.reduce(|best, c| {
                    let score = |c: &glutin::config::Config| -> i32 {
                        let mut s = 0;
                        if c.num_samples() == 0 { s += 100; }
                        if c.srgb_capable() { s += 10; }
                        s
                    };
                    if score(&c) > score(&best) { c } else { best }
                })
                .expect("no matching gl config")
            })
            .expect("failed to create gl_config");

        let gl_display = gl_config.display();
        let raw_window_handle = window_opt
            .as_ref()
            .map(|w| w.window_handle().expect("window handle").as_raw());

        let context_attrs =
            glutin::context::ContextAttributesBuilder::new().build(raw_window_handle);
        let fallback_attrs = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(raw_window_handle);

        let not_current = unsafe {
            gl_display
                .create_context(&gl_config, &context_attrs)
                .unwrap_or_else(|_| {
                    gl_display
                        .create_context(&gl_config, &fallback_attrs)
                        .expect("failed to create gl context")
                })
        };

        let window = window_opt.take().unwrap_or_else(|| {
            glutin_winit::finalize_window(event_loop, window_attrs.clone(), &gl_config)
                .expect("failed to finalize window")
        });

        let (w, h): (u32, u32) = window.inner_size().into();
        let surface_attrs =
            glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
                .build(
                    window.window_handle().expect("window handle").as_raw(),
                    NonZeroU32::new(w).unwrap_or(NonZeroU32::MIN),
                    NonZeroU32::new(h).unwrap_or(NonZeroU32::MIN),
                );

        let gl_surface = unsafe {
            gl_display
                .create_window_surface(&gl_config, &surface_attrs)
                .expect("failed to create surface")
        };
        let gl_context = not_current
            .make_current(&gl_surface)
            .expect("failed to make context current");

        // macOS: vsync OFF. macOS OpenGL's swap-vsync is capped at ~108 Hz on
        // ProMotion panels regardless of what tier the display is actually
        // running at, so letting it block here would clamp us to 108 fps even
        // though the display link is firing at 240 Hz. Pacing is owned entirely
        // by the bg-thread display link in macos_display.rs.
        //
        // X11 / Windows: there is no display-link equivalent, so the blocking
        // swap itself is the pacer — `redraw()` re-arms itself with
        // `request_redraw()` right after painting, and `Wait(1)` throttles that
        // loop to the monitor's actual refresh rate via the GL driver/compositor.
        //
        // Wayland: a blocking swap waits on compositor frame callbacks, which
        // stop entirely for hidden/occluded surfaces (and winit can't tell us —
        // Occluded is unsupported there), so a Wait(1) swap can freeze the app.
        // Swap DontWait and pace with a WaitUntil timer at the monitor's rate.
        let is_wayland = matches!(
            window.window_handle().map(|h| h.as_raw()),
            Ok(raw_window_handle::RawWindowHandle::Wayland(_))
        );
        let paces_via_swap = !cfg!(target_os = "macos") && !is_wayland;
        let swap_interval = if paces_via_swap {
            glutin::surface::SwapInterval::Wait(NonZeroU32::new(1).unwrap())
        } else {
            glutin::surface::SwapInterval::DontWait
        };
        let _ = gl_surface.set_swap_interval(&gl_context, swap_interval);

        let gl = unsafe {
            glow::Context::from_loader_function(|s| {
                let cstr = std::ffi::CString::new(s).unwrap();
                gl_display.get_proc_address(&cstr)
            })
        };

        (
            Self {
                window,
                gl_context,
                gl_display,
                gl_surface,
                paces_via_swap,
            },
            gl,
        )
    }

    fn window(&self) -> &Window {
        &self.window
    }

    /// One display refresh period, for the Wayland timer pacer.
    #[cfg(not(target_os = "macos"))]
    fn frame_interval(&self) -> Duration {
        self.window
            .current_monitor()
            .and_then(|m| m.refresh_rate_millihertz())
            .filter(|&mhz| mhz > 0)
            .map(|mhz| Duration::from_secs_f64(1000.0 / mhz as f64))
            .unwrap_or(Duration::from_micros(16_667))
    }

    fn resize(&self, size: winit::dpi::PhysicalSize<u32>) {
        let _ = &self.gl_display; // silence "unused"
        self.gl_surface.resize(
            &self.gl_context,
            NonZeroU32::new(size.width).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(size.height).unwrap_or(NonZeroU32::MIN),
        );
    }

    fn swap_buffers(&self) -> glutin::error::Result<()> {
        self.gl_surface.swap_buffers(&self.gl_context)
    }
}

// ─── Fonts ────────────────────────────────────────────────────────────────────

/// Load a prioritised font fallback chain (appended after egui's defaults) so
/// egui can render Nerd Font symbols, CJK, and misc Unicode where the system
/// has suitable fonts. Everything here is best-effort: a missing font just
/// means those glyphs fall back to egui's built-ins (or tofu).
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, path) in font_candidates() {
        if let Ok(data) = std::fs::read(&path) {
            fonts
                .font_data
                .insert(name.clone(), egui::FontData::from_owned(data));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push(name.clone());
            }
        }
    }
    ctx.set_fonts(fonts);
}

#[cfg(target_os = "macos")]
fn font_candidates() -> Vec<(String, std::path::PathBuf)> {
    let home = std::env::var("HOME").unwrap_or_default();
    [
        ("nerd",     format!("{home}/Library/Fonts/MesloLGLNerdFont-Regular.ttf")),
        ("jp",       "/Library/Fonts/NotoSansJP-Regular.otf".into()),
        ("unicode",  "/Library/Fonts/Arial Unicode.ttf".into()),
        ("symbols2", "/Library/Fonts/NotoSansSymbols2-Regular.ttf".into()),
    ]
    .into_iter()
    .map(|(n, p)| (n.to_string(), std::path::PathBuf::from(p)))
    .collect()
}

/// Linux: font locations aren't standardized across distros, so walk the usual
/// font roots (bounded) looking for well-known single-face files. Deliberately
/// skips .ttc collections — we only hand egui files it is known to parse.
#[cfg(all(unix, not(target_os = "macos")))]
fn font_candidates() -> Vec<(String, std::path::PathBuf)> {
    use std::path::PathBuf;
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(&home).join(".local/share/fonts"));
        roots.push(PathBuf::from(&home).join(".fonts"));
    }
    roots.push(PathBuf::from("/usr/share/fonts"));
    roots.push(PathBuf::from("/usr/local/share/fonts"));

    // slot → exact filenames accepted for it, in preference order. .ttc/.otc
    // collections are fine — egui's FontData loads face 0.
    let wanted: &[(&str, &[&str])] = &[
        ("jp",       &["NotoSansJP-Regular.otf", "NotoSansJP-Regular.ttf", "NotoSansCJKjp-Regular.otf",
                       "NotoSansCJK-Regular.ttc", "NotoSansCJKjp-Regular.otc", "NotoSansCJK.ttc"]),
        ("unicode",  &["DejaVuSans.ttf", "NotoSans-Regular.ttf"]),
        ("symbols2", &["NotoSansSymbols2-Regular.ttf"]),
    ];

    let mut found: Vec<(String, PathBuf)> = Vec::new();
    let mut nerd: Option<PathBuf> = None;
    let mut stack: Vec<(PathBuf, u8)> = roots.into_iter().map(|r| (r, 0u8)).collect();
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > 800 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < 4 {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if nerd.is_none() && fname.contains("NerdFont") && fname.ends_with("-Regular.ttf") {
                nerd = Some(path.clone());
            }
            for (slot, names) in wanted {
                if names.contains(&fname) && !found.iter().any(|(s, _)| s == slot) {
                    found.push((slot.to_string(), path.clone()));
                }
            }
        }
    }
    if let Some(p) = nerd {
        found.insert(0, ("nerd".to_string(), p));
    }
    found
}

#[cfg(windows)]
fn font_candidates() -> Vec<(String, std::path::PathBuf)> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let fonts = std::path::PathBuf::from(windir).join("Fonts");
    // .ttc collections are fine — egui's FontData loads face 0. Windows ships
    // no single-face JP font, so the JP slot has to be a collection.
    let mut out = Vec::new();
    for jp in ["YuGothM.ttc", "yugothm.ttc", "meiryo.ttc", "msgothic.ttc"] {
        let p = fonts.join(jp);
        if p.is_file() {
            out.push(("jp".to_string(), p));
            break;
        }
    }
    out.push(("symbols".to_string(), fonts.join("seguisym.ttf")));
    out
}

/// Decode the bundled icon for the window/taskbar (Windows + X11).
#[cfg(not(target_os = "macos"))]
fn load_window_icon() -> Option<winit::window::Icon> {
    let png = include_bytes!("../../assets/icon.png");
    let img = image::load_from_memory(png).ok()?.thumbnail(64, 64).into_rgba8();
    let (w, h) = img.dimensions();
    winit::window::Icon::from_rgba(img.into_raw(), w, h).ok()
}
