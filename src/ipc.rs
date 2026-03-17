//! IPC: write workspace state for waybar.
//!
//! Writes JSON to $XDG_RUNTIME_DIR/notion-river-workspaces on every state change.
//! Waybar reads this via a custom module with short polling interval.

use std::io::Write;
use std::path::PathBuf;

use crate::workspace::WorkspaceManager;

fn ipc_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("notion-river-workspaces")
}

/// IPC state.
#[derive(Debug)]
pub struct IpcState {
    path: PathBuf,
    last_json: String,
}

impl IpcState {
    pub fn new() -> Self {
        Self {
            path: ipc_path(),
            last_json: String::new(),
        }
    }

    /// Write workspace state if changed.
    pub fn update(&mut self, workspaces: &WorkspaceManager) {
        let json = workspace_json(workspaces);
        if json == self.last_json {
            return;
        }
        self.last_json = json.clone();
        let _ = std::fs::write(&self.path, format!("{json}\n"));
    }
}

/// Generate waybar JSON grouped by monitor.
pub fn workspace_json(workspaces: &WorkspaceManager) -> String {
    let mut output_groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();

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

        let output_name = ws.preferred_output.as_deref().unwrap_or("none").to_string();
        output_groups
            .entry(output_name)
            .or_default()
            .push(format!("{marker} {}", ws.name));
    }

    let text = output_groups
        .values()
        .map(|group| group.join("  "))
        .collect::<Vec<_>>()
        .join("  │  ");

    let focused_name = workspaces
        .workspaces
        .get(workspaces.focused_workspace.0)
        .map(|ws| ws.name.as_str())
        .unwrap_or("");

    format!(r#"{{"text": "{text}", "tooltip": "Focused: {focused_name}", "class": "workspaces"}}"#)
}
