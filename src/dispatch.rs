//! Wayland protocol dispatch implementations.
//!
//! Each River protocol interface needs a `Dispatch` impl that handles
//! events from the compositor. This follows the same pattern as tinyrwm.

use wayland_backend::client::ObjectId;
use wayland_client::{
    protocol::{
        wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_registry, wl_shm::WlShm,
        wl_shm_pool::WlShmPool, wl_surface::WlSurface,
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
            }
            Event::Seat { id } => {
                log::info!("New seat: {:?}", id.id());
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
            Event::Identifier { .. } => {}
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
            Event::WlOutput { name } => {
                log::info!("Output {oid:?} wl_output name: {name}");
                // The WlOutput event gives us the wl_output global name (u32),
                // not the output name string. Output name comes from the
                // wl_output description/name which we'd need to bind separately.
                // For now, we'll set the name when we get it from elsewhere.
                let _ = name;
                // if let Some(output) = state.wm.workspaces.output_mut(oid) {
                //     output.name = Some(...);
                // }
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
            Event::WlSeat { .. } => {}
            Event::PointerEnter { window } => seat.hovered = Some(window),
            Event::PointerLeave => seat.hovered = None,
            Event::WindowInteraction { window } => seat.interacted = Some(window),
            Event::ShellSurfaceInteraction { .. } => {}
            Event::OpDelta { dx, dy } => {
                seat.op_dx = dx;
                seat.op_dy = dy;
            }
            Event::OpRelease => seat.op_release = true,
            Event::PointerPosition { .. } => {}
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
        let seat = match state.wm.seats.get_mut(data) {
            Some(s) => s,
            None => return,
        };
        let binding = match seat.pointer_bindings.get(&proxy.id()) {
            Some(b) => b,
            None => return,
        };
        let is_move = binding.is_move;
        match event {
            Event::Pressed => {
                if let Some(hovered) = &seat.hovered {
                    let hovered_id = hovered.id().protocol_id() as u64;
                    if let Some(win) = state.wm.windows.iter().find(|w| w.id == hovered_id) {
                        // Get current position: use float coords if floating,
                        // otherwise compute from layout
                        let (sx, sy) = if win.floating {
                            (win.float_x, win.float_y)
                        } else {
                            // Find the frame rect for this window
                            let gap = state.wm.config.general.gap as i32;
                            let border = state.wm.config.general.border_width as i32;
                            let pos = state.wm.workspaces.workspaces.iter().find_map(|ws| {
                                let output = ws
                                    .active_output
                                    .and_then(|oid| state.wm.workspaces.output(oid))?;
                                let area = output.usable_rect();
                                let layouts = ws.root.calculate_layout(area, gap);
                                let frame_id = ws.root.find_frame_with_window(win.id)?;
                                layouts
                                    .into_iter()
                                    .find(|(id, _)| *id == frame_id)
                                    .map(|(_, r)| {
                                        (
                                            r.x + border,
                                            r.y + border + crate::decorations::TAB_BAR_HEIGHT,
                                        )
                                    })
                            });
                            pos.unwrap_or((win.float_x, win.float_y))
                        };

                        seat.proxy.op_start_pointer();
                        seat.op_dx = 0;
                        seat.op_dy = 0;
                        if is_move {
                            log::info!(
                                "Pointer move start on window {} at ({},{})",
                                win.id,
                                sx,
                                sy
                            );
                            seat.op = SeatOp::Move {
                                window_id: win.id,
                                start_x: sx,
                                start_y: sy,
                            };
                        } else {
                            log::info!("Pointer resize start on window {}", win.id);
                            let edges = crate::protocol::river_window_v1::Edges::Right
                                | crate::protocol::river_window_v1::Edges::Bottom;
                            win.proxy.inform_resize_start();
                            seat.op = SeatOp::Resize {
                                window_id: win.id,
                                start_x: sx,
                                start_y: sy,
                                start_width: win.width,
                                start_height: win.height,
                                edges,
                            };
                        }
                    }
                }
            }
            Event::Released => {}
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
