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

/// Monitor colors for workspace grouping in waybar.
const MONITOR_COLORS: &[&str] = &["#89b4fa", "#a6e3a1", "#f9e2af", "#f38ba8"];

/// Generate waybar JSON grouped by monitor with Pango markup.
pub fn workspace_json(workspaces: &WorkspaceManager) -> String {
    // Collect outputs in order
    let mut output_names: Vec<String> = Vec::new();
    for ws in &workspaces.workspaces {
        let name = ws
            .preferred_output
            .as_deref()
            .unwrap_or("none")
            .to_string();
        if !output_names.contains(&name) {
            output_names.push(name);
        }
    }

    let focused_output = workspaces
        .workspaces
        .get(workspaces.focused_workspace.0)
        .and_then(|ws| ws.preferred_output.as_deref())
        .unwrap_or("");

    let mut groups = Vec::new();

    for (i, output_name) in output_names.iter().enumerate() {
        let color = MONITOR_COLORS[i % MONITOR_COLORS.len()];
        let is_focused_output = output_name == focused_output;

        let mut parts = Vec::new();
        for ws in &workspaces.workspaces {
            let ws_output = ws.preferred_output.as_deref().unwrap_or("none");
            if ws_output != output_name {
                continue;
            }

            let is_focused = ws.id == workspaces.focused_workspace;
            let is_visible = ws.active_output.is_some();
            let has_windows = ws
                .root
                .all_frame_ids()
                .iter()
                .any(|fid| ws.root.find_frame(*fid).is_some_and(|f| !f.is_empty()));

            let ws_text = if is_focused {
                format!("<b>{}</b>", ws.name)
            } else if has_windows {
                ws.name.clone()
            } else {
                format!("<span alpha='50%'>{}</span>", ws.name)
            };

            let marker = if is_focused {
                "▶"
            } else if is_visible {
                "●"
            } else if has_windows {
                "○"
            } else {
                "·"
            };

            parts.push(format!("{marker} {ws_text}"));
        }

        let group_text = parts.join("  ");
        if is_focused_output {
            groups.push(format!("<span color='{color}'><b>[{group_text}]</b></span>"));
        } else {
            groups.push(format!("<span color='{color}'>{group_text}</span>"));
        }
    }

    let text = groups.join("  ");
    let focused_name = workspaces
        .workspaces
        .get(workspaces.focused_workspace.0)
        .map(|ws| ws.name.as_str())
        .unwrap_or("");

    // Escape for JSON
    let text = text.replace('"', "&quot;");

    format!(
        r#"{{"text": "{text}", "tooltip": "Focused: {focused_name}", "class": "workspaces"}}"#
    )
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
