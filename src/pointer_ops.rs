//! Pointer operation handling: move-drop, seat ops (resize), resize axis
//! detection, and cursor warping.

use crate::layout::{FrameId, Orientation, Rect};
use crate::wm::{SeatOp, WindowManager};
use crate::workspace::WorkspaceId;

/// Fraction of a frame's width/height taken by each split-triggering edge band.
/// Deliberately narrow: splitting on drop is a rare action, and a fat band made
/// it far too easy to split by accident when all you wanted was a new tab.
const SPLIT_BAND_FRACTION: f32 = 0.05;
/// Floor for the edge band so the split zones stay hittable in small frames.
const SPLIT_BAND_MIN: i32 = 10;
/// Ceiling for the edge band so huge frames don't get a huge split target.
const SPLIT_BAND_MAX: i32 = 60;
/// Breathing room between the tab/swap preview boxes and the split bands,
/// and between the tab and swap boxes themselves.
const PREVIEW_GUTTER: i32 = 5;

/// Thickness of the horizontal and vertical split bands for a frame, in pixels.
fn split_bands(rect: &Rect) -> (i32, i32) {
    let band = |extent: i32| {
        ((extent as f32 * SPLIT_BAND_FRACTION) as i32)
            .clamp(SPLIT_BAND_MIN, SPLIT_BAND_MAX)
            // Never let the two opposing bands eat the whole frame.
            .min((extent / 3).max(1))
    };
    (band(rect.width), band(rect.height))
}

/// Where within a frame a drop will land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// Add as a tab (upper half of the center area)
    Tab,
    /// Trade places with the target frame's active window (lower half of center)
    Swap,
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
    /// Each edge owns a narrow band (see [`SPLIT_BAND_FRACTION`]); in the
    /// corners the proportionally nearest edge wins. Everything else is the
    /// center, whose upper half tabs and lower half swaps.
    pub fn from_position(px: i32, py: i32, rect: &Rect) -> Self {
        let (band_x, band_y) = split_bands(rect);

        let candidates = [
            ((py - rect.y) as f32 / band_y as f32, DropZone::Top),
            (
                (rect.y + rect.height - 1 - py) as f32 / band_y as f32,
                DropZone::Bottom,
            ),
            ((px - rect.x) as f32 / band_x as f32, DropZone::Left),
            (
                (rect.x + rect.width - 1 - px) as f32 / band_x as f32,
                DropZone::Right,
            ),
        ];

        let nearest = candidates
            .into_iter()
            .filter(|(dist, _)| *dist < 1.0)
            .min_by(|a, b| a.0.total_cmp(&b.0));

        if let Some((_, zone)) = nearest {
            return zone;
        }

        if py < rect.y + rect.height / 2 {
            DropZone::Tab
        } else {
            DropZone::Swap
        }
    }

    /// Short human-readable name of the action this zone performs, shown in
    /// the drag preview box.
    pub fn label(&self) -> &'static str {
        match self {
            DropZone::Tab => "Add as tab",
            DropZone::Swap => "Swap windows",
            DropZone::Top => "Split top",
            DropZone::Bottom => "Split bottom",
            DropZone::Left => "Split left",
            DropZone::Right => "Split right",
        }
    }

    /// The area the drag preview should highlight for this zone.
    ///
    /// Split zones show where the dropped window will actually *end up* (the
    /// resulting half of the split), not the narrow band you have to aim at.
    /// Tab and swap show inset boxes that clear the split bands, so the whole
    /// cell never lights up as one undifferentiated blob.
    pub fn preview_rect(&self, rect: &Rect, ratio: f32, gap: i32) -> Rect {
        let first = |extent: i32| ((extent - gap) as f32 * ratio) as i32;

        match self {
            DropZone::Top => {
                let h = first(rect.height);
                Rect::new(rect.x, rect.y, rect.width, h.max(1))
            }
            DropZone::Bottom => {
                let h = first(rect.height);
                Rect::new(
                    rect.x,
                    rect.y + h + gap,
                    rect.width,
                    (rect.height - gap - h).max(1),
                )
            }
            DropZone::Left => {
                let w = first(rect.width);
                Rect::new(rect.x, rect.y, w.max(1), rect.height)
            }
            DropZone::Right => {
                let w = first(rect.width);
                Rect::new(
                    rect.x + w + gap,
                    rect.y,
                    (rect.width - gap - w).max(1),
                    rect.height,
                )
            }
            DropZone::Tab | DropZone::Swap => {
                let (band_x, band_y) = split_bands(rect);
                let inset_x = band_x + PREVIEW_GUTTER;
                let inset_y = band_y + PREVIEW_GUTTER;
                let inner_w = (rect.width - inset_x * 2).max(1);
                let inner_h = (rect.height - inset_y * 2).max(1);
                let top_h = ((inner_h - PREVIEW_GUTTER) / 2).max(1);
                if *self == DropZone::Tab {
                    Rect::new(rect.x + inset_x, rect.y + inset_y, inner_w, top_h)
                } else {
                    Rect::new(
                        rect.x + inset_x,
                        rect.y + inset_y + top_h + PREVIEW_GUTTER,
                        inner_w,
                        (inner_h - top_h - PREVIEW_GUTTER).max(1),
                    )
                }
            }
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

        // A swap onto an empty frame (or onto the frame we came from) has
        // nothing to trade with, so it degrades to a plain tab move.
        let zone = if zone == DropZone::Swap {
            if self.swap_dropped_window(ws_id, src_fid, target_frame_id, window_id) {
                log::info!(
                    "Pointer drag: window {} swapped with active window of frame {:?}",
                    window_id,
                    target_frame_id
                );
                return;
            }
            DropZone::Tab
        } else {
            zone
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
            DropZone::Tab | DropZone::Swap => {
                // Add as tab to existing frame
                if let Some(frame) = ws.root.find_frame_mut(target_frame_id) {
                    frame.add_window(win_ref);
                }
                ws.focused_frame = target_frame_id;
            }
            DropZone::Top | DropZone::Bottom | DropZone::Left | DropZone::Right => {
                let orientation = match zone {
                    DropZone::Top | DropZone::Bottom => Orientation::Vertical,
                    _ => Orientation::Horizontal,
                };
                // The dropped window takes the side the pointer was on. The
                // existing frame keeps its id and windows; only the position of
                // the freshly created frame in the split changes.
                let new_first = matches!(zone, DropZone::Top | DropZone::Left);
                if let Some(new_fid) =
                    ws.root
                        .split_frame_at(target_frame_id, orientation, ratio, new_first)
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

    /// Trade the dragged window with the target frame's *active* window,
    /// leaving both frames' tab order, tab index and focus history intact.
    ///
    /// Returns `false` when there is nothing to trade with — the target is the
    /// source frame, or it holds no windows — so the caller can fall back to a
    /// plain move.
    fn swap_dropped_window(
        &mut self,
        ws_id: WorkspaceId,
        src_fid: FrameId,
        dst_fid: FrameId,
        window_id: u64,
    ) -> bool {
        if src_fid == dst_fid {
            return false;
        }

        let dragged = self.workspaces.workspaces.iter().find_map(|ws| {
            ws.root
                .find_frame(src_fid)
                .and_then(|f| f.windows.iter().find(|w| w.window_id == window_id).cloned())
        });
        let displaced = self.workspaces.workspaces.iter().find_map(|ws| {
            ws.root
                .find_frame(dst_fid)
                .and_then(|f| f.active_window().cloned())
        });

        let (Some(dragged), Some(displaced)) = (dragged, displaced) else {
            return false;
        };
        let displaced_id = displaced.window_id;

        // The frames may live in different workspaces, so resolve the owning
        // workspaces first rather than holding two mutable borrows at once.
        let owner = |fid: FrameId| {
            self.workspaces
                .workspaces
                .iter()
                .position(|ws| ws.root.find_frame(fid).is_some())
        };
        let (Some(dst_ws), Some(src_ws)) = (owner(dst_fid), owner(src_fid)) else {
            return false;
        };

        if let Some(frame) = self.workspaces.workspaces[dst_ws].root.find_frame_mut(dst_fid) {
            frame.replace_window(displaced_id, dragged);
        }
        if let Some(frame) = self.workspaces.workspaces[src_ws].root.find_frame_mut(src_fid) {
            frame.replace_window(window_id, displaced);
        }

        for win in &mut self.windows {
            if win.id == window_id {
                win.frame_id = Some(dst_fid);
            } else if win.id == displaced_id {
                win.frame_id = Some(src_fid);
            }
        }

        self.workspaces.workspaces[ws_id.0].focused_frame = dst_fid;
        true
    }

    pub(crate) fn handle_seat_ops(&mut self) {
        // Collect move ops for floating windows
        struct FloatMoveCmd {
            window_id: u64,
            start_x: i32,
            start_y: i32,
            dx: i32,
            dy: i32,
        }
        struct TiledResizeCmd {
            pointer_x: i32,
            pointer_y: i32,
            h_boundary_path: Option<Vec<bool>>,
            v_boundary_path: Option<Vec<bool>>,
        }

        let mut float_moves: Vec<FloatMoveCmd> = Vec::new();
        let mut tiled_resizes: Vec<TiledResizeCmd> = Vec::new();

        for s in self.seats.values_mut().filter(|s| !s.op_release) {
            let ddx = s.op_dx - s.op_prev_dx;
            let ddy = s.op_dy - s.op_prev_dy;
            s.op_prev_dx = s.op_dx;
            s.op_prev_dy = s.op_dy;
            if ddx == 0 && ddy == 0 {
                continue;
            }

            match &s.op {
                SeatOp::Move {
                    window_id,
                    start_x,
                    start_y,
                } => {
                    // Check if the window is floating
                    let is_floating = self
                        .windows
                        .iter()
                        .find(|w| w.id == *window_id)
                        .is_some_and(|w| w.floating);
                    if is_floating {
                        float_moves.push(FloatMoveCmd {
                            window_id: *window_id,
                            start_x: *start_x,
                            start_y: *start_y,
                            dx: s.op_dx,
                            dy: s.op_dy,
                        });
                    }
                    // Tiled moves show preview overlay — handled elsewhere
                }
                SeatOp::Resize {
                    h_boundary_path,
                    v_boundary_path,
                    ..
                } => {
                    let cur_x = s.op_start_pointer_x + s.op_dx;
                    let cur_y = s.op_start_pointer_y + s.op_dy;
                    tiled_resizes.push(TiledResizeCmd {
                        pointer_x: cur_x,
                        pointer_y: cur_y,
                        h_boundary_path: h_boundary_path.clone(),
                        v_boundary_path: v_boundary_path.clone(),
                    });
                }
                SeatOp::ResizeEmpty {
                    h_boundary_path,
                    v_boundary_path,
                    ..
                } => {
                    let cur_x = s.op_start_pointer_x + s.op_dx;
                    let cur_y = s.op_start_pointer_y + s.op_dy;
                    tiled_resizes.push(TiledResizeCmd {
                        pointer_x: cur_x,
                        pointer_y: cur_y,
                        h_boundary_path: h_boundary_path.clone(),
                        v_boundary_path: v_boundary_path.clone(),
                    });
                }
                _ => {}
            }
        }

        // Apply floating moves
        for cmd in float_moves {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == cmd.window_id) {
                win.float_x = cmd.start_x + cmd.dx;
                win.float_y = cmd.start_y + cmd.dy;
            }
        }

        // Apply tiled resizes
        let gap = self.config.general.gap as i32;
        for cmd in tiled_resizes {
            let ws_idx = self.workspaces.focused_workspace.0;
            let area = {
                let ws = &self.workspaces.workspaces[ws_idx];
                ws.active_output
                    .and_then(|oid| self.workspaces.output(oid))
                    .map(|o| o.usable_rect())
            };
            if let Some(area) = area {
                let ws = &mut self.workspaces.workspaces[ws_idx];
                let has_paths = cmd.h_boundary_path.is_some() || cmd.v_boundary_path.is_some();
                if !has_paths {
                    // Fallback: no paths, use closest boundary (legacy behavior)
                    ws.root
                        .adjust_ratio_at(area, cmd.pointer_x, cmd.pointer_y, gap);
                } else {
                    // Adjust each axis independently using its stored path
                    if let Some(ref h_path) = cmd.h_boundary_path {
                        ws.root.adjust_ratio_at_path(
                            area,
                            h_path,
                            cmd.pointer_x,
                            cmd.pointer_y,
                            gap,
                        );
                    }
                    if let Some(ref v_path) = cmd.v_boundary_path {
                        ws.root.adjust_ratio_at_path(
                            area,
                            v_path,
                            cmd.pointer_x,
                            cmd.pointer_y,
                            gap,
                        );
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Rect {
        Rect::new(0, 0, 1000, 800)
    }

    #[test]
    fn split_bands_are_narrow() {
        let r = cell();
        let (bx, by) = split_bands(&r);
        assert_eq!(bx, 50); // 5% of 1000
        assert_eq!(by, 40); // 5% of 800
    }

    #[test]
    fn split_bands_respect_min_and_max() {
        // Tiny frame: the pixel floor wins, but never more than a third.
        let (bx, _) = split_bands(&Rect::new(0, 0, 60, 60));
        assert_eq!(bx, 10);
        // Huge frame: the pixel ceiling wins.
        let (bx, _) = split_bands(&Rect::new(0, 0, 4000, 4000));
        assert_eq!(bx, SPLIT_BAND_MAX);
    }

    #[test]
    fn edges_split_center_tabs_or_swaps() {
        let r = cell();
        assert_eq!(DropZone::from_position(500, 5, &r), DropZone::Top);
        assert_eq!(DropZone::from_position(500, 795, &r), DropZone::Bottom);
        assert_eq!(DropZone::from_position(5, 400, &r), DropZone::Left);
        assert_eq!(DropZone::from_position(995, 400, &r), DropZone::Right);
        assert_eq!(DropZone::from_position(500, 200, &r), DropZone::Tab);
        assert_eq!(DropZone::from_position(500, 600, &r), DropZone::Swap);
    }

    #[test]
    fn quarter_way_in_is_no_longer_a_split() {
        // The old 25% bands made this a split; it must now be a plain tab.
        let r = cell();
        assert_eq!(DropZone::from_position(500, 100, &r), DropZone::Tab);
        assert_eq!(DropZone::from_position(100, 300, &r), DropZone::Tab);
    }

    #[test]
    fn corners_pick_the_proportionally_nearest_edge() {
        let r = cell();
        // 5px from the top, 30px from the left: bands are 40 tall / 50 wide, so
        // 5/40 beats 30/50 and the top wins.
        assert_eq!(DropZone::from_position(30, 5, &r), DropZone::Top);
        assert_eq!(DropZone::from_position(5, 30, &r), DropZone::Left);
    }

    #[test]
    fn split_preview_shows_the_resulting_half_not_the_band() {
        let r = cell();
        let top = DropZone::Top.preview_rect(&r, 0.5, 0);
        assert_eq!((top.x, top.y, top.width, top.height), (0, 0, 1000, 400));
        let bottom = DropZone::Bottom.preview_rect(&r, 0.5, 0);
        assert_eq!(
            (bottom.x, bottom.y, bottom.width, bottom.height),
            (0, 400, 1000, 400)
        );
        let left = DropZone::Left.preview_rect(&r, 0.5, 0);
        assert_eq!((left.x, left.y, left.width, left.height), (0, 0, 500, 800));
        let right = DropZone::Right.preview_rect(&r, 0.5, 0);
        assert_eq!(
            (right.x, right.y, right.width, right.height),
            (500, 0, 500, 800)
        );
    }

    #[test]
    fn tab_and_swap_previews_clear_the_split_bands_and_each_other() {
        let r = cell();
        let (bx, by) = split_bands(&r);
        let tab = DropZone::Tab.preview_rect(&r, 0.5, 0);
        let swap = DropZone::Swap.preview_rect(&r, 0.5, 0);

        // Inset by the band plus a gutter on every side.
        assert_eq!(tab.x, bx + PREVIEW_GUTTER);
        assert_eq!(tab.y, by + PREVIEW_GUTTER);
        assert_eq!(tab.width, r.width - (bx + PREVIEW_GUTTER) * 2);
        assert_eq!(swap.x, tab.x);
        assert_eq!(swap.width, tab.width);
        assert_eq!(swap.y + swap.height, r.height - (by + PREVIEW_GUTTER));

        // Tab sits above swap with a gutter between them.
        assert_eq!(swap.y - (tab.y + tab.height), PREVIEW_GUTTER);
    }
}
