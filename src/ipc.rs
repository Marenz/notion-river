//! IPC: write workspace state for waybar.
//!
//! Writes JSON to $XDG_RUNTIME_DIR/notion-river-workspaces on every state change.
//! Waybar reads this via a custom module with short polling interval.

use std::path::PathBuf;

use crate::config::AppearanceConfig;
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
    pub fn update(&mut self, workspaces: &WorkspaceManager, appearance: &AppearanceConfig) {
        let json = workspace_json(workspaces, appearance);
        if json == self.last_json {
            return;
        }
        self.last_json = json.clone();
        let _ = std::fs::write(&self.path, format!("{json}\n"));
    }
}

/// Fallback monitor colors if none configured.
const DEFAULT_MONITOR_COLORS: &[&str] = &["#b4a0e5", "#a6c9a1", "#e5cfa6", "#d68ba8"];

/// Generate waybar JSON grouped by monitor with Pango markup.
pub fn workspace_json(workspaces: &WorkspaceManager, appearance: &AppearanceConfig) -> String {
    // Collect outputs in order
    let mut output_names: Vec<String> = Vec::new();
    for ws in &workspaces.workspaces {
        let name = ws.preferred_output.as_deref().unwrap_or("none").to_string();
        if !output_names.contains(&name) {
            output_names.push(name);
        }
    }

    let mut groups = Vec::new();

    let monitor_colors: Vec<&str> = if appearance.monitor_colors.is_empty() {
        DEFAULT_MONITOR_COLORS.to_vec()
    } else {
        appearance
            .monitor_colors
            .iter()
            .map(|s| s.as_str())
            .collect()
    };
    let focused_bg = &appearance.waybar_focused_bg;

    for (i, output_name) in output_names.iter().enumerate() {
        let color = monitor_colors[i % monitor_colors.len()];

        let mut parts = Vec::new();
        for ws in &workspaces.workspaces {
            let ws_output = ws.preferred_output.as_deref().unwrap_or("none");
            if ws_output != output_name {
                continue;
            }

            let is_focused = ws.id == workspaces.focused_workspace;
            let has_windows = ws
                .root
                .all_frame_ids()
                .iter()
                .any(|fid| ws.root.find_frame(*fid).is_some_and(|f| !f.is_empty()));

            let ws_text = if is_focused {
                format!(
                    "<span color='{color}' background='{focused_bg}' bgalpha='80%'><b> {} </b></span>",
                    ws.name
                )
            } else if has_windows {
                format!("<span alpha='70%' color='{color}'>{}</span>", ws.name)
            } else {
                format!("<span alpha='35%' color='{color}'>{}</span>", ws.name)
            };

            let marker = if is_focused {
                String::new() // no marker needed, the box highlights it
            } else if has_windows {
                format!("<span alpha='70%' color='{color}'>○ </span>")
            } else {
                format!("<span alpha='35%' color='{color}'>· </span>")
            };

            parts.push(format!("{marker} {ws_text}"));
        }

        let group_text = parts.join("  ");
        groups.push(group_text);
    }

    let text = groups.join("  ");
    let focused_name = workspaces
        .workspaces
        .get(workspaces.focused_workspace.0)
        .map(|ws| ws.name.as_str())
        .unwrap_or("");

    // Escape for JSON
    let text = text.replace('"', "&quot;");

    format!(r#"{{"text": "{text}", "tooltip": "Focused: {focused_name}", "class": "workspaces"}}"#)
}
