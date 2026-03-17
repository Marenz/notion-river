//! IPC: write workspace state for waybar's custom module.
//!
//! Writes a JSON line to a named pipe whenever workspace state changes.
//! Waybar reads this via a `custom/workspaces` module.

use std::io::Write;
use std::path::PathBuf;

use crate::workspace::WorkspaceManager;

fn ipc_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("notion-river-workspaces")
}

/// Generate waybar JSON for workspace state.
/// Format: {"text": "display text", "tooltip": "hover text", "class": "css-class"}
pub fn workspace_json(workspaces: &WorkspaceManager) -> String {
    let mut parts = Vec::new();

    for ws in &workspaces.workspaces {
        let is_focused = ws.id == workspaces.focused_workspace;
        let is_visible = ws.active_output.is_some();
        let has_windows = ws
            .root
            .all_frame_ids()
            .iter()
            .any(|fid| ws.root.find_frame(*fid).is_some_and(|f| !f.is_empty()));

        let marker = if is_focused {
            "▶"
        } else if is_visible {
            "●"
        } else if has_windows {
            "○"
        } else {
            "·"
        };

        parts.push(format!("{marker} {}", ws.name));
    }

    let text = parts.join("  ");
    let focused_name = workspaces
        .workspaces
        .get(workspaces.focused_workspace.0)
        .map(|ws| ws.name.as_str())
        .unwrap_or("");

    format!(r#"{{"text": "{text}", "tooltip": "Focused: {focused_name}", "class": "workspaces"}}"#)
}

/// Write workspace state to the IPC file. Called after every manage cycle.
pub fn update_workspace_status(workspaces: &WorkspaceManager) {
    let path = ipc_path();
    let json = workspace_json(workspaces);

    match std::fs::write(&path, format!("{json}\n")) {
        Ok(_) => {}
        Err(e) => {
            // Only log once, not every cycle
            static LOGGED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                log::warn!(
                    "Failed to write workspace status to {}: {e}",
                    path.display()
                );
            }
        }
    }
}
