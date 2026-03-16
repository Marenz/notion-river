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
            removed: false,
        }
    }

    /// The usable area for tiling (full output for now; bars claim
    /// space via layer-shell which River handles automatically).
    pub fn usable_rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
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
            // First, try to find a workspace that prefers this output
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
    fn assign_workspace_to_output(&mut self, ws_id: WorkspaceId, output_id: OutputId) {
        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
            ws.active_output = Some(output_id);
        }
        self.output_workspace.insert(output_id, ws_id);
    }

    /// Switch the active workspace on the output that currently has focus.
    pub fn switch_workspace(&mut self, target_name: &str) {
        let target_ws = match self.workspaces.iter().find(|w| w.name == target_name) {
            Some(ws) => ws.id,
            None => {
                log::warn!("Workspace '{target_name}' not found");
                return;
            }
        };

        // Find the output of the currently focused workspace
        let current_ws = &self.workspaces[self.focused_workspace.0];
        let current_output = current_ws.active_output;

        if let Some(output_id) = current_output {
            // If target workspace is already on another output, just focus it
            if let Some(target_ws_data) = self.workspaces.iter().find(|w| w.id == target_ws) {
                if let Some(other_output) = target_ws_data.active_output {
                    if other_output != output_id {
                        // Just switch focus to that output's workspace
                        self.focused_workspace = target_ws;
                        return;
                    }
                }
            }

            // Unassign current workspace from this output
            if let Some(ws) = self
                .workspaces
                .iter_mut()
                .find(|w| w.id == self.focused_workspace)
            {
                ws.active_output = None;
            }

            // Assign target workspace to this output
            self.assign_workspace_to_output(target_ws, output_id);
        }

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
