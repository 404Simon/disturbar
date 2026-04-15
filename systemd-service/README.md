# disturbar systemd setup (global mode)

This directory contains system-wide unit files for running both disturbar processes.

- `disturbar-input-backend.service`: listens for Super key state
- `disturbar-ui.service`: runs raw Wayland layer-shell bar process

## 1) Build and install binary

From project root:

```bash
cargo build --release
sudo install -m 0755 ./target/release/disturbar /usr/local/bin/disturbar
```

## 2) Copy unit files

```bash
sudo install -m 0644 ./systemd-service/disturbar-input-backend.service /etc/systemd/system/
sudo install -m 0644 ./systemd-service/disturbar-ui.service /etc/systemd/system/
```

## 3) Edit UI unit for your user/session

Open `/etc/systemd/system/disturbar-ui.service` and adjust:

- `User=` and `Group=` (your login user)
- `XDG_RUNTIME_DIR=/run/user/<UID>`
- `WAYLAND_DISPLAY=` (usually `wayland-0`)

Tip: check your current values in Hyprland terminal:

```bash
echo "$UID"
echo "$XDG_RUNTIME_DIR"
echo "$WAYLAND_DISPLAY"
```

## 4) Optional: override env in `/etc/default/disturbar`

The UI unit reads optional env file:

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
sudo systemctl enable --now disturbar-ui.service
```

## 6) Verify and debug

```bash
systemctl status disturbar-input-backend.service
systemctl status disturbar-ui.service
systemctl show disturbar-ui.service -p Environment --no-pager
journalctl -u disturbar-input-backend.service -f
journalctl -u disturbar-ui.service -f
```

## Notes

- Backend needs access to input devices (`/dev/input/event*`). Running as system service solves this on most setups.
- UI service must run as logged-in desktop user so Wayland session variables are valid.
