//! Layout application and rendering: window management (dimensions, focus,
//! visibility) and position/border/decoration drawing.

use wayland_client::protocol::{wl_compositor::WlCompositor, wl_shm::WlShm};
use wayland_client::{Proxy, QueueHandle};

use crate::decorations::TAB_BAR_HEIGHT;
use crate::layout::FrameId;
use crate::protocol::river_window_manager_v1::RiverWindowManagerV1;
use crate::protocol::river_window_v1::Edges;
use crate::wm::{AppData, WindowManager};

impl WindowManager {
    pub(crate) fn apply_window_management(&mut self, _wm_proxy: &RiverWindowManagerV1) {
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
                            let bw = border as i32 * 2;
                            let tab_h = TAB_BAR_HEIGHT;
                            // Check for fixed dimensions from app bindings
                            let frame_idx = ws
                                .root
                                .all_frame_ids()
                                .iter()
                                .position(|id| *id == *frame_id);
                            let fixed = frame_idx.and_then(|fi| {
                                self.app_bindings.fixed_dimensions_for(
                                    &active_win.app_id,
                                    &ws.name,
                                    fi,
                                )
                            });
                            if let Some((fw, fh)) = fixed {
                                win.proxy.propose_dimensions(fw, fh);
                            } else {
                                win.proxy
                                    .propose_dimensions(rect.width - bw, rect.height - bw - tab_h);
                            }
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
        }

        // Manage floating windows: propose dimensions + show
        // Recenter windows that just got their first real dimensions.
        // Auto-focus newly positioned floating windows.
        let focused_ws = &self.workspaces.workspaces[self.workspaces.focused_workspace.0];
        let float_area = focused_ws
            .active_output
            .and_then(|oid| self.workspaces.output(oid))
            .map(|o| o.usable_rect());
        let mut newly_positioned_float: Option<u64> = None;
        for win in &mut self.windows {
            if win.floating {
                if win.width > 0 && win.height > 0 {
                    // Recenter if the window hasn't been positioned with real dims yet
                    if !win.float_positioned {
                        if let Some(area) = float_area {
                            win.float_x = area.x + (area.width - win.width) / 2;
                            win.float_y = area.y + (area.height - win.height) / 2;
                        }
                        win.float_positioned = true;
                        newly_positioned_float = Some(win.id);
                    }
                    win.proxy.propose_dimensions(win.width, win.height);
                    win.proxy.show();
                } else {
                    // Client hasn't committed dimensions yet — propose 0,0 to let
                    // the client pick, but don't show until we have real dimensions.
                    win.proxy.propose_dimensions(0, 0);
                }
            }
        }
        // Auto-focus the most recently positioned floating window + warp cursor to it
        if let Some(fid) = newly_positioned_float {
            self.focused_floating = Some(fid);
            if let Some(win) = self.windows.iter().find(|w| w.id == fid) {
                let cx = win.float_x + win.width / 2;
                let cy = win.float_y + win.height / 2;
                for seat in self.seats.values() {
                    seat.proxy.pointer_warp(cx, cy);
                }
            }
        }

        // Hide windows on non-visible workspaces
        for win in &self.windows {
            if win.floating {
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

        // Focus: floating window takes priority if one is active
        // Clean up focused_floating if the window no longer exists or isn't floating
        if let Some(fid) = self.focused_floating {
            let still_valid = self
                .windows
                .iter()
                .find(|w| w.id == fid)
                .is_some_and(|w| w.floating && !w.closed);
            if !still_valid {
                self.focused_floating = None;
            }
        }

        if let Some(float_id) = self.focused_floating {
            if let Some(win) = self.windows.iter().find(|w| w.id == float_id) {
                for seat in self.seats.values() {
                    seat.proxy.focus_window(&win.proxy);
                }
            }
        } else {
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
        }
    }

    pub(crate) fn apply_layout_positions(
        &mut self,
        wm_proxy: &RiverWindowManagerV1,
        shm: Option<&WlShm>,
        compositor: Option<&WlCompositor>,
        viewporter: Option<&crate::protocol::wp_viewporter::WpViewporter>,
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
            #[allow(dead_code)]
            rect_height: i32,
            is_focused: bool,
            border_color: (u32, u32, u32, u32),
            fractional_scale: f64,
        }
        let mut draw_cmds: Vec<DrawCmd> = Vec::new();

        for ws in &self.workspaces.workspaces {
            let output = match ws.active_output.and_then(|oid| self.workspaces.output(oid)) {
                Some(o) => o,
                None => continue,
            };

            let area = output.usable_rect();
            let frame_layouts = ws.root.calculate_layout(area, gap);

            // Resize mode indicator color (bright orange)
            let resize_color = parse_hex_color("#ff9e64");
            let in_resize_mode = self.mode == crate::wm::InputMode::Resize;

            for (frame_id, rect) in &frame_layouts {
                if let Some(frame) = ws.root.find_frame(*frame_id)
                    && let Some(active_win) = frame.active_window()
                {
                    let is_focused = *frame_id == focused_frame_id;
                    let color = if is_focused && in_resize_mode {
                        resize_color
                    } else if is_focused {
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
                            fractional_scale: output.fractional_scale(),
                        });
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
                if let Some(frame) = ws.root.find_frame(*frame_id)
                    && frame.is_empty()
                {
                    empty_cmds.push(EmptyCmd {
                        frame_id: *frame_id,
                        rect: *rect,
                        is_focused: *frame_id == focused_frame_id,
                    });
                }
            }
        }

        // Draw tab bars and empty frame indicators
        if let (Some(shm), Some(compositor)) = (shm, compositor) {
            for cmd in &draw_cmds {
                let frame = self
                    .workspaces
                    .workspaces
                    .iter()
                    .find_map(|ws| ws.root.find_frame(cmd.frame_id));

                if let Some(frame) = frame {
                    // Check if this frame has an app binding
                    let is_bound = self
                        .workspaces
                        .workspaces
                        .iter()
                        .find_map(|ws| {
                            let ids = ws.root.all_frame_ids();
                            ids.iter()
                                .position(|id| *id == cmd.frame_id)
                                .map(|fi| self.app_bindings.is_bound(&ws.name, fi))
                        })
                        .unwrap_or(false);

                    // Compute hovered tab index if pointer is on this decoration.
                    // surface_x is in surface-local coords (unscaled).
                    let hovered_tab = self.hover_surface_id.and_then(|sid| {
                        let dec = self.decorations.decorations.get(&cmd.window_id)?;
                        if dec.surface.id().protocol_id() != sid {
                            return None;
                        }
                        let num_tabs = frame.windows.len();
                        if num_tabs <= 1 {
                            return None;
                        }
                        let tab_width = cmd.rect_width as f64 / num_tabs as f64;
                        let idx = (self.hover_surface_x / tab_width) as usize;
                        Some(idx.min(num_tabs - 1))
                    });

                    let win = &self.windows[cmd.win_idx];
                    self.decorations.draw_tab_bar(
                        cmd.window_id,
                        &win.proxy,
                        frame,
                        cmd.rect_width,
                        cmd.is_focused,
                        is_bound,
                        cmd.fractional_scale,
                        hovered_tab,
                        &self.colors,
                        shm,
                        compositor,
                        viewporter,
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
                    &self.colors,
                    shm,
                    compositor,
                    wm_proxy,
                    qh,
                );
            }
            self.empty_frames.cleanup(&empty_ids);
        }

        // Position and style floating windows (only if they have real dimensions)
        let float_border_color = parse_hex_color(&self.config.appearance.active_border);
        for win in &self.windows {
            if win.floating && win.width > 0 && win.height > 0 {
                win.node.set_position(win.float_x, win.float_y);
                win.node.place_top();
                let all_edges = Edges::Left | Edges::Right | Edges::Top | Edges::Bottom;
                win.proxy.set_borders(
                    all_edges,
                    border,
                    float_border_color.0,
                    float_border_color.1,
                    float_border_color.2,
                    float_border_color.3,
                );
            }
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

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
