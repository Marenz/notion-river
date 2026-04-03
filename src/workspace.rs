use crate::layout::{FrameId, Rect, SplitNode};

/// Identifies a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(pub usize);

/// A workspace owns a layout tree and is assigned to an output.
#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// Ordered list of output matchers from config (semantic names, positions,
    /// or connector names). First match wins.
    pub preferred_output: Vec<String>,
    /// Which output this workspace is currently displayed on (runtime).
    pub active_output: Option<OutputId>,
    /// The static tiling tree.
    pub root: SplitNode,
    /// The currently focused frame within this workspace.
    pub focused_frame: FrameId,
    /// Whether this workspace was auto-created (not from config).
    #[allow(dead_code)]
    pub auto_created: bool,
}

// ── Output geometry helpers ──────────────────────────────────────────────

/// Returns a geometry key for an output, e.g. "2560x1440@0,0".
/// Returns None if dimensions are not yet known.
pub fn output_geometry_key(output: &Output) -> Option<String> {
    if output.width > 0 && output.height > 0 {
        Some(format!(
            "{}x{}@{},{}",
            output.width, output.height, output.x, output.y
        ))
    } else {
        None
    }
}

/// Find the output matching a semantic specifier.
///
/// Supported specifiers:
/// - `"primary"` — monitor whose center is closest to the bounding-box center
/// - `"portrait"` — first monitor where height > width
/// - `"laptop"` — first monitor with eDP-* connector name
/// - `"X,Y"` — monitor at exact logical position
/// - anything else — connector name fallback
fn find_matching_output(specifier: &str, outputs: &[Output]) -> Option<OutputId> {
    let ready: Vec<&Output> = outputs
        .iter()
        .filter(|o| !o.removed && o.width > 0 && o.height > 0)
        .collect();

    if ready.is_empty() {
        return None;
    }

    match specifier {
        "primary" => {
            // Most centered: closest center to the bounding-box center of all outputs.
            let min_x = ready.iter().map(|o| o.x).min()?;
            let max_x = ready.iter().map(|o| o.x + o.width).max()?;
            let min_y = ready.iter().map(|o| o.y).min()?;
            let max_y = ready.iter().map(|o| o.y + o.height).max()?;
            let cx = (min_x + max_x) / 2;
            let cy = (min_y + max_y) / 2;
            ready
                .iter()
                .min_by_key(|o| {
                    let ox = o.x + o.width / 2;
                    let oy = o.y + o.height / 2;
                    (ox - cx).pow(2) + (oy - cy).pow(2)
                })
                .map(|o| o.id)
        }
        "portrait" => ready.iter().find(|o| o.height > o.width).map(|o| o.id),
        "laptop" => ready
            .iter()
            .find(|o| {
                o.name
                    .as_ref()
                    .is_some_and(|n| n.starts_with("eDP"))
            })
            .map(|o| o.id),
        s if s.contains(',') && s.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '-') => {
            // Position match "X,Y"
            let parts: Vec<&str> = s.split(',').collect();
            if parts.len() == 2
                && let (Ok(x), Ok(y)) = (parts[0].parse::<i32>(), parts[1].parse::<i32>())
            {
                return ready.iter().find(|o| o.x == x && o.y == y).map(|o| o.id);
            }
            None
        }
        name => {
            // Connector name fallback
            ready
                .iter()
                .find(|o| o.name.as_deref() == Some(name))
                .map(|o| o.id)
        }
    }
}

/// Try a fallback chain of specifiers against the current outputs.
/// Returns the first matching output.
pub(crate) fn find_preferred_output(specifiers: &[String], outputs: &[Output]) -> Option<OutputId> {
    for spec in specifiers {
        if let Some(id) = find_matching_output(spec, outputs) {
            return Some(id);
        }
    }
    None
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
    /// Output scale factor (integer from wl_output.scale).
    pub scale: i32,
    /// Physical mode dimensions (from wl_output.mode event).
    pub physical_width: i32,
    pub physical_height: i32,
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
            scale: 1,
            physical_width: 0,
            physical_height: 0,
            removed: false,
        }
    }

    /// Compute the actual fractional scale from physical vs logical dimensions.
    /// Falls back to the integer wl_output.scale if physical dims aren't known.
    pub fn fractional_scale(&self) -> f64 {
        if self.physical_width > 0 && self.width > 0 {
            self.physical_width as f64 / self.width as f64
        } else {
            self.scale.max(1) as f64
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
                    preferred_output: cfg
                        .output
                        .as_ref()
                        .map(|o| o.matchers().into_iter().map(str::to_owned).collect())
                        .unwrap_or_default(),
                    active_output: None,
                    root,
                    focused_frame,
                    auto_created: false,
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

    /// Add or update an output. Actual workspace assignment is deferred to
    /// `reassign_outputs()` which runs when geometry and name are known.
    pub fn add_output(&mut self, output: Output) {
        let output_id = output.id;
        if let Some(existing) = self.outputs.iter_mut().find(|o| o.id == output_id) {
            *existing = output;
        } else {
            self.outputs.push(output);
        }
        // Assignment deferred to reassign_outputs() — geometry may not be available yet.
        // If this is the only output and it has geometry, try immediate assignment.
        let has_geometry = self
            .output(output_id)
            .is_some_and(|o| o.width > 0 && o.height > 0);
        if has_geometry && !self.output_workspace.contains_key(&output_id) {
            self.reassign_outputs();
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

    /// Re-assign workspaces to outputs using geometry-based semantic matching.
    /// Called when output geometry or names become known.
    pub fn reassign_outputs(&mut self) {
        // Phase 1: Collect assignments without mutating (avoids borrow issues).
        let mut assignments: Vec<(WorkspaceId, OutputId)> = Vec::new();

        let unassigned_outputs: Vec<OutputId> = self
            .outputs
            .iter()
            .filter(|o| !o.removed && o.width > 0 && !self.output_workspace.contains_key(&o.id))
            .map(|o| o.id)
            .collect();

        for &output_id in &unassigned_outputs {
            let geo_key = self.output(output_id).and_then(output_geometry_key);

            // 1. Check saved_visible (geometry-based keys from last session)
            let saved_ws = geo_key.as_ref().and_then(|gk| {
                self.saved_visible
                    .iter()
                    .find(|(saved_geo, _)| saved_geo == gk)
                    .and_then(|(_, ws_name)| {
                        self.workspaces
                            .iter()
                            .find(|ws| {
                                ws.name == *ws_name
                                    && ws.active_output.is_none()
                                    && !assignments.iter().any(|(wid, _)| *wid == ws.id)
                            })
                            .map(|ws| ws.id)
                    })
            });

            // 2. Fall back to preferred_output semantic matching
            let ws_id = saved_ws.or_else(|| {
                self.workspaces
                    .iter()
                    .find(|ws| {
                        ws.active_output.is_none()
                            && !assignments.iter().any(|(wid, _)| *wid == ws.id)
                            && find_preferred_output(&ws.preferred_output, &self.outputs)
                                == Some(output_id)
                    })
                    .map(|ws| ws.id)
            });

            if let Some(ws_id) = ws_id {
                assignments.push((ws_id, output_id));
            }
        }

        // Phase 1b: Any outputs still unassigned get any remaining unassigned workspace.
        let assigned_outputs: std::collections::HashSet<OutputId> = assignments
            .iter()
            .map(|(_, oid)| *oid)
            .chain(self.output_workspace.keys().copied())
            .collect();
        let still_empty: Vec<OutputId> = self
            .outputs
            .iter()
            .filter(|o| !o.removed && o.width > 0 && !assigned_outputs.contains(&o.id))
            .map(|o| o.id)
            .collect();

        for &output_id in &still_empty {
            let ws_id = self
                .workspaces
                .iter()
                .find(|ws| {
                    ws.active_output.is_none()
                        && !assignments.iter().any(|(wid, _)| *wid == ws.id)
                })
                .map(|ws| ws.id);
            if let Some(ws_id) = ws_id {
                assignments.push((ws_id, output_id));
            }
        }

        // Phase 2: Apply assignments.
        for (ws_id, output_id) in &assignments {
            if let Some(&old_ws) = self.output_workspace.get(output_id)
                && let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == old_ws)
            {
                ws.active_output = None;
            }
            self.assign_workspace_to_output(*ws_id, *output_id);
            let geo = self
                .output(*output_id)
                .and_then(output_geometry_key)
                .unwrap_or_default();
            log::info!(
                "Assigned workspace '{}' to output {geo}",
                self.workspaces[ws_id.0].name
            );
        }

        // Phase 3: Auto-create workspaces for monitors that still have nothing.
        self.ensure_all_outputs_have_workspace();
    }

    /// Create temporary workspaces for any output that has no workspace assigned.
    fn ensure_all_outputs_have_workspace(&mut self) {
        let empty_outputs: Vec<OutputId> = self
            .outputs
            .iter()
            .filter(|o| !o.removed && o.width > 0 && !self.output_workspace.contains_key(&o.id))
            .map(|o| o.id)
            .collect();

        for output_id in empty_outputs {
            let output_label = self
                .output(output_id)
                .and_then(|o| o.name.clone())
                .unwrap_or_else(|| format!("{}", output_id.0));

            let name = format!("auto:{output_label}");
            let id = WorkspaceId(self.workspaces.len());
            log::info!("Auto-creating workspace '{name}' for unoccupied output");
            self.workspaces.push(Workspace {
                id,
                name,
                preferred_output: Vec::new(),
                active_output: None,
                root: SplitNode::single_frame(),
                focused_frame: FrameId(0),
                auto_created: true,
            });
            self.assign_workspace_to_output(id, output_id);
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
        // 1. Use the workspace's preferred output (semantic match) if available
        // 2. Otherwise use the currently focused output
        let preferred_output =
            find_preferred_output(&self.workspaces[target_ws.0].preferred_output, &self.outputs);

        let target_output = preferred_output.unwrap_or_else(|| {
            self.workspaces[self.focused_workspace.0]
                .active_output
                .unwrap_or(OutputId(0))
        });

        // Unassign whatever workspace is currently on that output
        let displaced_ws: Option<WorkspaceId> = self.output_workspace.get(&target_output).copied();
        if let Some(old_ws_id) = displaced_ws
            && let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == old_ws_id)
        {
            ws.active_output = None;
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
