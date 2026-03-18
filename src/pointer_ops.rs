//! Pointer operation handling: move-drop, seat ops (resize), resize axis
//! detection, and cursor warping.

use crate::layout::FrameId;
use crate::wm::{SeatOp, WindowManager};

impl WindowManager {
    pub(crate) fn handle_move_drop(&mut self, window_id: u64, drop_x: i32, drop_y: i32, gap: i32) {
        let target_frame = self.workspaces.workspaces.iter().find_map(|ws| {
            let output = ws
                .active_output
                .and_then(|oid| self.workspaces.output(oid))?;
            let area = output.usable_rect();
            let layouts = ws.root.calculate_layout(area, gap);
            layouts.into_iter().find_map(|(frame_id, rect)| {
                if drop_x >= rect.x
                    && drop_x < rect.x + rect.width
                    && drop_y >= rect.y
                    && drop_y < rect.y + rect.height
                {
                    Some((ws.id, frame_id))
                } else {
                    None
                }
            })
        });

        if let Some((ws_id, target_frame_id)) = target_frame {
            let source_frame_id = self
                .workspaces
                .workspaces
                .iter()
                .find_map(|ws| ws.root.find_frame_with_window(window_id));

            if let Some(src_fid) = source_frame_id {
                if src_fid != target_frame_id {
                    let win_ref = self.workspaces.workspaces.iter().find_map(|ws| {
                        ws.root.find_frame(src_fid).and_then(|f| {
                            f.windows.iter().find(|w| w.window_id == window_id).cloned()
                        })
                    });

                    if let Some(win_ref) = win_ref {
                        for ws in &mut self.workspaces.workspaces {
                            if let Some(frame) = ws.root.find_frame_mut(src_fid) {
                                frame.remove_window(window_id);
                            }
                        }
                        let ws = &mut self.workspaces.workspaces[ws_id.0];
                        if let Some(frame) = ws.root.find_frame_mut(target_frame_id) {
                            frame.add_window(win_ref);
                        }
                        if let Some(win) = self.windows.iter_mut().find(|w| w.id == window_id) {
                            win.frame_id = Some(target_frame_id);
                        }
                        ws.focused_frame = target_frame_id;
                        log::info!(
                            "Pointer drag moved window {} from {:?} to {:?}",
                            window_id,
                            src_fid,
                            target_frame_id
                        );
                    }
                }
            }
        }
    }

    pub(crate) fn handle_seat_ops(&mut self) {
        // Collect resize ops with axis flags
        struct ResizeCmd {
            frame_id: FrameId,
            dx: i32,
            dy: i32,
            resize_h: bool,
            resize_v: bool,
        }
        let resize_ops: Vec<ResizeCmd> = self
            .seats
            .values_mut()
            .filter(|s| !s.op_release)
            .filter_map(|s| {
                let (frame_id, rh, rv) = match &s.op {
                    SeatOp::Resize {
                        window_id,
                        resize_h,
                        resize_v,
                        ..
                    } => {
                        let fid = self
                            .workspaces
                            .workspaces
                            .iter()
                            .find_map(|ws| ws.root.find_frame_with_window(*window_id))?;
                        (fid, *resize_h, *resize_v)
                    }
                    SeatOp::ResizeEmpty {
                        frame_id,
                        resize_h,
                        resize_v,
                    } => (*frame_id, *resize_h, *resize_v),
                    _ => return None,
                };

                let ddx = s.op_dx - s.op_prev_dx;
                let ddy = s.op_dy - s.op_prev_dy;
                s.op_prev_dx = s.op_dx;
                s.op_prev_dy = s.op_dy;
                if ddx != 0 || ddy != 0 {
                    Some(ResizeCmd {
                        frame_id,
                        dx: ddx,
                        dy: ddy,
                        resize_h: rh,
                        resize_v: rv,
                    })
                } else {
                    None
                }
            })
            .collect();

        for cmd in resize_ops {
            let ws_idx = self.workspaces.focused_workspace.0;
            let area = {
                let ws = &self.workspaces.workspaces[ws_idx];
                ws.active_output
                    .and_then(|oid| self.workspaces.output(oid))
                    .map(|o| o.usable_rect())
            };
            if let Some(area) = area {
                let ws = &mut self.workspaces.workspaces[ws_idx];
                let ratio_dx = if cmd.resize_h && area.width > 0 {
                    cmd.dx as f32 / area.width as f32
                } else {
                    0.0
                };
                let ratio_dy = if cmd.resize_v && area.height > 0 {
                    cmd.dy as f32 / area.height as f32
                } else {
                    0.0
                };
                ws.root.adjust_ratio(cmd.frame_id, ratio_dx, ratio_dy);
            }
        }
    }

    /// Determine which resize axes are active based on pointer proximity
    /// to split boundaries. Returns (resize_h, resize_v).
    pub fn detect_resize_axes(&self, frame_id: FrameId, px: i32, py: i32) -> (bool, bool) {
        let gap = self.config.general.gap as i32;
        let ws = self.workspaces.focused_workspace();
        let output = match ws.active_output.and_then(|oid| self.workspaces.output(oid)) {
            Some(o) => o,
            None => return (true, true),
        };
        let area = output.usable_rect();
        let layouts = ws.root.calculate_layout(area, gap);

        let my_rect = match layouts.iter().find(|(id, _)| *id == frame_id) {
            Some((_, r)) => *r,
            None => return (true, true),
        };

        // Check which axes have split neighbors at all
        let has_h_neighbor = layouts.iter().any(|(id, rect)| {
            *id != frame_id
                && crate::layout::vertical_overlap(my_rect, *rect) > 0
                && (rect.x + rect.width <= my_rect.x + gap
                    || rect.x >= my_rect.x + my_rect.width - gap)
        });
        let has_v_neighbor = layouts.iter().any(|(id, rect)| {
            *id != frame_id
                && crate::layout::horizontal_overlap(my_rect, *rect) > 0
                && (rect.y + rect.height <= my_rect.y + gap
                    || rect.y >= my_rect.y + my_rect.height - gap)
        });

        if has_h_neighbor && has_v_neighbor {
            // Both axes have neighbors — allow both near corners (25% from edge),
            // otherwise pick the nearest boundary axis
            let dist_h = (px - my_rect.x)
                .abs()
                .min(((my_rect.x + my_rect.width) - px).abs());
            let dist_v = (py - my_rect.y)
                .abs()
                .min(((my_rect.y + my_rect.height) - py).abs());
            let corner_h = my_rect.width / 4;
            let corner_v = my_rect.height / 4;

            if dist_h < corner_h && dist_v < corner_v {
                (true, true) // corner — both axes
            } else {
                // Pick the axis with the closer boundary (proportionally)
                let rel_h = dist_h as f32 / my_rect.width.max(1) as f32;
                let rel_v = dist_v as f32 / my_rect.height.max(1) as f32;
                (rel_h < rel_v, rel_v <= rel_h)
            }
        } else {
            (has_h_neighbor, has_v_neighbor)
        }
    }

    pub(crate) fn warp_cursor_to_frame(&self, frame_id: FrameId) {
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
}
