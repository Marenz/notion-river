use crate::layout::{FrameId, Rect, SplitNode};

/// Identifies a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(pub usize);

/// A workspace owns a layout tree and is assigned to an output.
#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// Which output name this workspace prefers (from config).
    pub preferred_output: Option<String>,
    /// Which output this workspace is currently displayed on (runtime).
    pub active_output: Option<OutputId>,
    /// The static tiling tree.
    pub root: SplitNode,
    /// The currently focused frame within this workspace.
    pub focused_frame: FrameId,
}

/// Identifier for an output (monitor), using the River object id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(pub u64);

/// Runtime state for a connected output (monitor).
#[derive(Debug)]
pub struct Output {
    pub id: OutputId,
    /// The wl_output name string (e.g. "HDMI-0").
    pub name: Option<String>,
    /// Position in the compositor's logical coordinate space.
    pub x: i32,
    pub y: i32,
    /// Dimensions in logical pixels.
    pub width: i32,
    pub height: i32,
    /// Usable area after layer-shell exclusive zones (bars, panels).
    pub usable_x: i32,
    pub usable_y: i32,
    pub usable_width: i32,
    pub usable_height: i32,
    pub has_exclusive_zone: bool,
    /// Whether the output has been removed.
    pub removed: bool,
}

impl Output {
    pub fn new(id: OutputId) -> Self {
        Self {
            id,
            name: None,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            usable_x: 0,
            usable_y: 0,
            usable_width: 0,
            usable_height: 0,
            has_exclusive_zone: false,
            removed: false,
        }
    }

    /// The usable area for tiling, respecting layer-shell exclusive zones.
    pub fn usable_rect(&self) -> Rect {
        if self.has_exclusive_zone {
            Rect::new(
                self.usable_x,
                self.usable_y,
                self.usable_width,
                self.usable_height,
            )
        } else {
            Rect::new(self.x, self.y, self.width, self.height)
        }
    }
}

/// Manages all workspaces and their assignment to outputs.
#[derive(Debug)]
pub struct WorkspaceManager {
    pub workspaces: Vec<Workspace>,
    pub outputs: Vec<Output>,
    /// Which workspace is focused on each output.
    /// Key = OutputId, Value = WorkspaceId.
    pub output_workspace: std::collections::HashMap<OutputId, WorkspaceId>,
    /// The globally focused workspace.
    pub focused_workspace: WorkspaceId,
    /// Saved visible workspaces from state restore: (output_name, workspace_name)
    pub saved_visible: Vec<(String, String)>,
}

impl WorkspaceManager {
    pub fn new(workspace_configs: &[crate::config::WorkspaceConfig], default_ratio: f32) -> Self {
        let workspaces: Vec<Workspace> = workspace_configs
            .iter()
            .enumerate()
            .map(|(i, cfg)| {
                let root = match cfg.initial_layout.as_deref() {
                    Some("hsplit") => SplitNode::hsplit(default_ratio),
                    Some("vsplit") => SplitNode::vsplit(default_ratio),
                    _ => SplitNode::single_frame(),
                };
                let focused_frame = root.first_frame_id();
                Workspace {
                    id: WorkspaceId(i),
                    name: cfg.name.clone(),
                    preferred_output: cfg.output.clone(),
                    active_output: None,
                    root,
                    focused_frame,
                }
            })
            .collect();

        let focused_workspace = WorkspaceId(0);

        Self {
            workspaces,
            outputs: Vec::new(),
            output_workspace: std::collections::HashMap::new(),
            focused_workspace,
            saved_visible: Vec::new(),
        }
    }

    /// Add or update an output. Assigns workspaces to outputs based on preferences.
    pub fn add_output(&mut self, output: Output) {
        let output_id = output.id;
        let output_name = output.name.clone();

        // Check if output already exists (update)
        if let Some(existing) = self.outputs.iter_mut().find(|o| o.id == output_id) {
            *existing = output;
        } else {
            self.outputs.push(output);
        }

        // If no workspace is assigned to this output yet, find one
        if !self.output_workspace.values().any(|&ws| {
            self.workspaces
                .iter()
                .find(|w| w.id == ws)
                .is_some_and(|w| w.active_output == Some(output_id))
        }) {
            // If output name is known, try preferred match
            // If name is not known yet, wait for reassign_outputs() after wl_output.name
            if output_name.is_none() {
                return;
            }
            let preferred = self.workspaces.iter().find(|ws| {
                ws.active_output.is_none()
                    && ws.preferred_output.as_deref() == output_name.as_deref()
            });

            // Otherwise, find the first unassigned workspace
            let ws_id = preferred
                .or_else(|| self.workspaces.iter().find(|ws| ws.active_output.is_none()))
                .map(|ws| ws.id);

            if let Some(ws_id) = ws_id {
                self.assign_workspace_to_output(ws_id, output_id);
            }
        }
    }

    /// Remove an output. Unassigns its workspace.
    pub fn remove_output(&mut self, output_id: OutputId) {
        // Unassign any workspace from this output
        self.output_workspace.retain(|oid, _| *oid != output_id);

        for ws in &mut self.workspaces {
            if ws.active_output == Some(output_id) {
                ws.active_output = None;
            }
        }

        self.outputs.retain(|o| o.id != output_id);
    }

    /// Assign a workspace to an output.
    pub fn assign_workspace_to_output(&mut self, ws_id: WorkspaceId, output_id: OutputId) {
        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
            ws.active_output = Some(output_id);
        }
        self.output_workspace.insert(output_id, ws_id);
    }

    /// Re-assign workspaces to outputs based on preferred_output names.
    /// Called when output names become known (after wl_output.name event).
    pub fn reassign_outputs(&mut self) {
        // Collect (output_id, output_name) pairs
        let outputs: Vec<(OutputId, String)> = self
            .outputs
            .iter()
            .filter_map(|o| o.name.as_ref().map(|n| (o.id, n.clone())))
            .collect();

        for (output_id, output_name) in &outputs {
            // Prefer the saved visible workspace for this output (from state restore)
            let saved_ws = self
                .saved_visible
                .iter()
                .find(|(oname, _)| oname == output_name)
                .and_then(|(_, ws_name)| {
                    self.workspaces
                        .iter()
                        .find(|ws| ws.name == *ws_name && ws.active_output.is_none())
                        .map(|ws| ws.id)
                });

            // Fall back to first preferred workspace for this output
            let ws_id = saved_ws.or_else(|| {
                self.workspaces
                    .iter()
                    .find(|ws| {
                        ws.preferred_output.as_deref() == Some(output_name.as_str())
                            && ws.active_output.is_none()
                    })
                    .map(|ws| ws.id)
            });

            if let Some(ws_id) = ws_id {
                // Unassign whatever was on this output before
                if let Some(&old_ws) = self.output_workspace.get(output_id) {
                    if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == old_ws) {
                        ws.active_output = None;
                    }
                }
                self.assign_workspace_to_output(ws_id, *output_id);
                log::info!(
                    "Assigned workspace '{}' to output '{}'",
                    self.workspaces[ws_id.0].name,
                    output_name
                );
            }
        }
    }

    /// Switch to a workspace. The workspace appears on its preferred output
    /// if it has one, otherwise on the currently focused output.
    pub fn switch_workspace(&mut self, target_name: &str) {
        let target_ws = match self.workspaces.iter().find(|w| w.name == target_name) {
            Some(ws) => ws.id,
            None => {
                log::warn!("Workspace '{target_name}' not found");
                return;
            }
        };

        // If target is already visible on some output, just focus it
        if self.workspaces[target_ws.0].active_output.is_some() {
            self.focused_workspace = target_ws;
            return;
        }

        // Determine which output to show this workspace on:
        // 1. Use the workspace's preferred output if it has one and the output exists
        // 2. Otherwise use the currently focused output
        let preferred_output = self.workspaces[target_ws.0]
            .preferred_output
            .as_ref()
            .and_then(|name| {
                self.outputs
                    .iter()
                    .find(|o| o.name.as_deref() == Some(name.as_str()))
                    .map(|o| o.id)
            });

        let target_output = preferred_output.unwrap_or_else(|| {
            self.workspaces[self.focused_workspace.0]
                .active_output
                .unwrap_or(OutputId(0))
        });

        // Unassign whatever workspace is currently on that output
        let displaced_ws: Option<WorkspaceId> = self.output_workspace.get(&target_output).copied();
        if let Some(old_ws_id) = displaced_ws {
            if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == old_ws_id) {
                ws.active_output = None;
            }
        }

        // Assign target workspace to that output
        self.assign_workspace_to_output(target_ws, target_output);
        self.focused_workspace = target_ws;
    }

    /// Get the currently focused workspace.
    pub fn focused_workspace(&self) -> &Workspace {
        &self.workspaces[self.focused_workspace.0]
    }

    /// Get the currently focused workspace mutably.
    #[allow(dead_code)]
    pub fn focused_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.focused_workspace.0]
    }

    /// Get a workspace by name.
    #[allow(dead_code)]
    pub fn workspace_by_name(&self, name: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.name == name)
    }

    /// Get a workspace by name mutably.
    pub fn workspace_by_name_mut(&mut self, name: &str) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.name == name)
    }

    /// Get output by id.
    pub fn output(&self, id: OutputId) -> Option<&Output> {
        self.outputs.iter().find(|o| o.id == id)
    }

    /// Get output by id mutably.
    pub fn output_mut(&mut self, id: OutputId) -> Option<&mut Output> {
        self.outputs.iter_mut().find(|o| o.id == id)
    }

    /// Get all workspaces that are currently visible (assigned to an output).
    pub fn visible_workspaces(&self) -> Vec<&Workspace> {
        self.workspaces
            .iter()
            .filter(|w| w.active_output.is_some())
            .collect()
    }
}
