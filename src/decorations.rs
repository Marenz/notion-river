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
use wayland_client::{Proxy, QueueHandle};

use crate::layout::Frame;
use crate::protocol::river_decoration_v1::RiverDecorationV1;
use crate::protocol::river_window_v1::RiverWindowV1;
use crate::wm::AppData;

/// Height of the tab bar in pixels.
pub const TAB_BAR_HEIGHT: i32 = 20;

/// ARGB8888 colors (premultiplied alpha).
const COLOR_TAB_ACTIVE: u32 = 0xFF4c7899;
const COLOR_TAB_INACTIVE: u32 = 0xFF222222;
const COLOR_FOCUSED_ACTIVE: u32 = 0xFF5294c4;
const COLOR_SEPARATOR: u32 = 0xFF888888;

/// A decoration attached above a specific window.
pub struct WindowDecoration {
    pub surface: WlSurface,
    pub decoration: RiverDecorationV1,
    pub buffer: Option<WlBuffer>,
    pub pool: Option<WlShmPool>,
    /// Last drawn width (to avoid unnecessary redraws).
    pub last_width: i32,
    pub last_hash: u64,
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
    pub fn draw_tab_bar(
        &mut self,
        window_id: u64,
        window_proxy: &RiverWindowV1,
        frame: &Frame,
        frame_width: i32,
        is_focused_frame: bool,
        shm: &WlShm,
        compositor: &WlCompositor,
        qh: &QueueHandle<AppData>,
    ) {
        let width = frame_width;
        let height = TAB_BAR_HEIGHT;

        if width <= 0 || height <= 0 {
            return;
        }

        // Compute a simple hash to avoid unnecessary redraws
        let content_hash = compute_hash(frame, is_focused_frame, width);

        let dec = self.decorations.entry(window_id).or_insert_with(|| {
            let surface = compositor.create_surface(qh, ());
            let decoration = window_proxy.get_decoration_above(&surface, qh, ());
            WindowDecoration {
                surface,
                decoration,
                buffer: None,
                pool: None,
                last_width: 0,
                last_hash: 0,
            }
        });

        // Position above the window
        dec.decoration.set_offset(0, -TAB_BAR_HEIGHT);

        let needs_redraw = dec.last_width != width || dec.last_hash != content_hash;

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
        }

        dec.decoration.sync_next_commit();
        dec.surface.commit();
    }

    /// Remove decoration for a window that's gone.
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

        for y in 0..height {
            for x in x_start..x_end {
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

        // Draw title text
        if let Some(win_ref) = frame.windows.get(tab_idx) {
            let title = if win_ref.title.is_empty() {
                &win_ref.app_id
            } else {
                &win_ref.title
            };
            let text_color = if is_active { 0xFFFFFFFF } else { 0xFFAAAAAA };
            draw_text(
                pixels,
                width,
                x_start + 4,
                4,
                title,
                text_color,
                x_end.saturating_sub(4),
            );
        }
    }
}

/// Minimal 5x7 bitmap font renderer.
fn draw_text(
    pixels: &mut [u32],
    stride: usize,
    x0: usize,
    y0: usize,
    text: &str,
    color: u32,
    x_max: usize,
) {
    let mut x = x0;
    for ch in text.chars() {
        if x + 6 > x_max {
            break;
        }
        let g = glyph(ch);
        for row in 0..7usize {
            for col in 0..5usize {
                if g[row] & (1 << (4 - col)) != 0 {
                    let px = x + col;
                    let py = y0 + row;
                    if py < pixels.len() / stride && px < stride {
                        pixels[py * stride + px] = color;
                    }
                }
            }
        }
        x += 6;
    }
}

fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_lowercase() {
        'a' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'b' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'c' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'd' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'e' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'f' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'g' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0E],
        'h' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'i' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'j' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'k' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'l' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'm' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'n' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'o' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'p' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'r' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        's' => [0x0E, 0x11, 0x10, 0x0E, 0x01, 0x11, 0x0E],
        't' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'u' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'v' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'w' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'x' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x0E, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x0E],
        ' ' => [0; 7],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04],
        ':' => [0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        '~' => [0x00, 0x00, 0x08, 0x15, 0x02, 0x00, 0x00],
        '@' => [0x0E, 0x11, 0x17, 0x15, 0x16, 0x10, 0x0E],
        _ => [0x0E, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0E], // box
    }
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
