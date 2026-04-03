//! Output profiles: remember workspace assignments per monitor configuration.
//!
//! Hashes the set of connected output names to create a profile key.
//! Saves which workspace was visible on which output for each profile.
//! When the same monitor configuration is detected, restores the assignments.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::workspace::{find_preferred_output, output_geometry_key, WorkspaceManager};

fn profiles_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("notion-river");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("output-profiles.json")
}

/// Saved workspace assignments for a specific output configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputProfile {
    /// Which workspace was visible on each output: output_name → workspace_name
    pub assignments: HashMap<String, String>,
    /// Which workspace was focused
    pub focused_workspace: String,
}

/// All output profiles keyed by the geometry signature of connected outputs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputProfiles {
    pub profiles: HashMap<String, OutputProfile>,
}

impl OutputProfiles {
    pub fn load() -> Self {
        let path = profiles_path();
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = profiles_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Compute a profile key from the current set of connected outputs.
    /// Uses geometry (e.g. "2560x1440@0,0+3840x2160@2560,0") instead of
    /// connector names, so the same physical setup is recognized regardless
    /// of which ports the monitors are plugged into.
    pub fn profile_key(workspaces: &WorkspaceManager) -> String {
        let mut geos: Vec<String> = workspaces
            .outputs
            .iter()
            .filter(|o| !o.removed)
            .filter_map(output_geometry_key)
            .collect();
        geos.sort();
        geos.join("+")
    }

    /// Save the current workspace-to-output assignments for this output config.
    pub fn save_current(&mut self, workspaces: &WorkspaceManager) {
        let key = Self::profile_key(workspaces);
        if key.is_empty() {
            return;
        }

        let mut assignments = HashMap::new();
        for ws in &workspaces.workspaces {
            if let Some(oid) = ws.active_output
                && let Some(output) = workspaces.output(oid)
                && let Some(geo) = output_geometry_key(output)
            {
                assignments.insert(geo, ws.name.clone());
            }
        }

        let focused = workspaces
            .workspaces
            .get(workspaces.focused_workspace.0)
            .map(|ws| ws.name.clone())
            .unwrap_or_default();

        self.profiles.insert(
            key.clone(),
            OutputProfile {
                assignments,
                focused_workspace: focused,
            },
        );
        log::info!("Saved output profile '{key}'");
        self.save();
    }

    /// Try to restore workspace assignments for the current output config.
    /// Returns true if a profile was found and applied.
    pub fn restore_for_current(&self, workspaces: &mut WorkspaceManager) -> bool {
        let key = Self::profile_key(workspaces);
        let Some(profile) = self.profiles.get(&key) else {
            return false;
        };

        log::info!("Restoring output profile '{key}'");

        for (geo_key, ws_name) in &profile.assignments {
            let output_id = match workspaces
                .outputs
                .iter()
                .find(|o| output_geometry_key(o).as_deref() == Some(geo_key.as_str()))
            {
                Some(o) => o.id,
                None => continue,
            };

            let ws_id = match workspaces.workspaces.iter().find(|w| w.name == *ws_name) {
                Some(ws) => ws.id,
                None => continue,
            };

            if let Some(preferred_output) = find_preferred_output(
                &workspaces.workspaces[ws_id.0].preferred_output,
                &workspaces.outputs,
            )
                && preferred_output != output_id
            {
                log::info!(
                    "Skipping profile assignment of '{}' to {} because its preferred output is different",
                    ws_name,
                    geo_key
                );
                continue;
            }

            // Unassign whatever is on this output
            if let Some(&old_ws) = workspaces.output_workspace.get(&output_id)
                && let Some(ws) = workspaces.workspaces.iter_mut().find(|w| w.id == old_ws)
            {
                ws.active_output = None;
            }
            workspaces.assign_workspace_to_output(ws_id, output_id);
        }

        // Restore focused workspace
        if let Some(ws) = workspaces
            .workspaces
            .iter()
            .find(|w| w.name == profile.focused_workspace)
        {
            workspaces.focused_workspace = ws.id;
        }

        true
    }
}
