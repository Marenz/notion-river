//! Layout application and rendering: window management (dimensions, focus,
//! visibility) and position/border/decoration drawing.

use wayland_client::protocol::{wl_compositor::WlCompositor, wl_shm::WlShm};
use wayland_client::QueueHandle;

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
                                fractional_scale: output.fractional_scale(),
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
                        cmd.fractional_scale,
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

        // Position floating windows
        for win in &self.windows {
            if win.floating {
                win.node.set_position(win.float_x, win.float_y);
                win.node.place_top();
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
