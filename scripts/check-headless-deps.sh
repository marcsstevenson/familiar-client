#!/usr/bin/env bash
# Fails if a non-GUI crate pulls in a GUI, windowing or webview library.
# Only familiar-gui may depend on these.
set -euo pipefail
cd "$(dirname "$0")/.."
forbidden='^(tauri|tauri-[a-z-]+|wry|tao|webkit2gtk|webkit2gtk-sys|javascriptcore-rs|gtk|gtk-sys|gdk|gdk-sys|gdkwayland-sys|gdkx11|webview2-com|winit|gpui|cairo-rs|cairo-sys-rs|x11|x11-dl|wayland-client|libappindicator) '
status=0
for pkg in familiar-config familiar-core familiar-provider-graph familiar-store familiar-search familiar-rules familiar-daemon familiar-cli familiar-mock-graph; do
  hits=$(cargo tree -p "$pkg" -e normal,build --prefix none --target all | sort -u | grep -E "$forbidden" || true)
  if [ -n "$hits" ]; then
    echo "$pkg depends on GUI libraries:"
    echo "$hits" | sed 's/^/  /'
    status=1
  fi
done
[ "$status" -eq 0 ] && echo "headless crates have no GUI dependencies"
exit "$status"
