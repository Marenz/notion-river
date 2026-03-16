#!/bin/bash
# Test launcher for notion-river inside nested Weston+River
set -e

# Kill any old instances
pkill -9 -f "notion-river" 2>/dev/null || true
pkill -9 -f "zig-out/bin/river" 2>/dev/null || true
pkill -9 -f "weston.*kiosk" 2>/dev/null || true
sleep 2

# Clean sockets
rm -f /run/user/$(id -u)/wayland-{1,2,3}*
rm -f /tmp/notion-river.log /tmp/river.log

# Start weston
weston --backend=x11-backend.so --width=1920 --height=1080 --shell=kiosk-shell.so 2>/tmp/weston.log &
sleep 3

if ! pgrep -x weston > /dev/null; then
    echo "ERROR: weston failed to start"
    exit 1
fi

# Start River with notion-river
WAYLAND_DISPLAY=wayland-1 \
XKB_DEFAULT_LAYOUT=de \
XKB_DEFAULT_VARIANT=neo \
XKB_DEFAULT_MODEL=pc105 \
~/repos/river/zig-out/bin/river \
  -c ~/Projects/notion-river/target/release/notion-river \
  -no-xwayland 2>/tmp/river.log &
sleep 3

if ! pgrep -f "zig-out/bin/river" > /dev/null; then
    echo "ERROR: river failed to start"
    tail -5 /tmp/river.log
    exit 1
fi

# Launch a terminal
WAYLAND_DISPLAY=wayland-2 foot &
sleep 1

echo "=== Running ==="
echo "Log: /tmp/notion-river.log"
echo "Use: WAYLAND_DISPLAY=wayland-2 foot"
echo ""
cat /tmp/notion-river.log 2>/dev/null
