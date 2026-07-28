use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use crate::graph::Node;

#[cfg(unix)]
static EMACSCLIENT_CANDIDATES: &[&str] = &[
    "emacsclient",
    "/opt/homebrew/bin/emacsclient",
    "/usr/local/bin/emacsclient",
    "/usr/bin/emacsclient",
    "/opt/local/bin/emacsclient",
    "/Applications/Emacs.app/Contents/MacOS/bin/emacsclient",
    "/Applications/Emacs.app/Contents/MacOS/emacsclient",
    "/run/current-system/sw/bin/emacsclient",
    "/snap/bin/emacsclient",
    "~/.local/bin/emacsclient",
    "~/.nix-profile/bin/emacsclient",
];

// emacsclientw.exe first: it has no console, so spawning it from a GUI app
// never flashes a cmd window. Plain emacsclient.exe is the fallback.
#[cfg(windows)]
static EMACSCLIENT_CANDIDATES: &[&str] = &[
    "emacsclientw",
    "emacsclient",
    r"C:\Program Files\Emacs\emacs\bin\emacsclientw.exe",
    r"C:\Program Files\Emacs\bin\emacsclientw.exe",
    r"C:\Program Files (x86)\Emacs\emacs\bin\emacsclientw.exe",
    r"C:\msys64\mingw64\bin\emacsclientw.exe",
    r"C:\msys64\ucrt64\bin\emacsclientw.exe",
    r"~\scoop\apps\emacs\current\bin\emacsclientw.exe",
    r"C:\ProgramData\chocolatey\bin\emacsclientw.exe",
];

pub struct EmacsClient {
    pub binary: String,
    pub server_name: String,
}

impl EmacsClient {
    pub fn new(cfg_binary: &str, server_name: &str) -> Self {
        let cfg_binary = expand_home(cfg_binary);
        let binary = if !cfg_binary.is_empty() && Path::new(&cfg_binary).exists() {
            cfg_binary
        } else {
            detect_emacsclient()
        };
        EmacsClient { binary, server_name: server_name.to_string() }
    }

    pub fn open_node(&self, node: &Node) -> Result<(), String> {
        if node.file.is_empty() || !Path::new(&node.file).exists() {
            return Err(format!("File not found: {}", node.file));
        }

        let mut cmd_args: Vec<String> = Vec::new();
        match find_emacs_server(&self.server_name) {
            Some(ServerLocation::Socket(s)) => {
                cmd_args.push("--socket-name".into());
                cmd_args.push(s);
            }
            Some(ServerLocation::AuthFile(f)) => {
                cmd_args.push("--server-file".into());
                cmd_args.push(f);
            }
            None => {}
        }
        cmd_args.push("--no-wait".into());
        cmd_args.push("--alternate-editor=".into());
        cmd_args.push("--eval".into());

        let path = node.file.replace('\\', "\\\\").replace('"', "\\\"");
        let goto = if node.level > 0 && node.pos > 0 {
            format!(" (goto-char {})", node.pos)
        } else {
            String::new()
        };
        let elisp = format!(
            "(progn (find-file \"{path}\"){goto} (delete-other-windows) (when (display-graphic-p) (raise-frame)))"
        );
        cmd_args.push(elisp);

        let mut cmd = Command::new(&self.binary);
        cmd.args(&cmd_args).stdout(Stdio::null()).stderr(Stdio::piped());
        // If we ended up with the console flavour of emacsclient on Windows,
        // keep its console window from flashing over the app.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.spawn()
            .map(|mut child| {
                // Reap the child off-thread so it doesn't linger as a zombie
                // (one per opened node adds up on Linux).
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            })
            .map_err(|e| format!("emacsclient failed: {e}"))
    }
}

/// Where a running Emacs server can be reached.
enum ServerLocation {
    /// Unix domain socket path → `--socket-name`.
    Socket(String),
    /// TCP auth file (the Windows default; `server-use-tcp t` elsewhere) → `--server-file`.
    AuthFile(String),
}

fn detect_emacsclient() -> String {
    for cand in EMACSCLIENT_CANDIDATES {
        let expanded = expand_home(cand);
        if Path::new(&expanded).is_file() {
            return expanded;
        }
        if let Some(found) = find_in_path(cand) {
            return found;
        }
    }
    // Official Windows installer unpacks to a versioned dir:
    //   C:\Program Files\Emacs\emacs-<version>\bin\emacsclientw.exe
    #[cfg(windows)]
    for root in [r"C:\Program Files\Emacs", r"C:\Program Files (x86)\Emacs", r"C:\tools\emacs"] {
        if let Ok(entries) = std::fs::read_dir(root) {
            let mut vers: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            vers.sort();
            for dir in vers.iter().rev() {
                for exe in ["emacsclientw.exe", "emacsclient.exe"] {
                    let p = dir.join("bin").join(exe);
                    if p.is_file() {
                        return p.to_string_lossy().into_owned();
                    }
                }
            }
        }
    }
    "emacsclient".to_string()
}

fn expand_home(path: &str) -> String {
    let rest = path
        .strip_prefix("~/")
        .or_else(|| path.strip_prefix(r"~\"));
    if let Some(rest) = rest {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// Search PATH for `name` without shelling out to `which` (which doesn't exist
/// on Windows and inherits a GUI app's often-minimal PATH anyway).
fn find_in_path(name: &str) -> Option<String> {
    if name.contains('/') || name.contains('\\') {
        return None;
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand.to_string_lossy().into_owned());
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Locate the running Emacs server.
///
/// Unix: the server is a domain socket — `$XDG_RUNTIME_DIR/emacs/<name>` on
/// Linux, `$TMPDIR/emacs<uid>/<name>` on macOS. We search explicitly instead of
/// trusting emacsclient's defaults because a GUI-launched app frequently has a
/// different TMPDIR/environment than the shell Emacs was started from.
///
/// Windows: Emacs defaults to TCP (`server-use-tcp t`); the server writes an
/// auth file to `server-auth-dir` (default `~/.emacs.d/server/<name>`), where
/// `~` follows Emacs' own home logic (HOME, then APPDATA). Recent Emacs can
/// also use local sockets on Windows, but the TCP auth file remains the
/// default, so that's what we look for.
fn find_emacs_server(server_name: &str) -> Option<ServerLocation> {
    // --socket-name doesn't exist in Windows emacsclient builds, so the
    // socket-flavoured env vars are unix-only.
    #[cfg(unix)]
    for key in &["EMACS_SOCKET_NAME", "EMACS_SERVER_SOCKET"] {
        if let Ok(v) = std::env::var(key) {
            if Path::new(&v).exists() {
                return Some(ServerLocation::Socket(v));
            }
        }
    }
    if let Ok(v) = std::env::var("EMACS_SERVER_FILE") {
        if Path::new(&v).exists() {
            return Some(ServerLocation::AuthFile(v));
        }
    }

    let names: Vec<&str> = if server_name != "server" {
        vec![server_name, "server"]
    } else {
        vec!["server"]
    };

    #[cfg(unix)]
    {
        // XDG_RUNTIME_DIR (Linux)
        if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
            for name in &names {
                let p = format!("{}/emacs/{}", xdg, name);
                if Path::new(&p).exists() {
                    return Some(ServerLocation::Socket(p));
                }
            }
        }

        // Temp dirs (macOS default, plus /tmp fallbacks)
        let uid = unsafe { getuid() };
        let bases: Vec<PathBuf> = {
            let mut b = Vec::new();
            for key in &["TMPDIR", "TMP", "TEMP"] {
                if let Ok(v) = std::env::var(key) {
                    b.push(PathBuf::from(v));
                }
            }
            b.push(PathBuf::from("/tmp"));
            b.push(PathBuf::from("/private/tmp"));
            b
        };
        for base in bases {
            for name in &names {
                let p = base.join(format!("emacs{}", uid)).join(name);
                if p.exists() {
                    return Some(ServerLocation::Socket(p.to_string_lossy().into_owned()));
                }
            }
        }

        // No socket — the server may be running in TCP mode (`server-use-tcp`),
        // which writes an auth file to server-auth-dir instead.
        if let Some(home) = dirs::home_dir() {
            let xdg_cfg = std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".config"));
            let dirs_ = [
                home.join(".emacs.d").join("server"),
                xdg_cfg.join("emacs").join("server"),
            ];
            for dir in dirs_ {
                for name in &names {
                    let p = dir.join(name);
                    if p.is_file() {
                        return Some(ServerLocation::AuthFile(p.to_string_lossy().into_owned()));
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        // Emacs resolves `~` as HOME first, then APPDATA. USERPROFILE and the
        // XDG-style config dir cover Emacs 29+ setups.
        let mut homes: Vec<PathBuf> = Vec::new();
        for key in &["HOME", "APPDATA", "USERPROFILE"] {
            if let Ok(v) = std::env::var(key) {
                if !v.is_empty() {
                    homes.push(PathBuf::from(v));
                }
            }
        }
        let mut server_dirs: Vec<PathBuf> = Vec::new();
        for h in &homes {
            server_dirs.push(h.join(".emacs.d").join("server"));
            server_dirs.push(h.join(".config").join("emacs").join("server"));
        }
        for dir in server_dirs {
            for name in &names {
                let p = dir.join(name);
                if p.is_file() {
                    return Some(ServerLocation::AuthFile(p.to_string_lossy().into_owned()));
                }
            }
        }
    }

    None
}

#[cfg(unix)]
extern "C" {
    fn getuid() -> u32;
}
