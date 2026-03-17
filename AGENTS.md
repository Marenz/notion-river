# notion-river

Notion/Ion3-style static tiling window manager for the River Wayland compositor (0.4.x+).

## Project Overview

A window manager process for River 0.4.x that implements "static tiling" from the Notion WM: the screen layout is a persistent wireframe of frames that exist independently of windows. Windows are placed into frames as tabs. Opening/closing windows never changes the layout — only explicit user actions (split/unsplit) do.

## Build / Test / Run

```sh
cargo build            # debug build
cargo build --release  # release build
cargo test             # run unit tests (layout tree tests)
```

### Nested testing (inside X11/i3)

River 0.4.x is built from source at `~/repos/river`. Run nested inside Weston:

```sh
# Start weston (kiosk shell = no decorations)
weston --backend=x11-backend.so --width=1920 --height=1080 --shell=kiosk-shell.so &

# Start River with notion-river inside weston
# IMPORTANT: don't resize the Weston window — it crashes River's wayland backend
WAYLAND_DISPLAY=wayland-1 \
XKB_DEFAULT_LAYOUT=de XKB_DEFAULT_VARIANT=neo XKB_DEFAULT_MODEL=pc105 \
~/repos/river/zig-out/bin/river \
  -c ~/Projects/notion-river/target/release/notion-river &

# Launch apps inside River
WAYLAND_DISPLAY=wayland-2 foot &
```

**Known issue**: stale wayland socket locks (`/run/user/1000/wayland-2.lock`) persist after crashes. Delete them manually: `rm -f /run/user/$(id -u)/wayland-2*`

**Logging**: output goes to `/tmp/notion-river.log` (line-buffered).

**Test config**: `~/.config/notion-river/config.toml` uses `active_profile = "test"` with Ctrl+Alt modifier to avoid i3 conflicts.

### Native (from TTY / login manager)

River is built from source at `~/repos/river` and **must be built with XWayland support**:

```sh
cd ~/repos/river && zig build -Dxwayland=true
```

The resulting binary is `~/repos/river/zig-out/bin/river`. XWayland is required for X11 apps like Steam to work.

lightdm is configured to autologin into a `river-custom` session (`/usr/share/wayland-sessions/river-custom.desktop`) which runs:

```sh
~/repos/river/zig-out/bin/river -c ~/.config/river/init
```

To start manually from TTY:

```sh
~/repos/river/zig-out/bin/river -c ~/.config/river/init
```

XWayland spawns automatically on `:1` (`DISPLAY=:1`). Steam and other X11 apps work without any extra env vars when launched from within the River session.

## Architecture

- `src/main.rs` — entry point, Wayland connection, event loop, log file setup
- `src/protocol.rs` — wayland-scanner generated protocol bindings (from XML)
- `src/dispatch.rs` — Wayland `Dispatch` impls for all River protocol interfaces
- `src/wm.rs` — core WM state, manage/render cycle, action execution, pointer ops
- `src/layout.rs` — static split tree (binary tree of frames), geometry calculation, neighbor finding, ratio adjustment
- `src/decorations.rs` — tab bar rendering (per-window decoration surfaces) + empty frame indicators (shell surfaces), 5x7 bitmap font
- `src/workspace.rs` — workspace manager, output assignment, multi-monitor
- `src/bindings.rs` — keybinding parsing, built-in profiles (i3_neo, notion), modifier constants
- `src/actions.rs` — action enum and config string parsing
- `src/config.rs` — TOML config loading and defaults
- `protocol/` — River protocol XML files (vendored from `~/repos/river/protocol/`)

## Key Concepts

- **SplitNode**: Binary tree. Leaves are `Frame`s, interior nodes are `Split`s with orientation + ratio.
- **Frame**: A cell that holds 0+ windows as tabs. Empty frames are valid and render as bordered outlines.
- **Workspace**: Owns a SplitNode tree, assigned to an Output.
- **Physical keys**: `set_layout_override(0)` on xkb bindings to use base layout key positions regardless of active layout (Neo, Dvorak, etc).
- **Two-phase commit**: River's manage/render sequence. Management state (focus, dimensions, binding modes) in manage phase; rendering state (positions, borders, decorations, z-order) in render phase.
- **Focus-follows-mouse**: uses `pointer_position` coordinates to detect which frame the pointer is over (works for both windows and empty frames).
- **Cursor-follows-focus**: `pointer_warp` on keyboard focus changes only (not mouse).
- **Pointer ops**: left-drag moves windows between frames (drop-on-release); right-drag resizes split boundaries with edge-aware axis detection.

## Built-in Keybinding Profiles

- `i3_neo`: Matches user's i3 config with Neo layout directions (i/a/l/e)
- `notion`: Vim-style (h/j/k/l), Super+Tab for tab cycling, Super+s/v/x for split/vsplit/unsplit, Super+t for toggle split orientation

## Common Pitfalls

- TOML top-level keys (like `active_profile`) must come before any `[section]` headers, otherwise they get parsed as belonging to the preceding section.
- The nested test environment (Weston X11 backend) crashes if the Weston window is resized. This is a wlroots/NVIDIA issue, not our WM.
- Stale wayland socket locks after crashes prevent River from starting. Always clean them.
- `env_logger` output goes to a socket when River is the parent process. The binary redirects to `/tmp/notion-river.log`.

## Dependencies

- `wayland-client` / `wayland-scanner` / `wayland-backend` — Wayland protocol handling
- `xkbcommon` — keysym resolution
- `serde` / `toml` — config parsing
- `bitflags` — modifier bitmasks
- `log` / `env_logger` — logging (to file)
- `dirs` — XDG config directory lookup
- `libc` — memfd_create, mmap for shared memory buffers (decoration rendering)
