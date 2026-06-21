#!/usr/bin/env bash
# Install the MUD Mobile Connector icon + desktop entry into the user's local
# data dir so GNOME/Wayland (and X11) show the brand icon for the running window.
# Safe to re-run. Uninstall: rm the two icon files + the .desktop listed below.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app_id="com.mudmobile.connector"
icons="$HOME/.local/share/icons/hicolor"
apps="$HOME/.local/share/applications"

mkdir -p "$icons/256x256/apps" "$icons/scalable/apps" "$apps"
cp "$here/assets/icon.png" "$icons/256x256/apps/$app_id.png"
cp "$here/assets/icon.svg" "$icons/scalable/apps/$app_id.svg"

# Point Exec at a real binary if one is built (so clicking the entry also works);
# the running-window icon match only needs StartupWMClass + Icon below.
bin="$here/target/release/mudmobile-connector"
[ -x "$bin" ] || bin="$here/target/debug/mudmobile-connector"
[ -x "$bin" ] || bin="mudmobile-connector"

cat > "$apps/$app_id.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=MUD Mobile Connector
Comment=Launch Simutronics front ends through MUD Mobile
Exec=$bin
Icon=$app_id
Terminal=false
Categories=Game;Network;
StartupWMClass=$app_id
EOF

update-desktop-database "$apps" 2>/dev/null || true
gtk-update-icon-cache -f -t "$icons" 2>/dev/null || true

echo "Installed:"
echo "  $icons/256x256/apps/$app_id.png"
echo "  $icons/scalable/apps/$app_id.svg"
echo "  $apps/$app_id.desktop  (Exec=$bin)"
echo "Restart the app (close the window and 'cargo run' again) so the compositor picks up the icon."
