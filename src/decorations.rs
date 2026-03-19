//! Frame decorations: tab bars drawn as per-window decoration surfaces.
//!
//! Uses river_decoration_v1 (via get_decoration_above) to attach a tab bar
//! surface above each window. The decoration moves with the window automatically.

use std::collections::HashMap;
use std::os::unix::io::{AsFd, AsRawFd, FromRawFd};

use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_compositor::WlCompositor,
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::Proxy;
use wayland_client::QueueHandle;

use crate::layout::{Frame, FrameId, Rect};
use crate::protocol::river_decoration_v1::RiverDecorationV1;
use crate::protocol::river_node_v1::RiverNodeV1;
use crate::protocol::river_shell_surface_v1::RiverShellSurfaceV1;
use crate::protocol::river_window_manager_v1::RiverWindowManagerV1;
use crate::protocol::river_window_v1::RiverWindowV1;
use crate::wm::AppData;

/// Height of the tab bar in pixels.
pub const TAB_BAR_HEIGHT: i32 = 24;

/// ARGB8888 colors (premultiplied alpha).
const COLOR_TAB_ACTIVE: u32 = 0xFF4C7899;
const COLOR_TAB_INACTIVE: u32 = 0xFF222222;
const COLOR_FOCUSED_ACTIVE: u32 = 0xFF5294C4;
const COLOR_SEPARATOR: u32 = 0xFF888888;

/// A decoration attached above a specific window.
pub struct WindowDecoration {
    pub surface: WlSurface,
    pub decoration: RiverDecorationV1,
    pub viewport: Option<crate::protocol::wp_viewport::WpViewport>,
    pub buffer: Option<WlBuffer>,
    pub pool: Option<WlShmPool>,
    /// Last drawn width (to avoid unnecessary redraws).
    pub last_width: i32,
    pub last_hash: u64,
    /// Scale the buffer was last rendered at.
    pub last_scale: i32,
}

impl std::fmt::Debug for WindowDecoration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowDecoration")
            .field("last_width", &self.last_width)
            .finish()
    }
}

/// Manages decorations keyed by window id.
#[derive(Debug, Default)]
pub struct DecorationManager {
    pub decorations: HashMap<u64, WindowDecoration>,
    /// Reverse map: wl_surface protocol id → window_id (for click-to-tab)
    pub surface_to_window: HashMap<u32, u64>,
}

impl DecorationManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw a tab bar above a window. Called during the render phase.
    ///
    /// `window_id` - the managed window's id
    /// `window_proxy` - the river window to attach the decoration to
    /// `frame` - the frame this window belongs to
    /// `frame_width` - width of the frame area
    /// `is_focused_frame` - whether this frame has focus
    #[allow(clippy::too_many_arguments)]
    pub fn draw_tab_bar(
        &mut self,
        window_id: u64,
        window_proxy: &RiverWindowV1,
        frame: &Frame,
        frame_width: i32,
        is_focused_frame: bool,
        is_bound: bool,
        fractional_scale: f64,
        shm: &WlShm,
        compositor: &WlCompositor,
        viewporter: Option<&crate::protocol::wp_viewporter::WpViewporter>,
        qh: &QueueHandle<AppData>,
    ) {
        // TODO: fix scale detection timing. For now hardcode 2x for HiDPI.
        let scale = if fractional_scale > 1.0 {
            fractional_scale
        } else {
            2.0
        };
        let buffer_scale = scale.ceil() as i32;
        let width = (frame_width as f64 * scale).round() as i32;
        let height = (TAB_BAR_HEIGHT as f64 * scale).round() as i32;

        if width <= 0 || height <= 0 {
            return;
        }

        // Compute a simple hash to avoid unnecessary redraws
        let content_hash = compute_hash(frame, is_focused_frame, width)
            ^ (buffer_scale as u64 * 0x9e3779b9)
            ^ (is_bound as u64 * 0x517cc1b7);

        let surface_to_window = &mut self.surface_to_window;
        let dec = self.decorations.entry(window_id).or_insert_with(|| {
            let surface = compositor.create_surface(qh, ());
            surface_to_window.insert(surface.id().protocol_id(), window_id);
            let decoration = window_proxy.get_decoration_above(&surface, qh, ());
            let viewport = viewporter.map(|vp| vp.get_viewport(&surface, qh, ()));
            WindowDecoration {
                surface,
                decoration,
                viewport,
                buffer: None,
                pool: None,
                last_width: 0,
                last_hash: 0,
                last_scale: 0,
            }
        });

        // Use viewport for pixel-perfect fractional scaling:
        // buffer_scale=1, render at exact physical resolution,
        // viewport sets the logical destination size
        if let Some(ref vp) = dec.viewport {
            dec.surface.set_buffer_scale(1);
            vp.set_destination(frame_width, TAB_BAR_HEIGHT);
        } else {
            dec.surface.set_buffer_scale(buffer_scale);
        }

        // Position above the window
        dec.decoration.set_offset(0, -TAB_BAR_HEIGHT);

        let needs_redraw = dec.last_width != width
            || dec.last_hash != content_hash
            || dec.last_scale != buffer_scale;

        if needs_redraw {
            // Destroy old buffer/pool
            if let Some(buf) = dec.buffer.take() {
                buf.destroy();
            }
            if let Some(pool) = dec.pool.take() {
                pool.destroy();
            }

            let stride = width * 4;
            let size = stride * height;

            let fd = match create_shm_file(size as usize) {
                Ok(fd) => fd,
                Err(e) => {
                    log::error!("Failed to create shm file: {e}");
                    return;
                }
            };

            // Map and draw
            let map = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    size as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd.as_fd().as_raw_fd(),
                    0,
                )
            };
            if map == libc::MAP_FAILED {
                log::error!("mmap failed");
                return;
            }

            let pixels = unsafe {
                std::slice::from_raw_parts_mut(map as *mut u32, (width * height) as usize)
            };

            draw_tab_bar_pixels(
                pixels,
                width as usize,
                height as usize,
                frame,
                is_focused_frame,
                is_bound,
            );

            unsafe {
                libc::munmap(map, size as usize);
            }

            let pool = shm.create_pool(fd.as_fd(), size, qh, ());
            let buffer =
                pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());

            dec.surface.attach(Some(&buffer), 0, 0);
            dec.surface.damage_buffer(0, 0, width, height);

            dec.buffer = Some(buffer);
            dec.pool = Some(pool);
            dec.last_width = width;
            dec.last_hash = content_hash;
            dec.last_scale = buffer_scale;
        }

        dec.decoration.sync_next_commit();
        dec.surface.commit();
    }

    /// Given a surface protocol id and click x coordinate, return
    /// (window_id, tab_index) if this is a tab bar click.
    #[allow(dead_code)]
    pub fn tab_click(
        &self,
        surface_id: u32,
        surface_x: f64,
        num_tabs: usize,
        frame_width: i32,
    ) -> Option<(u64, usize)> {
        let window_id = self.surface_to_window.get(&surface_id)?;
        if num_tabs == 0 || frame_width <= 0 {
            return None;
        }
        let tab_width = frame_width as f64 / num_tabs as f64;
        let tab_index = (surface_x / tab_width) as usize;
        let tab_index = tab_index.min(num_tabs - 1);
        Some((*window_id, tab_index))
    }

    /// Remove decoration for a window that's gone.
    #[allow(dead_code)]
    pub fn remove(&mut self, window_id: u64) {
        if let Some(dec) = self.decorations.remove(&window_id) {
            if let Some(buf) = dec.buffer {
                buf.destroy();
            }
            if let Some(pool) = dec.pool {
                pool.destroy();
            }
            dec.decoration.destroy();
            dec.surface.destroy();
        }
    }

    /// Remove decorations for windows not in the provided set.
    #[allow(dead_code)]
    pub fn cleanup(&mut self, active_window_ids: &[u64]) {
        let to_remove: Vec<u64> = self
            .decorations
            .keys()
            .filter(|id| !active_window_ids.contains(id))
            .copied()
            .collect();
        for id in to_remove {
            self.remove(id);
        }
    }
}

// ── Empty frame indicators using shell surfaces ──────────────────────────

const COLOR_EMPTY_FOCUSED: u32 = 0xFF4C7899;
const COLOR_EMPTY_UNFOCUSED: u32 = 0xFF444444;

/// A shell surface indicator for an empty frame.
pub struct EmptyFrameIndicator {
    pub surface: WlSurface,
    pub shell_surface: RiverShellSurfaceV1,
    pub node: RiverNodeV1,
    pub buffer: Option<WlBuffer>,
    pub pool: Option<WlShmPool>,
    pub last_width: i32,
    pub last_height: i32,
    pub last_focused: bool,
}

impl std::fmt::Debug for EmptyFrameIndicator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmptyFrameIndicator").finish()
    }
}

/// Manages empty frame indicators.
#[derive(Debug, Default)]
pub struct EmptyFrameManager {
    pub indicators: HashMap<FrameId, EmptyFrameIndicator>,
}

impl EmptyFrameManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw an empty frame border indicator.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_empty_frame(
        &mut self,
        frame_id: FrameId,
        rect: Rect,
        is_focused: bool,
        shm: &WlShm,
        compositor: &WlCompositor,
        wm_proxy: &RiverWindowManagerV1,
        qh: &QueueHandle<AppData>,
    ) {
        let width = rect.width;
        let height = rect.height;
        if width <= 0 || height <= 0 {
            return;
        }

        let ind = self.indicators.entry(frame_id).or_insert_with(|| {
            let surface = compositor.create_surface(qh, ());
            let shell_surface = wm_proxy.get_shell_surface(&surface, qh, ());
            let node = shell_surface.get_node(qh, ());
            EmptyFrameIndicator {
                surface,
                shell_surface,
                node,
                buffer: None,
                pool: None,
                last_width: 0,
                last_height: 0,
                last_focused: false,
            }
        });

        let needs_redraw =
            ind.last_width != width || ind.last_height != height || ind.last_focused != is_focused;

        if needs_redraw {
            if let Some(buf) = ind.buffer.take() {
                buf.destroy();
            }
            if let Some(pool) = ind.pool.take() {
                pool.destroy();
            }

            let stride = width * 4;
            let size = stride * height;

            let fd = match create_shm_file(size as usize) {
                Ok(fd) => fd,
                Err(e) => {
                    log::error!("Failed to create shm file for empty frame: {e}");
                    return;
                }
            };

            let map = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    size as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd.as_fd().as_raw_fd(),
                    0,
                )
            };
            if map == libc::MAP_FAILED {
                return;
            }

            let pixels = unsafe {
                std::slice::from_raw_parts_mut(map as *mut u32, (width * height) as usize)
            };

            // Draw border with a barely-visible interior fill.
            // The fill must be non-transparent (alpha > 0) so the surface
            // receives pointer input events (Wayland ignores fully transparent areas).
            let border_w = 2usize;
            let color = if is_focused {
                COLOR_EMPTY_FOCUSED
            } else {
                COLOR_EMPTY_UNFOCUSED
            };
            // 0x01000000 = alpha=1 (out of 255), practically invisible but receives input
            let fill = 0x01000000u32;
            let w = width as usize;
            let h = height as usize;
            let radius = 6usize;
            for y in 0..h {
                for x in 0..w {
                    // Round all four corners
                    let in_corner = (x < radius && y < radius && !in_rounded_corner(x, y, radius))
                        || (w - 1 - x < radius
                            && y < radius
                            && !in_rounded_corner(w - 1 - x, y, radius))
                        || (x < radius
                            && h - 1 - y < radius
                            && !in_rounded_corner(x, h - 1 - y, radius))
                        || (w - 1 - x < radius
                            && h - 1 - y < radius
                            && !in_rounded_corner(w - 1 - x, h - 1 - y, radius));
                    if in_corner {
                        pixels[y * w + x] = 0x00000000;
                        continue;
                    }
                    let on_border =
                        y < border_w || y >= h - border_w || x < border_w || x >= w - border_w;
                    pixels[y * w + x] = if on_border { color } else { fill };
                }
            }

            unsafe {
                libc::munmap(map, size as usize);
            }

            let pool = shm.create_pool(fd.as_fd(), size, qh, ());
            let buffer =
                pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());

            ind.surface.attach(Some(&buffer), 0, 0);
            ind.surface.damage_buffer(0, 0, width, height);

            ind.buffer = Some(buffer);
            ind.pool = Some(pool);
            ind.last_width = width;
            ind.last_height = height;
            ind.last_focused = is_focused;
        }

        ind.node.set_position(rect.x, rect.y);
        ind.node.place_top();
        ind.shell_surface.sync_next_commit();
        ind.surface.commit();
    }

    /// Remove indicators for frames that no longer exist or are no longer empty.
    pub fn cleanup(&mut self, empty_frame_ids: &[FrameId]) {
        let to_remove: Vec<FrameId> = self
            .indicators
            .keys()
            .filter(|id| !empty_frame_ids.contains(id))
            .copied()
            .collect();
        for id in to_remove {
            if let Some(ind) = self.indicators.remove(&id) {
                if let Some(buf) = ind.buffer {
                    buf.destroy();
                }
                if let Some(pool) = ind.pool {
                    pool.destroy();
                }
                ind.node.destroy();
                ind.shell_surface.destroy();
                ind.surface.destroy();
            }
        }
    }
}

fn compute_hash(frame: &Frame, is_focused: bool, width: i32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    frame.active_tab.hash(&mut hasher);
    frame.windows.len().hash(&mut hasher);
    is_focused.hash(&mut hasher);
    width.hash(&mut hasher);
    for w in &frame.windows {
        w.window_id.hash(&mut hasher);
        w.title.hash(&mut hasher);
        w.app_id.hash(&mut hasher);
    }
    hasher.finish()
}

/// Draw the tab bar pixels.
fn draw_tab_bar_pixels(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    frame: &Frame,
    is_focused: bool,
    is_bound: bool,
) {
    let num_tabs = frame.windows.len();
    if num_tabs == 0 {
        // Shouldn't happen (only called for windows that exist) but fill transparent
        pixels.fill(0x00000000);
        return;
    }

    let tab_width = width / num_tabs;

    for tab_idx in 0..num_tabs {
        let is_active = tab_idx == frame.active_tab;
        let bg = if is_active && is_focused {
            COLOR_FOCUSED_ACTIVE
        } else if is_active {
            COLOR_TAB_ACTIVE
        } else {
            COLOR_TAB_INACTIVE
        };

        let x_start = tab_idx * tab_width;
        let x_end = if tab_idx == num_tabs - 1 {
            width
        } else {
            (tab_idx + 1) * tab_width
        };

        let radius = (height / 6).max(2); // corner radius

        for y in 0..height {
            for x in x_start..x_end {
                // Round top corners of first and last tab
                let in_corner = (tab_idx == 0
                    && x - x_start < radius
                    && y < radius
                    && !in_rounded_corner(x - x_start, y, radius))
                    || (tab_idx == num_tabs - 1
                        && x_end - 1 - x < radius
                        && y < radius
                        && !in_rounded_corner(x_end - 1 - x, y, radius));
                if in_corner {
                    pixels[y * width + x] = 0x00000000;
                    continue;
                }

                let color = if x == x_end - 1 && tab_idx < num_tabs - 1 {
                    COLOR_SEPARATOR
                } else if y >= height - 2 && is_active {
                    if is_focused {
                        0xFFFFFFFF
                    } else {
                        0xFF888888
                    }
                } else {
                    bg
                };
                pixels[y * width + x] = color;
            }
        }

        // Draw title text (with binding indicator)
        if let Some(win_ref) = frame.windows.get(tab_idx) {
            let base_title = if win_ref.title.is_empty() {
                &win_ref.app_id
            } else {
                &win_ref.title
            };
            let title = if is_bound && tab_idx == 0 {
                format!("⊙ {base_title}")
            } else {
                base_title.to_string()
            };
            let title = &title;
            let text_color = if is_active { 0xFFFFFFFF } else { 0xFFAAAAAA };
            let padding = 4 * height / TAB_BAR_HEIGHT as usize;
            draw_text(
                pixels,
                width,
                height,
                x_start + padding,
                padding,
                title,
                text_color,
                x_end.saturating_sub(padding),
            );
        }
    }
}

// ── Font rendering via Cairo + Pango (same stack as waybar/GTK) ──────────

/// Render text using cairo+pango for identical quality to waybar.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    pixels: &mut [u32],
    stride: usize,
    height: usize,
    x0: usize,
    _y0: usize,
    text: &str,
    color: u32,
    x_max: usize,
) {
    let width = x_max - x0;
    if width == 0 || height == 0 {
        return;
    }

    let color_r = ((color >> 16) & 0xFF) as f64 / 255.0;
    let color_g = ((color >> 8) & 0xFF) as f64 / 255.0;
    let color_b = (color & 0xFF) as f64 / 255.0;

    // Create a cairo image surface backed by our pixel buffer region
    // We render into a temporary buffer then copy to the right position
    let mut surface =
        match cairo::ImageSurface::create(cairo::Format::ARgb32, width as i32, height as i32) {
            Ok(s) => s,
            Err(_) => return,
        };
    let cairo_stride = surface.stride() as usize;

    let cr = match cairo::Context::new(&surface) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Set up pango layout with font size in pixels (not points)
    let layout = pangocairo::functions::create_layout(&cr);
    let font_size_px = (height as f64 * 0.58).max(10.0);
    let mut font_desc = pango::FontDescription::from_string("Noto Sans Bold");
    font_desc.set_absolute_size(font_size_px * pango::SCALE as f64);
    layout.set_font_description(Some(&font_desc));
    layout.set_text(text);
    layout.set_width(width as i32 * pango::SCALE);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_single_paragraph_mode(true);

    // Vertically center
    let (_, text_height) = layout.pixel_size();
    let y_offset = ((height as i32 - text_height) / 2).max(0) as f64;

    cr.move_to(0.0, y_offset);
    cr.set_source_rgb(color_r, color_g, color_b);
    pangocairo::functions::show_layout(&cr, &layout);

    drop(cr);

    // Copy cairo buffer to our pixel buffer
    let surface_data = match surface.data() {
        Ok(d) => d,
        Err(_) => return,
    };

    for row in 0..height {
        for col in 0..width {
            let src_offset = row * cairo_stride + col * 4;
            if src_offset + 3 >= surface_data.len() {
                continue;
            }
            // Cairo uses native-endian ARGB (on little-endian: BGRA in memory)
            let b = surface_data[src_offset] as u32;
            let g = surface_data[src_offset + 1] as u32;
            let r = surface_data[src_offset + 2] as u32;
            let a = surface_data[src_offset + 3] as u32;

            if a == 0 {
                continue;
            }

            let dst_x = x0 + col;
            let dst_y = row;
            if dst_x >= stride || dst_y >= pixels.len() / stride {
                continue;
            }

            // Alpha blend onto existing background
            let alpha = a as f32 / 255.0;
            let bg = pixels[dst_y * stride + dst_x];
            let bg_r = ((bg >> 16) & 0xFF) as f32;
            let bg_g = ((bg >> 8) & 0xFF) as f32;
            let bg_b = (bg & 0xFF) as f32;

            let out_r = (r as f32 * alpha / alpha.max(0.01) * alpha + bg_r * (1.0 - alpha)) as u32;
            let out_g = (g as f32 * alpha / alpha.max(0.01) * alpha + bg_g * (1.0 - alpha)) as u32;
            let out_b = (b as f32 * alpha / alpha.max(0.01) * alpha + bg_b * (1.0 - alpha)) as u32;

            pixels[dst_y * stride + dst_x] =
                0xFF000000 | (out_r.min(255) << 16) | (out_g.min(255) << 8) | out_b.min(255);
        }
    }
}

// ── Drag preview overlay ─────────────────────────────────────────────────

/// Visual overlay showing drop zones during window drag.
#[derive(Debug, Default)]
pub struct DragPreview {
    surface: Option<WlSurface>,
    shell_surface: Option<RiverShellSurfaceV1>,
    node: Option<RiverNodeV1>,
    buffer: Option<WlBuffer>,
    pool: Option<WlShmPool>,
    visible: bool,
}

/// Color with alpha for preview zones (ARGB premultiplied)
const PREVIEW_TAB: u32 = 0x4089b4fa; // blue, semi-transparent
const PREVIEW_SPLIT: u32 = 0x40a6e3a1; // green, semi-transparent
const PREVIEW_BORDER: u32 = 0x8089b4fa; // blue, more opaque

impl DragPreview {
    pub fn show(
        &mut self,
        rect: &Rect,
        zone: &crate::pointer_ops::DropZone,
        compositor: &WlCompositor,
        wm_proxy: &RiverWindowManagerV1,
        shm: &WlShm,
        qh: &QueueHandle<crate::wm::AppData>,
    ) {
        use crate::pointer_ops::DropZone;

        // Create surface if needed
        if self.surface.is_none() {
            let surface = compositor.create_surface(qh, ());
            let shell_surface = wm_proxy.get_shell_surface(&surface, qh, ());
            let node = shell_surface.get_node(qh, ());
            self.surface = Some(surface);
            self.shell_surface = Some(shell_surface);
            self.node = Some(node);
        }

        // Compute the preview area based on zone
        let (px, py, pw, ph) = match zone {
            DropZone::Tab => (rect.x, rect.y, rect.width, rect.height),
            DropZone::Top => (rect.x, rect.y, rect.width, rect.height / 4),
            DropZone::Bottom => (
                rect.x,
                rect.y + rect.height * 3 / 4,
                rect.width,
                rect.height / 4,
            ),
            DropZone::Left => (rect.x, rect.y, rect.width / 4, rect.height),
            DropZone::Right => (
                rect.x + rect.width * 3 / 4,
                rect.y,
                rect.width / 4,
                rect.height,
            ),
        };

        if pw <= 0 || ph <= 0 {
            return;
        }

        let color = match zone {
            DropZone::Tab => PREVIEW_TAB,
            _ => PREVIEW_SPLIT,
        };

        // Destroy old buffer
        if let Some(buf) = self.buffer.take() {
            buf.destroy();
        }
        if let Some(pool) = self.pool.take() {
            pool.destroy();
        }

        let stride = pw * 4;
        let size = stride * ph;
        let fd = match create_shm_file(size as usize) {
            Ok(fd) => fd,
            Err(_) => return,
        };

        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_fd().as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return;
        }

        let pixels = unsafe { std::slice::from_raw_parts_mut(map as *mut u32, (pw * ph) as usize) };

        // Draw the preview: filled area with border
        let w = pw as usize;
        let h = ph as usize;
        let bw = 2usize;
        for y in 0..h {
            for x in 0..w {
                let on_border = y < bw || y >= h - bw || x < bw || x >= w - bw;
                pixels[y * w + x] = if on_border { PREVIEW_BORDER } else { color };
            }
        }

        unsafe {
            libc::munmap(map, size as usize);
        }

        let pool = shm.create_pool(fd.as_fd(), size, qh, ());
        let buffer = pool.create_buffer(0, pw, ph, stride, wl_shm::Format::Argb8888, qh, ());

        if let Some(ref surface) = self.surface {
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, pw, ph);
        }
        if let Some(ref ss) = self.shell_surface {
            ss.sync_next_commit();
        }
        if let Some(ref surface) = self.surface {
            surface.commit();
        }
        if let Some(ref node) = self.node {
            node.set_position(px, py);
            node.place_top();
        }

        self.buffer = Some(buffer);
        self.pool = Some(pool);
        self.visible = true;
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        // Attach null buffer to hide
        if let Some(ref surface) = self.surface {
            surface.attach(None, 0, 0);
        }
        if let Some(ref ss) = self.shell_surface {
            ss.sync_next_commit();
        }
        if let Some(ref surface) = self.surface {
            surface.commit();
        }
        self.visible = false;
    }
}

fn in_rounded_corner(dx: usize, dy: usize, radius: usize) -> bool {
    let rx = radius as f64 - dx as f64 - 0.5;
    let ry = radius as f64 - dy as f64 - 0.5;
    rx * rx + ry * ry <= (radius as f64) * (radius as f64)
}

fn create_shm_file(size: usize) -> std::io::Result<std::os::fd::OwnedFd> {
    let name = std::ffi::CString::new("notion-river-dec").unwrap();
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    let ret = unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}
