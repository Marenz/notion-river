# notion-river

A **static tiling** window manager for the [River](https://codeberg.org/river/river) Wayland compositor (0.4.x+), inspired by [Notion](https://notionwm.net/) (formerly Ion3).

## What is "static tiling"?

Unlike dynamic tiling WMs (i3, Sway, Hyprland) where the layout reflows every time a window opens or closes, notion-river uses **persistent frames**:

- The screen is divided into a tree of frames (cells) that you create manually with split commands
- Frames persist even when empty — they're the skeleton of your workspace
- Windows are placed *into* frames as **tabs** — multiple windows share a frame, one visible at a time
- Opening/closing a window never changes the layout — only explicit user actions (split, unsplit) do

This gives you a predictable, stable workspace layout that doesn't rearrange itself.

## Features

- **Static tiling** with manual split/unsplit (horizontal, vertical, toggle)
- **Tabbed frames** — multiple windows per frame with tab bar
- **Empty frame indicators** — visible wireframe for empty cells
- **Focus-follows-mouse** across frames and monitors (including empty frames)
- **Cursor-follows-focus** on keyboard navigation
- **Click-to-tab** — click tab bar to switch tabs
- **Pointer drag** — left-drag moves windows between frames, right-drag resizes splits
- **Multi-monitor** with per-output workspace assignment
- **Cross-monitor focus** navigation with edge-matching
- **Physical key bindings** — work across keyboard layouts (Neo, Dvorak, etc.)
- **Two built-in keybinding profiles** — `i3_neo` (Neo layout) and `notion` (Vim-style)
- **State persistence** — layout and window placement saved/restored across WM restarts
- **TOML configuration**

## Requirements

- [River](https://codeberg.org/river/river) 0.4.x+ (built from source, uses `river-window-management-v1` protocol)
- Rust 1.75+
- `wlr-randr` or `kanshi` for monitor configuration
- A Wayland terminal (e.g. `foot`)

## Building

```sh
git clone https://github.com/Marenz/notion-river
cd notion-river
cargo build --release
```

## Setup

1. Copy the binary:
```sh
cp target/release/notion-river ~/.local/bin/
```

2. Create the River init script at `~/.config/river/init`:
```sh
#!/bin/sh
export XKB_DEFAULT_LAYOUT=us  # adjust to your layout
export XDG_CURRENT_DESKTOP=river

kanshi &  # monitor configuration

while notion-river; do
    sleep 0.5
done
```

3. Create the config at `~/.config/notion-river/config.toml`:
```toml
active_profile = "notion"

[general]
physical_keys = true
focus_follows_mouse = true
cursor_follows_focus = true
gap = 4
border_width = 2

[commands]
terminal = "foot"
launcher = ["fuzzel"]

[[workspaces]]
name = "main"
output = "HDMI-A-1"
initial_layout = "hsplit"

[[workspaces]]
name = "secondary"
output = "HDMI-A-1"
```

4. Start from a TTY:
```sh
river -c ~/.config/river/init
```

## Default keybindings (notion profile)

| Binding | Action |
|---|---|
| `Super+Return` | Terminal |
| `Super+p` | Launcher |
| `Super+c` | Close window / unsplit empty frame |
| `Super+h/j/k/l` | Focus left/down/up/right |
| `Super+Shift+h/j/k/l` | Move window |
| `Super+s` | Split horizontal |
| `Super+v` | Split vertical |
| `Super+t` | Toggle split orientation |
| `Super+x` | Remove empty frame |
| `Super+Tab` | Next tab |
| `Super+Shift+Tab` | Previous tab |
| `Super+1..6` | Switch workspace |
| `Super+Shift+R` | Restart WM (preserves windows) |

Mouse: `Super+Left-drag` moves windows, `Super+Right-drag` resizes splits.

## Status

Early development. Usable as a daily driver with some rough edges. See [AGENTS.md](AGENTS.md) for architecture details.

## License

TBD
