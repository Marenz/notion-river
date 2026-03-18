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
mod output_profiles;
mod pointer_ops;
mod protocol;
mod rendering;
mod state;
mod window_actions;
mod wm;
mod workspace;

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Log to /tmp/notion-river.log since River's child stderr goes to a socket.
    let log_target = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/notion-river.log");

    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if let Ok(file) = log_target {
        builder.target(env_logger::Target::Pipe(Box::new(LineFlush(file))));
    }
    builder.init();

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
    }
    extern "C" fn signal_handler(_sig: libc::c_int) {
        SHUTDOWN.store(true, Ordering::Relaxed);
    }

    // Main event loop.
    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            log::info!("Signal received, saving state and exiting");
            crate::state::save_state(&app_data.wm.workspaces, &app_data.wm.windows);
            std::process::exit(0);
        }
        event_queue.blocking_dispatch(&mut app_data)?;
    }
}
