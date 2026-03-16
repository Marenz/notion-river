use std::collections::HashMap;

use wayland_backend::client::ObjectId;
use wayland_client::{Proxy, QueueHandle};

use wayland_client::protocol::{wl_compositor::WlCompositor, wl_shm::WlShm};

use crate::actions::Action;
use crate::bindings::{get_profile_bindings, parse_all_bindings, Binding};
use crate::config::Config;
use crate::decorations::{DecorationManager, EmptyFrameManager, TAB_BAR_HEIGHT};
use crate::layout::{FrameId, Orientation, WindowRef};
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
    pub wl_compositor: Option<WlCompositor>,
    pub wl_shm: Option<WlShm>,
    pub wm: WindowManager,
}

impl Default for AppData {
    fn default() -> Self {
        Self {
            river_wm: None,
            river_xkb: None,
            wl_compositor: None,
            wl_shm: None,
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
    pub width: i32,
    pub height: i32,
    pub new: bool,
    pub closed: bool,
    /// Which frame this window is placed in.
    pub frame_id: Option<FrameId>,
    /// Whether this window is floating.
    pub floating: bool,
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
    pub focused_window: Option<RiverWindowV1>,
    pub hovered: Option<RiverWindowV1>,
    pub interacted: Option<RiverWindowV1>,
    pub xkb_bindings: HashMap<ObjectId, XkbBindingEntry>,
    pub pointer_bindings: HashMap<ObjectId, PointerBindingEntry>,
    pub pending_action: Action,
    pub op: SeatOp,
    pub op_dx: i32,
    pub op_dy: i32,
    pub op_release: bool,
}

#[derive(Debug)]
pub struct XkbBindingEntry {
    pub proxy: RiverXkbBindingV1,
    pub action: Action,
    pub mode: InputMode,
}

#[derive(Debug)]
pub struct PointerBindingEntry {
    pub proxy: RiverPointerBindingV1,
    pub action: Action,
}

#[derive(Debug, Clone)]
pub enum SeatOp {
    None,
    Move {
        window_id: u64,
        start_x: i32,
        start_y: i32,
    },
    Resize {
        window_id: u64,
        start_x: i32,
        start_y: i32,
        start_width: i32,
        start_height: i32,
        edges: Edges,
    },
}

impl WindowManager {
    pub fn new(config: Config) -> Self {
        let (normal_cfgs, resize_cfgs) = get_profile_bindings(&config);
        let physical = config.general.physical_keys;
        let layout_idx = config.general.physical_layout_index;

        let normal_bindings = parse_all_bindings(&normal_cfgs, physical, layout_idx);
        let resize_bindings = parse_all_bindings(&resize_cfgs, physical, layout_idx);

        let workspaces =
            WorkspaceManager::new(&config.workspaces, config.general.default_split_ratio);

        Self {
            config,
            workspaces,
            windows: Vec::new(),
            seats: HashMap::new(),
            mode: InputMode::Normal,
            normal_bindings,
            resize_bindings,
            decorations: DecorationManager::new(),
            empty_frames: EmptyFrameManager::new(),
        }
    }

    // ── Manage/Render cycle ──────────────────────────────────────────────

    pub fn handle_manage_start(
        &mut self,
        proxy: &RiverWindowManagerV1,
        river_xkb: &RiverXkbBindingsV1,
        qh: &QueueHandle<AppData>,
    ) {
        let prev_focused_frame = self.workspaces.focused_workspace().focused_frame;

        self.remove_closed_outputs();
        self.remove_closed_windows();
        self.remove_closed_seats();
        self.init_new_windows();
        self.init_new_seats(river_xkb, qh);
        self.handle_pending_actions(proxy);
        self.apply_window_management(proxy);
        self.update_binding_modes();

        // Cursor follows focus: warp pointer when focus changed via keyboard
        let new_focused_frame = self.workspaces.focused_workspace().focused_frame;
        if self.config.general.cursor_follows_focus && new_focused_frame != prev_focused_frame {
            self.warp_cursor_to_frame(new_focused_frame);
        }

        proxy.manage_finish();
    }

    pub fn handle_render_start(
        &mut self,
        proxy: &RiverWindowManagerV1,
        shm: Option<&WlShm>,
        compositor: Option<&WlCompositor>,
        qh: &QueueHandle<AppData>,
    ) {
        self.apply_layout_positions(proxy, shm, compositor, qh);
        self.handle_seat_ops();
        proxy.render_finish();
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
        for id in removed {
            self.workspaces.remove_output(id);
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
        for window in self.windows.iter_mut().filter(|w| w.new) {
            // Place window into the focused frame of the focused workspace
            let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
            let frame_id = ws.focused_frame;

            if let Some(frame) = ws.root.find_frame_mut(frame_id) {
                frame.add_window(WindowRef {
                    window_id: window.id,
                    app_id: window.app_id.clone(),
                    title: window.title.clone(),
                });
                window.frame_id = Some(frame_id);
            }

            // Set initial properties
            window.proxy.use_ssd();
            window
                .proxy
                .set_tiled(Edges::Left | Edges::Right | Edges::Top | Edges::Bottom);
            window.new = false;
        }
    }

    fn init_new_seats(&mut self, river_xkb: &RiverXkbBindingsV1, qh: &QueueHandle<AppData>) {
        for seat in self.seats.values_mut() {
            if !seat.new {
                continue;
            }

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
                // proxy.enable();

                seat.xkb_bindings.insert(
                    proxy.id(),
                    XkbBindingEntry {
                        proxy,
                        action: binding.action.clone(),
                        mode: InputMode::Resize,
                    },
                );
            }

            // Register pointer bindings (Super+Left=move, Super+Right=resize)
            {
                const BTN_LEFT: u32 = 0x110;
                const BTN_RIGHT: u32 = 0x111;
                let mods = Modifiers::Mod4;

                let move_proxy =
                    seat.proxy
                        .get_pointer_binding(BTN_LEFT, mods, qh, seat.proxy.id());
                move_proxy.enable();
                seat.pointer_bindings.insert(
                    move_proxy.id(),
                    PointerBindingEntry {
                        proxy: move_proxy,
                        action: Action::None, // handled via SeatOp
                    },
                );

                let resize_proxy =
                    seat.proxy
                        .get_pointer_binding(BTN_RIGHT, mods, qh, seat.proxy.id());
                resize_proxy.enable();
                seat.pointer_bindings.insert(
                    resize_proxy.id(),
                    PointerBindingEntry {
                        proxy: resize_proxy,
                        action: Action::None,
                    },
                );
            }

            seat.new = false;
        }
    }

    // ── Action execution ─────────────────────────────────────────────────

    fn handle_pending_actions(&mut self, wm_proxy: &RiverWindowManagerV1) {
        // Focus-follows-mouse: when pointer enters a window, focus its frame
        if self.config.general.focus_follows_mouse {
            let hovered_ids: Vec<u64> = self
                .seats
                .values()
                .filter_map(|seat| seat.hovered.as_ref().map(|w| w.id().protocol_id() as u64))
                .collect();

            for hovered_id in hovered_ids {
                // Find which frame this window is in
                for ws in &self.workspaces.workspaces {
                    if let Some(frame_id) = ws.root.find_frame_with_window(hovered_id) {
                        if ws.focused_frame != frame_id {
                            let ws_id = ws.id;
                            let ws_mut = &mut self.workspaces.workspaces[ws_id.0];
                            ws_mut.focused_frame = frame_id;
                            self.workspaces.focused_workspace = ws_id;
                        }
                        break;
                    }
                }
            }
        }

        // Collect actions from all seats
        let actions: Vec<Action> = self
            .seats
            .values_mut()
            .map(|seat| {
                let action = std::mem::replace(&mut seat.pending_action, Action::None);
                // Handle interactions: bring interacted window to top
                if let Some(window_proxy) = seat.interacted.take() {
                    let _ = window_proxy;
                }
                action
            })
            .collect();

        for action in actions {
            self.perform_action(action, wm_proxy);
        }

        // Handle seat op releases
        for seat in self.seats.values_mut() {
            if seat.op_release {
                if let SeatOp::Resize { window_id, .. } = &seat.op {
                    if let Some(win) = self.windows.iter().find(|w| w.id == *window_id) {
                        win.proxy.inform_resize_end();
                    }
                }
                seat.proxy.op_end();
                seat.op = SeatOp::None;
                seat.op_release = false;
            }
        }
    }

    fn perform_action(&mut self, action: Action, wm_proxy: &RiverWindowManagerV1) {
        if !matches!(action, Action::None) {
            log::info!("Action: {action:?}");
        }
        match action {
            Action::None => {}

            Action::Close => {
                // Close window if frame has one; if frame is empty, unsplit it
                let ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame(frame_id) {
                    if let Some(win_ref) = frame.active_window() {
                        let window_id = win_ref.window_id;
                        if let Some(win) = self.windows.iter().find(|w| w.id == window_id) {
                            win.proxy.close();
                        }
                    } else {
                        // Frame is empty — remove it (unsplit)
                        self.perform_unsplit();
                    }
                }
            }

            Action::ToggleFullscreen => {
                let ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame(frame_id) {
                    if let Some(win_ref) = frame.active_window() {
                        if let Some(win) = self.windows.iter().find(|w| w.id == win_ref.window_id) {
                            // TODO: track fullscreen state to toggle properly
                            // For now, need to get the output proxy to fullscreen on
                            // win.proxy.fullscreen(&output_proxy);
                            // For toggle, we'd need to track state. Skip for now.
                            log::info!("Fullscreen toggle not yet implemented");
                        }
                    }
                }
            }

            Action::ToggleFloat => {
                let ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame(frame_id) {
                    if let Some(win_ref) = frame.active_window() {
                        let wid = win_ref.window_id;
                        if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                            win.floating = !win.floating;
                        }
                    }
                }
            }

            Action::FocusDirection(dir) => {
                let ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                let gap = self.config.general.gap as i32;

                if let Some(output_id) = ws.active_output {
                    if let Some(output) = self.workspaces.output(output_id) {
                        let area = output.usable_rect();
                        if let Some(neighbor) = ws.root.find_neighbor(frame_id, dir, area, gap) {
                            let ws_mut = &mut self.workspaces.workspaces
                                [self.workspaces.focused_workspace.0];
                            ws_mut.focused_frame = neighbor;
                        }
                    }
                }
            }

            Action::FocusNextTab => {
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame_mut(frame_id) {
                    frame.next_tab();
                }
            }

            Action::FocusPrevTab => {
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(frame) = ws.root.find_frame_mut(frame_id) {
                    frame.prev_tab();
                }
            }

            Action::FocusParent => {
                // TODO: implement focus parent for nested container navigation
            }

            Action::MoveDirection(dir) => {
                let ws_idx = self.workspaces.focused_workspace.0;
                let frame_id = self.workspaces.workspaces[ws_idx].focused_frame;
                let gap = self.config.general.gap as i32;

                // Compute neighbor without holding a mutable borrow
                let neighbor = {
                    let ws = &self.workspaces.workspaces[ws_idx];
                    ws.active_output.and_then(|oid| {
                        self.workspaces.output(oid).and_then(|output| {
                            let area = output.usable_rect();
                            ws.root.find_neighbor(frame_id, dir, area, gap)
                        })
                    })
                };

                if let Some(target_frame_id) = neighbor {
                    // Move the active window from current frame to target frame
                    let ws = &mut self.workspaces.workspaces[ws_idx];
                    if let Some(frame) = ws.root.find_frame(frame_id) {
                        if let Some(win_ref) = frame.active_window().cloned() {
                            let wid = win_ref.window_id;
                            // Remove from source frame
                            if let Some(src) = ws.root.find_frame_mut(frame_id) {
                                src.remove_window(wid);
                            }
                            // Add to target frame
                            if let Some(dst) = ws.root.find_frame_mut(target_frame_id) {
                                dst.add_window(win_ref);
                            }
                            // Update window's frame_id
                            if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                                win.frame_id = Some(target_frame_id);
                            }
                            // Focus follows the window
                            self.workspaces.workspaces[ws_idx].focused_frame = target_frame_id;
                        }
                    }
                }
            }

            Action::MoveToWorkspace(name) => {
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;

                if let Some(frame) = ws.root.find_frame(frame_id) {
                    if let Some(win_ref) = frame.active_window().cloned() {
                        let wid = win_ref.window_id;
                        // Remove from current frame
                        if let Some(src) = ws.root.find_frame_mut(frame_id) {
                            src.remove_window(wid);
                        }
                        // Find target workspace and add to its focused frame
                        if let Some(target_ws) = self.workspaces.workspace_by_name_mut(&name) {
                            let target_frame = target_ws.focused_frame;
                            if let Some(dst) = target_ws.root.find_frame_mut(target_frame) {
                                dst.add_window(win_ref);
                            }
                            if let Some(win) = self.windows.iter_mut().find(|w| w.id == wid) {
                                win.frame_id = Some(target_frame);
                            }
                        }
                    }
                }
            }

            Action::SplitHorizontal => {
                let ratio = self.config.general.default_split_ratio;
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(new_id) = ws
                    .root
                    .split_frame(frame_id, Orientation::Horizontal, ratio)
                {
                    log::info!("Split frame {frame_id:?} horizontally, new frame {new_id:?}");
                }
            }

            Action::SplitVertical => {
                let ratio = self.config.general.default_split_ratio;
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if let Some(new_id) = ws.root.split_frame(frame_id, Orientation::Vertical, ratio) {
                    log::info!("Split frame {frame_id:?} vertically, new frame {new_id:?}");
                }
            }

            Action::Unsplit => {
                self.perform_unsplit();
            }

            Action::ToggleSplit => {
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                if ws.root.toggle_orientation(frame_id) {
                    log::info!("Toggled split orientation for frame {frame_id:?}");
                }
            }

            Action::SwitchWorkspace(name) => {
                self.workspaces.switch_workspace(&name);
            }

            Action::EnterResizeMode => {
                self.mode = InputMode::Resize;
                log::info!("Entering resize mode");
            }

            Action::ExitResizeMode => {
                self.mode = InputMode::Normal;
                log::info!("Exiting resize mode");
            }

            Action::Resize(dir) => {
                let delta = 0.05; // 5% per resize step
                let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
                let frame_id = ws.focused_frame;
                ws.root.resize_frame(frame_id, dir, delta);
            }

            Action::SpawnTerminal => {
                let cmd = self.config.commands.terminal.clone();
                spawn_command(&[&cmd]);
            }

            Action::SpawnLauncher => {
                let args: Vec<String> = self.config.commands.launcher.clone();
                let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                spawn_command(&refs);
            }

            Action::Spawn(args) => {
                let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                spawn_command(&refs);
            }

            Action::Exit => {
                wm_proxy.exit_session();
            }

            Action::ReloadConfig => {
                self.config = Config::load();
                log::info!("Configuration reloaded");
                // TODO: re-parse bindings and re-register with seats
            }
        }
    }

    fn warp_cursor_to_frame(&self, frame_id: FrameId) {
        let gap = self.config.general.gap as i32;
        let ws = self.workspaces.focused_workspace();
        let output = ws.active_output.and_then(|oid| self.workspaces.output(oid));
        if let Some(output) = output {
            let area = output.usable_rect();
            let layouts = ws.root.calculate_layout(area, gap);
            if let Some((_, rect)) = layouts.iter().find(|(id, _)| *id == frame_id) {
                let cx = rect.x + rect.width / 2;
                let cy = rect.y + rect.height / 2;
                for seat in self.seats.values() {
                    seat.proxy.pointer_warp(cx, cy);
                }
            }
        }
    }

    fn perform_unsplit(&mut self) {
        let ws = &mut self.workspaces.workspaces[self.workspaces.focused_workspace.0];
        let frame_id = ws.focused_frame;

        // Only unsplit if frame is empty
        if let Some(frame) = ws.root.find_frame(frame_id) {
            if !frame.is_empty() {
                log::info!("Cannot unsplit non-empty frame");
                return;
            }
        }

        // Get all frame IDs before removal to find a new focus target
        let all_ids = ws.root.all_frame_ids();
        if all_ids.len() <= 1 {
            log::info!("Cannot unsplit the last frame");
            return;
        }

        if ws.root.remove_frame(frame_id) {
            // Focus the first remaining frame
            ws.focused_frame = ws.root.first_frame_id();
            log::info!("Removed frame {frame_id:?}");
        }
    }

    // ── Layout application ───────────────────────────────────────────────

    fn apply_window_management(&mut self, _wm_proxy: &RiverWindowManagerV1) {
        let gap = self.config.general.gap as i32;
        let border = self.config.general.border_width;

        // For each visible workspace, calculate layout and apply dimensions
        for ws in &self.workspaces.workspaces {
            let output = match ws.active_output.and_then(|oid| self.workspaces.output(oid)) {
                Some(o) => o,
                None => continue, // workspace not visible
            };

            let area = output.usable_rect();
            let frame_layouts = ws.root.calculate_layout(area, gap);

            for (frame_id, rect) in &frame_layouts {
                if let Some(frame) = ws.root.find_frame(*frame_id) {
                    if let Some(active_win) = frame.active_window() {
                        let wid = active_win.window_id;
                        if let Some(win) = self.windows.iter().find(|w| w.id == wid) {
                            // Propose dimensions (minus border and tab bar)
                            let bw = border as i32 * 2;
                            let tab_h = TAB_BAR_HEIGHT;
                            win.proxy
                                .propose_dimensions(rect.width - bw, rect.height - bw - tab_h);
                        }
                    }
                    // Hide non-active tabs
                    for (i, win_ref) in frame.windows.iter().enumerate() {
                        if let Some(win) = self.windows.iter().find(|w| w.id == win_ref.window_id) {
                            if i == frame.active_tab {
                                win.proxy.show();
                            } else {
                                win.proxy.hide();
                            }
                        }
                    }
                }
            }

            // Hide all windows in non-visible workspaces
            // (handled by the fact that we only process visible workspaces above)
        }

        // Hide windows on non-visible workspaces
        for win in &self.windows {
            if win.floating {
                win.proxy.show();
                continue;
            }
            if let Some(frame_id) = win.frame_id {
                let in_visible_ws = self
                    .workspaces
                    .visible_workspaces()
                    .iter()
                    .any(|ws| ws.root.find_frame(frame_id).is_some());
                if !in_visible_ws {
                    win.proxy.hide();
                }
            }
        }

        // Focus the active window in the focused frame
        let ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
        let frame_id = ws.focused_frame;
        if let Some(frame) = ws.root.find_frame(frame_id) {
            if let Some(active_win) = frame.active_window() {
                let wid = active_win.window_id;
                if let Some(win) = self.windows.iter().find(|w| w.id == wid) {
                    for seat in self.seats.values() {
                        seat.proxy.focus_window(&win.proxy);
                    }
                }
            } else {
                // Empty frame — clear focus
                for seat in self.seats.values() {
                    seat.proxy.clear_focus();
                }
            }
        }

        // Set borders (done in render phase, not manage phase)
        // Border colors are stored for use in apply_layout_positions.
    }

    fn apply_layout_positions(
        &mut self,
        wm_proxy: &RiverWindowManagerV1,
        shm: Option<&WlShm>,
        compositor: Option<&WlCompositor>,
        qh: &QueueHandle<AppData>,
    ) {
        let gap = self.config.general.gap as i32;
        let border = self.config.general.border_width as i32;
        let tab_bar_h = TAB_BAR_HEIGHT;
        let active_color = parse_hex_color(&self.config.appearance.active_border);
        let inactive_color = parse_hex_color(&self.config.appearance.inactive_border);

        let focused_ws_id = self.workspaces.focused_workspace;
        let focused_frame_id = self.workspaces.workspaces[focused_ws_id.0].focused_frame;

        // Collect draw commands to avoid borrow conflicts
        struct DrawCmd {
            window_id: u64,
            win_idx: usize,
            frame_id: FrameId,
            rect_x: i32,
            rect_y: i32,
            rect_width: i32,
            rect_height: i32,
            is_focused: bool,
            border_color: (u32, u32, u32, u32),
        }
        let mut draw_cmds: Vec<DrawCmd> = Vec::new();

        for ws in &self.workspaces.workspaces {
            let output = match ws.active_output.and_then(|oid| self.workspaces.output(oid)) {
                Some(o) => o,
                None => continue,
            };

            let area = output.usable_rect();
            let frame_layouts = ws.root.calculate_layout(area, gap);

            for (frame_id, rect) in &frame_layouts {
                if let Some(frame) = ws.root.find_frame(*frame_id) {
                    if let Some(active_win) = frame.active_window() {
                        let is_focused = *frame_id == focused_frame_id;
                        let color = if is_focused {
                            active_color
                        } else {
                            inactive_color
                        };

                        if let Some((idx, _)) = self
                            .windows
                            .iter()
                            .enumerate()
                            .find(|(_, w)| w.id == active_win.window_id)
                        {
                            draw_cmds.push(DrawCmd {
                                window_id: active_win.window_id,
                                win_idx: idx,
                                frame_id: *frame_id,
                                rect_x: rect.x,
                                rect_y: rect.y,
                                rect_width: rect.width,
                                rect_height: rect.height,
                                is_focused,
                                border_color: color,
                            });
                        }
                    }
                }
            }
        }

        // Execute draw commands
        for cmd in &draw_cmds {
            let win = &self.windows[cmd.win_idx];
            // Position window below the tab bar
            win.node
                .set_position(cmd.rect_x + border, cmd.rect_y + border + tab_bar_h);
            win.node.place_top();

            // Borders
            let all_edges = Edges::Left | Edges::Right | Edges::Top | Edges::Bottom;
            win.proxy.set_borders(
                all_edges,
                border,
                cmd.border_color.0,
                cmd.border_color.1,
                cmd.border_color.2,
                cmd.border_color.3,
            );
        }

        // Collect empty frames
        struct EmptyCmd {
            frame_id: FrameId,
            rect: crate::layout::Rect,
            is_focused: bool,
        }
        let mut empty_cmds: Vec<EmptyCmd> = Vec::new();

        for ws in &self.workspaces.workspaces {
            let output = match ws.active_output.and_then(|oid| self.workspaces.output(oid)) {
                Some(o) => o,
                None => continue,
            };
            let area = output.usable_rect();
            let frame_layouts = ws.root.calculate_layout(area, gap);
            for (frame_id, rect) in &frame_layouts {
                if let Some(frame) = ws.root.find_frame(*frame_id) {
                    if frame.is_empty() {
                        empty_cmds.push(EmptyCmd {
                            frame_id: *frame_id,
                            rect: *rect,
                            is_focused: *frame_id == focused_frame_id,
                        });
                    }
                }
            }
        }

        // Draw tab bars and empty frame indicators
        if let (Some(shm), Some(compositor)) = (shm, compositor) {
            let wm_proxy_ref: Option<&RiverWindowManagerV1> = None; // we need it for shell surfaces

            for cmd in &draw_cmds {
                let frame = self
                    .workspaces
                    .workspaces
                    .iter()
                    .find_map(|ws| ws.root.find_frame(cmd.frame_id));

                if let Some(frame) = frame {
                    let win = &self.windows[cmd.win_idx];
                    self.decorations.draw_tab_bar(
                        cmd.window_id,
                        &win.proxy,
                        frame,
                        cmd.rect_width,
                        cmd.is_focused,
                        shm,
                        compositor,
                        qh,
                    );
                }
            }

            // Draw empty frame indicators
            let empty_ids: Vec<FrameId> = empty_cmds.iter().map(|c| c.frame_id).collect();
            for cmd in &empty_cmds {
                self.empty_frames.draw_empty_frame(
                    cmd.frame_id,
                    cmd.rect,
                    cmd.is_focused,
                    shm,
                    compositor,
                    wm_proxy,
                    qh,
                );
            }
            self.empty_frames.cleanup(&empty_ids);
        }

        // Position floating windows
        for win in &self.windows {
            if win.floating {
                win.node.set_position(win.float_x, win.float_y);
                win.node.place_top();
            }
        }
    }

    fn handle_seat_ops(&self) {
        for seat in self.seats.values() {
            match &seat.op {
                SeatOp::None => {}
                SeatOp::Move {
                    window_id,
                    start_x,
                    start_y,
                } => {
                    if let Some(win) = self.windows.iter().find(|w| w.id == *window_id) {
                        win.node
                            .set_position(start_x + seat.op_dx, start_y + seat.op_dy);
                    }
                }
                SeatOp::Resize {
                    window_id,
                    start_x,
                    start_y,
                    start_width,
                    start_height,
                    edges,
                } => {
                    if let Some(win) = self.windows.iter().find(|w| w.id == *window_id) {
                        let (mut x, mut y) = (*start_x, *start_y);
                        if edges.contains(Edges::Left) {
                            x += start_width - win.width;
                        }
                        if edges.contains(Edges::Top) {
                            y += start_height - win.height;
                        }
                        win.node.set_position(x, y);
                    }
                }
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
            op_release: false,
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
            width: 0,
            height: 0,
            new: true,
            closed: false,
            frame_id: None,
            floating: false,
            float_x: 100,
            float_y: 100,
            pointer_move_requested: None,
            pointer_resize_requested: None,
            pointer_resize_requested_edges: Edges::None,
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn spawn_command(args: &[&str]) {
    if args.is_empty() {
        return;
    }
    match std::process::Command::new(args[0]).args(&args[1..]).spawn() {
        Ok(_) => log::info!("Spawned: {}", args.join(" ")),
        Err(e) => log::error!("Failed to spawn {}: {e}", args[0]),
    }
}

/// Parse "#RRGGBB" or "#RRGGBBAA" to (r, g, b, a) as 32-bit RGBA values.
/// River expects pre-multiplied alpha, 32-bit per channel (scaled from 8-bit).
fn parse_hex_color(hex: &str) -> (u32, u32, u32, u32) {
    let hex = hex.trim_start_matches('#');
    let (r, g, b, a) = match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
            (r, g, b, 255u8)
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
            let a = u8::from_str_radix(&hex[6..8], 16).unwrap_or(255);
            (r, g, b, a)
        }
        _ => (80, 80, 80, 255),
    };
    // Scale 8-bit to 32-bit range and pre-multiply alpha
    let a32 = (a as u32) * 0x01010101;
    let scale = a as u32;
    let r32 = (r as u32 * scale / 255) * 0x01010101;
    let g32 = (g as u32 * scale / 255) * 0x01010101;
    let b32 = (b as u32 * scale / 255) * 0x01010101;
    (r32, g32, b32, a32)
}
