//! State persistence: save and restore layout + window placement across restarts.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::layout::{Frame, FrameId, Orientation, SplitNode};
use crate::workspace::{WorkspaceId, WorkspaceManager};

const STATE_FILE: &str = "notion-river-state.json";

fn state_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join(STATE_FILE)
}

// ── Serializable state ───────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
pub struct SavedState {
    pub workspaces: Vec<SavedWorkspace>,
    pub focused_workspace: String,
    /// Which workspace was visible on each output: (output_name, workspace_name)
    #[serde(default)]
    pub visible_workspaces: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SavedWorkspace {
    pub name: String,
    pub root: SavedNode,
    pub focused_frame_index: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum SavedNode {
    Leaf {
        windows: Vec<SavedWindow>,
        active_tab: usize,
    },
    Split {
        orientation: String, // "h" or "v"
        ratio: f32,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedWindow {
    pub app_id: String,
    pub title: String,
    /// Stable River window identifier (persists across WM reconnects).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
}

// ── Save ─────────────────────────────────────────────────────────────────

pub fn save_state(workspaces: &WorkspaceManager, windows: &[crate::wm::ManagedWindow]) {
    let state = SavedState {
        workspaces: workspaces
            .workspaces
            .iter()
            .map(|ws| {
                let all_ids = ws.root.all_frame_ids();
                let focused_index = all_ids
                    .iter()
                    .position(|id| *id == ws.focused_frame)
                    .unwrap_or(0);
                SavedWorkspace {
                    name: ws.name.clone(),
                    root: save_node(&ws.root, windows),
                    focused_frame_index: focused_index,
                }
            })
            .collect(),
        focused_workspace: workspaces
            .workspaces
            .get(workspaces.focused_workspace.0)
            .map(|ws| ws.name.clone())
            .unwrap_or_default(),
        visible_workspaces: workspaces
            .workspaces
            .iter()
            .filter_map(|ws| {
                let output_id = ws.active_output?;
                let output_name = workspaces.output(output_id)?.name.as_ref()?.clone();
                Some((output_name, ws.name.clone()))
            })
            .collect(),
    };

    let path = state_path();
    match serde_json::to_string_pretty(&state) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log::error!("Failed to save state to {}: {e}", path.display());
            } else {
                log::info!("Saved state to {}", path.display());
            }
        }
        Err(e) => log::error!("Failed to serialize state: {e}"),
    }
}

fn save_node(node: &SplitNode, windows: &[crate::wm::ManagedWindow]) -> SavedNode {
    match node {
        SplitNode::Leaf(frame) => SavedNode::Leaf {
            windows: frame
                .windows
                .iter()
                .map(|w| {
                    // Look up the stable identifier from ManagedWindow
                    let identifier = windows
                        .iter()
                        .find(|mw| mw.id == w.window_id)
                        .and_then(|mw| mw.identifier.clone());
                    SavedWindow {
                        app_id: w.app_id.clone(),
                        title: w.title.clone(),
                        identifier,
                    }
                })
                .collect(),
            active_tab: frame.active_tab,
        },
        SplitNode::Split {
            orientation,
            ratio,
            first,
            second,
        } => SavedNode::Split {
            orientation: match orientation {
                Orientation::Horizontal => "h".to_string(),
                Orientation::Vertical => "v".to_string(),
            },
            ratio: *ratio,
            first: Box::new(save_node(first, windows)),
            second: Box::new(save_node(second, windows)),
        },
    }
}

// ── Restore ──────────────────────────────────────────────────────────────

/// Load saved state from disk. Returns None if no state file or parse error.
pub fn load_state() -> Option<SavedState> {
    let path = state_path();
    let json = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&json) {
        Ok(state) => {
            log::info!("Loaded saved state from {}", path.display());
            // Delete the state file after loading (one-shot restore)
            let _ = std::fs::remove_file(&path);
            Some(state)
        }
        Err(e) => {
            log::error!("Failed to parse state file {}: {e}", path.display());
            None
        }
    }
}

/// Restore layout trees from saved state into the workspace manager.
/// Returns a map of FrameId → saved active_tab index for later application.
pub fn restore_layout(
    workspaces: &mut WorkspaceManager,
    state: &SavedState,
) -> std::collections::HashMap<crate::layout::FrameId, usize> {
    let mut active_tabs = std::collections::HashMap::new();

    for saved_ws in &state.workspaces {
        if let Some(ws) = workspaces
            .workspaces
            .iter_mut()
            .find(|w| w.name == saved_ws.name)
        {
            ws.root = restore_node(&saved_ws.root);
            // Collect active_tab for each frame
            let all_ids = ws.root.all_frame_ids();
            collect_active_tabs(&saved_ws.root, &all_ids, 0, &mut active_tabs);
            // Restore focused frame
            if saved_ws.focused_frame_index < all_ids.len() {
                ws.focused_frame = all_ids[saved_ws.focused_frame_index];
            }
            log::info!("Restored layout for workspace '{}'", saved_ws.name);
        }
    }
    // Restore focused workspace
    if let Some(ws) = workspaces
        .workspaces
        .iter()
        .find(|w| w.name == state.focused_workspace)
    {
        workspaces.focused_workspace = ws.id;
    }

    active_tabs
}

#[allow(dead_code)]
/// Restore which workspaces were visible on each output.
/// Called after output names are known (from reassign_outputs).
fn collect_active_tabs(
    node: &SavedNode,
    frame_ids: &[crate::layout::FrameId],
    base_index: usize,
    out: &mut std::collections::HashMap<crate::layout::FrameId, usize>,
) {
    match node {
        SavedNode::Leaf {
            active_tab,
            windows,
            ..
        } => {
            if base_index < frame_ids.len() && !windows.is_empty() {
                out.insert(frame_ids[base_index], *active_tab);
            }
        }
        SavedNode::Split { first, second, .. } => {
            let first_count = count_leaves(first);
            collect_active_tabs(first, frame_ids, base_index, out);
            collect_active_tabs(second, frame_ids, base_index + first_count, out);
        }
    }
}

fn restore_node(saved: &SavedNode) -> SplitNode {
    match saved {
        SavedNode::Leaf { .. } => {
            // Restore as empty frame — windows will be matched later
            SplitNode::Leaf(Frame::new())
        }
        SavedNode::Split {
            orientation,
            ratio,
            first,
            second,
        } => SplitNode::Split {
            orientation: match orientation.as_str() {
                "h" => Orientation::Horizontal,
                _ => Orientation::Vertical,
            },
            ratio: *ratio,
            first: Box::new(restore_node(first)),
            second: Box::new(restore_node(second)),
        },
    }
}

/// Try to place a new window into the frame that previously held a window
/// with the same app_id. Consumes the matched slot so the same saved
/// position isn't used twice. Returns the target (WorkspaceId, FrameId).
pub fn match_window_to_saved_frame(
    workspaces: &WorkspaceManager,
    state: &mut SavedState,
    app_id: &str,
    title: &str,
    identifier: Option<&str>,
) -> Option<(WorkspaceId, FrameId)> {
    for saved_ws in &mut state.workspaces {
        let ws = match workspaces
            .workspaces
            .iter()
            .find(|w| w.name == saved_ws.name)
        {
            Some(ws) => ws,
            None => continue,
        };
        if let Some(frame_index) =
            find_and_consume_match(&mut saved_ws.root, app_id, title, identifier, 0)
        {
            let all_ids = ws.root.all_frame_ids();
            if frame_index < all_ids.len() {
                return Some((ws.id, all_ids[frame_index]));
            }
        }
    }
    None
}

/// Find the leaf index matching app_id and remove the matched window entry
/// so it won't match again.
fn find_and_consume_match(
    node: &mut SavedNode,
    app_id: &str,
    title: &str,
    identifier: Option<&str>,
    base_index: usize,
) -> Option<usize> {
    match node {
        SavedNode::Leaf { windows, .. } => {
            // If we have an identifier, match ONLY by identifier (it's unique).
            // Fall back to app_id+title only when no identifier is available.
            let pos = if let Some(id) = identifier {
                windows
                    .iter()
                    .position(|w| w.identifier.as_deref() == Some(id))
            } else {
                windows
                    .iter()
                    .position(|w| w.app_id == app_id && w.title == title)
                    .or_else(|| windows.iter().position(|w| w.app_id == app_id))
            };
            if let Some(pos) = pos {
                windows.remove(pos); // consume the slot
                Some(base_index)
            } else {
                None
            }
        }
        SavedNode::Split { first, second, .. } => {
            let first_count = count_leaves(first);
            find_and_consume_match(first, app_id, title, identifier, base_index).or_else(|| {
                find_and_consume_match(second, app_id, title, identifier, base_index + first_count)
            })
        }
    }
}

/// Check if the saved state has any remaining window entries to match.
pub fn has_remaining_matches(state: &SavedState) -> bool {
    state.workspaces.iter().any(|ws| node_has_windows(&ws.root))
}

fn node_has_windows(node: &SavedNode) -> bool {
    match node {
        SavedNode::Leaf { windows, .. } => !windows.is_empty(),
        SavedNode::Split { first, second, .. } => {
            node_has_windows(first) || node_has_windows(second)
        }
    }
}

fn count_leaves(node: &SavedNode) -> usize {
    match node {
        SavedNode::Leaf { .. } => 1,
        SavedNode::Split { first, second, .. } => count_leaves(first) + count_leaves(second),
    }
}
