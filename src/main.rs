//! notion-river: A Notion/Ion3-style static tiling window manager for River.
//!
//! This WM implements the key concept from Notion: the screen layout is a
//! persistent wireframe of frames that exist independently of windows.
//! Windows are placed into frames as tabs. Opening/closing windows never
//! changes the layout — only explicit user actions (split/unsplit) do.

mod actions;
mod bindings;
mod config;
mod dispatch;
mod layout;
mod protocol;
mod wm;
mod workspace;

use wayland_client::Connection;

use crate::wm::AppData;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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

    // Main event loop.
    loop {
        event_queue.blocking_dispatch(&mut app_data)?;
    }
}
