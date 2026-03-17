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
    pub fn draw_tab_bar(
        &mut self,
        window_id: u64,
        window_proxy: &RiverWindowV1,
        frame: &Frame,
        frame_width: i32,
        is_focused_frame: bool,
        fractional_scale: f64,
        shm: &WlShm,
        compositor: &WlCompositor,
        qh: &QueueHandle<AppData>,
    ) {
        let scale = fractional_scale.max(1.0);
        let buffer_scale = scale.ceil() as i32;
        let width = (frame_width as f64 * scale).round() as i32;
        let height = (TAB_BAR_HEIGHT as f64 * scale).round() as i32;

        if width <= 0 || height <= 0 {
            return;
        }

        // Compute a simple hash to avoid unnecessary redraws
        let content_hash =
            compute_hash(frame, is_focused_frame, width) ^ (buffer_scale as u64 * 0x9e3779b9);

        let surface_to_window = &mut self.surface_to_window;
        let dec = self.decorations.entry(window_id).or_insert_with(|| {
            let surface = compositor.create_surface(qh, ());
            surface_to_window.insert(surface.id().protocol_id(), window_id);
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

        // Set buffer scale for HiDPI rendering
        dec.surface.set_buffer_scale(buffer_scale);

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

    /// Given a surface protocol id and click x coordinate, return
    /// (window_id, tab_index) if this is a tab bar click.
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

const COLOR_EMPTY_FOCUSED: u32 = 0xFF4c7899;
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
            for y in 0..h {
                for x in 0..w {
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

// ── Font rendering via FreeType ───────────────────────────────────────────

use std::sync::OnceLock;

static FREETYPE_LIB: OnceLock<freetype::Library> = OnceLock::new();
static FONT_PATH: OnceLock<String> = OnceLock::new();

fn get_font_path() -> &'static str {
    FONT_PATH.get_or_init(|| {
        let paths = [
            "/usr/share/fonts/truetype/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/LiberationMono-Regular.ttf",
        ];
        for path in &paths {
            if std::path::Path::new(path).exists() {
                log::info!("Using font: {path}");
                return path.to_string();
            }
        }
        panic!("No system font found for tab bar rendering");
    })
}

fn get_freetype_lib() -> &'static freetype::Library {
    FREETYPE_LIB.get_or_init(|| freetype::Library::init().expect("Failed to init FreeType"))
}

/// Render text using FreeType with proper hinting and kerning.
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
    let lib = get_freetype_lib();
    let font_path = get_font_path();
    let face = match lib.new_face(font_path, 0) {
        Ok(f) => f,
        Err(e) => {
            log::error!("Failed to load font face: {e}");
            return;
        }
    };

    let font_size_px = (height as f64 * 0.6).max(10.0) as u32;
    let _ = face.set_pixel_sizes(0, font_size_px);

    let color_r = ((color >> 16) & 0xFF) as f32;
    let color_g = ((color >> 8) & 0xFF) as f32;
    let color_b = (color & 0xFF) as f32;

    // Compute baseline: vertically center the text
    let ascender = face.ascender() as f64 * font_size_px as f64 / face.em_size() as f64;
    let descender = face.descender() as f64 * font_size_px as f64 / face.em_size() as f64;
    let text_height = ascender - descender;
    let baseline_y = ((height as f64 - text_height) / 2.0 + ascender) as i32;

    let mut pen_x = (x0 as i64) << 6; // 26.6 fixed point
    let mut prev_glyph_idx = None;

    for ch in text.chars() {
        if (pen_x >> 6) as usize >= x_max {
            break;
        }

        let glyph_idx = match face.get_char_index(ch as usize) {
            Some(idx) => idx,
            None => {
                // Unknown glyph — try load_char for spaces etc.
                let _ = face.load_char(ch as usize, freetype::face::LoadFlag::RENDER);
                pen_x += face.glyph().advance().x as i64;
                continue;
            }
        };

        // Kerning
        if let Some(prev) = prev_glyph_idx {
            if let Ok(delta) =
                face.get_kerning(prev, glyph_idx, freetype::face::KerningMode::KerningDefault)
            {
                pen_x += delta.x as i64;
            }
        }
        prev_glyph_idx = Some(glyph_idx);

        // Use LCD subpixel rendering for maximum sharpness
        if face
            .load_glyph(
                glyph_idx,
                freetype::face::LoadFlag::RENDER | freetype::face::LoadFlag::TARGET_LCD,
            )
            .is_err()
        {
            continue;
        }
        let glyph = face.glyph();
        let bitmap = glyph.bitmap();
        let bmp_left = glyph.bitmap_left();
        let bmp_top = glyph.bitmap_top();

        let gx = (pen_x >> 6) as i32 + bmp_left;
        let gy = baseline_y - bmp_top;

        let bmp_rows = bitmap.rows() as usize;
        let bmp_pitch = bitmap.pitch().unsigned_abs() as usize;
        let buffer = bitmap.buffer();

        // LCD rendering: bitmap width is 3x the pixel width (R, G, B subpixels)
        let bmp_pixel_width = bitmap.width() as usize / 3;

        for row in 0..bmp_rows {
            for col in 0..bmp_pixel_width {
                let r_alpha = buffer[row * bmp_pitch + col * 3] as f32 / 255.0;
                let g_alpha = buffer[row * bmp_pitch + col * 3 + 1] as f32 / 255.0;
                let b_alpha = buffer[row * bmp_pitch + col * 3 + 2] as f32 / 255.0;

                if r_alpha < 0.01 && g_alpha < 0.01 && b_alpha < 0.01 {
                    continue;
                }

                let px = (gx as usize) + col;
                let py = (gy as usize) + row;

                if px >= x_max || px >= stride || py >= pixels.len() / stride {
                    continue;
                }

                let bg = pixels[py * stride + px];
                let bg_r = ((bg >> 16) & 0xFF) as f32;
                let bg_g = ((bg >> 8) & 0xFF) as f32;
                let bg_b = (bg & 0xFF) as f32;

                // Blend each channel independently for subpixel rendering
                let r = (color_r * r_alpha + bg_r * (1.0 - r_alpha)) as u32;
                let g = (color_g * g_alpha + bg_g * (1.0 - g_alpha)) as u32;
                let b = (color_b * b_alpha + bg_b * (1.0 - b_alpha)) as u32;

                pixels[py * stride + px] = 0xFF000000 | (r << 16) | (g << 8) | b;
            }
        }

        pen_x += glyph.advance().x as i64;
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
