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
};

use crate::wm::{AppData, ManagedWindow, Seat, SeatOp};
use crate::workspace::{Output, OutputId};

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
                        if let Some(frame) = ws.root.find_frame_mut(frame_id) {
                            if tab_index < frame.windows.len() {
                                log::info!("Tab click: frame {:?} tab {}", frame_id, tab_index);
                                frame.active_tab = tab_index;
                            }
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
                state.wm.handle_manage_start(proxy, river_xkb, qh);
            }
            Event::RenderStart => {
                state.wm.handle_render_start(
                    proxy,
                    state.wl_shm.as_ref(),
                    state.wl_compositor.as_ref(),
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
            Event::DimensionsHint { .. } => {}
            Event::AppId { app_id } => {
                if let Some(ref id) = app_id {
                    log::info!("Window {} app_id: {id}", window.id);
                }
                window.app_id = app_id.unwrap_or_default();
            }
            Event::Title { title } => {
                window.title = title.unwrap_or_default();
            }
            Event::Parent { .. } => {}
            Event::DecorationHint { .. } => {}
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
                // Check if wl_output.name already arrived for this global
                if let Some(connector_name) = state.wl_output_names.get(&global_name).cloned() {
                    log::info!("Output {oid:?} applying stored connector name: {connector_name}");
                    if let Some(output) = state.wm.workspaces.output_mut(oid) {
                        output.name = Some(connector_name);
                    }
                }
                state.wm.workspaces.reassign_outputs();
            }
            Event::Position { x, y } => {
                if let Some(output) = state.wm.workspaces.output_mut(oid) {
                    output.x = x;
                    output.y = y;
                }
            }
            Event::Dimensions { width, height } => {
                log::info!("Output {oid:?} dimensions: {width}x{height}");
                if let Some(output) = state.wm.workspaces.output_mut(oid) {
                    output.width = width;
                    output.height = height;
                }
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
                let hovered_win =
                    hovered_id.and_then(|hid| state.wm.windows.iter().find(|w| w.id == hid));

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
                        Some(SeatOp::Move {
                            window_id: win.id,
                            start_x: sx,
                            start_y: sy,
                        })
                    } else {
                        let frame_id = state
                            .wm
                            .workspaces
                            .workspaces
                            .iter()
                            .find_map(|ws| ws.root.find_frame_with_window(win.id));
                        let (rh, rv) = frame_id
                            .map(|fid| state.wm.detect_resize_axes(fid, ptr_x, ptr_y))
                            .unwrap_or((true, true));
                        log::info!(
                            "Pointer resize start on window {} (h={}, v={})",
                            win.id,
                            rh,
                            rv
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
                        })
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
                        log::info!(
                            "Pointer resize start on empty frame {:?} (h={}, v={})",
                            frame_id,
                            rh,
                            rv
                        );
                        SeatOp::ResizeEmpty {
                            frame_id,
                            resize_h: rh,
                            resize_v: rv,
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
                    state.wm.workspaces.reassign_outputs();
                }
            }
            Event::Scale { factor } => {
                log::info!("wl_output global {} scale: {factor}", data);
                if let Some(&oid) = state.wl_output_map.get(data) {
                    if let Some(output) = state.wm.workspaces.output_mut(oid) {
                        output.scale = factor;
                    }
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
                if let Some(wm_proxy) = &state.river_wm {
                    wm_proxy.manage_dirty();
                }
            }
            Event::Enter {
                surface, surface_x, ..
            } => {
                state.wl_pointer_surface = Some(surface.id().protocol_id());
                state.wl_pointer_surface_x = surface_x;
                if let Some(wm_proxy) = &state.river_wm {
                    wm_proxy.manage_dirty();
                }
            }
            Event::Leave { .. } => {
                state.wl_pointer_surface = None;
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

                            if let Some((ws_id, frame_id, num_tabs, frame_width)) = tab_info {
                                if num_tabs > 0 {
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
wayland_client::delegate_noop!(AppData: ignore WlBuffer);
