# notion-river

Notion/Ion3-style static tiling window manager for the River Wayland compositor (0.4.x+).

## Project Overview

A window manager process for River 0.4.x that implements "static tiling" from the Notion WM: the screen layout is a persistent wireframe of frames that exist independently of windows. Windows are placed into frames as tabs. Opening/closing windows never changes the layout — only explicit user actions (split/unsplit) do.

## Build / Test / Run

```sh
cargo build            # debug build
cargo build --release  # release build
cargo test             # run unit tests (layout + focus tests)
cp target/release/notion-river ~/.local/bin/
```

After installing, press `Super+Shift+R` inside River to restart the WM with the new binary. Windows survive restarts.

### Native (from TTY / login manager)

River is built from source at `~/repos/river` with XWayland support:

```sh
cd ~/repos/river && zig build -Doptimize=ReleaseSafe -Dxwayland=true
cp zig-out/bin/river ~/.local/bin/
```

lightdm is configured with a "Notion River" session (`/usr/share/wayland-sessions/river-custom.desktop`) pointing to `~/.local/bin/start-river`.

The `start-river` script sets XKB layout (de/neo), Wayland env vars, and execs River.

The init script (`~/.config/river/init`) starts kanshi, waybar, nm-applet, keepassxc, and runs notion-river in a restart loop (always restarts, not just on exit 0). kanshi sets DPI at scale 1.5 for HiDPI (clean fraction, no wlroots blur); wp_viewporter protocol handles fractional scaling.

### Nested testing (inside X11)

```sh
weston --backend=x11-backend.so --width=1920 --height=1080 --shell=kiosk-shell.so &
WAYLAND_DISPLAY=wayland-1 XKB_DEFAULT_LAYOUT=de XKB_DEFAULT_VARIANT=neo \
  river -c ~/Projects/notion-river/target/release/notion-river -no-xwayland &
WAYLAND_DISPLAY=wayland-2 foot &
```

## Architecture

- `src/main.rs` — entry point, Wayland connection, event loop, signal handler, log file setup
- `src/protocol.rs` — wayland-scanner generated bindings (river-window-management-v1, river-xkb-bindings-v1, river-layer-shell-v1)
- `src/dispatch.rs` — Wayland `Dispatch` impls for all protocol interfaces (WM, output, seat, window, pointer, layer-shell, decorations)
- `src/wm.rs` — core WM state, manage/render cycle, focus logic integration
- `src/window_actions.rs` — action execution: perform_action, perform_split, perform_unsplit, cross-monitor moves, command spawning
- `src/rendering.rs` — layout application: window dimensions, focus, visibility, position/border/decoration drawing
- `src/pointer_ops.rs` — pointer operation handling: move-drop, seat ops (resize), resize axis detection, cursor warping
- `src/layout.rs` — static split tree (binary tree of frames), geometry calculation, neighbor finding, ratio adjustment
- `src/decorations.rs` — tab bar rendering (per-window decoration surfaces via Cairo+Pango) + empty frame indicators (shell surfaces)
- `src/control.rs` — IPC control socket server: accepts commands on `$XDG_RUNTIME_DIR/notion-river.sock`
- `src/bin/notion-ctl.rs` — CLI client for the control socket
- `src/workspace.rs` — workspace manager, output assignment, multi-monitor, saved visible workspace restore
- `src/bindings.rs` — keybinding parsing, built-in profiles (i3_neo, notion), media keys, modifier constants
- `src/actions.rs` — action enum and config string parsing
- `src/config.rs` — TOML config loading and defaults
- `src/focus.rs` — focus-follows-mouse logic, extracted for testability with 12 unit tests
- `src/state.rs` — state persistence: save/restore layout tree, window placement, active tabs, visible workspaces to `~/.config/notion-river/`
- `src/app_bindings.rs` — app-to-frame bindings: bind/unbind apps to frames, wildcard app_id matching, fixed dimensions, persistence to `~/.config/notion-river/bindings.json`, enforce_app_bindings auto-move
- `src/output_profiles.rs` — output profile management: hashes connected output names, saves/restores workspace-to-output assignments in `~/.config/notion-river/output-profiles.json`
- `src/ipc.rs` — waybar workspace status: writes JSON to `$XDG_RUNTIME_DIR/notion-river-workspaces`, streams updates to IPC subscribers
- `protocol/` — River protocol XML files (vendored)

## Key Concepts

- **SplitNode**: Binary tree. Leaves are `Frame`s, interior nodes are `Split`s with orientation + ratio.
- **Frame**: A cell that holds 0+ windows as tabs. Empty frames are valid and render as bordered outlines.
- **Workspace**: Owns a SplitNode tree, assigned to an Output by preferred output name.
- **Physical keys**: `set_layout_override(0)` on xkb bindings for layout-independent keybindings.
- **Two-phase commit**: River's manage/render sequence. manage_start → WM decisions → manage_finish → render_start → positioning → render_finish.
- **manage_dirty**: Called from wl_pointer events on shell surfaces to trigger manage cycles for focus-follows-mouse on empty frames.
- **Focus-follows-mouse**: uses PointerEnter for windows, pointer_position coordinates for empty frames. Extracted to `focus.rs` with unit tests.
- **Cursor-follows-focus**: pointer_warp on keyboard-triggered focus changes only.
- **Pointer ops**: left-drag moves windows between frames (drop on release); right-drag resizes splits with dual-axis corner detection.
- **Layer-shell**: river-layer-shell-v1 for waybar/rofi/notifications. non_exclusive_area adjusts tiling area.
- **State persistence**: layout tree + window-to-frame mapping + active tabs + visible workspaces saved to JSON in `~/.config/notion-river/` on restart/signal (survives reboots). Windows matched by River's stable identifier only (no app_id fallback). Active tab correctly preserved via `add_window_quiet`.
- **Title sync**: WindowRef titles updated from ManagedWindow every manage cycle for live tab bar updates.
- **Control IPC**: `$XDG_RUNTIME_DIR/notion-river.sock` accepts `list-windows`, `list-workspaces`, `subscribe-workspaces`, `subscribe-workspace <name>`, `focus-window <id>`, `switch-workspace <name>`, `bind <app_id> <workspace> <frame_path>`, `unbind <app_id>`, `set-fixed-dimensions <app_id> <w>x<h>` for `notion-ctl` and rofi integration. `focus-window` switches to hidden workspaces if the target window is on one.
- **IPC subscriptions**: `subscribe-workspaces` and `subscribe-workspace <name>` keep the connection open and stream waybar JSON lines on every workspace state change. Used by waybar for event-driven (zero-polling) workspace modules. Subscribers are `Arc<Mutex<Vec<Subscriber>>>` shared between the IPC writer (main thread) and control socket (listener thread). Per-subscriber dedup avoids redundant writes.
- **App bindings**: Apps can be bound to specific frames via `Super+f` (toggle) / `Super+Shift+f` (exclusive). Bindings persist in `~/.config/notion-river/bindings.json`. `enforce_app_bindings` auto-moves bound windows to visible workspace frames during manage cycle.
- **Wildcard app_id matching**: Binding patterns like `steam_app_*` match all Steam games, useful for binding categories of apps.
- **Fixed dimensions**: Per-binding fixed window dimensions (e.g. 1920x1080 for Steam streaming) — bound windows are forced to these dimensions regardless of frame size.
- **Floating windows**: Windows can float above the tiled layout. River requires `propose_dimensions()` on every manage cycle for floating windows to render. Floating windows are positioned on the focused output's usable area — dialogs centered, notifications top-right. `focused_floating` tracks which floating window has keyboard focus, with focus-follows-mouse support.
- **Auto-float popups**: Windows with DimensionsHint or a parent are auto-floated. Untitled windows from apps that already have a tiled window are auto-floated as notifications (top-right). Secondary windows from bound apps (where `find_target` returns `AlreadyPlaced`) are auto-floated as dialogs (centered).
- **FindTargetResult**: Three-way enum for app binding placement: `Target(ws, frame)` = place here, `AlreadyPlaced` = app already in bound frame so float as secondary, `NoBinding` = no binding or frame missing.
- **Floating move**: Super+LMB on floating windows updates `float_x`/`float_y` live (no frame drop). Drop zone preview suppressed for floating drags.
- **Floating focus**: `focused_floating` in WindowManager tracks the active floating window. Takes priority over tiled focus for keyboard input and actions (Close, etc.). Cleared when clicking a tiled window or when the floating window closes/disappears.
- **Fullscreen toggle**: `Super+Return` toggles fullscreen for the focused window.
- **Resize mode**: `Super+R` enters/exits resize mode with absolute direction semantics (Up always moves the boundary up, regardless of which side).
- **Tab drag specificity**: Dragging a tab in the tab bar drags that specific clicked tab, not the active window.
- **wp_viewporter**: Wayland protocol for fractional scaling support with HiDPI (scale 1.5x via kanshi).
- **Output profiles**: Per-monitor-config workspace assignment memory. Hashes connected output names into a profile key, saves workspace-to-output assignments in `~/.config/notion-river/output-profiles.json`. When the same monitor set reconnects, previous workspace assignments are restored automatically.
- **Monitor disconnect**: Workspaces stay intact (layout preserved), they just become invisible. Focus moves to a visible workspace. No window migration or layout tearing.
- **Monitor reconnect**: Output profile restores previous workspace-to-output assignments for the reconnected monitor set.
- **Runtime keyboard layout switching**: `Ctrl+F12` toggles between `de/neo` and `de` layouts at runtime.

## Built-in Keybinding Profiles

- `i3_neo`: Neo layout directions (i/a/l/e), Super+Space terminal, Super+o launcher, Super+Shift+o window switcher, Super+b/v split, Super+n/p tabs
- `notion`: Vim-style (h/j/k/l), Super+Return terminal, Super+p launcher, Super+Shift+p window switcher, Super+s/v split, Super+Tab tabs
- Both: media keys (XF86Audio*, XF86MonBrightness*), Super+Shift+R restart, Super+t toggle split, Super+f bind app to frame, Super+Shift+f exclusive bind, Super+Return fullscreen toggle, Super+R resize mode, Ctrl+F12 keyboard layout toggle

## Config Files

- `~/.config/notion-river/config.toml` — WM config (profile, workspaces, commands, appearance)
- `~/.config/notion-river/bindings.json` — persisted app-to-frame bindings (auto-managed, survives reboots)
- `~/.config/notion-river/state.json` — persisted layout/window state (auto-managed, survives reboots)
- `~/.config/notion-river/output-profiles.json` — per-monitor-set workspace assignment profiles (auto-managed, hashed by connected output names)
- `~/.config/river/init` — River init script (env vars, kanshi, waybar, notion-river restart loop)
- `~/.local/bin/start-river` — Session launcher (XKB layout, env vars, exec river)
- `~/.config/kanshi/config` — Monitor layout (position, scale, transform)
- `~/.config/waybar/config.jsonc` — Waybar modules (per-workspace event-driven modules via `notion-ctl subscribe-workspace <name>`, CPU, MEM, DSK, VOL, NET, tray)
- `~/.config/waybar/style.css` — Waybar styling (Catppuccin Mocha, per-monitor colors, floating pill modules, rounded corners)
- `~/.config/rofi/config.rasi` — Rofi config (Catppuccin Mocha Mauve theme)
- `~/.config/rofi/catppuccin-mocha.rasi` — Rofi theme file
- `~/.local/bin/notion-rofi-windows` — rofi window switcher using `notion-ctl`
- `/usr/share/wayland-sessions/river-custom.desktop` — lightdm session entry

## Common Pitfalls

- TOML top-level keys must come before any `[section]` headers.
- River reads XKB env vars at startup, not from the init script. Set them in `start-river` before `exec river`.
- `kill -9` on River leaves stale logind sessions that block GPU access. Use `loginctl terminate-session` to clean up.
- Electron apps need `ELECTRON_OZONE_PLATFORM_HINT=wayland` env var (set in init script).
- `env_logger` output goes to `/tmp/notion-river.log` via LineFlush wrapper.
- Stale wayland socket locks after crashes: `rm -f /run/user/$(id -u)/wayland-*`
- The init restart loop always restarts notion-river (not conditional on exit code). This means crashes also trigger a restart.
- Contour terminal works under Wayland — no special flags needed.
- Fractional scale 1.5x (kanshi) is the best fractional scale — it's a clean fraction wlroots handles well. 1.75x causes blur due to wlroots rounding bug (#953). Stick to 1.5x or integer scales (1x, 2x).
- XWayland support requires rebuilding River with `-Dxwayland=true`. Some apps (Steam) need it.

## HiDPI / Scaling Deep Dive

This was a hard-fought battle. Documenting for posterity.

### The problem
Tab bar text appeared blurry compared to waybar text rendered by GTK/Pango.

### Root cause (discovered after many iterations)
`wl_output.scale` and `wl_output.mode` events arrive AFTER the first manage/render cycle. On the first frame:
- `Output.physical_width = 0`, `Output.scale = 1` (defaults)
- `fractional_scale()` returns 1.0
- Tab bar renders at 1x into a `buffer_scale=1` surface
- Compositor displays this 1x surface on a 2x display → bilinear upscale = **blur**

The `wl_output.scale=2` event arrives moments later, but:
- The tab bar hash doesn't change (same frame content)
- No re-render is triggered
- The 1x buffer persists for the lifetime of the decoration

### What didn't work
- **fontdue** — poor kerning, no hinting, wrong spacing
- **FreeType directly** — better but still soft compared to GTK
- **Cairo+Pango with intermediate surface + pixel copy** — the copy step introduced alpha blending artifacts (premultiplied alpha was applied twice: `r * alpha / alpha * alpha`)
- **Zero-copy cairo (create_for_data_unsafe)** — eliminated the copy but didn't fix the scale timing issue
- **wp_viewporter** — River doesn't expose this protocol, so we can't render at exact fractional resolution
- **Fractional scale 1.75x** — wlroots has a known rounding bug (#953) that causes blur at non-integer scales. Integer 2x is the only reliable option.
- **Subpixel rendering (TARGET_LCD)** — actually makes text worse at integer 2x because RGB subpixels don't align with the 2x pixel grid
- **Various font options (Slight/Full hinting, Subpixel/Gray antialias)** — minimal difference; the real issue was the 1x vs 2x buffer, not the font rendering settings

### What worked
1. **Integer output scale** — kanshi `scale 2` not `1.75`. Fractional scaling + wlroots = blur.
2. **Force minimum 2x scale in decoration rendering** — `let scale = if fractional_scale > 1.0 { fractional_scale } else { 2.0 }`. This bypasses the timing issue where scale detection arrives too late.
3. **Cairo+Pango rendering** — same stack as waybar/GTK. `set_absolute_size` for pixel-perfect font sizing. Default fontconfig options (don't override antialias/hinting).
4. **Track `last_scale` per decoration** — force redraw when scale changes (via `manage_dirty` on `wl_output.scale` and `wl_output.mode` events).

### Key architectural insight
River's WM protocol (river-window-management-v1) operates in two phases:
- **Manage phase**: WM sets focus, dimensions, bindings
- **Render phase**: WM sets positions, borders, draws decorations

`wl_output` events (scale, mode) arrive asynchronously between cycles. Calling `manage_dirty()` from these handlers triggers a new cycle, but the first render has already committed a 1x buffer. The `last_scale` tracking detects this and forces a re-render — but only if the scale actually changes from the default.

### The 2.0 fallback
The current fix (`else { 2.0 }`) assumes HiDPI. For 1x displays, `fractional_scale` will correctly return 1.0 from the computed `physical/logical` ratio once `wl_output.mode` arrives. The fallback only applies to the first render before scale is known. On a 1x display, this means the first render is at 2x (wastes memory but looks fine — compositor downscales). On the next cycle the correct 1.0 scale takes over. Not perfect but acceptable.

## Dependencies

- `wayland-client` / `wayland-scanner` / `wayland-backend` — Wayland protocol handling
- `xkbcommon` — keysym resolution
- `serde` / `toml` / `serde_json` — config and state serialization
- `bitflags` — modifier bitmasks
- `log` / `env_logger` — logging (to file)
- `dirs` — XDG config directory lookup
- `libc` — memfd_create, mmap for shared memory buffers (decoration rendering)
- `cairo-rs` (with freetype feature) — 2D rendering for tab bars and decoration surfaces
- `pangocairo` — Cairo integration for Pango text layout
- `pango` — font rendering and text shaping for tab bar labels
