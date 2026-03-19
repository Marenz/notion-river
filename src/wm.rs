use std::collections::HashMap;

use wayland_backend::client::ObjectId;
use wayland_client::{Proxy, QueueHandle};

use wayland_client::protocol::{wl_compositor::WlCompositor, wl_shm::WlShm};

use crate::actions::Action;
use crate::bindings::{get_profile_bindings, parse_all_bindings, Binding};
use crate::config::Config;
use crate::decorations::{DecorationManager, EmptyFrameManager};
use crate::layout::{FrameId, WindowRef};
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
    /// wl_seat global name (for binding wl_pointer).
    pub wl_seat_name: Option<u32>,
    /// Pending tab click: (workspace_index, frame_id, tab_index) from decoration click
    pub pending_tab_click: Option<(usize, FrameId, usize)>,
    /// Current wl_pointer surface (protocol id) and surface-local x
    pub wl_pointer_surface: Option<u32>,
    pub wl_pointer_surface_x: f64,
    pub wm: WindowManager,
}

impl Default for AppData {
    fn default() -> Self {
        Self {
            river_wm: None,
            river_xkb: None,
            river_layer_shell: None,
            wl_compositor: None,
            wl_shm: None,
            wp_viewporter: None,
            wl_output_map: std::collections::HashMap::new(),
            river_outputs: std::collections::HashMap::new(),
            wl_output_names: std::collections::HashMap::new(),
            wl_seat_name: None,
            pending_tab_click: None,
            wl_pointer_surface: None,
            wl_pointer_surface_x: 0.0,
            wm: WindowManager::new(Config::load()),
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
    /// Per-output-config workspace assignment memory.
    pub output_profiles: crate::output_profiles::OutputProfiles,
    /// Control socket state for window/workspace switching.
    pub control: crate::control::ControlState,
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
    pub new: bool,
    pub closed: bool,
    /// Which frame this window is placed in.
    pub frame_id: Option<FrameId>,
    /// Whether this window is floating.
    pub floating: bool,
    pub fullscreen: bool,
    /// Floating position.
    pub float_x: i32,
    pub float_y: i32,
    pub pointer_move_requested: Option<RiverSeatV1>,
    pub pointer_resize_requested: Option<RiverSeatV1>,
    pub pointer_resize_requested_edges: Edges,
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
    },
    /// Resize split boundary from an empty frame area.
    #[allow(dead_code)]
    ResizeEmpty {
        frame_id: FrameId,
        resize_h: bool,
        resize_v: bool,
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
            let tabs = crate::state::restore_layout(&mut workspaces, state);
            // Store visible workspace preferences for later (after output names arrive)
            workspaces.saved_visible = state.visible_workspaces.to_vec();
            tabs
        } else {
            std::collections::HashMap::new()
        };

        let colors = config.appearance.colors();
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
            ipc: crate::ipc::IpcState::new(),
            app_bindings: crate::app_bindings::AppBindings::load(),
            drag_preview: crate::decorations::DragPreview::default(),
            output_profiles: crate::output_profiles::OutputProfiles::load(),
            control: crate::control::ControlState::new(),
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
        let prev_focused_frame = self.workspaces.focused_workspace().focused_frame;

        self.remove_closed_outputs();
        self.remove_closed_windows();
        self.remove_closed_seats();
        self.sync_window_titles();
        self.init_new_windows();
        self.init_new_seats(river_xkb, qh);

        // Check if any keyboard action is pending (before handle_pending_actions consumes it)
        let has_keyboard_action = self
            .seats
            .values()
            .any(|s| !matches!(s.pending_action, Action::None));

        self.handle_pending_actions(proxy, river_outputs);
        self.handle_control_requests();
        self.enforce_app_bindings();
        self.apply_window_management(proxy);
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

        // Save output profile when workspace state changes
        self.output_profiles.save_current(&self.workspaces);

        // Update waybar workspace display via FIFO
        self.ipc.update(&self.workspaces, &self.config.appearance);
        self.control
            .update_snapshot(crate::control::build_snapshot(self));

        proxy.manage_finish();
    }

    /// Enforce app bindings: if a bound window is on a hidden workspace but
    /// has a binding on a visible workspace, move it there.
    fn enforce_app_bindings(&mut self) {
        // Collect moves to avoid borrow issues: (window_id, src_frame_id, dst_ws_idx, dst_frame_id)
        let mut moves: Vec<(u64, crate::layout::FrameId, usize, crate::layout::FrameId)> =
            Vec::new();

        for (app_id, locations) in &self.app_bindings.bindings {
            // Find all windows with this app_id
            let window_ids: Vec<u64> = self
                .windows
                .iter()
                .filter(|w| w.app_id == *app_id)
                .map(|w| w.id)
                .collect();

            for &wid in &window_ids {
                // Find which workspace/frame this window is in
                let current = self.workspaces.workspaces.iter().find_map(|ws| {
                    ws.root
                        .find_frame_with_window(wid)
                        .map(|fid| (ws.id, fid, ws.active_output.is_some()))
                });

                let (_current_ws, current_frame, currently_visible) = match current {
                    Some(c) => c,
                    None => continue,
                };

                // If already visible, no action needed
                if currently_visible {
                    continue;
                }

                // Find a visible bound frame for this app
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

                if let Some((dst_ws_idx, dst_fid)) = target
                    && dst_fid != current_frame
                {
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
                log::info!("Auto-moved bound window {wid} to visible workspace");
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
                crate::control::ControlRequest::SwitchWorkspace(name) => {
                    self.workspaces.switch_workspace(&name);
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
                self.workspaces.switch_workspace(&ws_name);
            }

            if let Some(ws) = self.workspaces.workspaces.get_mut(ws_id.0) {
                if let Some(frame) = ws.root.find_frame_mut(frame_id)
                    && let Some(tab_idx) = frame.windows.iter().position(|w| w.window_id == id)
                {
                    frame.active_tab = tab_idx;
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
        self.apply_layout_positions(proxy, shm, compositor, viewporter, qh);
        self.handle_seat_ops();

        // Show/hide drag preview overlay
        if let (Some(shm), Some(compositor)) = (shm, compositor) {
            self.update_drag_preview(proxy, shm, compositor, qh);
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
        // Check if there's an active move drag — use pointer position for accuracy
        let drag_pos: Option<(i32, i32)> = self.seats.values().find_map(|s| {
            if s.op_release {
                return None;
            }
            match &s.op {
                SeatOp::Move { .. } => Some((s.pointer_x, s.pointer_y)),
                _ => None,
            }
        });

        if let Some((px, py)) = drag_pos {
            let gap = self.config.general.gap as i32;
            let target = crate::pointer_ops::find_drop_target(&self.workspaces, px, py, gap);
            if let Some((_ws_id, _frame_id, rect, zone)) = target {
                self.drag_preview.show(&rect, &zone, compositor, wm_proxy, shm, qh);
                return;
            }
        }

        self.drag_preview.hide();
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
        // Don't migrate windows — workspaces keep their layout.
        // User can switch to them with Super+N, or they'll be
        // restored to the monitor when it reconnects.
        // Just make sure we're focused on a visible workspace.
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
            log::info!(
                "Placing window '{}' (id={}, identifier={:?}, title='{}')",
                window.app_id,
                window.id,
                window.identifier.as_deref().unwrap_or("none"),
                &window.title[..window.title.len().min(40)],
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
                log::info!("Auto-floating popup {} (untitled, app '{}' already open)", window.id, window.app_id);
            }

            if window.floating {
                window.proxy.use_csd();
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

            let (target_ws_idx, frame_id) = if let Some((ws_id, fid)) = restored {
                log::info!(
                    "Restoring window '{}' to workspace '{}' frame {:?}",
                    window.app_id,
                    self.workspaces.workspaces[ws_id.0].name,
                    fid
                );
                (ws_id.0, fid)
            } else if let Some((ws_id, fid)) = self
                .app_bindings
                .find_target(&window.app_id, &self.workspaces)
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
                        frame.active_tab = *active_tab;
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
                // Find which frame this window is in and make it the active tab
                for ws in &mut self.workspaces.workspaces {
                    if let Some(frame_id) = ws.root.find_frame_with_window(*wid) {
                        if let Some(frame) = ws.root.find_frame_mut(frame_id)
                            && let Some(tab_idx) =
                                frame.windows.iter().position(|w| w.window_id == *wid)
                        {
                            frame.active_tab = tab_idx;
                        }
                        ws.focused_frame = frame_id;
                        self.workspaces.focused_workspace = ws.id;
                        break;
                    }
                }
            }
        }

        // Focus-follows-mouse
        if self.config.general.focus_follows_mouse && !has_keyboard_action {
            let inputs: Vec<crate::focus::FocusInput> = self
                .seats
                .values()
                .map(|seat| crate::focus::FocusInput {
                    hovered_window_id: seat.hovered.as_ref().map(|w| w.id().protocol_id() as u64),
                    pointer_x: seat.pointer_x,
                    pointer_y: seat.pointer_y,
                })
                .collect();
            self.apply_focus_follows_mouse(&inputs);
        }

        for (action, _) in actions {
            self.perform_action(action, wm_proxy, river_outputs);
        }

        // Handle seat op releases
        // First collect move-drop data before clearing ops
        let move_drops: Vec<(u64, i32, i32)> = self
            .seats
            .values()
            .filter(|s| s.op_release)
            .filter_map(|s| match &s.op {
                SeatOp::Move { window_id, .. } => Some((*window_id, s.pointer_x, s.pointer_y)),
                _ => None,
            })
            .collect();

        // Process move drops
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
            new: true,
            closed: false,
            frame_id: None,
            floating: false,
            fullscreen: false,
            float_x: 100,
            float_y: 100,
            pointer_move_requested: None,
            pointer_resize_requested: None,
            pointer_resize_requested_edges: Edges::None,
        }
    }
}
