use std::fmt;

/// Unique identifier for a frame within the layout tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId(pub u64);

static NEXT_FRAME_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl FrameId {
    pub fn new() -> Self {
        Self(NEXT_FRAME_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// A rectangle in logical pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Orientation of a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Horizontal, // children are side by side (left | right)
    Vertical,   // children are stacked (top / bottom)
}

/// A node in the static tiling tree.
///
/// This is the core Notion concept: the tree is a persistent wireframe.
/// Frames (leaves) exist independently of windows. Splitting creates new
/// frames; closing a frame (when empty) removes it. Windows are placed
/// *into* frames and never alter the tree structure.
#[derive(Debug)]
pub enum SplitNode {
    /// A leaf: a frame that can hold zero or more tabbed windows.
    Leaf(Frame),
    /// An interior node: two children separated by a split.
    Split {
        orientation: Orientation,
        /// Fraction of space given to the first child (0.0..1.0).
        ratio: f32,
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

/// A frame is a single cell in the tiling grid.
/// It holds zero or more windows as tabs (only one visible at a time).
///
/// Key Notion property: a frame with zero windows is perfectly valid
/// and renders as an empty cell with a border.
#[derive(Debug)]
pub struct Frame {
    /// Unique identifier for this frame.
    pub id: FrameId,
    /// Optional name for winprop targeting (e.g. "browser", "terminal").
    pub name: Option<String>,
    /// Windows in this frame, as (window_id, app_id, title) tracking.
    /// The actual River window proxies are stored in the WM state;
    /// here we just track which windows belong to this frame.
    pub windows: Vec<WindowRef>,
    /// Index of the currently visible tab.
    pub active_tab: usize,
}

/// A reference to a window stored in a frame.
#[derive(Debug, Clone)]
pub struct WindowRef {
    /// The River protocol object identifier (as u32 for serialization).
    pub window_id: u64,
    pub app_id: String,
    pub title: String,
}

impl Frame {
    pub fn new() -> Self {
        Self {
            id: FrameId::new(),
            name: None,
            windows: Vec::new(),
            active_tab: 0,
        }
    }

    #[allow(dead_code)]
    pub fn named(name: &str) -> Self {
        Self {
            id: FrameId::new(),
            name: Some(name.to_string()),
            windows: Vec::new(),
            active_tab: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    #[allow(dead_code)]
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Get the currently active window reference, if any.
    pub fn active_window(&self) -> Option<&WindowRef> {
        self.windows.get(self.active_tab)
    }

    /// Add a window to this frame. It becomes the active tab.
    pub fn add_window(&mut self, win: WindowRef) {
        self.windows.push(win);
        self.active_tab = self.windows.len() - 1;
    }

    /// Add a window without changing the active tab (used during restore).
    pub fn add_window_quiet(&mut self, win: WindowRef) {
        self.windows.push(win);
    }

    /// Remove a window by id. Returns the removed WindowRef if found.
    /// Adjusts active_tab to stay in bounds.
    pub fn remove_window(&mut self, window_id: u64) -> Option<WindowRef> {
        if let Some(pos) = self.windows.iter().position(|w| w.window_id == window_id) {
            let removed = self.windows.remove(pos);
            if self.active_tab >= self.windows.len() && !self.windows.is_empty() {
                self.active_tab = self.windows.len() - 1;
            }
            Some(removed)
        } else {
            None
        }
    }

    /// Cycle to the next tab.
    pub fn next_tab(&mut self) {
        if !self.windows.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.windows.len();
        }
    }

    /// Cycle to the previous tab.
    pub fn prev_tab(&mut self) {
        if !self.windows.is_empty() {
            self.active_tab = if self.active_tab == 0 {
                self.windows.len() - 1
            } else {
                self.active_tab - 1
            };
        }
    }

    pub fn contains_window(&self, window_id: u64) -> bool {
        self.windows.iter().any(|w| w.window_id == window_id)
    }
}

// ── SplitNode operations ─────────────────────────────────────────────────

impl SplitNode {
    /// Create a single empty frame (the default workspace layout).
    pub fn single_frame() -> Self {
        SplitNode::Leaf(Frame::new())
    }

    /// Create a horizontal split with two empty frames.
    pub fn hsplit(ratio: f32) -> Self {
        SplitNode::Split {
            orientation: Orientation::Horizontal,
            ratio,
            first: Box::new(SplitNode::Leaf(Frame::new())),
            second: Box::new(SplitNode::Leaf(Frame::new())),
        }
    }

    /// Create a vertical split with two empty frames.
    pub fn vsplit(ratio: f32) -> Self {
        SplitNode::Split {
            orientation: Orientation::Vertical,
            ratio,
            first: Box::new(SplitNode::Leaf(Frame::new())),
            second: Box::new(SplitNode::Leaf(Frame::new())),
        }
    }

    /// Split the frame with the given id. The existing frame becomes the
    /// first child; a new empty frame becomes the second child.
    /// Returns the FrameId of the newly created frame.
    pub fn split_frame(
        &mut self,
        target_id: FrameId,
        orientation: Orientation,
        ratio: f32,
    ) -> Option<FrameId> {
        match self {
            SplitNode::Leaf(frame) => {
                if frame.id == target_id {
                    let new_frame = Frame::new();
                    let new_id = new_frame.id;
                    // Take ownership of the current leaf, replace self with a Split.
                    let old_self = std::mem::replace(self, SplitNode::single_frame());
                    *self = SplitNode::Split {
                        orientation,
                        ratio,
                        first: Box::new(old_self),
                        second: Box::new(SplitNode::Leaf(new_frame)),
                    };
                    Some(new_id)
                } else {
                    None
                }
            }
            SplitNode::Split { first, second, .. } => first
                .split_frame(target_id, orientation, ratio)
                .or_else(|| second.split_frame(target_id, orientation, ratio)),
        }
    }

    /// Remove a frame by id (unsplit). Only works if the frame is empty.
    /// The sibling takes the parent's place.
    /// Returns true if the frame was removed.
    pub fn remove_frame(&mut self, target_id: FrameId) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split { first, second, .. } => {
                // Check if first child is the target leaf
                if let SplitNode::Leaf(frame) = first.as_ref()
                    && frame.id == target_id && frame.is_empty()
                {
                    let sibling = std::mem::replace(second.as_mut(), SplitNode::single_frame());
                    *self = sibling;
                    return true;
                }
                // Check if second child is the target leaf
                if let SplitNode::Leaf(frame) = second.as_ref()
                    && frame.id == target_id && frame.is_empty()
                {
                    let sibling = std::mem::replace(first.as_mut(), SplitNode::single_frame());
                    *self = sibling;
                    return true;
                }
                // Recurse
                first.remove_frame(target_id) || second.remove_frame(target_id)
            }
        }
    }

    /// Find a frame by id, return mutable reference.
    pub fn find_frame_mut(&mut self, target_id: FrameId) -> Option<&mut Frame> {
        match self {
            SplitNode::Leaf(frame) => {
                if frame.id == target_id {
                    Some(frame)
                } else {
                    None
                }
            }
            SplitNode::Split { first, second, .. } => first
                .find_frame_mut(target_id)
                .or_else(|| second.find_frame_mut(target_id)),
        }
    }

    /// Find a frame by id, return immutable reference.
    pub fn find_frame(&self, target_id: FrameId) -> Option<&Frame> {
        match self {
            SplitNode::Leaf(frame) => {
                if frame.id == target_id {
                    Some(frame)
                } else {
                    None
                }
            }
            SplitNode::Split { first, second, .. } => first
                .find_frame(target_id)
                .or_else(|| second.find_frame(target_id)),
        }
    }

    /// Find the frame containing a specific window.
    pub fn find_frame_with_window(&self, window_id: u64) -> Option<FrameId> {
        match self {
            SplitNode::Leaf(frame) => {
                if frame.contains_window(window_id) {
                    Some(frame.id)
                } else {
                    None
                }
            }
            SplitNode::Split { first, second, .. } => first
                .find_frame_with_window(window_id)
                .or_else(|| second.find_frame_with_window(window_id)),
        }
    }

    /// Find a frame by name (for winprop targeting).
    #[allow(dead_code)]
    pub fn find_frame_by_name(&self, name: &str) -> Option<FrameId> {
        match self {
            SplitNode::Leaf(frame) => {
                if frame.name.as_deref() == Some(name) {
                    Some(frame.id)
                } else {
                    None
                }
            }
            SplitNode::Split { first, second, .. } => first
                .find_frame_by_name(name)
                .or_else(|| second.find_frame_by_name(name)),
        }
    }

    /// Get the id of the first leaf frame (leftmost/topmost).
    pub fn first_frame_id(&self) -> FrameId {
        match self {
            SplitNode::Leaf(frame) => frame.id,
            SplitNode::Split { first, .. } => first.first_frame_id(),
        }
    }

    /// Calculate geometry for all frames given the available area.
    /// Returns a list of (FrameId, Rect) for each leaf.
    pub fn calculate_layout(&self, area: Rect, gap: i32) -> Vec<(FrameId, Rect)> {
        let mut result = Vec::new();
        self.layout_recursive(area, gap, &mut result);
        result
    }

    fn layout_recursive(&self, area: Rect, gap: i32, out: &mut Vec<(FrameId, Rect)>) {
        match self {
            SplitNode::Leaf(frame) => {
                out.push((frame.id, area));
            }
            SplitNode::Split {
                orientation,
                ratio,
                first,
                second,
            } => match orientation {
                Orientation::Horizontal => {
                    let first_width = ((area.width - gap) as f32 * ratio) as i32;
                    let second_width = area.width - gap - first_width;
                    let first_area = Rect::new(area.x, area.y, first_width, area.height);
                    let second_area = Rect::new(
                        area.x + first_width + gap,
                        area.y,
                        second_width,
                        area.height,
                    );
                    first.layout_recursive(first_area, gap, out);
                    second.layout_recursive(second_area, gap, out);
                }
                Orientation::Vertical => {
                    let first_height = ((area.height - gap) as f32 * ratio) as i32;
                    let second_height = area.height - gap - first_height;
                    let first_area = Rect::new(area.x, area.y, area.width, first_height);
                    let second_area = Rect::new(
                        area.x,
                        area.y + first_height + gap,
                        area.width,
                        second_height,
                    );
                    first.layout_recursive(first_area, gap, out);
                    second.layout_recursive(second_area, gap, out);
                }
            },
        }
    }

    /// Collect all frame IDs in tree order.
    pub fn all_frame_ids(&self) -> Vec<FrameId> {
        let mut ids = Vec::new();
        self.collect_frame_ids(&mut ids);
        ids
    }

    fn collect_frame_ids(&self, out: &mut Vec<FrameId>) {
        match self {
            SplitNode::Leaf(frame) => out.push(frame.id),
            SplitNode::Split { first, second, .. } => {
                first.collect_frame_ids(out);
                second.collect_frame_ids(out);
            }
        }
    }

    /// Find the neighbor frame in a given direction from the target frame.
    /// Uses the calculated layout geometry to determine adjacency.
    pub fn find_neighbor(
        &self,
        target_id: FrameId,
        direction: Direction,
        area: Rect,
        gap: i32,
    ) -> Option<FrameId> {
        let layout = self.calculate_layout(area, gap);
        let target_rect = layout
            .iter()
            .find(|(id, _)| *id == target_id)
            .map(|(_, r)| *r)?;

        let mut best: Option<(FrameId, i32)> = None;

        for &(id, rect) in &layout {
            if id == target_id {
                continue;
            }

            let (is_candidate, distance) = match direction {
                Direction::Left => {
                    let overlap = vertical_overlap(target_rect, rect);
                    (
                        rect.x + rect.width <= target_rect.x && overlap > 0,
                        target_rect.x - (rect.x + rect.width),
                    )
                }
                Direction::Right => {
                    let overlap = vertical_overlap(target_rect, rect);
                    (
                        rect.x >= target_rect.x + target_rect.width && overlap > 0,
                        rect.x - (target_rect.x + target_rect.width),
                    )
                }
                Direction::Up => {
                    let overlap = horizontal_overlap(target_rect, rect);
                    (
                        rect.y + rect.height <= target_rect.y && overlap > 0,
                        target_rect.y - (rect.y + rect.height),
                    )
                }
                Direction::Down => {
                    let overlap = horizontal_overlap(target_rect, rect);
                    (
                        rect.y >= target_rect.y + target_rect.height && overlap > 0,
                        rect.y - (target_rect.y + target_rect.height),
                    )
                }
            };

            if is_candidate {
                if let Some((_, best_dist)) = best {
                    if distance < best_dist {
                        best = Some((id, distance));
                    }
                } else {
                    best = Some((id, distance));
                }
            }
        }

        best.map(|(id, _)| id)
    }

    /// Adjust the split ratio of the nearest ancestor split in the given direction.
    pub fn resize_frame(&mut self, target_id: FrameId, direction: Direction, delta: f32) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split {
                orientation,
                ratio,
                first,
                second,
            } => {
                let axis_matches = match direction {
                    Direction::Left | Direction::Right => *orientation == Orientation::Horizontal,
                    Direction::Up | Direction::Down => *orientation == Orientation::Vertical,
                };

                if axis_matches {
                    let in_first = first.contains_frame(target_id);
                    let in_second = second.contains_frame(target_id);

                    if in_first || in_second {
                        // Try to recurse first
                        let handled = if in_first {
                            first.resize_frame(target_id, direction, delta)
                        } else {
                            second.resize_frame(target_id, direction, delta)
                        };

                        if !handled {
                            // Absolute direction: Up/Left always shrinks the
                            // first child (moves boundary up/left).
                            // Down/Right always grows first child.
                            let adjustment = match direction {
                                Direction::Left | Direction::Up => -delta,
                                Direction::Right | Direction::Down => delta,
                            };
                            *ratio = (*ratio + adjustment).clamp(0.1, 0.9);
                            return true;
                        }
                        return handled;
                    }
                }

                // Axis doesn't match or target not found, recurse
                first.resize_frame(target_id, direction, delta)
                    || second.resize_frame(target_id, direction, delta)
            }
        }
    }

    /// Toggle the orientation of the parent split containing the target frame.
    pub fn toggle_orientation(&mut self, target_id: FrameId) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split {
                orientation,
                first,
                second,
                ..
            } => {
                let in_first = first.contains_frame(target_id);
                let in_second = second.contains_frame(target_id);
                if in_first || in_second {
                    // Try children first (deepest match wins)
                    let handled =
                        first.toggle_orientation(target_id) || second.toggle_orientation(target_id);
                    if !handled {
                        // We are the direct parent — toggle
                        *orientation = match *orientation {
                            Orientation::Horizontal => Orientation::Vertical,
                            Orientation::Vertical => Orientation::Horizontal,
                        };
                        return true;
                    }
                    return handled;
                }
                false
            }
        }
    }

    /// Adjust the split ratio closest to the pointer position.
    /// `area` is the output rect, `px`/`py` is the pointer position.
    pub fn adjust_ratio_at(
        &mut self,
        area: Rect,
        px: i32,
        py: i32,
        dx: f32,
        dy: f32,
        gap: i32,
    ) -> bool {
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split {
                orientation,
                ratio,
                first,
                second,
            } => {
                let half_gap = gap / 2;
                // Compute the boundary position for this split
                let boundary = match orientation {
                    Orientation::Horizontal => {
                        let first_w = ((area.width - gap) as f32 * *ratio) as i32;
                        area.x + first_w + half_gap
                    }
                    Orientation::Vertical => {
                        let first_h = ((area.height - gap) as f32 * *ratio) as i32;
                        area.y + first_h + half_gap
                    }
                };

                // Distance from pointer to this boundary
                let dist = match orientation {
                    Orientation::Horizontal => (px - boundary).abs(),
                    Orientation::Vertical => (py - boundary).abs(),
                };

                // Compute child areas
                let (first_area, second_area) = match orientation {
                    Orientation::Horizontal => {
                        let fw = ((area.width - gap) as f32 * *ratio) as i32;
                        let sw = area.width - gap - fw;
                        (
                            Rect::new(area.x, area.y, fw, area.height),
                            Rect::new(area.x + fw + gap, area.y, sw, area.height),
                        )
                    }
                    Orientation::Vertical => {
                        let fh = ((area.height - gap) as f32 * *ratio) as i32;
                        let sh = area.height - gap - fh;
                        (
                            Rect::new(area.x, area.y, area.width, fh),
                            Rect::new(area.x, area.y + fh + gap, area.width, sh),
                        )
                    }
                };

                // Try children first — they might have a closer boundary
                let child_handled =
                    first.adjust_ratio_at(first_area, px, py, dx, dy, gap)
                        || second.adjust_ratio_at(second_area, px, py, dx, dy, gap);

                if child_handled {
                    return true;
                }

                // No child handled it — check if we should
                // Only adjust if pointer is within threshold of our boundary
                let threshold = gap + 40;
                if dist < threshold {
                    let delta = match orientation {
                        Orientation::Horizontal => dx,
                        Orientation::Vertical => dy,
                    };
                    if delta.abs() > 0.0001 {
                        *ratio = (*ratio + delta).clamp(0.1, 0.9);
                        return true;
                    }
                }

                false
            }
        }
    }

    /// Legacy: adjust ratio by frame id (for keyboard resize mode).
    pub fn adjust_ratio(&mut self, target_id: FrameId, dx: f32, dy: f32) -> bool {
        self.adjust_ratio_axis(target_id, Orientation::Horizontal, dx)
            | self.adjust_ratio_axis(target_id, Orientation::Vertical, dy)
    }

    fn adjust_ratio_axis(&mut self, target_id: FrameId, axis: Orientation, delta: f32) -> bool {
        if delta.abs() < 0.0001 {
            return false;
        }
        match self {
            SplitNode::Leaf(_) => false,
            SplitNode::Split {
                orientation,
                ratio,
                first,
                second,
            } => {
                let in_first = first.contains_frame(target_id);
                let in_second = second.contains_frame(target_id);

                if !(in_first || in_second) {
                    return first.adjust_ratio_axis(target_id, axis, delta)
                        || second.adjust_ratio_axis(target_id, axis, delta);
                }

                let handled = if in_first {
                    first.adjust_ratio_axis(target_id, axis, delta)
                } else {
                    second.adjust_ratio_axis(target_id, axis, delta)
                };

                if !handled && *orientation == axis {
                    *ratio = (*ratio + delta).clamp(0.1, 0.9);
                    return true;
                }
                handled
            }
        }
    }

    fn contains_frame(&self, target_id: FrameId) -> bool {
        match self {
            SplitNode::Leaf(frame) => frame.id == target_id,
            SplitNode::Split { first, second, .. } => {
                first.contains_frame(target_id) || second.contains_frame(target_id)
            }
        }
    }
}

/// Direction for focus/move navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "left" => Some(Direction::Left),
            "right" => Some(Direction::Right),
            "up" => Some(Direction::Up),
            "down" => Some(Direction::Down),
            _ => None,
        }
    }
}

pub fn vertical_overlap(a: Rect, b: Rect) -> i32 {
    let top = a.y.max(b.y);
    let bottom = (a.y + a.height).min(b.y + b.height);
    (bottom - top).max(0)
}

pub fn horizontal_overlap(a: Rect, b: Rect) -> i32 {
    let left = a.x.max(b.x);
    let right = (a.x + a.width).min(b.x + b.width);
    (right - left).max(0)
}

impl fmt::Display for SplitNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_indent(f, 0)
    }
}

impl SplitNode {
    fn fmt_indent(&self, f: &mut fmt::Formatter<'_>, indent: usize) -> fmt::Result {
        let pad = " ".repeat(indent);
        match self {
            SplitNode::Leaf(frame) => {
                let name = frame.name.as_deref().unwrap_or("(unnamed)");
                write!(
                    f,
                    "{pad}Frame[{}] {name} ({} windows)",
                    frame.id.0,
                    frame.windows.len()
                )
            }
            SplitNode::Split {
                orientation,
                ratio,
                first,
                second,
            } => {
                let dir = match orientation {
                    Orientation::Horizontal => "H",
                    Orientation::Vertical => "V",
                };
                writeln!(f, "{pad}Split({dir}, {ratio:.2})")?;
                first.fmt_indent(f, indent + 2)?;
                writeln!(f)?;
                second.fmt_indent(f, indent + 2)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_frame_layout() {
        let tree = SplitNode::single_frame();
        let layout = tree.calculate_layout(Rect::new(0, 0, 1920, 1080), 4);
        assert_eq!(layout.len(), 1);
        assert_eq!(layout[0].1, Rect::new(0, 0, 1920, 1080));
    }

    #[test]
    fn test_hsplit_layout() {
        let tree = SplitNode::hsplit(0.5);
        let layout = tree.calculate_layout(Rect::new(0, 0, 1920, 1080), 4);
        assert_eq!(layout.len(), 2);
        assert_eq!(layout[0].1.width, 958); // (1920-4)/2 = 958
        assert_eq!(layout[1].1.width, 958);
        assert_eq!(layout[1].1.x, 962); // 958 + 4
    }

    #[test]
    fn test_split_frame() {
        let mut tree = SplitNode::single_frame();
        let first_id = tree.first_frame_id();
        let new_id = tree.split_frame(first_id, Orientation::Horizontal, 0.5);
        assert!(new_id.is_some());
        let layout = tree.calculate_layout(Rect::new(0, 0, 1920, 1080), 4);
        assert_eq!(layout.len(), 2);
    }

    #[test]
    fn test_remove_empty_frame() {
        let mut tree = SplitNode::hsplit(0.5);
        let ids = tree.all_frame_ids();
        assert_eq!(ids.len(), 2);
        // Remove second frame (it's empty)
        assert!(tree.remove_frame(ids[1]));
        // Tree should now be a single leaf
        let layout = tree.calculate_layout(Rect::new(0, 0, 1920, 1080), 4);
        assert_eq!(layout.len(), 1);
    }

    #[test]
    fn test_frame_tabbing() {
        let mut frame = Frame::new();
        assert!(frame.is_empty());
        frame.add_window(WindowRef {
            window_id: 1,
            app_id: "foot".to_string(),
            title: "terminal".to_string(),
        });
        frame.add_window(WindowRef {
            window_id: 2,
            app_id: "firefox".to_string(),
            title: "browser".to_string(),
        });
        assert_eq!(frame.active_tab, 1); // last added is active
        frame.prev_tab();
        assert_eq!(frame.active_tab, 0);
        frame.next_tab();
        assert_eq!(frame.active_tab, 1);
        frame.remove_window(2);
        assert_eq!(frame.active_tab, 0);
        assert_eq!(frame.window_count(), 1);
    }

    #[test]
    fn test_neighbor_finding() {
        let tree = SplitNode::hsplit(0.5);
        let ids = tree.all_frame_ids();
        let area = Rect::new(0, 0, 1920, 1080);
        // From first frame, right neighbor should be second frame
        let neighbor = tree.find_neighbor(ids[0], Direction::Right, area, 4);
        assert_eq!(neighbor, Some(ids[1]));
        // From second frame, left neighbor should be first frame
        let neighbor = tree.find_neighbor(ids[1], Direction::Left, area, 4);
        assert_eq!(neighbor, Some(ids[0]));
        // No vertical neighbors in a horizontal split
        let neighbor = tree.find_neighbor(ids[0], Direction::Up, area, 4);
        assert_eq!(neighbor, None);
    }

    #[test]
    fn test_adjust_ratio_vsplit_from_second_child() {
        // Vertical split: top frame (first) and bottom frame (second)
        let mut tree = SplitNode::vsplit(0.5);
        let ids = tree.all_frame_ids();
        let bottom = ids[1];

        // Dragging up from bottom frame (negative dy) should shrink top (decrease ratio)
        let initial_ratio = 0.5;
        tree.adjust_ratio(bottom, 0.0, -0.1);
        match &tree {
            SplitNode::Split { ratio, .. } => {
                assert!(
                    *ratio < initial_ratio,
                    "Dragging up from bottom should decrease ratio (shrink top), got {}",
                    ratio
                );
            }
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn test_adjust_ratio_vsplit_from_first_child() {
        // Dragging down from top frame (positive dy) should grow top (increase ratio)
        let mut tree = SplitNode::vsplit(0.5);
        let ids = tree.all_frame_ids();
        let top = ids[0];

        tree.adjust_ratio(top, 0.0, 0.1);
        match &tree {
            SplitNode::Split { ratio, .. } => {
                assert!(
                    *ratio > 0.5,
                    "Dragging down from top should increase ratio (grow top), got {}",
                    ratio
                );
            }
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn test_adjust_ratio_hsplit_from_second_child() {
        // Horizontal split: left (first) and right (second)
        // Dragging left from right frame (negative dx) should shrink left (decrease ratio)
        let mut tree = SplitNode::hsplit(0.5);
        let ids = tree.all_frame_ids();
        let right = ids[1];

        tree.adjust_ratio(right, -0.1, 0.0);
        match &tree {
            SplitNode::Split { ratio, .. } => {
                assert!(
                    *ratio < 0.5,
                    "Dragging left from right should decrease ratio (shrink left), got {}",
                    ratio
                );
            }
            _ => panic!("Expected split"),
        }
    }
}
