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
/// - `"center"` — monitor whose horizontal center is closest to the setup center
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
        "center" => {
            // Treat "center" as the horizontal middle monitor. Using full 2D distance
            // lets a tall portrait display win just because it is vertically closer
            // to the bounding-box center.
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
                    ((ox - cx).abs(), (oy - cy).abs(), -(o.width * o.height))
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
    find_preferred_output_ranked(specifiers, outputs).map(|(_, id)| id)
}

/// Like `find_preferred_output` but also returns the index in `specifiers`
/// where the match occurred. Lower index = stronger preference. Used to break
/// ties when several workspaces all want the same output via different
/// preference depths.
pub(crate) fn find_preferred_output_ranked(
    specifiers: &[String],
    outputs: &[Output],
) -> Option<(usize, OutputId)> {
    for (rank, spec) in specifiers.iter().enumerate() {
        if let Some(id) = find_matching_output(spec, outputs) {
            return Some((rank, id));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{find_preferred_output, Output, OutputId, WorkspaceManager};
    use crate::config::{OutputSpec, WorkspaceConfig};

    fn output(id: u64, x: i32, y: i32, width: i32, height: i32) -> Output {
        let mut output = Output::new(OutputId(id));
        output.width = width;
        output.height = height;
        output.x = x;
        output.y = y;
        output
    }

    fn named_output(
        id: u64,
        name: &str,
        desc: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Output {
        let mut o = output(id, x, y, width, height);
        o.name = Some(name.to_owned());
        o.description = Some(desc.to_owned());
        o
    }

    fn ws(name: &str, output: Option<OutputSpec>) -> WorkspaceConfig {
        WorkspaceConfig {
            name: name.to_owned(),
            output,
            initial_layout: None,
        }
    }

    fn three_workspace_setup() -> WorkspaceManager {
        let configs = vec![
            ws("main", Some(OutputSpec::Single("center".into()))),
            ws("secondary", Some(OutputSpec::Single("center".into()))),
            ws("utility", Some(OutputSpec::Single("center".into()))),
            ws(
                "social",
                Some(OutputSpec::Fallback(vec![
                    "portrait".into(),
                    "laptop".into(),
                ])),
            ),
            ws("work", Some(OutputSpec::Single("laptop".into()))),
            ws(
                "term",
                Some(OutputSpec::Fallback(vec![
                    "portrait".into(),
                    "laptop".into(),
                ])),
            ),
        ];
        let mut wm = WorkspaceManager::new(&configs, 0.5);
        // Tests must not touch (read or write) the user's real
        // monitor-memory.json. Replace whatever load() pulled in.
        wm.monitor_memory = crate::monitor_memory::MonitorMemory::default();
        wm
    }

    fn workspace_for(wm: &WorkspaceManager, oid: OutputId) -> Option<&str> {
        wm.workspaces
            .iter()
            .find(|w| w.active_output == Some(oid))
            .map(|w| w.name.as_str())
    }

    #[test]
    fn center_prefers_horizontal_middle_over_portrait() {
        let outputs = vec![
            output(1, 0, 1692, 1280, 800),
            output(2, 1280, 611, 2560, 1440),
            output(3, 3840, 0, 1440, 2560),
        ];

        let preferred = find_preferred_output(&["center".to_owned()], &outputs);

        assert_eq!(preferred, Some(OutputId(2)));
    }

    #[test]
    fn first_assignment_uses_preferred_outputs() {
        let mut wm = three_workspace_setup();
        wm.outputs = vec![
            named_output(10, "eDP-1", "Sharp 0x1515", 0, 1692, 1280, 800),
            named_output(20, "DP-2", "LG 503NTWG54001", 1280, 611, 2560, 1440),
            named_output(30, "DP-10", "LG 507NTFALB971", 3840, 0, 1440, 2560),
        ];
        wm.reassign_outputs();

        assert_eq!(workspace_for(&wm, OutputId(10)), Some("work"));
        assert_eq!(workspace_for(&wm, OutputId(20)), Some("main"));
        assert_eq!(workspace_for(&wm, OutputId(30)), Some("social"));
    }

    #[test]
    fn memory_overrides_preferred_on_subsequent_runs() {
        let mut wm = three_workspace_setup();
        wm.outputs = vec![
            named_output(10, "eDP-1", "Sharp 0x1515", 0, 1692, 1280, 800),
            named_output(20, "DP-2", "LG 503NTWG54001", 1280, 611, 2560, 1440),
            named_output(30, "DP-10", "LG 507NTFALB971", 3840, 0, 1440, 2560),
        ];
        // Pretend the user moved utility to the portrait monitor and that got
        // memorized.
        wm.monitor_memory
            .record("edid:LG 507NTFALB971".into(), "utility".into());
        wm.reassign_outputs();

        assert_eq!(workspace_for(&wm, OutputId(30)), Some("utility"));
        // main still belongs on center via preferred_output.
        assert_eq!(workspace_for(&wm, OutputId(20)), Some("main"));
        // social loses portrait but should not snap to center; it falls
        // through to laptop via its second preference.
        // (eDP-1 is taken by work first via config order; social ends up
        // unplaced or taking the center fallback.) See next test for detail.
    }

    #[test]
    fn unplugging_a_monitor_keeps_others_unchanged() {
        let mut wm = three_workspace_setup();
        wm.outputs = vec![
            named_output(10, "eDP-1", "Sharp 0x1515", 0, 1692, 1280, 800),
            named_output(20, "DP-2", "LG 503NTWG54001", 1280, 611, 2560, 1440),
            named_output(30, "DP-10", "LG 507NTFALB971", 3840, 0, 1440, 2560),
        ];
        wm.reassign_outputs();

        // Unplug the portrait monitor.
        wm.remove_output(OutputId(30));
        wm.reassign_outputs();

        // The two remaining monitors should keep what they had.
        assert_eq!(workspace_for(&wm, OutputId(10)), Some("work"));
        assert_eq!(workspace_for(&wm, OutputId(20)), Some("main"));
        // The portrait workspace is invisible but still exists.
        assert!(wm
            .workspaces
            .iter()
            .find(|w| w.name == "social")
            .is_some_and(|w| w.active_output.is_none()));
    }

    #[test]
    fn replugging_restores_remembered_workspace() {
        let mut wm = three_workspace_setup();
        wm.outputs = vec![
            named_output(10, "eDP-1", "Sharp 0x1515", 0, 1692, 1280, 800),
            named_output(20, "DP-2", "LG 503NTWG54001", 1280, 611, 2560, 1440),
            named_output(30, "DP-10", "LG 507NTFALB971", 3840, 0, 1440, 2560),
        ];
        wm.reassign_outputs();
        // Pretend the user moved term onto the portrait, persisting that.
        wm.monitor_memory
            .record("edid:LG 507NTFALB971".into(), "term".into());

        // Unplug.
        wm.remove_output(OutputId(30));
        wm.reassign_outputs();
        assert!(workspace_for(&wm, OutputId(30)).is_none());

        // Replug — same physical monitor, fresh wayland id (40 instead of 30).
        wm.outputs.push(named_output(
            40,
            "DP-10",
            "LG 507NTFALB971",
            3840,
            0,
            1440,
            2560,
        ));
        wm.reassign_outputs();

        assert_eq!(workspace_for(&wm, OutputId(40)), Some("term"));
    }

    #[test]
    fn temporary_switch_during_partial_replug_does_not_overwrite_memory() {
        let mut wm = three_workspace_setup();
        wm.outputs = vec![
            named_output(10, "eDP-1", "Sharp 0x1515", 640, 1440, 1280, 800),
            named_output(20, "DP-7", "LG 507NTFALB971", 2560, 0, 1440, 2560),
            named_output(30, "DP-3", "LG 503NTWG54001", 0, 0, 2560, 1440),
        ];
        wm.monitor_memory
            .record("edid:Sharp 0x1515".into(), "work".into());
        wm.monitor_memory
            .record("edid:LG 507NTFALB971".into(), "term".into());
        wm.monitor_memory
            .record("edid:LG 503NTWG54001".into(), "main".into());
        wm.reassign_outputs();

        wm.remove_output(OutputId(30));
        wm.reassign_outputs();

        wm.switch_workspace("main");

        assert_eq!(
            wm.monitor_memory.get("edid:LG 507NTFALB971"),
            Some("term")
        );
        assert_eq!(
            wm.monitor_memory.get("edid:LG 503NTWG54001"),
            Some("main")
        );
    }

    #[test]
    fn maybe_reassign_skips_when_set_unchanged() {
        let mut wm = three_workspace_setup();
        wm.outputs = vec![
            named_output(10, "eDP-1", "Sharp 0x1515", 0, 1692, 1280, 800),
            named_output(20, "DP-2", "LG 503NTWG54001", 1280, 611, 2560, 1440),
        ];
        wm.maybe_reassign_outputs();
        assert_eq!(workspace_for(&wm, OutputId(10)), Some("work"));
        assert_eq!(workspace_for(&wm, OutputId(20)), Some("main"));

        // User manually moves a workspace.
        wm.workspaces[0].active_output = Some(OutputId(20)); // pretend swap
        wm.workspaces[1].active_output = Some(OutputId(10));
        wm.output_workspace.insert(OutputId(20), super::WorkspaceId(0));
        wm.output_workspace.insert(OutputId(10), super::WorkspaceId(1));

        // Same set of monitors -> no reassignment, manual layout preserved.
        wm.maybe_reassign_outputs();
        assert_eq!(workspace_for(&wm, OutputId(20)), Some("main"));
        assert_eq!(workspace_for(&wm, OutputId(10)), Some("secondary"));
    }

    #[test]
    fn same_resolution_monitors_do_not_collide() {
        let mut wm = three_workspace_setup();
        wm.outputs = vec![
            named_output(10, "eDP-1", "Sharp 0x1515", 0, 1692, 1280, 800),
            named_output(
                20,
                "DP-3",
                "LG Electronics LG HDR 4K 503NTWG54001",
                1280,
                0,
                3840,
                2160,
            ),
            named_output(
                30,
                "DP-7",
                "LG Electronics LG HDR 4K 507NTFALB971",
                5120,
                0,
                3840,
                2160,
            ),
        ];
        wm.monitor_memory.record(
            "edid:LG Electronics LG HDR 4K 503NTWG54001".into(),
            "main".into(),
        );
        wm.monitor_memory.record(
            "edid:LG Electronics LG HDR 4K 507NTFALB971".into(),
            "social".into(),
        );
        wm.monitor_memory
            .record("edid:Sharp 0x1515".into(), "work".into());

        wm.maybe_reassign_outputs();
        assert_eq!(workspace_for(&wm, OutputId(10)), Some("work"));
        assert_eq!(workspace_for(&wm, OutputId(20)), Some("main"));
        assert_eq!(workspace_for(&wm, OutputId(30)), Some("social"));

        wm.workspaces[0].active_output = Some(OutputId(30));
        wm.workspaces[3].active_output = Some(OutputId(20));
        wm.output_workspace.insert(OutputId(30), super::WorkspaceId(0));
        wm.output_workspace.insert(OutputId(20), super::WorkspaceId(3));

        wm.reassign_outputs();
        assert_eq!(workspace_for(&wm, OutputId(10)), Some("work"));
        assert_eq!(workspace_for(&wm, OutputId(20)), Some("main"));
        assert_eq!(workspace_for(&wm, OutputId(30)), Some("social"));
    }

    #[test]
    fn reassign_deferred_until_description_arrives() {
        let mut wm = three_workspace_setup();
        let mut external = output(20, 1280, 611, 3840, 2160);
        external.name = Some("DP-3".to_owned());
        wm.outputs = vec![
            named_output(10, "eDP-1", "Sharp 0x1515", 0, 1692, 1280, 800),
            external,
        ];

        assert!(wm.all_outputs_have_geometry());
        assert!(!wm.all_outputs_have_stable_identity());

        wm.maybe_reassign_outputs();
        assert!(wm.output_workspace.is_empty());
        assert!(wm.workspaces.iter().all(|ws| ws.active_output.is_none()));

        wm.output_mut(OutputId(20)).unwrap().description =
            Some("LG Electronics LG HDR 4K 503NTWG54001".to_owned());

        assert!(wm.all_outputs_have_stable_identity());

        wm.maybe_reassign_outputs();
        assert_eq!(workspace_for(&wm, OutputId(10)), Some("work"));
        assert_eq!(workspace_for(&wm, OutputId(20)), Some("main"));
    }
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
    /// The wl_output description (EDID make/model/serial, e.g.
    /// "LG Electronics LG HDR 4K 503NTWG54001"). Stable monitor identity.
    pub description: Option<String>,
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
    /// Output transform (from wl_output.transform event).
    /// Values: 0=normal, 1=90°, 2=180°, 3=270°, 4-7 are flipped variants.
    #[allow(dead_code)]
    pub transform: i32,
    /// Whether the output has been removed.
    pub removed: bool,
}

impl Output {
    pub fn new(id: OutputId) -> Self {
        Self {
            id,
            name: None,
            description: None,
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
            transform: 0,
            removed: false,
        }
    }

    /// Compute the actual fractional scale from physical vs logical dimensions.
    /// Falls back to the integer wl_output.scale if physical dims aren't known.
    pub fn fractional_scale(&self) -> f64 {
        if self.width > 0 && self.height > 0 && self.physical_width > 0 && self.physical_height > 0 {
            let (physical_width, _physical_height) = match self.transform {
                // Rotated outputs swap their logical axes relative to the
                // physical mode dimensions reported by wl_output.mode.
                1 | 3 | 5 | 7 => (self.physical_height, self.physical_width),
                _ => (self.physical_width, self.physical_height),
            };
            physical_width as f64 / self.width as f64
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
    /// Per-monitor memory of "last workspace shown here". Persisted to disk.
    /// Loaded once on startup; updated whenever an assignment changes.
    pub monitor_memory: crate::monitor_memory::MonitorMemory,
    /// Set to true when output layout changes and stabilizes.
    /// Consumed by the manage cycle to fire the outputs-changed hook.
    pub outputs_changed: bool,
    /// Snapshot of monitor keys present the last time `reassign_outputs` ran.
    /// Used to decide whether the connected-monitor set actually changed.
    last_connected_keys: std::collections::BTreeSet<String>,
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
            monitor_memory: crate::monitor_memory::MonitorMemory::load(),
            outputs_changed: false,
            last_connected_keys: std::collections::BTreeSet::new(),
        }
    }

    /// Add or update an output. Just stores the data — reassignment is gated
    /// on the connected-monitor set actually changing and is triggered
    /// explicitly by the caller via `maybe_reassign_outputs()` once metadata
    /// has settled.
    pub fn add_output(&mut self, output: Output) {
        let output_id = output.id;
        if let Some(existing) = self.outputs.iter_mut().find(|o| o.id == output_id) {
            *existing = output;
        } else {
            self.outputs.push(output);
        }
    }

    /// Run `reassign_outputs` only if (a) all currently-connected outputs have
    /// geometry, (b) all of them have a stable monitor identity, and (c) the
    /// resulting set of monitor keys differs from the last assignment pass.
    /// This is the single entry point that callers should use after output
    /// events to keep the algorithm idempotent and cheap.
    pub fn maybe_reassign_outputs(&mut self) {
        if !self.all_outputs_have_geometry() {
            return;
        }
        if !self.all_outputs_have_stable_identity() {
            return;
        }
        if !self.connected_monitor_keys_changed() {
            return;
        }
        self.reassign_outputs();
    }

    /// True when every non-removed output has known dimensions.
    pub fn all_outputs_have_geometry(&self) -> bool {
        let non_removed: Vec<&Output> = self.outputs.iter().filter(|o| !o.removed).collect();
        !non_removed.is_empty() && non_removed.iter().all(|o| o.width > 0 && o.height > 0)
    }

    /// True when every non-removed output has a strict, stable monitor key.
    pub fn all_outputs_have_stable_identity(&self) -> bool {
        let non_removed: Vec<&Output> = self.outputs.iter().filter(|o| !o.removed).collect();
        !non_removed.is_empty()
            && non_removed
                .iter()
                .all(|output| crate::monitor_memory::monitor_key(output).is_some())
    }

    /// True when every non-removed output has complete metadata from wl_output
    /// (physical dimensions from Mode event, and logical dimensions from
    /// river_output_v1). This is stricter than `all_outputs_have_geometry()`
    /// and should be used before persisting or exporting output state.
    #[allow(dead_code)]
    pub fn all_outputs_have_metadata(&self) -> bool {
        let non_removed: Vec<&Output> = self.outputs.iter().filter(|o| !o.removed).collect();
        !non_removed.is_empty()
            && non_removed.iter().all(|o| {
                o.width > 0
                    && o.height > 0
                    && o.physical_width > 0
                    && o.physical_height > 0
            })
    }

    /// Remove an output. Unassigns its workspace; the workspace itself stays
    /// alive (just becomes invisible until the monitor returns or the user
    /// switches to it on another monitor).
    pub fn remove_output(&mut self, output_id: OutputId) {
        self.output_workspace.retain(|oid, _| *oid != output_id);

        for ws in &mut self.workspaces {
            if ws.active_output == Some(output_id) {
                ws.active_output = None;
            }
        }

        self.outputs.retain(|o| o.id != output_id);
        // Force the next maybe_reassign_outputs to run by invalidating the
        // snapshot — even if the new set happens to equal a previous one.
        self.last_connected_keys.clear();
        self.outputs_changed = true;
    }

    /// Assign a workspace to an output.
    pub fn assign_workspace_to_output(&mut self, ws_id: WorkspaceId, output_id: OutputId) {
        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
            ws.active_output = Some(output_id);
        }
        self.output_workspace.insert(output_id, ws_id);
    }

    /// Build the set of stable monitor keys for currently-connected outputs.
    /// Outputs without enough info to produce a key are skipped.
    fn connected_monitor_keys(&self) -> std::collections::BTreeSet<String> {
        self.outputs
            .iter()
            .filter(|o| !o.removed && o.width > 0)
            .filter_map(crate::monitor_memory::monitor_key)
            .collect()
    }

    /// Re-assign workspaces to outputs.
    ///
    /// One pass, fully deterministic, with three tiers per output:
    ///
    /// 1. **Per-monitor memory**: if `monitor_memory` knows what was last shown
    ///    on this physical monitor, put that workspace back.
    /// 2. **Preferred-output match**: walk workspaces in config order, place the
    ///    first unplaced workspace whose `preferred_output` chain resolves to
    ///    this output.
    /// 3. **Any remaining**: first unplaced workspace in config order.
    ///
    /// Outputs that gain a placement also have their memory updated. Workspaces
    /// that don't get placed remain invisible (`active_output = None`), which is
    /// fine — they're still switch-to-able.
    ///
    /// Wipes existing assignments first so this function is idempotent and
    /// independent of prior state. Called only when the connected-monitor set
    /// actually changed (see `connected_monitor_keys_changed`).
    pub fn reassign_outputs(&mut self) {
        let connected_keys = self.connected_monitor_keys();

        // Wipe assignments so this function is idempotent and prior partial
        // assignments don't leak into the result.
        self.output_workspace.clear();
        // Drop auto-created workspaces from previous runs so they don't
        // accumulate; real workspaces just have their active_output cleared.
        let auto_ids: Vec<WorkspaceId> = self
            .workspaces
            .iter()
            .filter(|w| w.auto_created)
            .map(|w| w.id)
            .collect();
        if !auto_ids.is_empty() {
            self.workspaces.retain(|w| !w.auto_created);
            // Re-pack ids so they remain dense (vec index = id).
            for (i, ws) in self.workspaces.iter_mut().enumerate() {
                ws.id = WorkspaceId(i);
            }
            // Heal focus if it pointed at a dropped workspace.
            if self.focused_workspace.0 >= self.workspaces.len() {
                self.focused_workspace = WorkspaceId(0);
            }
        }
        for ws in &mut self.workspaces {
            ws.active_output = None;
        }

        // Build a stable iteration order: outputs sorted by id.
        let mut output_ids: Vec<OutputId> = self
            .outputs
            .iter()
            .filter(|o| !o.removed && o.width > 0)
            .map(|o| o.id)
            .collect();
        output_ids.sort_by_key(|o| o.0);

        let mut placed: std::collections::HashSet<WorkspaceId> =
            std::collections::HashSet::new();

        // Tier 1: monitor memory.
        for &oid in &output_ids {
            let key = match self.output(oid).and_then(crate::monitor_memory::monitor_key) {
                Some(k) => k,
                None => continue,
            };
            let remembered_name = match self.monitor_memory.get(&key) {
                Some(name) => name.to_owned(),
                None => continue,
            };
            let ws_id = self
                .workspaces
                .iter()
                .find(|ws| ws.name == remembered_name && !placed.contains(&ws.id))
                .map(|ws| ws.id);
            if let Some(ws_id) = ws_id {
                self.assign_workspace_to_output(ws_id, oid);
                placed.insert(ws_id);
                log::info!(
                    "Assigned workspace '{}' to monitor '{key}' (from memory)",
                    self.workspaces[ws_id.0].name,
                );
            }
        }

        // Tier 2: preferred-output match. For each output pick the workspace
        // with the strongest (lowest-rank) preference that resolves to it,
        // falling back to config order on ties. This ensures a workspace whose
        // primary preference is a given monitor wins over one that only lists
        // it as a fallback.
        for &oid in &output_ids {
            if self.output_workspace.contains_key(&oid) {
                continue;
            }
            let ws_id = self
                .workspaces
                .iter()
                .filter(|ws| !placed.contains(&ws.id))
                .filter_map(|ws| {
                    find_preferred_output_ranked(&ws.preferred_output, &self.outputs)
                        .filter(|(_, matched)| *matched == oid)
                        .map(|(rank, _)| (rank, ws.id))
                })
                .min_by_key(|(rank, ws_id)| (*rank, ws_id.0))
                .map(|(_, ws_id)| ws_id);
            if let Some(ws_id) = ws_id {
                self.assign_workspace_to_output(ws_id, oid);
                placed.insert(ws_id);
                log::info!(
                    "Assigned workspace '{}' to output {} (from preferred_output)",
                    self.workspaces[ws_id.0].name,
                    self.output(oid).and_then(output_geometry_key).unwrap_or_default(),
                );
            }
        }

        // Tier 3: any remaining unplaced workspace, config order.
        for &oid in &output_ids {
            if self.output_workspace.contains_key(&oid) {
                continue;
            }
            let ws_id = self
                .workspaces
                .iter()
                .find(|ws| !placed.contains(&ws.id))
                .map(|ws| ws.id);
            if let Some(ws_id) = ws_id {
                self.assign_workspace_to_output(ws_id, oid);
                placed.insert(ws_id);
                log::info!(
                    "Assigned workspace '{}' to output {} (fallback)",
                    self.workspaces[ws_id.0].name,
                    self.output(oid).and_then(output_geometry_key).unwrap_or_default(),
                );
            }
        }

        // Auto-create workspaces for any outputs we still couldn't fill.
        self.ensure_all_outputs_have_workspace();

        self.last_connected_keys = connected_keys;
        self.outputs_changed = true;
    }

    /// Returns true when the set of stable monitor keys differs from the
    /// snapshot at the last `reassign_outputs` call. Used to suppress
    /// reassignment thrash when only secondary metadata (scale, transform)
    /// changes without affecting which physical monitors are connected.
    pub fn connected_monitor_keys_changed(&self) -> bool {
        self.connected_monitor_keys() != self.last_connected_keys
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

    /// Switch to a workspace.
    ///
    /// If the target is already visible somewhere, just focus it. Otherwise
    /// pick a monitor for it: walk the workspace's `preferred_output` chain
    /// against currently-connected outputs and use the first match. If none
    /// of the preferences are connected, fall back to whichever output the
    /// currently-focused workspace lives on.
    ///
    /// Whichever workspace was previously on the chosen output gets pushed to
    /// invisible — it isn't deleted, so the user can switch back to it.
    ///
    /// This intentionally does not persist monitor memory. Switching a
    /// workspace during dock hotplug is often just a rescue action while the
    /// preferred monitor is absent, not a new long-term monitor assignment.
    pub fn switch_workspace(&mut self, target_name: &str) {
        let target_ws = match self.workspaces.iter().find(|w| w.name == target_name) {
            Some(ws) => ws.id,
            None => {
                log::warn!("Workspace '{target_name}' not found");
                return;
            }
        };

        // Already visible -> just focus.
        if self.workspaces[target_ws.0].active_output.is_some() {
            self.focused_workspace = target_ws;
            return;
        }

        let preferred_output =
            find_preferred_output(&self.workspaces[target_ws.0].preferred_output, &self.outputs);

        let target_output = preferred_output.unwrap_or_else(|| {
            self.workspaces[self.focused_workspace.0]
                .active_output
                .unwrap_or(OutputId(0))
        });

        // Push the existing occupant aside.
        let displaced_ws: Option<WorkspaceId> = self.output_workspace.get(&target_output).copied();
        if let Some(old_ws_id) = displaced_ws
            && let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == old_ws_id)
        {
            ws.active_output = None;
        }

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
