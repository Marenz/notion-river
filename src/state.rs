//! State persistence: save and restore layout + window placement across restarts.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::layout::{Frame, FrameId, Orientation, SplitNode};
use crate::workspace::{WorkspaceId, WorkspaceManager};

const STATE_FILE: &str = "notion-river-state.json";
const STATE_BACKUP: &str = "notion-river-state.backup.json";

fn state_dir() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
        .join("notion-river");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn state_path() -> PathBuf {
    state_dir().join(STATE_FILE)
}

fn backup_path() -> PathBuf {
    state_dir().join(STATE_BACKUP)
}

// ── Serializable state ───────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
pub struct SavedState {
    pub workspaces: Vec<SavedWorkspace>,
    pub focused_workspace: String,
    /// Runtime workspace selection, keyed by stable physical monitor identity.
    /// Restored only on a WM restart with the same connected monitor set.
    #[serde(default)]
    pub output_workspaces: Vec<(String, String)>,
    /// Legacy field, kept only to tolerate old state files. Per-monitor workspace
    /// memory now lives in `monitor-memory.json`.
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
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
        /// Whether this split was in floating mode. Defaults false so old
        /// state files load unchanged.
        #[serde(default)]
        floating: bool,
        /// Fractional rect of the first half in floating mode.
        #[serde(default = "saved_float_first_default")]
        float_first: SavedFloatRect,
        /// Fractional rect of the second half in floating mode.
        #[serde(default = "saved_float_second_default")]
        float_second: SavedFloatRect,
        first: Box<SavedNode>,
        second: Box<SavedNode>,
    },
}

fn saved_float_first_default() -> SavedFloatRect {
    SavedFloatRect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    }
}

fn saved_float_second_default() -> SavedFloatRect {
    saved_float_first_default()
}

/// Serialized form of a [`crate::layout::FloatRect`].
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct SavedFloatRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedWindow {
    pub app_id: String,
    pub title: String,
    /// Stable River window identifier (persists across WM reconnects).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
}

/// Tab identity and order must survive the compositor announcing windows in
/// a different order on reconnect. A saved numeric active_tab alone cannot.
#[derive(Debug)]
pub struct SavedFrameTabs {
    windows: Vec<SavedWindow>,
    active_tab: usize,
}

impl SavedFrameTabs {
    pub fn restore(&self, frame: &mut Frame, identifiers: &std::collections::HashMap<u64, &str>) {
        let matches = |saved: &SavedWindow, window: &crate::layout::WindowRef| {
            if let Some(id) = identifiers.get(&window.window_id) {
                saved.identifier.as_deref() == Some(*id)
            } else {
                saved.app_id == window.app_id && saved.title == window.title
            }
        };
        frame.windows.sort_by_key(|window| {
            self.windows
                .iter()
                .position(|saved| matches(saved, window))
                .unwrap_or(usize::MAX)
        });
        if let Some(active) = self.windows.get(self.active_tab)
            && let Some(index) = frame.windows.iter().position(|window| matches(active, window))
        {
            frame.set_active_tab(index);
        }
    }
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
        output_workspaces: workspaces
            .visible_workspaces()
            .iter()
            .filter_map(|ws| {
                let output = workspaces.output(ws.active_output?)?;
                Some((crate::monitor_memory::monitor_key(output)?, ws.name.clone()))
            })
            .collect(),
        visible_workspaces: Vec::new(),
    };

    let path = state_path();
    let backup = backup_path();
    match serde_json::to_string_pretty(&state) {
        Ok(json) => {
            // Rotate: current → backup, then write new current
            if path.exists()
                && let Err(e) = std::fs::copy(&path, &backup)
            {
                log::warn!("Failed to rotate state backup: {e}");
            }
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
            floating,
            float_first,
            float_second,
            first,
            second,
        } => SavedNode::Split {
            orientation: match orientation {
                Orientation::Horizontal => "h".to_string(),
                Orientation::Vertical => "v".to_string(),
            },
            ratio: *ratio,
            floating: *floating,
            float_first: SavedFloatRect {
                x: float_first.x,
                y: float_first.y,
                width: float_first.width,
                height: float_first.height,
            },
            float_second: SavedFloatRect {
                x: float_second.x,
                y: float_second.y,
                width: float_second.width,
                height: float_second.height,
            },
            first: Box::new(save_node(first, windows)),
            second: Box::new(save_node(second, windows)),
        },
    }
}

// ── Restore ──────────────────────────────────────────────────────────────

/// Load saved state from disk. Tries the current file first; if it's missing
/// or corrupt, falls back to the backup. Never deletes either file.
pub fn load_state() -> Option<SavedState> {
    let path = state_path();
    let backup = backup_path();

    // Try current
    if let Some(state) = try_load_state(&path) {
        if validate_state(&state) {
            log::info!("Loaded saved state from {}", path.display());
            return Some(state);
        }
        log::warn!(
            "State file {} failed validation, trying backup",
            path.display()
        );
    }

    // Fall back to backup
    if let Some(state) = try_load_state(&backup) {
        if validate_state(&state) {
            log::info!("Loaded saved state from backup {}", backup.display());
            return Some(state);
        }
        log::warn!("Backup state file also failed validation");
    }

    None
}

fn try_load_state(path: &PathBuf) -> Option<SavedState> {
    let json = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&json) {
        Ok(state) => Some(state),
        Err(e) => {
            log::error!("Failed to parse state file {}: {e}", path.display());
            None
        }
    }
}

/// Basic validation: at least one workspace must have a layout tree with
/// splits or windows (not all empty single-frame workspaces).
fn validate_state(state: &SavedState) -> bool {
    if state.workspaces.is_empty() {
        return false;
    }
    state
        .workspaces
        .iter()
        .any(|ws| node_has_windows(&ws.root) || matches!(ws.root, SavedNode::Split { .. }))
}

/// Restore layout trees from saved state into the workspace manager.
/// Returns tab order and active window identities for later application.
pub fn restore_layout(
    workspaces: &mut WorkspaceManager,
    state: &SavedState,
) -> std::collections::HashMap<crate::layout::FrameId, SavedFrameTabs> {
    let mut active_tabs = std::collections::HashMap::new();

    for saved_ws in &state.workspaces {
        if let Some(ws) = workspaces
            .workspaces
            .iter_mut()
            .find(|w| w.name == saved_ws.name)
        {
            ws.root = restore_node(&saved_ws.root);
            // Keep the original tabs intact while saved matching slots are consumed.
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

/// Restore runtime workspace selections without changing long-term monitor
/// memory or config preferences. A changed monitor set uses normal assignment.
pub fn restore_output_workspaces(
    workspaces: &mut WorkspaceManager,
    saved: &[(String, String)],
) -> bool {
    if saved.is_empty() || !workspaces.all_outputs_have_geometry() {
        return false;
    }
    let outputs: Vec<_> = workspaces.outputs.iter().filter(|output| !output.removed).collect();
    if saved.len() != outputs.len() {
        return false;
    }
    let assignments: Option<Vec<_>> = saved
        .iter()
        .map(|(key, name)| {
            let output = outputs.iter().find(|output| {
                crate::monitor_memory::monitor_key(output).as_ref() == Some(key)
            })?;
            let workspace = workspaces.workspaces.iter().find(|ws| ws.name == *name)?;
            Some((workspace.id, output.id))
        })
        .collect();
    let Some(assignments) = assignments else { return false };
    let unique_workspaces: std::collections::HashSet<_> =
        assignments.iter().map(|(ws, _)| ws).collect();
    let unique_outputs: std::collections::HashSet<_> =
        assignments.iter().map(|(_, output)| output).collect();
    if unique_workspaces.len() != saved.len() || unique_outputs.len() != saved.len() {
        return false;
    }
    for ws in &mut workspaces.workspaces {
        ws.active_output = None;
    }
    workspaces.output_workspace.clear();
    for (workspace, output) in assignments {
        workspaces.assign_workspace_to_output(workspace, output);
    }
    true
}

fn collect_active_tabs(
    node: &SavedNode,
    frame_ids: &[crate::layout::FrameId],
    base_index: usize,
    out: &mut std::collections::HashMap<crate::layout::FrameId, SavedFrameTabs>,
) {
    match node {
        SavedNode::Leaf {
            active_tab,
            windows,
            ..
        } => {
            if base_index < frame_ids.len() && !windows.is_empty() {
                out.insert(
                    frame_ids[base_index],
                    SavedFrameTabs {
                        windows: windows.clone(),
                        active_tab: *active_tab,
                    },
                );
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
            floating,
            float_first,
            float_second,
            first,
            second,
        } => SplitNode::Split {
            orientation: match orientation.as_str() {
                "h" => Orientation::Horizontal,
                _ => Orientation::Vertical,
            },
            ratio: *ratio,
            floating: *floating,
            float_first: crate::layout::FloatRect {
                x: float_first.x,
                y: float_first.y,
                width: float_first.width,
                height: float_first.height,
            },
            float_second: crate::layout::FloatRect {
                x: float_second.x,
                y: float_second.y,
                width: float_second.width,
                height: float_second.height,
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkspaceConfig;
    use crate::layout::WindowRef;
    use crate::workspace::{Output, OutputId};

    fn workspace_config(name: &str) -> WorkspaceConfig {
        WorkspaceConfig { name: name.into(), output: None, initial_layout: None }
    }

    #[test]
    fn restart_restores_cells_tab_order_and_active_identity_from_shuffled_announcements() {
        let saved_window = |id: u64| SavedWindow {
            app_id: "contour".into(),
            title: "identical terminal title".into(),
            identifier: Some(format!("terminal-{id}")),
        };
        let mut state = SavedState {
            workspaces: vec![SavedWorkspace {
                name: "term".into(),
                focused_frame_index: 0,
                root: SavedNode::Split {
                    orientation: "v".into(), ratio: 0.49, floating: false,
                    float_first: saved_float_first_default(),
                    float_second: saved_float_second_default(),
                    first: Box::new(SavedNode::Leaf {
                        windows: (1..=3).map(saved_window).collect(), active_tab: 2,
                    }),
                    second: Box::new(SavedNode::Leaf {
                        windows: vec![saved_window(4)], active_tab: 0,
                    }),
                },
            }],
            focused_workspace: "term".into(),
            output_workspaces: vec![],
            visible_workspaces: vec![],
        };
        let mut workspaces = WorkspaceManager::new(&[workspace_config("term")], 0.5);
        let tabs = restore_layout(&mut workspaces, &state);
        let frames = workspaces.workspaces[0].root.all_frame_ids();
        let ids: std::collections::HashMap<_, _> = [
            (1, "terminal-1"), (2, "terminal-2"), (3, "terminal-3"), (4, "terminal-4"),
        ].into_iter().collect();
        // Arrival order differs both across cells and within the top cell.
        for id in [4, 3, 1, 2] {
            let saved = saved_window(id);
            let (ws, frame_id) = match_window_to_saved_frame(
                &workspaces, &mut state, &saved.app_id, &saved.title, saved.identifier.as_deref(),
            ).unwrap();
            let frame = workspaces.workspaces[ws.0].root.find_frame_mut(frame_id).unwrap();
            frame.add_window_quiet(WindowRef {
                window_id: id, app_id: saved.app_id, title: saved.title,
            });
            tabs[&frame_id].restore(frame, &ids);
            if id != 4 {
                assert_eq!(frame.active_window().unwrap().window_id, 3);
            }
        }
        let root = &workspaces.workspaces[0].root;
        let top = root.find_frame(frames[0]).unwrap();
        assert_eq!(top.windows.iter().map(|w| w.window_id).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(top.active_window().unwrap().window_id, 3);
        let bottom = root.find_frame(frames[1]).unwrap();
        assert_eq!(bottom.windows.len(), 1);
        assert_eq!(bottom.active_window().unwrap().window_id, 4);
        assert!(!has_remaining_matches(&state));
    }

    #[test]
    fn restart_restores_visible_workspaces_only_for_the_same_monitor_set() {
        let mut workspaces = WorkspaceManager::new(&[
            workspace_config("main"), workspace_config("social"), workspace_config("term"),
        ], 0.5);
        workspaces.monitor_memory = crate::monitor_memory::MonitorMemory::default();
        for (id, description) in [(1, "center"), (2, "portrait")] {
            let mut output = Output::new(OutputId(id));
            output.description = Some(description.into());
            output.width = 1440;
            output.height = 2560;
            workspaces.add_output(output);
        }
        workspaces.assign_workspace_to_output(WorkspaceId(0), OutputId(1));
        workspaces.assign_workspace_to_output(WorkspaceId(1), OutputId(2));
        let saved = vec![("edid:center".into(), "main".into()), ("edid:portrait".into(), "term".into())];
        workspaces.outputs[1].width = 0;
        assert!(!restore_output_workspaces(&mut workspaces, &saved));
        workspaces.outputs[1].width = 1440;
        workspaces.outputs[1].description = Some("different monitor".into());
        assert!(!restore_output_workspaces(&mut workspaces, &saved));
        assert_eq!(workspaces.output_workspace[&OutputId(2)], WorkspaceId(1));
        workspaces.outputs[1].description = Some("portrait".into());
        assert!(restore_output_workspaces(&mut workspaces, &saved));
        assert_eq!(workspaces.output_workspace[&OutputId(1)], WorkspaceId(0));
        assert_eq!(workspaces.output_workspace[&OutputId(2)], WorkspaceId(2));
        assert!(workspaces.workspaces[1].active_output.is_none());
        assert!(workspaces.monitor_memory.entries.is_empty());
    }

    fn floating_split() -> SavedNode {
        SavedNode::Split {
            orientation: "h".to_string(),
            ratio: 0.5,
            floating: true,
            float_first: SavedFloatRect {
                x: 0.0,
                y: 0.0,
                width: 0.8,
                height: 0.5,
            },
            float_second: SavedFloatRect {
                x: 0.4,
                y: 0.3,
                width: 0.6,
                height: 0.7,
            },
            first: Box::new(SavedNode::Leaf {
                windows: vec![],
                active_tab: 0,
            }),
            second: Box::new(SavedNode::Leaf {
                windows: vec![],
                active_tab: 0,
            }),
        }
    }

    #[test]
    fn saved_floating_split_round_trips() {
        let node = floating_split();
        let json = serde_json::to_string(&node).unwrap();
        let back: SavedNode = serde_json::from_str(&json).unwrap();

        match back {
            SavedNode::Split {
                floating,
                float_first,
                float_second,
                ..
            } => {
                assert!(floating);
                assert_eq!(float_first.width, 0.8);
                assert_eq!(float_second.height, 0.7);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn old_state_without_floating_fields_loads_as_normal_split() {
        // A state file written before floating existed has no floating or
        // float rect fields. It must deserialize to a non-floating split with
        // full default rects.
        let json = r#"{"Split": {
            "orientation": "h",
            "ratio": 0.5,
            "first": {"Leaf": {"windows": [], "active_tab": 0}},
            "second": {"Leaf": {"windows": [], "active_tab": 0}}
        }}"#;
        let node: SavedNode = serde_json::from_str(json).unwrap();
        match node {
            SavedNode::Split {
                floating,
                float_first,
                float_second,
                ..
            } => {
                assert!(!floating);
                assert_eq!(float_first.width, 1.0);
                assert_eq!(float_second.width, 1.0);
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn restore_node_rebuilds_floating_split() {
        let node = floating_split();
        let restored = restore_node(&node);
        match restored {
            crate::layout::SplitNode::Split {
                floating,
                float_first,
                float_second,
                ..
            } => {
                assert!(floating);
                assert!((float_first.width - 0.8).abs() < 1e-6);
                assert!((float_second.x - 0.4).abs() < 1e-6);
            }
            _ => panic!("expected split"),
        }
    }
}
