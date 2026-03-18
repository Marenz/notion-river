//! App-to-frame bindings: interactive window placement rules.
//!
//! Super+f binds the focused window's app_id to the current frame.
//! Super+Shift+f adds/removes additional frames for the same app_id.
//! New windows with a bound app_id are placed in their bound frame.
//!
//! Multiple frames can be bound to the same app_id (on different workspaces),
//! but only one instance should be visible at a time. When spawning:
//! - If already in a visible bound frame, stay there
//! - If coming from a non-visible state, go to the first-defined bound frame
//!   that is on a visible workspace

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::layout::FrameId;
use crate::workspace::{WorkspaceId, WorkspaceManager};

/// A bound location for an app: workspace name + frame index within that workspace.
/// We use workspace name + frame index (not FrameId) because FrameIds change on restart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoundLocation {
    pub workspace: String,
    pub frame_index: usize,
    /// Optional fixed dimensions (width, height) for windows in this binding.
    /// When set, propose these dimensions instead of the frame size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_dimensions: Option<(i32, i32)>,
}

/// All app bindings. Maps app_id → list of bound locations (ordered, first = primary).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppBindings {
    pub bindings: HashMap<String, Vec<BoundLocation>>,
}

fn bindings_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("notion-river");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("bindings.json")
}

impl AppBindings {
    pub fn load() -> Self {
        let path = bindings_path();
        match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str(&json) {
                Ok(bindings) => {
                    log::info!("Loaded app bindings from {}", path.display());
                    bindings
                }
                Err(e) => {
                    log::warn!("Failed to parse bindings: {e}");
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = bindings_path();
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::error!("Failed to save bindings: {e}");
                }
            }
            Err(e) => log::error!("Failed to serialize bindings: {e}"),
        }
    }

    /// Toggle binding for an app_id on this frame.
    /// If already bound here, remove. Otherwise add.
    pub fn toggle_binding(&mut self, app_id: &str, workspace: &str, frame_index: usize) {
        let loc = BoundLocation {
            workspace: workspace.to_string(),
            frame_index,
            fixed_dimensions: None,
        };
        let locations = self.bindings.entry(app_id.to_string()).or_default();
        if let Some(pos) = locations.iter().position(|l| l == &loc) {
            locations.remove(pos);
            log::info!(
                "Unbound app '{}' from {} frame #{}",
                app_id,
                workspace,
                frame_index
            );
        } else {
            locations.push(loc);
            log::info!(
                "Bound app '{}' to {} frame #{}",
                app_id,
                workspace,
                frame_index
            );
        }
        if locations.is_empty() {
            self.bindings.remove(app_id);
        }
        self.save();
    }

    /// Clear all bindings for an app except the current frame.
    /// If not bound here yet, binds it exclusively.
    pub fn bind_exclusive(&mut self, app_id: &str, workspace: &str, frame_index: usize) {
        let loc = BoundLocation {
            workspace: workspace.to_string(),
            frame_index,
            fixed_dimensions: None,
        };
        self.bindings.insert(app_id.to_string(), vec![loc]);
        log::info!(
            "Exclusively bound app '{}' to {} frame #{}",
            app_id,
            workspace,
            frame_index
        );
        self.save();
    }

    /// Find the best frame to place a new window with the given app_id.
    /// Returns (WorkspaceId, FrameId) or None if no binding exists.
    /// Find locations for an app_id, supporting wildcard prefixes (e.g. "steam_app_*").
    fn find_locations(&self, app_id: &str) -> Option<&Vec<BoundLocation>> {
        // Exact match first
        if let Some(locs) = self.bindings.get(app_id) {
            return Some(locs);
        }
        // Wildcard prefix match (e.g. "steam_app_*" matches "steam_app_1086940")
        for (pattern, locs) in &self.bindings {
            if let Some(prefix) = pattern.strip_suffix('*')
                && app_id.starts_with(prefix)
            {
                return Some(locs);
            }
        }
        None
    }

    pub fn find_target(
        &self,
        app_id: &str,
        workspaces: &WorkspaceManager,
    ) -> Option<(WorkspaceId, FrameId)> {
        let locations = self.find_locations(app_id)?;
        if locations.is_empty() {
            return None;
        }

        // First pass: find a bound frame on a visible workspace that doesn't
        // already have this app_id in it
        for loc in locations {
            if let Some(ws) = workspaces
                .workspaces
                .iter()
                .find(|w| w.name == loc.workspace)
                && ws.active_output.is_some()
            {
                let frame_ids = ws.root.all_frame_ids();
                if let Some(&fid) = frame_ids.get(loc.frame_index) {
                    // Check if this frame already has the app
                    let already_has = ws
                        .root
                        .find_frame(fid)
                        .is_some_and(|f| f.windows.iter().any(|w| w.app_id == app_id));
                    if !already_has {
                        return Some((ws.id, fid));
                    }
                }
            }
        }

        // Second pass: any bound frame (even on hidden workspace), first defined wins
        for loc in locations {
            if let Some(ws) = workspaces
                .workspaces
                .iter()
                .find(|w| w.name == loc.workspace)
            {
                let frame_ids = ws.root.all_frame_ids();
                if let Some(&fid) = frame_ids.get(loc.frame_index) {
                    return Some((ws.id, fid));
                }
            }
        }

        None
    }

    /// Get the frame index of a FrameId within a workspace.
    pub fn frame_index(
        workspaces: &WorkspaceManager,
        ws_id: WorkspaceId,
        frame_id: FrameId,
    ) -> Option<usize> {
        let ws = workspaces.workspaces.get(ws_id.0)?;
        ws.root
            .all_frame_ids()
            .iter()
            .position(|id| *id == frame_id)
    }

    /// Check if a frame has any bindings.
    pub fn is_bound(&self, workspace: &str, frame_index: usize) -> bool {
        self.bindings.values().any(|locs| {
            locs.iter()
                .any(|l| l.workspace == workspace && l.frame_index == frame_index)
        })
    }

    /// Get fixed dimensions for a window at this frame, if any binding specifies them.
    pub fn fixed_dimensions_for(
        &self,
        app_id: &str,
        workspace: &str,
        frame_index: usize,
    ) -> Option<(i32, i32)> {
        let locs = self.find_locations(app_id)?;
        locs.iter()
            .find(|l| l.workspace == workspace && l.frame_index == frame_index)
            .and_then(|l| l.fixed_dimensions)
    }

    /// Set fixed dimensions for an app binding at the given location.
    pub fn set_fixed_dimensions(
        &mut self,
        app_id: &str,
        workspace: &str,
        frame_index: usize,
        dims: Option<(i32, i32)>,
    ) {
        if let Some(locs) = self.bindings.get_mut(app_id) {
            for loc in locs.iter_mut() {
                if loc.workspace == workspace && loc.frame_index == frame_index {
                    loc.fixed_dimensions = dims;
                    log::info!(
                        "Set fixed dimensions {:?} for '{}' on {} frame #{}",
                        dims,
                        app_id,
                        workspace,
                        frame_index
                    );
                }
            }
        }
        self.save();
    }

    /// Get the app_id bound to a specific frame location, if any.
    #[allow(dead_code)]
    pub fn app_at(&self, workspace: &str, frame_index: usize) -> Option<&str> {
        for (app_id, locs) in &self.bindings {
            if locs
                .iter()
                .any(|l| l.workspace == workspace && l.frame_index == frame_index)
            {
                return Some(app_id);
            }
        }
        None
    }
}
