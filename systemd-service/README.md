# disturbar systemd setup (global mode)

This directory contains unit files for running both disturbar processes.

- `disturbar-input-backend.service`: system service that listens for Super key state (needs input device access)
- `disturbar-ui.service`: **user service** that runs the raw Wayland layer-shell bar process

## 1) Build and install binary

From project root:

```bash
cargo build --release
sudo install -m 0755 ./target/release/disturbar /usr/local/bin/disturbar
```

## 2) Copy unit files

```bash
# Backend runs as root for input device access
sudo install -m 0644 ./systemd-service/disturbar-input-backend.service /etc/systemd/system/

# UI runs as your desktop user, tied to the graphical session
sudo install -m 0644 ./systemd-service/disturbar-ui.service /etc/systemd/user/
```

## 3) Edit UI unit for your session

Open `/etc/systemd/user/disturbar-ui.service` and adjust:

- `XDG_RUNTIME_DIR=/run/user/<UID>`
- `WAYLAND_DISPLAY=` (usually `wayland-0`)

Tip: check your current values in Hyprland terminal:

```bash
echo "$UID"
echo "$XDG_RUNTIME_DIR"
echo "$WAYLAND_DISPLAY"
```

## 4) Optional: override env in `/etc/default/disturbar`

The UI unit reads an optional env file:

- `/etc/default/disturbar`

Example:

```bash
sudo tee /etc/default/disturbar >/dev/null <<'EOF'
XDG_RUNTIME_DIR=/run/user/1000
WAYLAND_DISPLAY=wayland-1
EOF
```

If this file exists, values there override unit defaults.

For Nix or other non-standard installs, keep `PATH` sane for standard runtime tools.

Recommended example:

```bash
sudo tee /etc/default/disturbar >/dev/null <<'EOF'
XDG_RUNTIME_DIR=/run/user/1000
WAYLAND_DISPLAY=wayland-1
PATH=/home/simon/.nix-profile/bin:/usr/local/bin:/usr/bin
EOF
```

## 5) Enable and start services

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now disturbar-input-backend.service

# User service for the UI
systemctl --user daemon-reload
systemctl --user enable --now disturbar-ui.service
```

## Notes

- Backend needs access to input devices (`/dev/input/event*`). Running as system service solves this on most setups.
- UI service must run as the logged-in desktop user so the Wayland session variables are valid.
