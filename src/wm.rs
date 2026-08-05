use std::collections::HashMap;

use wayland_backend::client::ObjectId;
use wayland_client::{Proxy, QueueHandle};

use wayland_client::protocol::{wl_compositor::WlCompositor, wl_shm::WlShm};

use crate::actions::Action;
use crate::bindings::{get_profile_bindings, parse_all_bindings, Binding};
use crate::config::Config;
use crate::decorations::{DecorationManager, EmptyFrameManager};
use crate::layout::{FrameId, Rect, WindowRef};
use crate::workspace::{OutputId, WorkspaceManager};

use crate::protocol::{
    river_node_v1::RiverNodeV1,
    river_pointer_binding_v1::RiverPointerBindingV1,
    river_seat_v1::{Modifiers, RiverSeatV1},
    river_window_manager_v1::RiverWindowManagerV1,
    river_window_v1::{Edges, RiverWindowV1},
    river_xkb_binding_v1::RiverXkbBindingV1,
    river_xkb_bindings_v1::RiverXkbBindingsV1,
};

/// Top-level application state.
#[derive(Debug)]
pub struct AppData {
    pub river_wm: Option<RiverWindowManagerV1>,
    pub wm_unavailable: bool,
    pub river_xkb: Option<RiverXkbBindingsV1>,
    pub river_layer_shell: Option<crate::protocol::river_layer_shell_v1::RiverLayerShellV1>,
    pub wl_compositor: Option<WlCompositor>,
    pub wl_shm: Option<WlShm>,
    pub wp_viewporter: Option<crate::protocol::wp_viewporter::WpViewporter>,
    /// Map from wl_output global name (u32) to river OutputId.
    pub wl_output_map: std::collections::HashMap<u32, OutputId>,
    /// Map from OutputId to river_output_v1 proxy (for fullscreen etc).
    pub river_outputs:
        std::collections::HashMap<u64, crate::protocol::river_output_v1::RiverOutputV1>,
    /// Map from wl_output global name (u32) to connector name string.
    pub wl_output_names: std::collections::HashMap<u32, String>,
    /// Buffered wl_output mode data for globals that arrived before the
    /// river_output_v1 → wl_output mapping was established.
    pub wl_output_modes: std::collections::HashMap<u32, (i32, i32)>,
    /// Buffered wl_output scale data (same race condition as modes).
    pub wl_output_scales: std::collections::HashMap<u32, i32>,
    /// Buffered wl_output transform data.
    pub wl_output_transforms: std::collections::HashMap<u32, i32>,
    /// Buffered wl_output description data (EDID make/model/serial).
    pub wl_output_descriptions: std::collections::HashMap<u32, String>,
    /// wl_seat global name (for binding wl_pointer).
    pub wl_seat_name: Option<u32>,
    /// Pointer created only while wl_seat advertises pointer capability.
    pub wl_pointer: Option<wayland_client::protocol::wl_pointer::WlPointer>,
    /// Pending tab click: (workspace_index, frame_id, tab_index) from decoration click
    pub pending_tab_click: Option<(usize, FrameId, usize)>,
    /// Current wl_pointer surface (protocol id) and surface-local x
    pub wl_pointer_surface: Option<u32>,
    pub wl_pointer_surface_x: f64,
    pub wm: WindowManager,
    pub monitors: crate::monitors::Monitors,
    pub output_head_proxies: std::collections::HashMap<
        u64,
        crate::protocol::zwlr_output_head_v1::ZwlrOutputHeadV1,
    >,
    pub output_mode_proxies: std::collections::HashMap<
        u64,
        crate::protocol::zwlr_output_mode_v1::ZwlrOutputModeV1,
    >,
}

impl Default for AppData {
    fn default() -> Self {
        Self {
            river_wm: None,
            wm_unavailable: false,
            river_xkb: None,
            river_layer_shell: None,
            wl_compositor: None,
            wl_shm: None,
            wp_viewporter: None,
            wl_output_map: std::collections::HashMap::new(),
            river_outputs: std::collections::HashMap::new(),
            wl_output_names: std::collections::HashMap::new(),
            wl_output_modes: std::collections::HashMap::new(),
            wl_output_scales: std::collections::HashMap::new(),
            wl_output_transforms: std::collections::HashMap::new(),
            wl_output_descriptions: std::collections::HashMap::new(),
            wl_seat_name: None,
            wl_pointer: None,
            pending_tab_click: None,
            wl_pointer_surface: None,
            wl_pointer_surface_x: 0.0,
            wm: WindowManager::new(Config::load()),
            monitors: crate::monitors::Monitors::load(),
            output_head_proxies: std::collections::HashMap::new(),
            output_mode_proxies: std::collections::HashMap::new(),
        }
    }
}

/// Input mode (normal or resize).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Resize,
}

/// The window manager state.
#[derive(Debug)]
pub struct WindowManager {
    pub config: Config,
    /// Pre-parsed ARGB8888 decoration colors.
    #[allow(dead_code)]
    pub colors: crate::config::Colors,
    pub workspaces: WorkspaceManager,
    pub windows: Vec<ManagedWindow>,
    pub seats: HashMap<ObjectId, Seat>,
    pub mode: InputMode,

    /// Normal mode bindings (parsed, ready to register).
    pub normal_bindings: Vec<Binding>,
    /// Resize mode bindings.
    pub resize_bindings: Vec<Binding>,
    /// Decoration manager for tab bars.
    pub decorations: DecorationManager,
    /// Empty frame indicator manager.
    pub empty_frames: EmptyFrameManager,
    /// Saved state for window matching on restart.
    pub saved_state: Option<crate::state::SavedState>,
    /// Manage cycles since last new window (for state restore timeout).
    pub restore_cycles_without_new: u32,
    /// Saved active tab indices to apply after window restore.
    pub saved_active_tabs: std::collections::HashMap<FrameId, usize>,
    /// Suppress WindowInteraction for one manage cycle (after tab click).
    pub suppress_interaction: bool,
    /// Whether a layer-shell surface (e.g. rofi overlay) has keyboard focus.
    pub layer_shell_has_focus: bool,
    /// IPC state for waybar workspace display.
    pub ipc: crate::ipc::IpcState,
    /// App-to-frame bindings for window placement.
    pub app_bindings: crate::app_bindings::AppBindings,
    /// Drag preview overlay.
    pub drag_preview: crate::decorations::DragPreview,
    pub resize_highlight_h: crate::decorations::ResizeHighlight,
    pub resize_highlight_v: crate::decorations::ResizeHighlight,
    /// Per-output-config workspace assignment memory.

    /// Control socket state for window/workspace switching.
    pub control: crate::control::ControlState,
    /// Currently focused floating window, if any. Takes priority over tiled focus.
    pub focused_floating: Option<u64>,
    /// Pointer hover state on decoration surfaces (for tab hover highlight).
    pub hover_surface_id: Option<u32>,
    pub hover_surface_x: f64,
    /// Set by ControlRequest::SaveMonitors. AppData snapshots live monitor
    /// state into `monitors.profiles` and persists it after the manage cycle.
    pub save_monitors_pending: bool,
    /// Set by ControlRequest::ForgetMonitors. AppData removes the saved
    /// profile for the current monitor set after the manage cycle.
    pub forget_monitors_pending: bool,
}

/// A window tracked by the WM.
#[derive(Debug)]
pub struct ManagedWindow {
    pub proxy: RiverWindowV1,
    pub node: RiverNodeV1,
    /// Unique ID derived from the proxy's ObjectId.
    pub id: u64,
    pub app_id: String,
    pub title: String,
    /// Stable identifier from River (persists across WM reconnects).
    pub identifier: Option<String>,
    pub width: i32,
    pub height: i32,
    /// Last dimensions proposed to the client via propose_dimensions.
    pub last_proposed_width: i32,
    pub last_proposed_height: i32,
    pub new: bool,
    pub closed: bool,
    /// Which frame this window is placed in.
    pub frame_id: Option<FrameId>,
    /// Whether this window is floating.
    pub floating: bool,
    pub fullscreen: bool,
    /// Number of active screen-capture sessions involving this window.
    pub capture_sessions: u32,
    /// Whether the window prefers server-side decorations.
    pub prefers_ssd: bool,
    /// How this floating window should be positioned and focused.
    pub floating_kind: FloatingKind,
    /// Floating position.
    pub float_x: i32,
    pub float_y: i32,
    /// Whether this floating window has been positioned with its real dimensions.
    pub float_positioned: bool,
    pub pointer_move_requested: Option<RiverSeatV1>,
    pub pointer_resize_requested: Option<RiverSeatV1>,
    pub pointer_resize_requested_edges: Edges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FloatingKind {
    #[default]
    Dialog,
    Notification,
}

impl FloatingKind {
    pub fn should_auto_focus(self) -> bool {
        !matches!(self, Self::Notification)
    }
}

/// Per-seat state.
#[derive(Debug)]
pub struct Seat {
    pub proxy: RiverSeatV1,
    pub new: bool,
    pub removed: bool,
    #[allow(dead_code)]
    pub focused_window: Option<RiverWindowV1>,
    pub hovered: Option<RiverWindowV1>,
    pub interacted: Option<RiverWindowV1>,
    pub xkb_bindings: HashMap<ObjectId, XkbBindingEntry>,
    pub pointer_bindings: HashMap<ObjectId, PointerBindingEntry>,
    pub pending_action: Action,
    pub op: SeatOp,
    pub op_dx: i32,
    pub op_dy: i32,
    /// Previous frame's dx/dy for computing per-frame deltas.
    pub op_prev_dx: i32,
    pub op_prev_dy: i32,
    pub op_release: bool,
    /// Pointer position at the start of the current op (for absolute positioning).
    pub op_start_pointer_x: i32,
    pub op_start_pointer_y: i32,
    /// Current absolute pointer position (from pointer_position event).
    pub pointer_x: i32,
    pub pointer_y: i32,
}

#[derive(Debug)]
pub struct XkbBindingEntry {
    pub proxy: RiverXkbBindingV1,
    pub action: Action,
    pub mode: InputMode,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct PointerBindingEntry {
    pub proxy: RiverPointerBindingV1,
    pub action: Action,
    pub is_move: bool,
}

#[derive(Debug, Clone)]
pub enum SeatOp {
    None,
    Move {
        window_id: u64,
        #[allow(dead_code)]
        start_x: i32,
        #[allow(dead_code)]
        start_y: i32,
    },
    #[allow(dead_code)]
    Resize {
        window_id: u64,
        #[allow(dead_code)]
        start_x: i32,
        #[allow(dead_code)]
        start_y: i32,
        #[allow(dead_code)]
        start_width: i32,
        #[allow(dead_code)]
        start_height: i32,
        #[allow(dead_code)]
        edges: Edges,
        /// Which axes to resize (determined by proximity to split boundaries).
        resize_h: bool,
        resize_v: bool,
        /// Path to the horizontal split boundary grabbed at drag start.
        h_boundary_path: Option<Vec<bool>>,
        /// Path to the vertical split boundary grabbed at drag start.
        v_boundary_path: Option<Vec<bool>>,
    },
    /// Resize split boundary from an empty frame area.
    #[allow(dead_code)]
    ResizeEmpty {
        frame_id: FrameId,
        resize_h: bool,
        resize_v: bool,
        /// Path to the horizontal split boundary grabbed at drag start.
        h_boundary_path: Option<Vec<bool>>,
        /// Path to the vertical split boundary grabbed at drag start.
        v_boundary_path: Option<Vec<bool>>,
    },
}

impl WindowManager {
    pub fn new(config: Config) -> Self {
        let (normal_cfgs, resize_cfgs) = get_profile_bindings(&config);
        let physical = config.general.physical_keys;
        let layout_idx = config.general.physical_layout_index;

        let normal_bindings = parse_all_bindings(&normal_cfgs, physical, layout_idx);
        let resize_bindings = parse_all_bindings(&resize_cfgs, physical, layout_idx);

        let mut workspaces =
            WorkspaceManager::new(&config.workspaces, config.general.default_split_ratio);

        // Try to restore saved state (from previous restart)
        let saved_state = crate::state::load_state();
        let saved_active_tabs = if let Some(ref state) = saved_state {
            crate::state::restore_layout(&mut workspaces, state)
        } else {
            std::collections::HashMap::new()
        };

        let colors = config.appearance.colors();
        let ipc = crate::ipc::IpcState::new();
        let control = crate::control::ControlState::new(std::sync::Arc::clone(&ipc.subscribers));
        Self {
            config,
            colors,
            workspaces,
            windows: Vec::new(),
            seats: HashMap::new(),
            mode: InputMode::Normal,
            normal_bindings,
            resize_bindings,
            decorations: DecorationManager::new(),
            empty_frames: EmptyFrameManager::new(),
            saved_state,
            restore_cycles_without_new: 0,
            saved_active_tabs,
            suppress_interaction: false,
            layer_shell_has_focus: false,
            ipc,
            app_bindings: crate::app_bindings::AppBindings::load(),
            drag_preview: crate::decorations::DragPreview::default(),
            resize_highlight_h: crate::decorations::ResizeHighlight::default(),
            resize_highlight_v: crate::decorations::ResizeHighlight::default(),
            control,
            focused_floating: None,
            hover_surface_id: None,
            hover_surface_x: 0.0,
            save_monitors_pending: false,
            forget_monitors_pending: false,
        }
    }

    // ── Manage/Render cycle ──────────────────────────────────────────────

    pub fn handle_manage_start(
        &mut self,
        proxy: &RiverWindowManagerV1,
        river_xkb: &RiverXkbBindingsV1,
        river_outputs: &std::collections::HashMap<
            u64,
            crate::protocol::river_output_v1::RiverOutputV1,
        >,
        qh: &QueueHandle<AppData>,
    ) {
        let manage_t0 = std::time::Instant::now();
        let prev_focused_frame = self.workspaces.focused_workspace().focused_frame;

        self.remove_closed_outputs();
        self.remove_closed_windows();
        self.remove_closed_seats();
        self.sync_window_titles();
        self.init_new_windows();
        self.init_new_seats(river_xkb, qh);
        let t_init = manage_t0.elapsed();

        // Check if any keyboard action is pending (before handle_pending_actions consumes it)
        let has_keyboard_action = self
            .seats
            .values()
            .any(|s| !matches!(s.pending_action, Action::None));

        self.handle_pending_actions(proxy, river_outputs);
        let t_actions = manage_t0.elapsed();
        self.handle_control_requests();
        let t_control = manage_t0.elapsed();
        self.enforce_app_bindings();
        let t_bindings = manage_t0.elapsed();
        self.apply_window_management(proxy);
        let t_wm = manage_t0.elapsed();
        self.update_binding_modes();

        // Cursor follows focus: only warp when a keyboard action changed focus,
        // not when focus-follows-mouse did (to avoid feedback loops)
        let new_focused_frame = self.workspaces.focused_workspace().focused_frame;
        if self.config.general.cursor_follows_focus
            && new_focused_frame != prev_focused_frame
            && has_keyboard_action
        {
            self.warp_cursor_to_frame(new_focused_frame);
        }

        // Output layout is fully owned by the monitors module via
        // wlr-output-management. We just clear the legacy flag here so the
        // workspace reassignment path stays consistent.
        if self.workspaces.outputs_changed {
            self.workspaces.outputs_changed = false;
        }
        let t_profile = manage_t0.elapsed();
        let t_hook = manage_t0.elapsed();

        // Update waybar workspace display via FIFO
        self.ipc.update(&self.workspaces, &self.config.appearance);
        let t_ipc = manage_t0.elapsed();
        self.control
            .update_snapshot(crate::control::build_snapshot(self));

        let manage_elapsed = manage_t0.elapsed();
        if manage_elapsed.as_millis() > 100 {
            log::warn!(
                "SLOW manage cycle: {:?} (init={:?} actions={:?} control={:?} bindings={:?} wm={:?} profile={:?} hook={:?} ipc={:?})",
                manage_elapsed, t_init, t_actions, t_control, t_bindings, t_wm, t_profile, t_hook, t_ipc
            );
        } else {
            log::debug!("manage cycle: {:?}", manage_elapsed);
        }

        proxy.manage_finish();
    }

    /// Enforce app bindings by moving bound windows into a visible bound frame
    /// whenever one exists.
    pub(crate) fn enforce_app_bindings(&mut self) {
        // Collect moves to avoid borrow issues: (window_id, src_frame_id, dst_ws_idx, dst_frame_id)
        let mut moves: Vec<(u64, crate::layout::FrameId, usize, crate::layout::FrameId)> =
            Vec::new();

        for (app_id, locations) in &self.app_bindings.bindings {
            // Find the first visible bound frame for this app.
            let target = locations.iter().find_map(|loc| {
                let ws = self
                    .workspaces
                    .workspaces
                    .iter()
                    .find(|w| w.name == loc.workspace)?;
                ws.active_output?;
                let frame_ids = ws.root.all_frame_ids();
                let fid = *frame_ids.get(loc.frame_index)?;
                Some((ws.id.0, fid))
            });

            let Some((dst_ws_idx, dst_fid)) = target else {
                continue;
            };

            // Find all non-floating windows with this app_id
            let window_ids: Vec<u64> = self
                .windows
                .iter()
                .filter(|w| w.app_id == *app_id && !w.floating)
                .map(|w| w.id)
                .collect();

            for &wid in &window_ids {
                // Find which workspace/frame this window is in
                let current = self.workspaces.workspaces.iter().find_map(|ws| {
                    ws.root.find_frame_with_window(wid).map(|fid| (ws.id.0, fid))
                });

                let (current_ws_idx, current_frame) = match current {
                    Some(c) => c,
                    None => continue,
                };

                if current_ws_idx != dst_ws_idx || current_frame != dst_fid {
                    moves.push((wid, current_frame, dst_ws_idx, dst_fid));
                }
            }
        }

        // Execute moves
        for (wid, src_fid, dst_ws_idx, dst_fid) in moves {
            // Get window ref
            let win_ref = self.workspaces.workspaces.iter().find_map(|ws| {
                ws.root
                    .find_frame(src_fid)
                    .and_then(|f| f.windows.iter().find(|w| w.window_id == wid).cloned())
            });

            if let Some(win_ref) = win_ref {
                // Remove from source
                for ws in &mut self.workspaces.workspaces {
                    if let Some(frame) = ws.root.find_frame_mut(src_fid) {
                        frame.remove_window(wid);
                    }
                }
                // Add to destination
                if let Some(frame) = self.workspaces.workspaces[dst_ws_idx]
                    .root
                    .find_frame_mut(dst_fid)
                {
                    frame.add_window(win_ref);
                }
                if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                    win.frame_id = Some(dst_fid);
                }
                log::info!("Auto-moved bound window {wid} to its visible bound frame");
            }
        }
    }

    fn handle_control_requests(&mut self) {
        let requests = self.control.take_pending();
        for req in requests {
            match req {
                crate::control::ControlRequest::FocusWindow(id) => {
                    self.focus_window_by_id(id);
                }
                crate::control::ControlRequest::FocusWindowByIdentifier(identifier) => {
                    self.focus_window_by_identifier(&identifier);
                }
                crate::control::ControlRequest::SwitchWorkspace(name) => {
                    self.switch_workspace_hiding_hidden_windows(&name);
                }
                crate::control::ControlRequest::Bind {
                    app_id,
                    workspace,
                    frame_index,
                    dimensions,
                } => {
                    use crate::app_bindings::BoundLocation;
                    let loc = BoundLocation {
                        workspace: workspace.clone(),
                        frame_index,
                        fixed_dimensions: dimensions,
                    };
                    self.app_bindings.bindings.insert(app_id.clone(), vec![loc]);
                    self.app_bindings.save();
                    log::info!(
                        "Bound '{}' to {} frame #{} dims={:?}",
                        app_id,
                        workspace,
                        frame_index,
                        dimensions
                    );
                }
                crate::control::ControlRequest::Unbind(app_id) => {
                    self.app_bindings.bindings.remove(&app_id);
                    self.app_bindings.save();
                    log::info!("Unbound '{}'", app_id);
                }
                crate::control::ControlRequest::SaveMonitors => {
                    // Snapshot live monitor state into the saved profile for
                    // the current monitor set. AppData handles the actual
                    // snapshot+save in `flush_save_monitors_request` because
                    // `Monitors` lives on AppData, not on WindowManager.
                    self.save_monitors_pending = true;
                }
                crate::control::ControlRequest::ForgetMonitors => {
                    self.forget_monitors_pending = true;
                }
                crate::control::ControlRequest::SetFixedDimensions(app_id, dims) => {
                    // Apply to all current bindings for this app
                    if let Some(locs) = self.app_bindings.bindings.get(&app_id) {
                        let locs: Vec<_> = locs
                            .iter()
                            .map(|l| (l.workspace.clone(), l.frame_index))
                            .collect();
                        for (ws, fi) in locs {
                            self.app_bindings
                                .set_fixed_dimensions(&app_id, &ws, fi, dims);
                        }
                    }
                    log::info!(
                        "Set fixed dimensions {:?} for all bindings of '{}'",
                        dims,
                        app_id
                    );
                }
            }
        }
    }

    fn focus_window_by_identifier(&mut self, identifier: &str) {
        let Some(id) = self
            .windows
            .iter()
            .find(|w| w.identifier.as_deref() == Some(identifier))
            .map(|w| w.id)
        else {
            log::warn!("focus-window-by-identifier: no window with identifier {identifier}");
            return;
        };
        self.focus_window_by_id(id);
    }

    fn focus_window_by_id(&mut self, id: u64) {
        for idx in 0..self.workspaces.workspaces.len() {
            let (ws_id, ws_name, frame_id, was_visible) = {
                let ws = &self.workspaces.workspaces[idx];
                let Some(frame_id) = ws.root.find_frame_with_window(id) else {
                    continue;
                };
                (ws.id, ws.name.clone(), frame_id, ws.active_output.is_some())
            };

            if !was_visible {
                self.switch_workspace_hiding_hidden_windows(&ws_name);
            }

            if let Some(ws) = self.workspaces.workspaces.get_mut(ws_id.0) {
                if let Some(frame) = ws.root.find_frame_mut(frame_id)
                    && let Some(tab_idx) = frame.windows.iter().position(|w| w.window_id == id)
                {
                    frame.set_active_tab(tab_idx);
                }
                ws.focused_frame = frame_id;
            }

            self.workspaces.focused_workspace = ws_id;
            return;
        }
    }

    pub fn handle_render_start(
        &mut self,
        proxy: &RiverWindowManagerV1,
        shm: Option<&WlShm>,
        compositor: Option<&WlCompositor>,
        viewporter: Option<&crate::protocol::wp_viewporter::WpViewporter>,
        qh: &QueueHandle<AppData>,
    ) {
        let render_t0 = std::time::Instant::now();
        self.handle_seat_ops();
        self.apply_layout_positions(proxy, shm, compositor, viewporter, qh);

        // Show/hide drag preview overlay
        if let (Some(shm), Some(compositor)) = (shm, compositor) {
            self.update_drag_preview(proxy, shm, compositor, qh);
            self.update_resize_highlight(proxy, shm, compositor, qh);
        }

        let render_elapsed = render_t0.elapsed();
        if render_elapsed.as_millis() > 100 {
            log::warn!("SLOW render cycle: {:?}", render_elapsed);
        } else {
            log::debug!("render cycle: {:?}", render_elapsed);
        }

        proxy.render_finish();
    }

    fn update_drag_preview(
        &mut self,
        wm_proxy: &RiverWindowManagerV1,
        shm: &WlShm,
        compositor: &WlCompositor,
        qh: &QueueHandle<AppData>,
    ) {
        // Check if there's an active move drag on a tiled window
        let drag_pos: Option<(i32, i32)> = self.seats.values().find_map(|s| {
            if s.op_release {
                return None;
            }
            match &s.op {
                SeatOp::Move { window_id, .. } => {
                    // Only show drop preview for tiled windows, not floating
                    let is_floating = self
                        .windows
                        .iter()
                        .find(|w| w.id == *window_id)
                        .is_some_and(|w| w.floating);
                    if is_floating {
                        None
                    } else {
                        Some((s.pointer_x, s.pointer_y))
                    }
                }
                _ => None,
            }
        });

        if let Some((px, py)) = drag_pos {
            let gap = self.config.general.gap as i32;
            let target = crate::pointer_ops::find_drop_target(&self.workspaces, px, py, gap);
            if let Some((_ws_id, _frame_id, rect, zone)) = target {
                let ratio = self.config.general.default_split_ratio;
                let area = zone.preview_rect(&rect, ratio, gap);
                self.drag_preview
                    .show(&area, &zone, compositor, wm_proxy, shm, qh);
                return;
            }
        }

        self.drag_preview.hide();
    }

    fn update_resize_highlight(
        &mut self,
        wm_proxy: &RiverWindowManagerV1,
        shm: &WlShm,
        compositor: &WlCompositor,
        qh: &QueueHandle<AppData>,
    ) {
        // Find active resize op's boundary paths (both axes)
        #[allow(clippy::type_complexity)]
        let active_paths: Option<(Option<Vec<bool>>, Option<Vec<bool>>)> =
            self.seats.values().find_map(|s| {
                if s.op_release {
                    return None;
                }
                match &s.op {
                    SeatOp::Resize {
                        h_boundary_path,
                        v_boundary_path,
                        ..
                    }
                    | SeatOp::ResizeEmpty {
                        h_boundary_path,
                        v_boundary_path,
                        ..
                    } => Some((h_boundary_path.clone(), v_boundary_path.clone())),
                    _ => None,
                }
            });

        let mut showed_h = false;
        let mut showed_v = false;

        if let Some((h_path, v_path)) = active_paths {
            let gap = self.config.general.gap as i32;
            let ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
            if let Some(area) = ws
                .active_output
                .and_then(|oid| self.workspaces.output(oid))
                .map(|o| o.usable_rect())
            {
                let color =
                    crate::config::hex_to_argb(&self.config.appearance.resize_highlight);

                // Show H boundary highlight (reads current boundary pos from the tree,
                // which reflects the already-adjusted ratio — no lag)
                if let Some(ref hp) = h_path
                    && let Some((pos, orient)) = ws.root.boundary_at_path(area, hp, gap)
                {
                    self.resize_highlight_h.show(
                        &orient, pos, &area, color, 4, compositor, wm_proxy, shm, qh,
                    );
                    showed_h = true;
                }

                // Show V boundary highlight
                if let Some(ref vp) = v_path
                    && let Some((pos, orient)) = ws.root.boundary_at_path(area, vp, gap)
                {
                    self.resize_highlight_v.show(
                        &orient, pos, &area, color, 4, compositor, wm_proxy, shm, qh,
                    );
                    showed_v = true;
                }
            }
        }

        if !showed_h {
            self.resize_highlight_h.hide();
        }
        if !showed_v {
            self.resize_highlight_v.hide();
        }
    }

    // ── Window lifecycle ─────────────────────────────────────────────────

    fn remove_closed_windows(&mut self) {
        let closed_ids: Vec<u64> = self
            .windows
            .iter()
            .filter(|w| w.closed)
            .map(|w| w.id)
            .collect();

        for id in &closed_ids {
            // Remove from frame
            for ws in &mut self.workspaces.workspaces {
                if let Some(frame) = ws
                    .root
                    .find_frame_with_window(*id)
                    .and_then(|fid| ws.root.find_frame_mut(fid))
                {
                    frame.remove_window(*id);
                }
            }

            // Cancel any seat ops referencing this window
            for seat in self.seats.values_mut() {
                match &seat.op {
                    SeatOp::Move { window_id, .. } | SeatOp::Resize { window_id, .. }
                        if *window_id == *id =>
                    {
                        seat.op = SeatOp::None;
                        seat.proxy.op_end();
                    }
                    _ => {}
                }
            }
        }

        self.windows.retain(|w| !w.closed);
    }

    fn remove_closed_outputs(&mut self) {
        let removed: Vec<OutputId> = self
            .workspaces
            .outputs
            .iter()
            .filter(|o| o.removed)
            .map(|o| o.id)
            .collect();
        if removed.is_empty() {
            return;
        }
        for id in &removed {
            self.workspaces.remove_output(*id);
        }
        // After a disconnect, the remaining outputs' assignments are still
        // valid — workspaces that lost their monitor are simply invisible.
        // Re-run the algorithm so any unplaced workspaces with matching
        // preferences can move into freed slots if appropriate.
        self.workspaces.maybe_reassign_outputs();
        // Make sure focus lands on something visible.
        let focused_visible = self
            .workspaces
            .workspaces
            .get(self.workspaces.focused_workspace.0)
            .is_some_and(|ws| ws.active_output.is_some());
        if !focused_visible
            && let Some(ws) = self
                .workspaces
                .workspaces
                .iter()
                .find(|ws| ws.active_output.is_some())
        {
            self.workspaces.focused_workspace = ws.id;
        }
    }

    fn sync_window_titles(&mut self) {
        for win in &self.windows {
            for ws in &mut self.workspaces.workspaces {
                if let Some(frame_id) = ws.root.find_frame_with_window(win.id)
                    && let Some(frame) = ws.root.find_frame_mut(frame_id)
                    && let Some(wref) = frame.windows.iter_mut().find(|w| w.window_id == win.id)
                    && (wref.title != win.title || wref.app_id != win.app_id)
                {
                    wref.title = win.title.clone();
                    wref.app_id = win.app_id.clone();
                }
            }
        }
    }

    fn remove_closed_seats(&mut self) {
        self.seats.retain(|_, seat| {
            if seat.removed {
                for entry in seat.xkb_bindings.values() {
                    entry.proxy.destroy();
                }
                for entry in seat.pointer_bindings.values() {
                    entry.proxy.destroy();
                }
                seat.proxy.destroy();
                false
            } else {
                true
            }
        });
    }

    fn init_new_windows(&mut self) {
        let existing_app_ids: Vec<String> = self.windows.iter()
            .filter(|w| !w.new)
            .map(|w| w.app_id.clone())
            .collect();

        for window in self.windows.iter_mut().filter(|w| w.new) {
            let title_preview: String = window.title.chars().take(40).collect();
            log::info!(
                "Placing window '{}' (id={}, identifier={:?}, title='{}')",
                window.app_id,
                window.id,
                window.identifier.as_deref().unwrap_or("none"),
                title_preview,
            );

            // Auto-float windows that look like popups/notifications:
            // - Already floating (from parent/dimensions_hint in dispatch)
            // - Window has no title but another window with same app_id exists
            //   (catches Thunderbird notifications, dialog popups, etc.)
            if !window.floating
                && !window.app_id.is_empty()
                && window.title.is_empty()
                && existing_app_ids.contains(&window.app_id)
            {
                window.floating = true;
                window.floating_kind = FloatingKind::Notification;
                log::info!("Auto-floating popup {} (untitled, app '{}' already open)", window.id, window.app_id);
            }

            // Note: when find_target returns AlreadyPlaced, the window is placed
            // as a tab in the bound frame (see below). We don't auto-float here
            // because we can't reliably distinguish dialogs from legitimate
            // second windows (e.g. Vivaldi's multiple browser windows).

            if window.floating {
                if window.frame_id.is_none() {
                    let focused_ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                    window.frame_id = Some(focused_ws.focused_frame);
                }
                // River requires propose_dimensions() for new windows to render.
                let (fw, fh) = if window.width > 0 && window.height > 0 {
                    (window.width, window.height)
                } else {
                    (0, 0)
                };
                window.proxy.propose_dimensions(fw, fh);

                // Position floating window on the focused output.
                // Notifications go to top-right, dialogs go to center.
                let focused_ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                if let Some(output) = focused_ws
                    .active_output
                    .and_then(|oid| self.workspaces.output(oid))
                {
                    let area = output.usable_rect();
                    window.position_floating(area);
                    // Mark as positioned only if we used real dimensions
                    window.float_positioned = fw > 0 && fh > 0;
                }

                window.new = false;
                continue;
            }

            // Try to restore window to its saved position
            let restored = self.saved_state.as_mut().and_then(|state| {
                crate::state::match_window_to_saved_frame(
                    &self.workspaces,
                    state,
                    &window.app_id,
                    &window.title,
                    window.identifier.as_deref(),
                )
            });

            let binding_target = self
                .app_bindings
                .find_target(&window.app_id, &self.workspaces);

            // Track whether placement came from an app binding (vs. restore or
            // default). Bound apps launching onto a hidden workspace should
            // auto-switch so the user actually sees them — otherwise a window
            // silently lands at 0x0 on an off-screen workspace and the only
            // way to find it is `notion-ctl focus-window-by-identifier`.
            // Restores are exempt (they preserve the user's last on-screen
            // view at startup); the default branch targets the focused
            // workspace which is already visible.
            let from_binding = restored.is_none()
                && matches!(
                    &binding_target,
                    crate::app_bindings::FindTargetResult::Target(_, _)
                        | crate::app_bindings::FindTargetResult::AlreadyPlaced(_, _)
                );

            let (target_ws_idx, frame_id) = if let Some((ws_id, fid)) = restored {
                log::info!(
                    "Restoring window '{}' to workspace '{}' frame {:?}",
                    window.app_id,
                    self.workspaces.workspaces[ws_id.0].name,
                    fid
                );
                (ws_id.0, fid)
            } else if let crate::app_bindings::FindTargetResult::Target(ws_id, fid)
            | crate::app_bindings::FindTargetResult::AlreadyPlaced(ws_id, fid) =
                binding_target
            {
                log::info!(
                    "Placing window '{}' in bound frame on workspace '{}'",
                    window.app_id,
                    self.workspaces.workspaces[ws_id.0].name,
                );
                (ws_id.0, fid)
            } else {
                // Default: place in focused frame of focused workspace
                let ws_idx = self.workspaces.focused_workspace.0;
                (ws_idx, self.workspaces.workspaces[ws_idx].focused_frame)
            };

            // Reveal the target workspace if the binding placed this window on
            // a hidden one. Without this, a bound app launched from the
            // keyboard/launcher vanishes onto an off-screen workspace — the
            // window gets 0x0 dimensions and rofi sees it but cannot surface
            // it (single-instance apps like KeePassXC won't even spawn a new
            // window to retarget). Switching here mirrors what
            // `focus-window-by-identifier` does manually.
            //
            // The decision is a pure predicate so the policy can be unit-tested
            // without spinning up Wayland proxies (see
            // [`should_reveal_bound_workspace`]).
            if should_reveal_bound_workspace(
                restored.is_some(),
                from_binding,
                self.workspaces.workspaces[target_ws_idx]
                    .active_output
                    .is_some(),
            ) {
                let ws_name = self.workspaces.workspaces[target_ws_idx].name.clone();
                log::info!(
                    "Bound target workspace '{}' is hidden; switching to reveal newly placed window",
                    ws_name
                );
                self.workspaces.switch_workspace(&ws_name);
            }

            if let Some(frame) = self.workspaces.workspaces[target_ws_idx]
                .root
                .find_frame_mut(frame_id)
            {
                let win_ref = WindowRef {
                    window_id: window.id,
                    app_id: window.app_id.clone(),
                    title: window.title.clone(),
                };
                // Use quiet add during restore to preserve saved active_tab
                if restored.is_some() {
                    frame.add_window_quiet(win_ref);
                } else {
                    frame.add_window(win_ref);
                }
                window.frame_id = Some(frame_id);
            }

            // Set initial properties
            window.proxy.use_ssd();
            window
                .proxy
                .set_tiled(Edges::Left | Edges::Right | Edges::Top | Edges::Bottom);
            window.new = false;
        }

        // Clear saved state once all saved slots have been consumed
        // Always apply saved active tabs (they were set during restore_layout)
        if !self.saved_active_tabs.is_empty() {
            for (frame_id, active_tab) in &self.saved_active_tabs {
                for ws in &mut self.workspaces.workspaces {
                    if let Some(frame) = ws.root.find_frame_mut(*frame_id)
                        && *active_tab < frame.windows.len()
                    {
                        frame.set_active_tab(*active_tab);
                    }
                }
            }
        }

        if let Some(ref state) = self.saved_state {
            // Clear saved state after 2 cycles with no new windows
            let had_new_windows = self.windows.iter().any(|w| w.new);
            if had_new_windows {
                self.restore_cycles_without_new = 0;
            } else {
                self.restore_cycles_without_new += 1;
            }
            if !crate::state::has_remaining_matches(state) || self.restore_cycles_without_new > 2 {
                log::info!("All saved windows restored, clearing saved state");
                self.saved_state = None;
                self.saved_active_tabs.clear();
            }
        }
    }

    fn init_new_seats(&mut self, river_xkb: &RiverXkbBindingsV1, qh: &QueueHandle<AppData>) {
        for seat in self.seats.values_mut() {
            if !seat.new {
                continue;
            }

            log::info!(
                "Initializing seat, registering {} normal + {} resize bindings",
                self.normal_bindings.len(),
                self.resize_bindings.len()
            );
            // Register normal mode bindings
            for binding in &self.normal_bindings {
                let mods = Modifiers::from_bits_truncate(binding.modifiers);
                let proxy = river_xkb.get_xkb_binding(
                    &seat.proxy,
                    binding.keysym,
                    mods,
                    qh,
                    seat.proxy.id(),
                );

                if let Some(layout) = binding.layout_override {
                    proxy.set_layout_override(layout);
                }
                proxy.enable();

                seat.xkb_bindings.insert(
                    proxy.id(),
                    XkbBindingEntry {
                        proxy,
                        action: binding.action.clone(),
                        mode: InputMode::Normal,
                    },
                );
            }

            // Register resize mode bindings (start disabled)
            for binding in &self.resize_bindings {
                let mods = Modifiers::from_bits_truncate(binding.modifiers);
                let proxy = river_xkb.get_xkb_binding(
                    &seat.proxy,
                    binding.keysym,
                    mods,
                    qh,
                    seat.proxy.id(),
                );

                if let Some(layout) = binding.layout_override {
                    proxy.set_layout_override(layout);
                }
                // Resize bindings start disabled

                seat.xkb_bindings.insert(
                    proxy.id(),
                    XkbBindingEntry {
                        proxy,
                        action: binding.action.clone(),
                        mode: InputMode::Resize,
                    },
                );
            }

            // Register pointer bindings (Mod+Left=move, Mod+Right=resize)
            {
                const BTN_LEFT: u32 = 0x110;
                const BTN_RIGHT: u32 = 0x111;

                // Derive pointer modifier from the first keybinding's modifier
                let pointer_mods = self
                    .normal_bindings
                    .first()
                    .map(|b| Modifiers::from_bits_truncate(b.modifiers))
                    .unwrap_or(Modifiers::Mod4);

                let move_proxy =
                    seat.proxy
                        .get_pointer_binding(BTN_LEFT, pointer_mods, qh, seat.proxy.id());
                move_proxy.enable();
                seat.pointer_bindings.insert(
                    move_proxy.id(),
                    PointerBindingEntry {
                        proxy: move_proxy,
                        action: Action::ToggleFloat, // marker: this is the move binding
                        is_move: true,
                    },
                );

                let resize_proxy =
                    seat.proxy
                        .get_pointer_binding(BTN_RIGHT, pointer_mods, qh, seat.proxy.id());
                resize_proxy.enable();
                seat.pointer_bindings.insert(
                    resize_proxy.id(),
                    PointerBindingEntry {
                        proxy: resize_proxy,
                        action: Action::None,
                        is_move: false,
                    },
                );
            }

            seat.new = false;
        }
    }

    // ── Action dispatch ──────────────────────────────────────────────────

    fn handle_pending_actions(
        &mut self,
        wm_proxy: &RiverWindowManagerV1,
        river_outputs: &std::collections::HashMap<
            u64,
            crate::protocol::river_output_v1::RiverOutputV1,
        >,
    ) {
        // Collect actions from all seats first — we need to know if there's
        // a keyboard action before applying focus-follows-mouse
        let actions: Vec<(Action, Option<u64>)> = self
            .seats
            .values_mut()
            .map(|seat| {
                let action = std::mem::replace(&mut seat.pending_action, Action::None);
                (
                    action,
                    seat.interacted.take().map(|w| w.id().protocol_id() as u64),
                )
            })
            .collect();

        let has_keyboard_action = actions.iter().any(|(a, _)| !matches!(a, Action::None));

        // Handle window interactions (click-to-focus, tab switching)
        // Skip if a tab click was just processed (would override the tab switch)
        let suppress = self.suppress_interaction;
        self.suppress_interaction = false;
        for (_, interacted_id) in &actions {
            if suppress {
                break;
            }
            if let Some(wid) = interacted_id {
                // Check if this is a floating window
                let is_floating = self
                    .windows
                    .iter()
                    .find(|w| w.id == *wid)
                    .is_some_and(|w| w.floating);

                if is_floating {
                    self.focused_floating = Some(*wid);
                    // Switch to the parent app's workspace if it's on a hidden workspace
                    if let Some(float_win) = self.windows.iter().find(|w| w.id == *wid) {
                        let app_id = float_win.app_id.clone();
                        // Find a tiled window with the same app_id
                        let parent_ws = self.windows.iter().find_map(|w| {
                            if w.id != *wid && w.app_id == app_id && !w.floating {
                                w.frame_id.and_then(|fid| {
                                    self.workspaces
                                        .workspaces
                                        .iter()
                                        .find(|ws| ws.root.find_frame(fid).is_some())
                                        .map(|ws| ws.id)
                                })
                            } else {
                                None
                            }
                        });
                        if let Some(ws_id) = parent_ws {
                            let ws = &self.workspaces.workspaces[ws_id.0];
                            if ws.active_output.is_none() {
                                // Hidden workspace — switch to it
                                let ws_name = ws.name.clone();
                                self.switch_workspace_hiding_hidden_windows(&ws_name);
                                log::info!(
                                    "Floating click: switched to workspace '{ws_name}' for parent app '{app_id}'",
                                );
                            }
                        }
                    }
                } else {
                    // Clicking a tiled window clears floating focus
                    self.focused_floating = None;
                    // Suppress tab switching during move/drop — the move handler
                    // will set focus after placing the window
                    let has_active_move = self.seats.values().any(|s| {
                        matches!(s.op, SeatOp::Move { .. }) || s.op_release
                    });
                    if !has_active_move {
                        // Find which frame this window is in and make it the active tab
                        for ws in &mut self.workspaces.workspaces {
                            if let Some(frame_id) = ws.root.find_frame_with_window(*wid) {
                                if let Some(frame) = ws.root.find_frame_mut(frame_id)
                                    && let Some(tab_idx) =
                                        frame.windows.iter().position(|w| w.window_id == *wid)
                                {
                                    frame.set_active_tab(tab_idx);
                                }
                                ws.focused_frame = frame_id;
                                self.workspaces.focused_workspace = ws.id;
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Focus-follows-mouse (including floating windows)
        if self.config.general.focus_follows_mouse && !has_keyboard_action {
            // Check if pointer is hovering a floating window
            let hovered_floating: Option<u64> = self.seats.values().find_map(|seat| {
                let wid = seat.hovered.as_ref().map(|w| w.id().protocol_id() as u64)?;
                let is_floating = self
                    .windows
                    .iter()
                    .find(|w| w.id == wid)
                    .is_some_and(|w| w.floating);
                is_floating.then_some(wid)
            });

            if let Some(fid) = hovered_floating {
                self.focused_floating = Some(fid);
            } else {
                // Pointer is over a tiled area — clear floating focus
                self.focused_floating = None;

                let inputs: Vec<crate::focus::FocusInput> = self
                    .seats
                    .values()
                    .map(|seat| crate::focus::FocusInput {
                        hovered_window_id: seat
                            .hovered
                            .as_ref()
                            .map(|w| w.id().protocol_id() as u64),
                        pointer_x: seat.pointer_x,
                        pointer_y: seat.pointer_y,
                    })
                    .collect();
                self.apply_focus_follows_mouse(&inputs);
            }
        }

        for (action, _) in actions {
            self.perform_action(action, wm_proxy, river_outputs);
        }

        // Handle seat op releases
        // First collect move-drop data before clearing ops (tiled windows only)
        let move_drops: Vec<(u64, i32, i32)> = self
            .seats
            .values()
            .filter(|s| s.op_release)
            .filter_map(|s| match &s.op {
                SeatOp::Move { window_id, .. } => {
                    // Only do frame-drop for tiled windows
                    let is_floating = self
                        .windows
                        .iter()
                        .find(|w| w.id == *window_id)
                        .is_some_and(|w| w.floating);
                    if is_floating {
                        None
                    } else {
                        Some((*window_id, s.pointer_x, s.pointer_y))
                    }
                }
                _ => None,
            })
            .collect();

        // Process move drops (tiled only)
        let gap = self.config.general.gap as i32;
        for (window_id, drop_x, drop_y) in move_drops {
            self.handle_move_drop(window_id, drop_x, drop_y, gap);
        }

        // Now clear the ops
        for seat in self.seats.values_mut() {
            if seat.op_release {
                if let SeatOp::Resize { window_id, .. } = &seat.op
                    && let Some(win) = self.windows.iter().find(|w| w.id == *window_id)
                {
                    win.proxy.inform_resize_end();
                }
                seat.proxy.op_end();
                seat.op = SeatOp::None;
                seat.op_release = false;
            }
        }
    }

    /// Apply focus-follows-mouse logic. Extracted for testability.
    pub fn apply_focus_follows_mouse(&mut self, inputs: &[crate::focus::FocusInput]) {
        let gap = self.config.general.gap as i32;
        let margin = 0; // no margin — focus changes at the exact frame boundary

        for input in inputs {
            if let Some(result) = crate::focus::compute_focus(input, &self.workspaces, gap, margin)
            {
                self.workspaces.workspaces[result.workspace.0].focused_frame = result.frame;
                self.workspaces.focused_workspace = result.workspace;
            }
        }
    }

    fn update_binding_modes(&self) {
        for seat in self.seats.values() {
            for entry in seat.xkb_bindings.values() {
                match (self.mode, entry.mode) {
                    (InputMode::Normal, InputMode::Normal) => entry.proxy.enable(),
                    (InputMode::Normal, InputMode::Resize) => entry.proxy.disable(),
                    (InputMode::Resize, InputMode::Resize) => entry.proxy.enable(),
                    (InputMode::Resize, InputMode::Normal) => entry.proxy.disable(),
                }
            }
        }
    }
}

// ── Type constructors ────────────────────────────────────────────────────

impl Seat {
    pub fn new(proxy: RiverSeatV1) -> Self {
        Self {
            proxy,
            new: true,
            removed: false,
            focused_window: None,
            hovered: None,
            interacted: None,
            xkb_bindings: HashMap::new(),
            pointer_bindings: HashMap::new(),
            pending_action: Action::None,
            op: SeatOp::None,
            op_dx: 0,
            op_dy: 0,
            op_prev_dx: 0,
            op_prev_dy: 0,
            op_release: false,
            op_start_pointer_x: 0,
            op_start_pointer_y: 0,
            pointer_x: 0,
            pointer_y: 0,
        }
    }
}

impl ManagedWindow {
    pub fn new(proxy: RiverWindowV1, qh: &QueueHandle<AppData>) -> Self {
        let id = proxy.id().protocol_id() as u64;
        let node = proxy.get_node(qh, ());
        Self {
            proxy,
            node,
            id,
            app_id: String::new(),
            title: String::new(),
            identifier: None,
            width: 0,
            height: 0,
            last_proposed_width: 0,
            last_proposed_height: 0,
            new: true,
            closed: false,
            frame_id: None,
            floating: false,
            fullscreen: false,
            capture_sessions: 0,
            prefers_ssd: false,
            floating_kind: FloatingKind::Dialog,
            float_x: 100,
            float_y: 100,
            float_positioned: false,
            pointer_move_requested: None,
            pointer_resize_requested: None,
            pointer_resize_requested_edges: Edges::None,
        }
    }

    fn floating_dimensions(&self) -> (i32, i32) {
        let win_w = if self.width > 0 { self.width } else { 640 };
        let win_h = if self.height > 0 { self.height } else { 480 };
        (win_w, win_h)
    }

    pub fn position_floating(&mut self, area: Rect) {
        let (win_w, win_h) = self.floating_dimensions();
        match self.floating_kind {
            FloatingKind::Notification => {
                self.float_x = area.x + area.width - win_w - 20;
                self.float_y = area.y + 20;
            }
            FloatingKind::Dialog => {
                self.float_x = area.x + (area.width - win_w) / 2;
                self.float_y = area.y + (area.height - win_h) / 2;
            }
        }
    }
}

/// Decide whether a newly placed bound window should trigger a workspace
/// switch to reveal its target frame.
///
/// Returns `true` only when *all* of:
/// - placement came from an app binding (`from_binding`), not a state
///   restore or the default focused-frame path,
/// - there is no saved-state restore taking precedence,
/// - the target workspace is currently hidden (no active output).
///
/// Exposing this as a pure predicate keeps the policy unit-testable without
/// standing up a live WindowManager + Wayland proxies.
pub(crate) fn should_reveal_bound_workspace(
    restored: bool,
    from_binding: bool,
    target_visible: bool,
) -> bool {
    from_binding && !restored && !target_visible
}

#[cfg(test)]
mod tests {
    use super::should_reveal_bound_workspace;

    #[test]
    fn reveal_bound_only_when_hidden_and_not_restored() {
        // Bound app landing on a hidden workspace -> reveal it.
        assert!(should_reveal_bound_workspace(false, true, false));
        // Already visible: nothing to do.
        assert!(!should_reveal_bound_workspace(false, true, true));
        // Restore path: never reveal, even if hidden (preserves startup view).
        assert!(!should_reveal_bound_workspace(true, true, false));
        // Default placement (not from a binding): never reveal.
        assert!(!should_reveal_bound_workspace(false, false, false));
        assert!(!should_reveal_bound_workspace(false, false, true));
    }
}
