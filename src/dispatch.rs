//! Wayland protocol dispatch implementations.
//!
//! Each River protocol interface needs a `Dispatch` impl that handles
//! events from the compositor. This follows the same pattern as tinyrwm.

use wayland_backend::client::ObjectId;
use wayland_client::{
    protocol::{
        wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_output::WlOutput, wl_registry,
        wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle,
};

use crate::protocol::{
    river_decoration_v1::RiverDecorationV1, river_node_v1::RiverNodeV1,
    river_output_v1::RiverOutputV1, river_pointer_binding_v1::RiverPointerBindingV1,
    river_seat_v1::RiverSeatV1, river_shell_surface_v1::RiverShellSurfaceV1,
    river_window_manager_v1::RiverWindowManagerV1, river_window_v1::RiverWindowV1,
    river_xkb_binding_v1::RiverXkbBindingV1, river_xkb_bindings_v1::RiverXkbBindingsV1,
    zwlr_output_configuration_v1::ZwlrOutputConfigurationV1,
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_head_v1::ZwlrOutputHeadV1, zwlr_output_manager_v1::ZwlrOutputManagerV1,
    zwlr_output_mode_v1::ZwlrOutputModeV1,
};

use crate::wm::{AppData, ManagedWindow, Seat, SeatOp};
use crate::workspace::{Output, OutputId};

impl AppData {
    /// Snapshot live monitor state and persist it as the saved profile for
    /// the current monitor set. Triggered by `notion-ctl save-monitors`.
    pub fn flush_save_monitors_request(&mut self) {
        let Some((set_key, snap)) =
            crate::monitors::snapshot(&self.monitors.heads, &self.monitors.modes)
        else {
            log::warn!("save-monitors: live monitor state is not yet stable; ignored");
            return;
        };
        if self.monitors.profiles.insert(set_key.clone(), snap) {
            self.monitors.profiles.save();
            log::info!("save-monitors: saved current live state for set '{set_key}'");
        } else {
            log::info!(
                "save-monitors: current live state already matches saved for set '{set_key}'"
            );
        }
        // Whatever we just saved is, by definition, what the user wants;
        // clear any failure state so future divergence will trigger reapply
        // again.
        self.monitors.failed_sets.remove(&set_key);
    }

    /// Forget the saved profile for the current monitor set. After this the
    /// next Done event with no profile will follow the "stay out of the way"
    /// path: do not apply, do not save. The user can then configure the
    /// layout via wdisplays and run `notion-ctl save-monitors`.
    pub fn flush_forget_monitors_request(&mut self) {
        let Some(set_key) = self.monitors.last_set_key.clone() else {
            log::warn!("forget-monitors: no current monitor set known yet; ignored");
            return;
        };
        if self.monitors.profiles.map.remove(&set_key).is_some() {
            self.monitors.profiles.save();
            self.monitors.failed_sets.remove(&set_key);
            log::info!("forget-monitors: removed saved profile for set '{set_key}'");
        } else {
            log::info!("forget-monitors: no saved profile for set '{set_key}'");
        }
    }

    /// Apply a saved profile (`edid -> SavedHead`) to the live heads via
    /// wlr-output-management.
    fn apply_monitor_profile(
        &mut self,
        target: &std::collections::HashMap<String, crate::monitors::SavedHead>,
        qh: &QueueHandle<Self>,
    ) -> Result<(), &'static str> {
        let manager = self.monitors.manager.as_ref().ok_or("no output manager")?;
        let serial = self.monitors.serial.ok_or("no output serial")?;
        if self.monitors.apply_in_flight {
            return Err("output config already in flight");
        }
        self.monitors.apply_in_flight = true;
        let config = manager.create_configuration(serial, qh, ());

        for (id, head_live) in &self.monitors.heads {
            let Some(head_proxy) = self.output_head_proxies.get(id) else {
                continue;
            };
            let key = crate::monitors::monitor_key(
                head_live.name.as_deref(),
                head_live.description.as_deref(),
            );
            let target_head = key.as_deref().and_then(|k| target.get(k));

            let Some(target_head) = target_head else {
                config.disable_head(head_proxy);
                continue;
            };
            if !target_head.enabled {
                config.disable_head(head_proxy);
                continue;
            }

            let head_config = config.enable_head(head_proxy, qh, ());

            // Pick the wl_output mode for this head: prefer exact match
            // (w+h+refresh, refresh from the live head's current mode), then
            // fall back to any mode with matching w+h.
            let target_refresh = head_live
                .current_mode_id
                .and_then(|m| self.monitors.modes.get(&m))
                .map(|m| m.refresh_mhz)
                .unwrap_or(0);
            let mode_proxy = head_live
                .mode_ids
                .iter()
                .find_map(|mid| {
                    let mode = self.monitors.modes.get(mid)?;
                    if mode.w == target_head.mode_w
                        && mode.h == target_head.mode_h
                        && target_refresh > 0
                        && mode.refresh_mhz == target_refresh
                    {
                        self.output_mode_proxies.get(mid)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    head_live.mode_ids.iter().find_map(|mid| {
                        let mode = self.monitors.modes.get(mid)?;
                        if mode.w == target_head.mode_w && mode.h == target_head.mode_h {
                            self.output_mode_proxies.get(mid)
                        } else {
                            None
                        }
                    })
                });
            if let Some(mp) = mode_proxy {
                head_config.set_mode(mp);
            }
            head_config.set_position(target_head.position_x, target_head.position_y);
            head_config.set_scale(target_head.scale.max(0.1));
            if let Some(transform) = output_transform(target_head.transform) {
                head_config.set_transform(transform);
            }
        }

        config.apply();
        Ok(())
    }
}

fn output_transform(value: i32) -> Option<wayland_client::protocol::wl_output::Transform> {
    use wayland_client::protocol::wl_output::Transform;
    match value {
        0 => Some(Transform::Normal),
        1 => Some(Transform::_90),
        2 => Some(Transform::_180),
        3 => Some(Transform::_270),
        4 => Some(Transform::Flipped),
        5 => Some(Transform::Flipped90),
        6 => Some(Transform::Flipped180),
        7 => Some(Transform::Flipped270),
        _ => None,
    }
}

// ── Registry ─────────────────────────────────────────────────────────────

impl Dispatch<wl_registry::WlRegistry, ()> for AppData {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            const WM_VERSION: u32 = 4;
            const XKB_VERSION: u32 = 2;
            match interface.as_str() {
                "river_window_manager_v1" => {
                    if version < WM_VERSION {
                        log::error!("river_window_manager_v1 v{version}, need >= v{WM_VERSION}");
                        std::process::exit(1);
                    }
                    let wm = registry.bind::<RiverWindowManagerV1, _, _>(name, WM_VERSION, qh, ());
                    state.river_wm = Some(wm);
                }
                "river_xkb_bindings_v1" => {
                    if version < XKB_VERSION {
                        log::error!("river_xkb_bindings_v1 v{version}, need >= v{XKB_VERSION}");
                        std::process::exit(1);
                    }
                    let xkb = registry.bind::<RiverXkbBindingsV1, _, _>(name, XKB_VERSION, qh, ());
                    state.river_xkb = Some(xkb);
                }
                "river_layer_shell_v1" => {
                    use crate::protocol::river_layer_shell_v1::RiverLayerShellV1;
                    let ls = registry.bind::<RiverLayerShellV1, _, _>(name, version.min(1), qh, ());
                    log::info!("Bound river_layer_shell_v1");
                    state.river_layer_shell = Some(ls);
                }
                "wl_output" => {
                    let _output = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, name);
                }
                "wl_seat" => {
                    use wayland_client::protocol::wl_seat::WlSeat;
                    let seat = registry.bind::<WlSeat, _, _>(name, version.min(8), qh, ());
                    // Get a wl_pointer to receive pointer events on shell surfaces
                    let _pointer = seat.get_pointer(qh, ());
                }
                "wl_compositor" => {
                    let comp = registry.bind::<WlCompositor, _, _>(name, version.min(6), qh, ());
                    state.wl_compositor = Some(comp);
                }
                "wl_shm" => {
                    let shm = registry.bind::<WlShm, _, _>(name, version.min(1), qh, ());
                    state.wl_shm = Some(shm);
                }
                "wp_viewporter" => {
                    use crate::protocol::wp_viewporter::WpViewporter;
                    let vp = registry.bind::<WpViewporter, _, _>(name, version.min(1), qh, ());
                    log::info!("Bound wp_viewporter");
                    state.wp_viewporter = Some(vp);
                }
                "zwlr_output_manager_v1" => {
                    // Vendored protocol XML is v1; binding at a higher version
                    // would decode unknown v2+ events as malformed messages.
                    let om = registry.bind::<ZwlrOutputManagerV1, _, _>(name, 1, qh, ());
                    log::info!("Bound zwlr_output_manager_v1 (v1)");
                    state.monitors.manager = Some(om);
                }
                _ => {}
            }
        }
    }
}

// ── Window Manager ───────────────────────────────────────────────────────

impl Dispatch<RiverWindowManagerV1, ()> for AppData {
    fn event(
        state: &mut Self,
        proxy: &RiverWindowManagerV1,
        event: <RiverWindowManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::river_window_manager_v1::Event;
        match event {
            Event::Unavailable => {
                log::error!("Another WM is already running");
                std::process::exit(1);
            }
            Event::Finished => {
                log::info!("Compositor finished, exiting");
                std::process::exit(0);
            }
            Event::ManageStart => {
                // Process pending tab click before manage
                if let Some((ws_idx, frame_id, tab_index)) = state.pending_tab_click.take() {
                    if let Some(ws) = state.wm.workspaces.workspaces.get_mut(ws_idx) {
                        if let Some(frame) = ws.root.find_frame_mut(frame_id)
                            && tab_index < frame.windows.len()
                        {
                            log::info!("Tab click: frame {:?} tab {}", frame_id, tab_index);
                            frame.set_active_tab(tab_index);
                        }
                        ws.focused_frame = frame_id;
                        state.wm.workspaces.focused_workspace = ws.id;
                    }
                    // Suppress WindowInteraction for this manage cycle
                    // so it doesn't override the tab switch
                    state.wm.suppress_interaction = true;
                }

                let river_xkb = state
                    .river_xkb
                    .as_ref()
                    .expect("river_xkb_bindings_v1 missing");
                state
                    .wm
                    .handle_manage_start(proxy, river_xkb, &state.river_outputs, qh);
                if state.wm.save_monitors_pending {
                    state.wm.save_monitors_pending = false;
                    state.flush_save_monitors_request();
                }
                if state.wm.forget_monitors_pending {
                    state.wm.forget_monitors_pending = false;
                    state.flush_forget_monitors_request();
                }
            }
            Event::RenderStart => {
                state.wm.handle_render_start(
                    proxy,
                    state.wl_shm.as_ref(),
                    state.wl_compositor.as_ref(),
                    state.wp_viewporter.as_ref(),
                    qh,
                );
            }
            Event::SessionLocked => {
                log::info!("Session locked");
            }
            Event::SessionUnlocked => {
                log::info!("Session unlocked");
            }
            Event::Window { id } => {
                let window = ManagedWindow::new(id, qh);
                log::info!("New window: id={}", window.id);
                state.wm.windows.push(window);
            }
            Event::Output { id } => {
                let oid = OutputId(id.id().protocol_id() as u64);
                log::info!("New output: {oid:?}");
                state.river_outputs.insert(oid.0, id.clone());
                let output = Output::new(oid);
                state.wm.workspaces.add_output(output);
                // Register layer-shell output for exclusive zone tracking
                if let Some(ref ls) = state.river_layer_shell {
                    let _ls_output = ls.get_output(&id, qh, oid.0);
                    log::info!("Registered layer-shell output for {oid:?}");
                }
            }
            Event::Seat { id } => {
                log::info!("New seat: {:?}", id.id());
                // Register layer-shell seat for focus events
                if let Some(ref ls) = state.river_layer_shell {
                    let _ls_seat = ls.get_seat(&id, qh, ());
                    log::info!("Registered layer-shell seat");
                }
                state.wm.seats.insert(id.id(), Seat::new(id));
            }
        }
    }

    wayland_client::event_created_child!(AppData, RiverWindowManagerV1, [
        crate::protocol::river_window_manager_v1::EVT_WINDOW_OPCODE => (RiverWindowV1, ()),
        crate::protocol::river_window_manager_v1::EVT_OUTPUT_OPCODE => (RiverOutputV1, ()),
        crate::protocol::river_window_manager_v1::EVT_SEAT_OPCODE => (RiverSeatV1, ())
    ]);
}

// ── Window ───────────────────────────────────────────────────────────────

impl Dispatch<RiverWindowV1, ()> for AppData {
    fn event(
        state: &mut Self,
        proxy: &RiverWindowV1,
        event: <RiverWindowV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::river_window_v1::Event;
        let window = match state.wm.windows.iter_mut().find(|w| w.proxy == *proxy) {
            Some(w) => w,
            None => return,
        };
        match event {
            Event::Closed => {
                log::info!("Window closed: id={}", window.id);
                window.closed = true;
            }
            Event::Dimensions { width, height } => {
                window.width = width;
                window.height = height;
            }
            Event::DimensionsHint {
                min_width,
                min_height,
                max_width,
                max_height,
            } => {
                // Auto-float small fixed-size windows (popups, notifications)
                let is_fixed = max_width > 0
                    && max_height > 0
                    && max_width == min_width
                    && max_height == min_height;
                let is_small =
                    max_width > 0 && max_width < 600 && max_height > 0 && max_height < 400;
                if (is_fixed || is_small)
                    && !window.floating
                {
                    window.floating = true;
                    window.floating_kind = crate::wm::FloatingKind::Dialog;
                    // If already placed in a frame, remove it (late DimensionsHint)
                    if let Some(frame_id) = window.frame_id {
                        for ws in &mut state.wm.workspaces.workspaces {
                            if let Some(frame) = ws.root.find_frame_mut(frame_id) {
                                frame.remove_window(window.id);
                            }
                        }
                        window.frame_id = None;
                    }
                    log::info!(
                        "Auto-floating window {} ({}x{}-{}x{})",
                        window.id,
                        min_width,
                        min_height,
                        max_width,
                        max_height
                    );
                }
            }
            Event::AppId { app_id } => {
                if let Some(ref id) = app_id {
                    log::info!("Window {} app_id: {id}", window.id);
                }
                window.app_id = app_id.unwrap_or_default();
            }
            Event::Title { title } => {
                window.title = title.unwrap_or_default();
            }
            Event::Parent { parent } => {
                if parent.is_some() && !window.floating {
                    // Child windows (dialogs, popups) should float
                    window.floating = true;
                    window.floating_kind = crate::wm::FloatingKind::Dialog;
                    // If already placed in a frame, remove it (late Parent event)
                    if let Some(frame_id) = window.frame_id {
                        for ws in &mut state.wm.workspaces.workspaces {
                            if let Some(frame) = ws.root.find_frame_mut(frame_id) {
                                frame.remove_window(window.id);
                            }
                        }
                        window.frame_id = None;
                    }
                    log::info!("Window {} has parent, setting floating", window.id);
                }
            }
            Event::DecorationHint { hint } => {
                log::info!("Window {} decoration_hint: {:?}", window.id, hint);
                if let wayland_client::WEnum::Value(h) = hint {
                    window.prefers_ssd = matches!(
                        h,
                        crate::protocol::river_window_v1::DecorationHint::PrefersSsd
                    );
                }
            }
            Event::PointerMoveRequested { seat } => {
                window.pointer_move_requested = Some(seat);
            }
            Event::PointerResizeRequested { seat, edges } => {
                window.pointer_resize_requested = Some(seat);
                window.pointer_resize_requested_edges = edges
                    .into_result()
                    .unwrap_or(crate::protocol::river_window_v1::Edges::None);
            }
            Event::ShowWindowMenuRequested { .. } => {}
            Event::MaximizeRequested => {}
            Event::UnmaximizeRequested => {}
            Event::FullscreenRequested { .. } => {}
            Event::ExitFullscreenRequested => {}
            Event::MinimizeRequested => {}
            Event::UnreliablePid { .. } => {}
            Event::PresentationHint { .. } => {}
            Event::Identifier { identifier } => {
                log::info!("Window {} identifier: {identifier}", window.id);
                window.identifier = Some(identifier);
            }
        }
    }
}

// ── Output ───────────────────────────────────────────────────────────────

impl Dispatch<RiverOutputV1, ()> for AppData {
    fn event(
        state: &mut Self,
        proxy: &RiverOutputV1,
        event: <RiverOutputV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::river_output_v1::Event;
        let oid = OutputId(proxy.id().protocol_id() as u64);
        match event {
            Event::Removed => {
                log::info!("Output removed: {oid:?}");
                if let Some(output) = state.wm.workspaces.output_mut(oid) {
                    output.removed = true;
                }
            }
            Event::WlOutput { name: global_name } => {
                log::info!("Output {oid:?} wl_output global name: {global_name}");
                state.wl_output_map.insert(global_name, oid);
                // Apply any buffered wl_output data that arrived before this mapping
                if let Some(connector_name) = state.wl_output_names.get(&global_name).cloned() {
                    log::info!("Output {oid:?} applying stored connector name: {connector_name}");
                    if let Some(output) = state.wm.workspaces.output_mut(oid) {
                        output.name = Some(connector_name);
                    }
                }
                if let Some((pw, ph)) = state.wl_output_modes.remove(&global_name) {
                    log::info!("Output {oid:?} applying buffered mode: {pw}x{ph}");
                    if let Some(output) = state.wm.workspaces.output_mut(oid) {
                        output.physical_width = pw;
                        output.physical_height = ph;
                    }
                }
                if let Some(scale) = state.wl_output_scales.remove(&global_name) {
                    log::info!("Output {oid:?} applying buffered scale: {scale}");
                    if let Some(output) = state.wm.workspaces.output_mut(oid) {
                        output.scale = scale;
                    }
                }
                if let Some(transform) = state.wl_output_transforms.remove(&global_name) {
                    log::info!("Output {oid:?} applying buffered transform: {transform}");
                    if let Some(output) = state.wm.workspaces.output_mut(oid) {
                        output.transform = transform;
                    }
                }
                if let Some(desc) = state.wl_output_descriptions.get(&global_name).cloned() {
                    log::info!("Output {oid:?} applying buffered description: {desc}");
                    if let Some(output) = state.wm.workspaces.output_mut(oid) {
                        output.description = Some(desc);
                    }
                }
                state.wm.workspaces.maybe_reassign_outputs();
            }
            Event::Position { x, y } => {
                if let Some(output) = state.wm.workspaces.output_mut(oid)
                    && (output.x != x || output.y != y) {
                        output.x = x;
                        output.y = y;
                        state.wm.workspaces.outputs_changed = true;
                    }
            }
            Event::Dimensions { width, height } => {
                log::info!("Output {oid:?} dimensions: {width}x{height}");
                if let Some(output) = state.wm.workspaces.output_mut(oid)
                    && (output.width != width || output.height != height) {
                        output.width = width;
                        output.height = height;
                        state.wm.workspaces.outputs_changed = true;
                    }
                // Dimensions can be the last piece of metadata to arrive; if so,
                // this is when reassignment should fire. The maybe_ guard makes
                // it cheap when the connected set hasn't really changed.
                state.wm.workspaces.maybe_reassign_outputs();
            }
        }
    }
}

// ── Seat ─────────────────────────────────────────────────────────────────

impl Dispatch<RiverSeatV1, ()> for AppData {
    fn event(
        state: &mut Self,
        proxy: &RiverSeatV1,
        event: <RiverSeatV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::river_seat_v1::Event;
        let seat = match state.wm.seats.get_mut(&proxy.id()) {
            Some(s) => s,
            None => return,
        };
        match event {
            Event::Removed => seat.removed = true,
            Event::WlSeat { name } => {
                log::info!("Seat wl_seat global name: {name}");
                state.wl_seat_name = Some(name);
            }
            Event::PointerEnter { window } => {
                log::debug!("PointerEnter window {:?}", window.id());
                seat.hovered = Some(window);
            }
            Event::PointerLeave => {
                log::debug!("PointerLeave");
                seat.hovered = None;
            }
            Event::WindowInteraction { window } => seat.interacted = Some(window),
            Event::ShellSurfaceInteraction { .. } => {}
            Event::OpDelta { dx, dy } => {
                seat.op_dx = dx;
                seat.op_dy = dy;
            }
            Event::OpRelease => seat.op_release = true,
            Event::PointerPosition { x, y } => {
                log::debug!("PointerPosition ({x}, {y})");
                seat.pointer_x = x;
                seat.pointer_y = y;
            }
        }
    }
}

// ── XKB Bindings ─────────────────────────────────────────────────────────

impl Dispatch<RiverXkbBindingV1, ObjectId> for AppData {
    fn event(
        state: &mut Self,
        proxy: &RiverXkbBindingV1,
        event: <RiverXkbBindingV1 as Proxy>::Event,
        data: &ObjectId,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::river_xkb_binding_v1::Event;
        let seat = match state.wm.seats.get_mut(data) {
            Some(s) => s,
            None => return,
        };
        let entry = match seat.xkb_bindings.get(&proxy.id()) {
            Some(e) => e,
            None => return,
        };
        match event {
            Event::Pressed => {
                seat.pending_action = entry.action.clone();
            }
            Event::Released => {}
            Event::StopRepeat => {}
        }
    }
}

// ── Pointer Bindings ─────────────────────────────────────────────────────

impl Dispatch<RiverPointerBindingV1, ObjectId> for AppData {
    fn event(
        state: &mut Self,
        proxy: &RiverPointerBindingV1,
        event: <RiverPointerBindingV1 as Proxy>::Event,
        data: &ObjectId,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::river_pointer_binding_v1::Event;

        // Extract what we need from the seat without holding a mutable borrow
        let (is_move, hovered_id, ptr_x, ptr_y) = {
            let seat = match state.wm.seats.get(data) {
                Some(s) => s,
                None => return,
            };
            let binding = match seat.pointer_bindings.get(&proxy.id()) {
                Some(b) => b,
                None => return,
            };
            (
                binding.is_move,
                seat.hovered.as_ref().map(|h| h.id().protocol_id() as u64),
                seat.pointer_x,
                seat.pointer_y,
            )
        };

        match event {
            Event::Pressed => {
                // Check if the click is on a tab bar — if so, determine which
                // tab's window to operate on instead of the hovered window.
                let tab_window_id = state.wl_pointer_surface.and_then(|surface_id| {
                    let &decoration_win_id =
                        state.wm.decorations.surface_to_window.get(&surface_id)?;
                    let surface_x = state.wl_pointer_surface_x;
                    // Find the frame this decoration belongs to
                    let (frame, frame_width) =
                        state.wm.workspaces.workspaces.iter().find_map(|ws| {
                            let fid = ws.root.find_frame_with_window(decoration_win_id)?;
                            let frame = ws.root.find_frame(fid)?;
                            let gap = state.wm.config.general.gap as i32;
                            let output = ws
                                .active_output
                                .and_then(|oid| state.wm.workspaces.output(oid))?;
                            let area = output.usable_rect();
                            let layouts = ws.root.calculate_layout(area, gap);
                            let (_, rect) = layouts.iter().find(|(id, _)| *id == fid)?;
                            Some((frame, rect.width))
                        })?;
                    if frame.windows.len() <= 1 {
                        return None; // single tab, use normal hovered window
                    }
                    let scale = state
                        .wm
                        .workspaces
                        .outputs
                        .first()
                        .map(|o| o.fractional_scale())
                        .unwrap_or(1.0)
                        .max(1.0);
                    // Match the fallback used in rendering (decorations.rs):
                    // use 2.0 when scale is unknown to avoid tab index mismatch
                    let scale = if scale > 1.0 { scale } else { 2.0 };
                    let tab_width = (frame_width as f64 * scale) / frame.windows.len() as f64;
                    let tab_idx = (surface_x / tab_width) as usize;
                    let tab_idx = tab_idx.min(frame.windows.len() - 1);
                    Some(frame.windows[tab_idx].window_id)
                });

                // For move operations, always use hovered_id (the active/visible
                // window). Tab-specific resolution only matters for resize where
                // you might want to resize a specific non-active tab's split.
                // For moves, the user wants to drag the window they can see.
                let effective_id = if is_move {
                    hovered_id
                } else {
                    tab_window_id.or(hovered_id)
                };
                let hovered_win =
                    effective_id.and_then(|hid| state.wm.windows.iter().find(|w| w.id == hid));

                // Compute the op to start (all immutable borrows)
                let new_op = if let Some(win) = hovered_win {
                    let gap = state.wm.config.general.gap as i32;
                    let border = state.wm.config.general.border_width as i32;
                    let (sx, sy) = if win.floating {
                        (win.float_x, win.float_y)
                    } else {
                        let pos = state.wm.workspaces.workspaces.iter().find_map(|ws| {
                            let output = ws
                                .active_output
                                .and_then(|oid| state.wm.workspaces.output(oid))?;
                            let area = output.usable_rect();
                            let layouts = ws.root.calculate_layout(area, gap);
                            let fid = ws.root.find_frame_with_window(win.id)?;
                            layouts
                                .into_iter()
                                .find(|(id, _)| *id == fid)
                                .map(|(_, r)| {
                                    (
                                        r.x + border,
                                        r.y + border + crate::decorations::TAB_BAR_HEIGHT,
                                    )
                                })
                        });
                        pos.unwrap_or((win.float_x, win.float_y))
                    };

                    if is_move {
                        log::info!("Pointer move start on window {} at ({},{})", win.id, sx, sy);
                        if win.floating {
                            state.wm.focused_floating = Some(win.id);
                        }
                        Some(SeatOp::Move {
                            window_id: win.id,
                            start_x: sx,
                            start_y: sy,
                        })
                    } else if !win.floating {
                        // Resize only works on tiled windows (split boundary adjustment)
                        let frame_id = state
                            .wm
                            .workspaces
                            .workspaces
                            .iter()
                            .find_map(|ws| ws.root.find_frame_with_window(win.id));
                        let (rh, rv) = frame_id
                            .map(|fid| state.wm.detect_resize_axes(fid, ptr_x, ptr_y))
                            .unwrap_or((true, true));
                        // Find the specific split boundaries closest to the pointer per axis
                        let gap = state.wm.config.general.gap as i32;
                        let (h_boundary_path, v_boundary_path) = {
                            let ws = state.wm.workspaces.focused_workspace();
                            ws.active_output
                                .and_then(|oid| state.wm.workspaces.output(oid))
                                .map(|o| {
                                    let area = o.usable_rect();
                                    let h_path = if rh {
                                        ws.root
                                            .find_closest_boundary_path_for_axis(
                                                area, ptr_x, ptr_y, gap,
                                                crate::layout::Orientation::Horizontal,
                                            )
                                            .map(|(p, _)| p)
                                    } else {
                                        None
                                    };
                                    let v_path = if rv {
                                        ws.root
                                            .find_closest_boundary_path_for_axis(
                                                area, ptr_x, ptr_y, gap,
                                                crate::layout::Orientation::Vertical,
                                            )
                                            .map(|(p, _)| p)
                                    } else {
                                        None
                                    };
                                    (h_path, v_path)
                                })
                                .unwrap_or((None, None))
                        };
                        log::info!(
                            "Pointer resize start on window {} (h={}, v={}, h_path={:?}, v_path={:?})",
                            win.id, rh, rv, h_boundary_path, v_boundary_path
                        );
                        let edges = crate::protocol::river_window_v1::Edges::Right
                            | crate::protocol::river_window_v1::Edges::Bottom;
                        win.proxy.inform_resize_start();
                        Some(SeatOp::Resize {
                            window_id: win.id,
                            start_x: sx,
                            start_y: sy,
                            start_width: win.width,
                            start_height: win.height,
                            edges,
                            resize_h: rh,
                            resize_v: rv,
                            h_boundary_path,
                            v_boundary_path,
                        })
                    } else {
                        None // No resize for floating windows (for now)
                    }
                } else if !is_move {
                    // Empty space resize
                    let gap = state.wm.config.general.gap as i32;
                    let frame_at_pointer = state.wm.workspaces.workspaces.iter().find_map(|ws| {
                        let output = ws
                            .active_output
                            .and_then(|oid| state.wm.workspaces.output(oid))?;
                        let area = output.usable_rect();
                        let layouts = ws.root.calculate_layout(area, gap);
                        layouts.into_iter().find_map(|(fid, rect)| {
                            if ptr_x >= rect.x
                                && ptr_x < rect.x + rect.width
                                && ptr_y >= rect.y
                                && ptr_y < rect.y + rect.height
                            {
                                Some(fid)
                            } else {
                                None
                            }
                        })
                    });
                    frame_at_pointer.map(|frame_id| {
                        let (rh, rv) = state.wm.detect_resize_axes(frame_id, ptr_x, ptr_y);
                        let (h_boundary_path, v_boundary_path) = {
                            let ws = state.wm.workspaces.focused_workspace();
                            ws.active_output
                                .and_then(|oid| state.wm.workspaces.output(oid))
                                .map(|o| {
                                    let area = o.usable_rect();
                                    let h_path = if rh {
                                        ws.root
                                            .find_closest_boundary_path_for_axis(
                                                area, ptr_x, ptr_y, gap,
                                                crate::layout::Orientation::Horizontal,
                                            )
                                            .map(|(p, _)| p)
                                    } else {
                                        None
                                    };
                                    let v_path = if rv {
                                        ws.root
                                            .find_closest_boundary_path_for_axis(
                                                area, ptr_x, ptr_y, gap,
                                                crate::layout::Orientation::Vertical,
                                            )
                                            .map(|(p, _)| p)
                                    } else {
                                        None
                                    };
                                    (h_path, v_path)
                                })
                                .unwrap_or((None, None))
                        };
                        log::info!(
                            "Pointer resize start on empty frame {:?} (h={}, v={}, h_path={:?}, v_path={:?})",
                            frame_id, rh, rv, h_boundary_path, v_boundary_path
                        );
                        SeatOp::ResizeEmpty {
                            frame_id,
                            resize_h: rh,
                            resize_v: rv,
                            h_boundary_path,
                            v_boundary_path,
                        }
                    })
                } else {
                    None
                };

                // Now mutably borrow the seat and apply
                if let Some(op) = new_op {
                    let seat = state.wm.seats.get_mut(data).unwrap();
                    seat.proxy.op_start_pointer();
                    seat.op_dx = 0;
                    seat.op_dy = 0;
                    seat.op_prev_dx = 0;
                    seat.op_prev_dy = 0;
                    seat.op_start_pointer_x = seat.pointer_x;
                    seat.op_start_pointer_y = seat.pointer_y;
                    seat.op = op;
                }
            }
            Event::Released => {}
        }
    }
}

// ── WlOutput (for connector name) ────────────────────────────────────────

impl Dispatch<WlOutput, u32> for AppData {
    fn event(
        state: &mut Self,
        _proxy: &WlOutput,
        event: <WlOutput as Proxy>::Event,
        data: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_output::Event;
        match event {
            Event::Name { name } => {
                log::info!("wl_output global {} connector name: {name}", data);
                state.wl_output_names.insert(*data, name.clone());
                if let Some(&oid) = state.wl_output_map.get(data) {
                    if let Some(output) = state.wm.workspaces.output_mut(oid) {
                        output.name = Some(name);
                    }
                    state.wm.workspaces.maybe_reassign_outputs();
                }
            }
            Event::Geometry { transform, .. } => {
                let transform = match transform {
                    wayland_client::WEnum::Value(value) => value as i32,
                    wayland_client::WEnum::Unknown(value) => value as i32,
                };
                log::info!("wl_output global {} geometry transform: {transform}", data);
                if let Some(&oid) = state.wl_output_map.get(data) {
                    if let Some(output) = state.wm.workspaces.output_mut(oid)
                        && output.transform != transform {
                            output.transform = transform;
                            state.wm.workspaces.outputs_changed = true;
                        }
                } else {
                    state.wl_output_transforms.insert(*data, transform);
                }
                if let Some(wm_proxy) = &state.river_wm {
                    wm_proxy.manage_dirty();
                }
            }
            Event::Scale { factor } => {
                log::info!("wl_output global {} scale: {factor}", data);
                if let Some(&oid) = state.wl_output_map.get(data) {
                    if let Some(output) = state.wm.workspaces.output_mut(oid)
                        && output.scale != factor {
                            output.scale = factor;
                            state.wm.workspaces.outputs_changed = true;
                        }
                } else {
                    state.wl_output_scales.insert(*data, factor);
                }
                if let Some(wm_proxy) = &state.river_wm {
                    wm_proxy.manage_dirty();
                }
            }
            Event::Mode { width, height, .. } => {
                log::info!("wl_output global {} mode: {width}x{height}", data);
                if let Some(&oid) = state.wl_output_map.get(data) {
                    if let Some(output) = state.wm.workspaces.output_mut(oid)
                        && (output.physical_width != width || output.physical_height != height) {
                            output.physical_width = width;
                            output.physical_height = height;
                            state.wm.workspaces.outputs_changed = true;
                        }
                } else {
                    state.wl_output_modes.insert(*data, (width, height));
                }
                if let Some(wm_proxy) = &state.river_wm {
                    wm_proxy.manage_dirty();
                }
            }
            Event::Description { description } => {
                log::info!("wl_output global {} description: {description}", data);
                state.wl_output_descriptions.insert(*data, description.clone());
                if let Some(&oid) = state.wl_output_map.get(data)
                    && let Some(output) = state.wm.workspaces.output_mut(oid) {
                        output.description = Some(description);
                    }
            }
            _ => {}
        }
    }
}

// ── Layer Shell ──────────────────────────────────────────────────────────

wayland_client::delegate_noop!(AppData: ignore crate::protocol::river_layer_shell_v1::RiverLayerShellV1);

impl Dispatch<crate::protocol::river_layer_shell_output_v1::RiverLayerShellOutputV1, u64>
    for AppData
{
    fn event(
        state: &mut Self,
        _proxy: &crate::protocol::river_layer_shell_output_v1::RiverLayerShellOutputV1,
        event: <crate::protocol::river_layer_shell_output_v1::RiverLayerShellOutputV1 as Proxy>::Event,
        data: &u64,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::river_layer_shell_output_v1::Event;
        let oid = crate::workspace::OutputId(*data);
        match event {
            Event::NonExclusiveArea {
                x,
                y,
                width,
                height,
            } => {
                log::info!(
                    "Layer-shell non-exclusive area for {oid:?}: ({x},{y}) {width}x{height}"
                );
                if let Some(output) = state.wm.workspaces.output_mut(oid) {
                    output.usable_x = x;
                    output.usable_y = y;
                    output.usable_width = width;
                    output.usable_height = height;
                    output.has_exclusive_zone = true;
                }
            }
        }
    }
}

impl Dispatch<crate::protocol::river_layer_shell_seat_v1::RiverLayerShellSeatV1, ()> for AppData {
    fn event(
        state: &mut Self,
        _proxy: &crate::protocol::river_layer_shell_seat_v1::RiverLayerShellSeatV1,
        event: <crate::protocol::river_layer_shell_seat_v1::RiverLayerShellSeatV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::river_layer_shell_seat_v1::Event;
        match event {
            Event::FocusExclusive => {
                log::info!("Layer-shell: exclusive focus");
                state.wm.layer_shell_has_focus = true;
            }
            Event::FocusNonExclusive => {
                log::info!("Layer-shell: non-exclusive focus");
                state.wm.layer_shell_has_focus = true;
            }
            Event::FocusNone => {
                log::info!("Layer-shell: focus none");
                state.wm.layer_shell_has_focus = false;
            }
        }
    }
}

// ── Output Management (wlr-output-management-v1) ─────────────────────────

impl Dispatch<ZwlrOutputManagerV1, ()> for AppData {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrOutputManagerV1,
        event: <ZwlrOutputManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::zwlr_output_manager_v1::Event;
        match event {
            Event::Head { head } => {
                let hid = head.id().protocol_id() as u64;
                log::info!("Output management: new head id={hid}");
                state.output_head_proxies.insert(hid, head);
                state
                    .monitors
                    .heads
                    .insert(hid, crate::monitors::HeadLive::default());
            }
            Event::Done { serial } => {
                state.monitors.serial = Some(serial);

                let Some((set_key, snap)) =
                    crate::monitors::snapshot(&state.monitors.heads, &state.monitors.modes)
                else {
                    // Metadata still settling; wait for next Done.
                    return;
                };

                state.monitors.last_set_key = Some(set_key.clone());

                // Case 1: we just issued an apply for this set; this Done is
                // the compositor's ack of our apply. Don't treat it as a
                // user edit, don't overwrite the saved profile.
                if state.monitors.pending_self_apply.as_deref() == Some(set_key.as_str()) {
                    state.monitors.pending_self_apply = None;
                    log::info!("Acknowledged self-apply for set '{set_key}'");
                    return;
                }

                // Case 2: we have a saved profile. It is authoritative.
                // Reapply if the live state diverges from it. We never
                // overwrite the saved profile from a Done event; explicit
                // user save is a separate path (not yet implemented; until
                // then, hand-edit monitors.json or use wdisplays + save).
                if let Some(target) = state.monitors.profiles.get(&set_key).cloned() {
                    if target == snap {
                        // Live already matches saved. Nothing to do.
                        state.monitors.failed_sets.remove(&set_key);
                    } else if state.monitors.failed_sets.contains(&set_key) {
                        // We already tried and the compositor rejected.
                        // Don't loop. The user can fix monitors.json and
                        // restart, or the next topology change clears this.
                    } else {
                        state.monitors.pending_self_apply = Some(set_key.clone());
                        match state.apply_monitor_profile(&target, qh) {
                            Ok(()) => log::info!(
                                "Set '{set_key}' diverged from saved profile; reapplying"
                            ),
                            Err(err) => {
                                state.monitors.apply_in_flight = false;
                                state.monitors.pending_self_apply = None;
                                log::warn!("Failed to apply saved profile: {err}");
                            }
                        }
                    }
                } else {
                    // Case 3: no saved profile for this set. Stay out of the
                    // way: do not apply, do not save. The user configures the
                    // layout (e.g. via wdisplays) and runs `notion-ctl
                    // save-monitors` to persist it.
                    log::info!(
                        "Monitor set '{set_key}' has no saved profile; not applying. Run 'notion-ctl save-monitors' to persist current live state."
                    );
                }
            }
            Event::Finished => {
                log::info!("Output management: manager finished");
                state.monitors.manager = None;
            }
        }
    }

    wayland_client::event_created_child!(AppData, ZwlrOutputManagerV1, [
        crate::protocol::zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for AppData {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrOutputHeadV1,
        event: <ZwlrOutputHeadV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::zwlr_output_head_v1::Event;
        use wayland_client::WEnum;
        let hid = _proxy.id().protocol_id() as u64;
        let head = match state.monitors.heads.get_mut(&hid) {
            Some(h) => h,
            None => return,
        };
        match event {
            Event::Name { name } => head.name = Some(name),
            Event::Description { description } => head.description = Some(description),
            Event::PhysicalSize { .. } => {}
            Event::Mode { mode } => {
                let mid = mode.id().protocol_id() as u64;
                head.mode_ids.push(mid);
                state.output_mode_proxies.insert(mid, mode);
            }
            Event::Enabled { enabled } => head.enabled = enabled != 0,
            Event::CurrentMode { mode } => {
                let mid = mode.id().protocol_id() as u64;
                head.current_mode_id = Some(mid);
                if let Some(mode) = state.monitors.modes.get(&mid) {
                    head.mode_w = mode.w;
                    head.mode_h = mode.h;
                    head.mode_refresh_mhz = mode.refresh_mhz;
                }
            }
            Event::Position { x, y } => {
                head.position_x = x;
                head.position_y = y;
            }
            Event::Transform { transform } => {
                if let WEnum::Value(t) = transform {
                    head.transform = t as i32;
                }
            }
            Event::Scale { scale } => head.scale_fixed = (scale * 120_000.0) as i32,
            Event::Finished => {
                state.monitors.heads.remove(&hid);
                state.output_head_proxies.remove(&hid);
            }
        }
    }

    wayland_client::event_created_child!(AppData, ZwlrOutputHeadV1, [
        crate::protocol::zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

// ── No-op dispatches for output management child types ──────────────────

wayland_client::delegate_noop!(AppData: ignore ZwlrOutputConfigurationHeadV1);

impl Dispatch<ZwlrOutputModeV1, ()> for AppData {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputModeV1,
        event: <ZwlrOutputModeV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::zwlr_output_mode_v1::Event;
        let mid = proxy.id().protocol_id() as u64;
        match event {
            Event::Size { width, height } => {
                let mode = state.monitors.modes.entry(mid).or_default();
                mode.w = width;
                mode.h = height;
                for head in state.monitors.heads.values_mut() {
                    if head.current_mode_id == Some(mid) {
                        head.mode_w = width;
                        head.mode_h = height;
                    }
                }
            }
            Event::Refresh { refresh } => {
                let mode = state.monitors.modes.entry(mid).or_default();
                mode.refresh_mhz = refresh;
                for head in state.monitors.heads.values_mut() {
                    if head.current_mode_id == Some(mid) {
                        head.mode_refresh_mhz = refresh;
                    }
                }
            }
            Event::Preferred => {
                state.monitors.modes.entry(mid).or_default().preferred = true;
            }
            Event::Finished => {
                state.monitors.modes.remove(&mid);
                state.output_mode_proxies.remove(&mid);
                for head in state.monitors.heads.values_mut() {
                    head.mode_ids.retain(|id| *id != mid);
                    if head.current_mode_id == Some(mid) {
                        head.current_mode_id = None;
                    }
                }
            }
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for AppData {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputConfigurationV1,
        event: <ZwlrOutputConfigurationV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::protocol::zwlr_output_configuration_v1::Event;
        state.monitors.apply_in_flight = false;
        match event {
            Event::Succeeded => {
                log::info!("Output config applied successfully");
                if let Some(key) = &state.monitors.pending_self_apply {
                    state.monitors.failed_sets.remove(key);
                }
            }
            Event::Failed => {
                if let Some(key) = state.monitors.pending_self_apply.clone() {
                    log::warn!("Output config failed for set '{key}'; will not retry this session");
                    state.monitors.failed_sets.insert(key);
                } else {
                    log::warn!("Output config failed");
                }
                state.monitors.pending_self_apply = None;
            }
            Event::Cancelled => {
                if let Some(key) = state.monitors.pending_self_apply.clone() {
                    log::warn!(
                        "Output config cancelled for set '{key}'; will not retry this session"
                    );
                    state.monitors.failed_sets.insert(key);
                } else {
                    log::warn!("Output config cancelled");
                }
                state.monitors.pending_self_apply = None;
            }
        }
        proxy.destroy();
    }
}

// ── WlSeat ───────────────────────────────────────────────────────────────

wayland_client::delegate_noop!(AppData: ignore wayland_client::protocol::wl_seat::WlSeat);

// ── WlPointer (for focus-follows-mouse on shell surfaces) ────────────────

impl Dispatch<wayland_client::protocol::wl_pointer::WlPointer, ()> for AppData {
    fn event(
        state: &mut Self,
        _proxy: &wayland_client::protocol::wl_pointer::WlPointer,
        event: <wayland_client::protocol::wl_pointer::WlPointer as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_pointer::Event;
        match event {
            Event::Motion { surface_x, .. } => {
                state.wl_pointer_surface_x = surface_x;
                state.wm.hover_surface_x = surface_x;

                if let Some(wm_proxy) = &state.river_wm {
                    wm_proxy.manage_dirty();
                }
            }
            Event::Enter {
                surface, surface_x, ..
            } => {
                let sid = surface.id().protocol_id();
                state.wl_pointer_surface = Some(sid);
                state.wl_pointer_surface_x = surface_x;
                state.wm.hover_surface_id = Some(sid);
                state.wm.hover_surface_x = surface_x;
                if let Some(wm_proxy) = &state.river_wm {
                    wm_proxy.manage_dirty();
                }
            }
            Event::Leave { .. } => {
                state.wm.hover_surface_id = None;
                // Don't clear wl_pointer_surface — keep the last known surface
                // so pointer bindings can reference it (River grabs the pointer
                // and sends Leave before the binding event arrives).
            }
            Event::Button {
                button,
                state: btn_state,
                ..
            } => {
                use wayland_client::protocol::wl_pointer::ButtonState;
                const BTN_LEFT: u32 = 0x110;
                if button == BTN_LEFT
                    && btn_state == wayland_client::WEnum::Value(ButtonState::Pressed)
                {
                    // Check if clicking a tab bar decoration
                    if let Some(surface_id) = state.wl_pointer_surface {
                        let surface_x = state.wl_pointer_surface_x;
                        // Find the window and frame for this decoration surface
                        if let Some(&window_id) =
                            state.wm.decorations.surface_to_window.get(&surface_id)
                        {
                            // Find frame containing this window to get tab count and width
                            let tab_info = state.wm.workspaces.workspaces.iter().find_map(|ws| {
                                let frame_id = ws.root.find_frame_with_window(window_id)?;
                                let frame = ws.root.find_frame(frame_id)?;
                                let gap = state.wm.config.general.gap as i32;
                                let output = ws
                                    .active_output
                                    .and_then(|oid| state.wm.workspaces.output(oid))?;
                                let area = output.usable_rect();
                                let layouts = ws.root.calculate_layout(area, gap);
                                let (_, rect) = layouts.iter().find(|(id, _)| *id == frame_id)?;
                                Some((ws.id, frame_id, frame.windows.len(), rect.width))
                            });

                            if let Some((ws_id, frame_id, num_tabs, frame_width)) = tab_info
                                && num_tabs > 0
                            {
                                let tab_width = frame_width as f64 / num_tabs as f64;
                                let tab_index = (surface_x / tab_width) as usize;
                                let tab_index = tab_index.min(num_tabs - 1);
                                state.pending_tab_click = Some((ws_id.0, frame_id, tab_index));
                                if let Some(wm_proxy) = &state.river_wm {
                                    wm_proxy.manage_dirty();
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// ── No-op dispatches ─────────────────────────────────────────────────────

wayland_client::delegate_noop!(AppData: ignore RiverXkbBindingsV1);
wayland_client::delegate_noop!(AppData: ignore RiverNodeV1);
wayland_client::delegate_noop!(AppData: ignore RiverDecorationV1);
wayland_client::delegate_noop!(AppData: ignore RiverShellSurfaceV1);
wayland_client::delegate_noop!(AppData: ignore WlCompositor);
wayland_client::delegate_noop!(AppData: ignore WlShm);
wayland_client::delegate_noop!(AppData: ignore WlShmPool);
wayland_client::delegate_noop!(AppData: ignore WlSurface);
wayland_client::delegate_noop!(AppData: ignore crate::protocol::wp_viewporter::WpViewporter);
wayland_client::delegate_noop!(AppData: ignore crate::protocol::wp_viewport::WpViewport);
wayland_client::delegate_noop!(AppData: ignore WlBuffer);
