//! IPC: write workspace state for waybar.
//!
//! Writes JSON to $XDG_RUNTIME_DIR/notion-river-workspaces on every state change,
//! and streams updates to connected subscribers via the control socket.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::config::AppearanceConfig;
use crate::workspace::WorkspaceManager;

fn ipc_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("notion-river-workspaces")
}

/// What a subscriber wants to receive.
pub enum SubscriptionKind {
    /// All workspaces as a single Pango-markup widget.
    AllWorkspaces,
    /// A single named workspace with class-based styling.
    SingleWorkspace(String),
    /// All workspaces on a specific output as Pango-markup widget.
    Output(String),
}

/// A connected subscriber.
pub struct Subscriber {
    pub stream: UnixStream,
    pub kind: SubscriptionKind,
    /// Last JSON sent to this subscriber (for dedup).
    pub last_json: String,
}

/// IPC state.
pub struct IpcState {
    path: PathBuf,
    last_json: String,
    /// Connected subscribers that receive workspace JSON on every change.
    pub subscribers: Arc<Mutex<Vec<Subscriber>>>,
}

impl std::fmt::Debug for IpcState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcState")
            .field("path", &self.path)
            .field("last_json", &self.last_json)
            .field("subscribers", &"[...]")
            .finish()
    }
}

impl IpcState {
    pub fn new() -> Self {
        Self {
            path: ipc_path(),
            last_json: String::new(),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Write workspace state if changed. Also notifies subscribers.
    pub fn update(&mut self, workspaces: &WorkspaceManager, appearance: &AppearanceConfig) {
        let all_json = workspace_json(workspaces, appearance);
        let changed = all_json != self.last_json;
        if !changed {
            return;
        }
        self.last_json = all_json.clone();
        let _ = std::fs::write(&self.path, format!("{all_json}\n"));

        // Notify subscribers, removing any with broken connections.
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain_mut(|sub| {
            let json = match &sub.kind {
                SubscriptionKind::AllWorkspaces => all_json.clone(),
                SubscriptionKind::SingleWorkspace(name) => single_workspace_json(workspaces, name),
                SubscriptionKind::Output(output) => {
                    output_workspaces_json(workspaces, output, appearance)
                }
            };
            // Only send if this subscriber's output actually changed.
            if json == sub.last_json {
                return true; // keep, but skip write
            }
            sub.last_json = json.clone();
            let line = format!("{json}\n");
            sub.stream.write_all(line.as_bytes()).is_ok()
        });
    }
}

/// Generate waybar JSON for a single workspace.
/// Returns JSON with `text` = workspace name and `class` = focused|visible|hidden|empty.
pub fn single_workspace_json(workspaces: &WorkspaceManager, name: &str) -> String {
    for ws in &workspaces.workspaces {
        if ws.name != name {
            continue;
        }
        let is_focused = ws.id == workspaces.focused_workspace;
        let is_visible = ws.active_output.is_some();
        let has_windows = ws
            .root
            .all_frame_ids()
            .iter()
            .any(|fid| ws.root.find_frame(*fid).is_some_and(|f| !f.is_empty()));

        let cls = if is_focused {
            "focused"
        } else if is_visible {
            "visible"
        } else if has_windows {
            "hidden"
        } else {
            "empty"
        };

        let output = ws.preferred_output.as_deref().unwrap_or("?");

        return format!(
            r#"{{"text": "{name}", "tooltip": "{name} on {output}", "class": "{cls}"}}"#
        );
    }
    // Workspace not found
    format!(r#"{{"text": "{name}", "class": "empty"}}"#)
}

/// Generate waybar JSON for all workspaces on a specific output.
/// Returns Pango markup text with workspace names styled by state, suitable for
/// a single waybar custom module per output.
pub fn output_workspaces_json(
    workspaces: &WorkspaceManager,
    output_name: &str,
    appearance: &AppearanceConfig,
) -> String {
    let monitor_colors: Vec<&str> = if appearance.monitor_colors.is_empty() {
        DEFAULT_MONITOR_COLORS.to_vec()
    } else {
        appearance
            .monitor_colors
            .iter()
            .map(|s| s.as_str())
            .collect()
    };

    // Find the color index for this output
    let mut output_names: Vec<String> = Vec::new();
    for ws in &workspaces.workspaces {
        let name = ws.preferred_output.as_deref().unwrap_or("none").to_string();
        if !output_names.contains(&name) {
            output_names.push(name);
        }
    }
    let color_idx = output_names
        .iter()
        .position(|n| n == output_name)
        .unwrap_or(0);
    let color = monitor_colors[color_idx % monitor_colors.len()];
    let focused_bg = &appearance.waybar_focused_bg;

    let mut parts = Vec::new();
    let mut focused_name = String::new();
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

        if is_focused {
            focused_name = ws.name.clone();
        }

        let ws_text = if is_focused {
            format!(
                "<span color='{color}' background='{focused_bg}' bgalpha='80%'><b> {} </b></span>",
                ws.name
            )
        } else if is_visible {
            format!("<span alpha='85%' color='{color}'>{}</span>", ws.name)
        } else if has_windows {
            format!("<span alpha='60%' color='{color}'>{}</span>", ws.name)
        } else {
            format!("<span alpha='35%' color='{color}'>{}</span>", ws.name)
        };
        parts.push(ws_text);
    }

    if parts.is_empty() {
        return format!(
            r#"{{"text": "", "tooltip": "No workspaces on {output_name}", "class": "empty"}}"#
        );
    }

    let text = parts.join("  ");
    let text = text.replace('"', "&quot;");
    let cls = if focused_name.is_empty() {
        "visible"
    } else {
        "focused"
    };

    format!(r#"{{"text": "{text}", "tooltip": "{output_name}: {focused_name}", "class": "{cls}"}}"#)
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
