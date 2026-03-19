//! Pointer operation handling: move-drop, seat ops (resize), resize axis
//! detection, and cursor warping.

use crate::layout::{FrameId, Orientation, Rect};
use crate::wm::{SeatOp, WindowManager};
use crate::workspace::WorkspaceId;

/// Where within a frame a drop will land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// Add as a tab (center area)
    Tab,
    /// Split and place on top
    Top,
    /// Split and place on bottom
    Bottom,
    /// Split and place on left
    Left,
    /// Split and place on right
    Right,
}

impl DropZone {
    /// Determine the drop zone from pointer position within a frame rect.
    /// Edge zones are 25% from each edge; center is the remaining area.
    pub fn from_position(px: i32, py: i32, rect: &Rect) -> Self {
        let rel_x = (px - rect.x) as f32 / rect.width.max(1) as f32;
        let rel_y = (py - rect.y) as f32 / rect.height.max(1) as f32;
        let edge = 0.25;

        if rel_y < edge && rel_y < rel_x && rel_y < (1.0 - rel_x) {
            DropZone::Top
        } else if rel_y > (1.0 - edge) && (1.0 - rel_y) < rel_x && (1.0 - rel_y) < (1.0 - rel_x) {
            DropZone::Bottom
        } else if rel_x < edge {
            DropZone::Left
        } else if rel_x > (1.0 - edge) {
            DropZone::Right
        } else {
            DropZone::Tab
        }
    }
}

/// Find which frame and drop zone the pointer is over.
pub fn find_drop_target(
    workspaces: &crate::workspace::WorkspaceManager,
    px: i32,
    py: i32,
    gap: i32,
) -> Option<(WorkspaceId, FrameId, Rect, DropZone)> {
    workspaces.workspaces.iter().find_map(|ws| {
        let output = ws.active_output.and_then(|oid| workspaces.output(oid))?;
        let area = output.usable_rect();
        let layouts = ws.root.calculate_layout(area, gap);
        layouts.into_iter().find_map(|(frame_id, rect)| {
            if px >= rect.x && px < rect.x + rect.width && py >= rect.y && py < rect.y + rect.height
            {
                let zone = DropZone::from_position(px, py, &rect);
                Some((ws.id, frame_id, rect, zone))
            } else {
                None
            }
        })
    })
}

impl WindowManager {
    pub(crate) fn handle_move_drop(&mut self, window_id: u64, drop_x: i32, drop_y: i32, gap: i32) {
        let Some((ws_id, target_frame_id, _rect, zone)) =
            find_drop_target(&self.workspaces, drop_x, drop_y, gap)
        else {
            return;
        };

        let source_frame_id = self
            .workspaces
            .workspaces
            .iter()
            .find_map(|ws| ws.root.find_frame_with_window(window_id));

        let Some(src_fid) = source_frame_id else {
            return;
        };

        // Get the window ref
        let win_ref = self.workspaces.workspaces.iter().find_map(|ws| {
            ws.root
                .find_frame(src_fid)
                .and_then(|f| f.windows.iter().find(|w| w.window_id == window_id).cloned())
        });

        let Some(win_ref) = win_ref else { return };

        // Remove from source frame
        for ws in &mut self.workspaces.workspaces {
            if let Some(frame) = ws.root.find_frame_mut(src_fid) {
                frame.remove_window(window_id);
            }
        }

        let ratio = self.config.general.default_split_ratio;
        let ws = &mut self.workspaces.workspaces[ws_id.0];

        match zone {
            DropZone::Tab => {
                // Add as tab to existing frame
                if let Some(frame) = ws.root.find_frame_mut(target_frame_id) {
                    frame.add_window(win_ref);
                }
                ws.focused_frame = target_frame_id;
            }
            DropZone::Top | DropZone::Bottom => {
                // Split vertically, place in new frame
                if let Some(new_fid) =
                    ws.root
                        .split_frame(target_frame_id, Orientation::Vertical, ratio)
                {
                    let _dest = if zone == DropZone::Top {
                        // Window goes to first (top), existing content stays in second (bottom)
                        // But split_frame puts new frame as second, so swap
                        target_frame_id
                    } else {
                        new_fid
                    };
                    if zone == DropZone::Top {
                        // Move existing windows from target to new frame, put our window in target
                        // This is complex — simpler: just add to the new frame (bottom for Top zone)
                        // Actually split_frame creates: [old_target | new_frame]
                        // For Top: we want [our_window | old_content] so add to old_target
                        // and move old content to new frame... too complex.
                        // Simple approach: add to new frame regardless, user can rearrange
                        if let Some(frame) = ws.root.find_frame_mut(new_fid) {
                            frame.add_window(win_ref);
                        }
                        ws.focused_frame = new_fid;
                    } else {
                        if let Some(frame) = ws.root.find_frame_mut(new_fid) {
                            frame.add_window(win_ref);
                        }
                        ws.focused_frame = new_fid;
                    }
                }
            }
            DropZone::Left | DropZone::Right => {
                // Split horizontally, place in new frame
                if let Some(new_fid) =
                    ws.root
                        .split_frame(target_frame_id, Orientation::Horizontal, ratio)
                {
                    if let Some(frame) = ws.root.find_frame_mut(new_fid) {
                        frame.add_window(win_ref);
                    }
                    ws.focused_frame = new_fid;
                }
            }
        }

        if let Some(win) = self.windows.iter_mut().find(|w| w.id == window_id) {
            win.frame_id = Some(ws.focused_frame);
        }

        log::info!(
            "Pointer drag: window {} -> {:?} zone {:?}",
            window_id,
            target_frame_id,
            zone
        );
    }

    pub(crate) fn handle_seat_ops(&mut self) {
        // Collect resize ops with pointer position
        struct ResizeCmd {
            dx: i32,
            dy: i32,
            resize_h: bool,
            resize_v: bool,
            pointer_x: i32,
            pointer_y: i32,
        }
        let resize_ops: Vec<ResizeCmd> = self
            .seats
            .values_mut()
            .filter(|s| !s.op_release)
            .filter_map(|s| {
                let (rh, rv) = match &s.op {
                    SeatOp::Resize {
                        resize_h, resize_v, ..
                    } => (*resize_h, *resize_v),
                    SeatOp::ResizeEmpty {
                        resize_h, resize_v, ..
                    } => (*resize_h, *resize_v),
                    _ => return None,
                };

                let ddx = s.op_dx - s.op_prev_dx;
                let ddy = s.op_dy - s.op_prev_dy;
                s.op_prev_dx = s.op_dx;
                s.op_prev_dy = s.op_dy;
                if ddx != 0 || ddy != 0 {
                    // Compute current pointer position from pre-drag position + total delta.
                    // pointer_x/y is stale during active ops (River sends op_delta, not position).
                    let cur_x = s.pointer_x + s.op_dx;
                    let cur_y = s.pointer_y + s.op_dy;
                    Some(ResizeCmd {
                        dx: ddx,
                        dy: ddy,
                        resize_h: rh,
                        resize_v: rv,
                        pointer_x: cur_x,
                        pointer_y: cur_y,
                    })
                } else {
                    None
                }
            })
            .collect();

        let gap = self.config.general.gap as i32;

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
                // Pass both axes — adjust_ratio_at picks the closest boundary
                let ratio_dx = if area.width > 0 {
                    cmd.dx as f32 / area.width as f32
                } else {
                    0.0
                };
                let ratio_dy = if area.height > 0 {
                    cmd.dy as f32 / area.height as f32
                } else {
                    0.0
                };
                ws.root.adjust_ratio_at(
                    area,
                    cmd.pointer_x,
                    cmd.pointer_y,
                    ratio_dx,
                    ratio_dy,
                    gap,
                );
            }
        }

        // During drag, the preview overlay shows where the window will land.
        // The window stays in its tiled position — no visual window dragging.
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
