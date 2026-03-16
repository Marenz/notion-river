# notion-river

Notion/Ion3-style static tiling window manager for the River Wayland compositor (0.4.x).

## Project Overview

This is a window manager process for River 0.4.x that implements the "static tiling" concept from the Notion WM: the screen layout is a persistent wireframe of frames that exist independently of windows. Windows are placed into frames as tabs. Opening/closing windows never changes the layout.

## Build / Test / Run

```sh
cargo build          # debug build
cargo build --release
cargo test           # run unit tests (layout tree tests)
```

To run with River (River 0.4.x must be installed):
```sh
river -c notion-river
```

Or set in `~/.config/river/init`:
```sh
#!/bin/sh
waybar &
exec notion-river
```

## Architecture

- `src/main.rs` — entry point, Wayland connection, event loop
- `src/protocol.rs` — wayland-scanner generated protocol bindings (from XML)
- `src/dispatch.rs` — Wayland `Dispatch` impls for all River protocol interfaces
- `src/wm.rs` — core WM state, manage/render cycle, action execution
- `src/layout.rs` — static split tree (binary tree of frames), geometry calculation
- `src/workspace.rs` — workspace manager, output assignment
- `src/bindings.rs` — keybinding parsing, built-in profiles (i3_neo, notion)
- `src/actions.rs` — action enum and config string parsing
- `src/config.rs` — TOML config loading and defaults
- `protocol/` — River protocol XML files (vendored)

## Key Concepts

- **SplitNode**: Binary tree. Leaves are `Frame`s, interior nodes are `Split`s with orientation + ratio.
- **Frame**: A cell that holds 0+ windows as tabs. Empty frames are valid.
- **Workspace**: Owns a SplitNode tree, assigned to an Output.
- **Physical keys**: `set_layout_override(0)` on xkb bindings to use base layout key positions.
- **Two-phase commit**: River's manage/render sequence. Management state (focus, dimensions) in manage phase, rendering state (positions, borders, z-order) in render phase.

## Built-in Keybinding Profiles

- `i3_neo`: Matches user's i3 config with Neo layout directions (i/a/l/e)
- `notion`: Vim-style (h/j/k/l), Super+Tab for tab cycling, Super+s/v/x for split/vsplit/unsplit

## Dependencies

- `wayland-client` / `wayland-scanner` / `wayland-backend` — Wayland protocol handling
- `xkbcommon` — keysym resolution
- `serde` / `toml` — config parsing
- `bitflags` — modifier bitmasks
- `log` / `env_logger` — logging
- `dirs` — XDG config directory lookup
