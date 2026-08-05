//! notion-river: A Notion/Ion3-style static tiling window manager for River.
//!
//! This WM implements the key concept from Notion: the screen layout is a
//! persistent wireframe of frames that exist independently of windows.
//! Windows are placed into frames as tabs. Opening/closing windows never
//! changes the layout — only explicit user actions (split/unsplit) do.

mod actions;
mod app_bindings;
mod bindings;
mod config;
mod control;
mod decorations;
mod dispatch;
mod focus;
mod ipc;
mod layout;
mod monitor_memory;
mod monitors;
mod pointer_ops;
mod protocol;
mod rendering;
mod state;
mod window_actions;
mod wm;
mod workspace;

use std::os::unix::io::AsRawFd;
use wayland_client::Connection;

use crate::wm::AppData;

/// Wrapper that flushes after every write (line-buffered).
struct LineFlush(std::fs::File);

impl std::io::Write for LineFlush {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.0.write(buf)?;
        self.0.flush()?;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

fn main() {
    if let Err(e) = run() {
        log::error!("Fatal: {e}");
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Handle --version / -V before anything else
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("notion-river {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Log rotation: current run → notion-river.log, previous run → notion-river.log.prev
    // On startup, rotate so the previous run's log is always preserved for crash investigation.
    let log_path = "/tmp/notion-river.log";
    let prev_path = "/tmp/notion-river.log.prev";
    let _ = std::fs::rename(log_path, prev_path);
    let log_target = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path);

    // Parse the desired log level from RUST_LOG, defaulting to "info".
    // We initialize env_logger with "trace" so it never filters anything itself,
    // then use log::set_max_level() to control the effective level at runtime.
    // This allows changing the log level via IPC (notion-ctl set-log-level debug).
    let initial_level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse::<log::LevelFilter>().ok())
        .unwrap_or(log::LevelFilter::Info);

    let mut builder = env_logger::Builder::new();
    builder.filter_level(log::LevelFilter::Trace);
    if let Ok(file) = log_target {
        builder.target(env_logger::Target::Pipe(Box::new(LineFlush(file))));
    }
    builder.init();
    log::set_max_level(initial_level);

    log::info!("notion-river starting");

    // Connect to the Wayland compositor (River).
    let conn = Connection::connect_to_env()?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let _registry = display.get_registry(&event_queue.handle(), ());

    let mut app_data = AppData::default();

    // Roundtrip to discover globals.
    event_queue.roundtrip(&mut app_data)?;

    if app_data.river_wm.is_none() {
        log::error!("river_window_manager_v1 global not found. Is River (0.4.x) running?");
        std::process::exit(1);
    }
    if app_data.river_xkb.is_none() {
        log::error!("river_xkb_bindings_v1 global not found.");
        std::process::exit(1);
    }

    log::info!(
        "Connected to River. Profile: '{}', physical_keys: {}",
        app_data.wm.config.active_profile,
        app_data.wm.config.general.physical_keys
    );

    // Set up signal handler for clean shutdown with state save
    use std::sync::atomic::{AtomicBool, Ordering};
    static SHUTDOWN: AtomicBool = AtomicBool::new(false);
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = signal_handler as *const () as usize;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());

        // Auto-reap spawned children (terminals, launchers, etc.) so they
        // don't become zombies. We never wait() on Child handles from
        // spawn_command — SA_NOCLDWAIT tells the kernel to discard exit
        // status automatically.
        let mut chld: libc::sigaction = std::mem::zeroed();
        chld.sa_sigaction = libc::SIG_DFL;
        chld.sa_flags = libc::SA_NOCLDWAIT;
        libc::sigemptyset(&mut chld.sa_mask);
        libc::sigaction(libc::SIGCHLD, &chld, std::ptr::null_mut());
    }
    extern "C" fn signal_handler(_sig: libc::c_int) {
        SHUTDOWN.store(true, Ordering::Relaxed);
    }

    // Main event loop — poll both Wayland fd and control socket notify fd.
    let wayland_fd = event_queue.prepare_read().map(|g| { let fd = g.connection_fd().as_raw_fd(); drop(g); fd }).unwrap_or(-1);
    let control_fd = app_data.wm.control.notify_fd;

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            log::info!("Signal received, saving state and exiting");
            crate::state::save_state(&app_data.wm.workspaces, &app_data.wm.windows);
            std::process::exit(0);
        }

        // Fire any debounced monitor-profile apply whose settle window has
        // elapsed. Returns the next wake-up deadline if an apply is still
        // pending, so we can shorten the poll timeout to service it promptly.
        let pending_deadline = {
            let qh = event_queue.handle();
            app_data.maybe_fire_pending_apply(&qh)
        };

        // Flush outgoing requests
        conn.flush()?;

        // Poll timeout: 1s for the shutdown check, shortened when a monitor
        // apply is pending so the debounce deadline is honored on time.
        let timeout_ms = match pending_deadline {
            Some(deadline) => {
                let remaining = deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .as_millis();
                (remaining as i32).clamp(10, 1000)
            }
            None => 1000,
        };

        // Poll both fds
        let mut fds = [
            libc::pollfd { fd: wayland_fd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: control_fd, events: libc::POLLIN, revents: 0 },
        ];
        unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout_ms) };

        // If control socket has data, drain it and trigger manage_dirty
        if fds[1].revents & libc::POLLIN != 0 {
            app_data.wm.control.drain_notify();
            if let Some(ref wm_proxy) = app_data.river_wm {
                wm_proxy.manage_dirty();
            }
        }

        // Process Wayland events (non-blocking dispatch)
        event_queue.dispatch_pending(&mut app_data)?;
        if let Some(guard) = event_queue.prepare_read() {
            if fds[0].revents & libc::POLLIN != 0 {
                guard.read()?;
            } else {
                drop(guard);
            }
            event_queue.dispatch_pending(&mut app_data)?;
        }
    }
}
