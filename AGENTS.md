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

The init script (`~/.config/river/init`) starts kanshi, waybar, nm-applet, keepassxc, and runs notion-river in a restart loop (always restarts, not just on exit 0). kanshi sets DPI at scale 2.0 for HiDPI; wp_viewporter protocol handles fractional scaling.

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
- `src/state.rs` — state persistence: save/restore layout tree, window placement, active tabs, visible workspaces
- `src/ipc.rs` — waybar workspace status: writes JSON to `$XDG_RUNTIME_DIR/notion-river-workspaces`
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
- **State persistence**: layout tree + window-to-frame mapping + visible workspaces saved to JSON on restart/signal. Windows matched by River's stable identifier, then app_id+title.
- **Title sync**: WindowRef titles updated from ManagedWindow every manage cycle for live tab bar updates.
- **Control IPC**: `$XDG_RUNTIME_DIR/notion-river.sock` accepts `list-windows`, `list-workspaces`, `focus-window <id>`, `switch-workspace <name>` for `notion-ctl` and rofi integration. `focus-window` switches to hidden workspaces if the target window is on one.
- **wp_viewporter**: Wayland protocol for fractional scaling support with HiDPI (scale 2.0 via kanshi).

## Built-in Keybinding Profiles

- `i3_neo`: Neo layout directions (i/a/l/e), Super+Space terminal, Super+o launcher, Super+Shift+o window switcher, Super+b/v split, Super+n/p tabs
- `notion`: Vim-style (h/j/k/l), Super+Return terminal, Super+p launcher, Super+Shift+p window switcher, Super+s/v split, Super+Tab tabs
- Both: media keys (XF86Audio*, XF86MonBrightness*), Super+Shift+R restart, Super+t toggle split

## Config Files

- `~/.config/notion-river/config.toml` — WM config (profile, workspaces, commands, appearance)
- `~/.config/river/init` — River init script (env vars, kanshi, waybar, notion-river restart loop)
- `~/.local/bin/start-river` — Session launcher (XKB layout, env vars, exec river)
- `~/.config/kanshi/config` — Monitor layout (position, scale, transform)
- `~/.config/waybar/config.jsonc` — Waybar modules (custom/workspaces with Pango markup, CPU, MEM, DSK, VOL, NET, tray)
- `~/.config/waybar/style.css` — Waybar styling (Catppuccin-inspired)
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
