#!/bin/bash

set -e

# Make all scripts in utilities directory executable
if [ -d "/workspace/.devcontainer/utilities" ]; then
    echo "Making scripts in /workspace/.devcontainer/utilities executable..."
    chmod +x /workspace/.devcontainer/utilities/*
fi

# Create VSCode MCP Bridge directory with proper permissions
echo "Setting up VSCode MCP Bridge directories..."
mkdir -p /home/node/.local/share/yutengjing-vscode-mcp
chmod 755 /home/node/.local/share/yutengjing-vscode-mcp
chown -R node:node /home/node/.local
echo "VSCode MCP Bridge directories created"

# Start system dbus daemon if not already running
echo "Setting up dbus for VS Code extension testing..."
if ! pgrep -x "dbus-daemon" > /dev/null; then
    # Ensure dbus directories exist with proper permissions
    sudo mkdir -p /run/dbus /var/run/dbus
    sudo chmod 755 /run/dbus /var/run/dbus

    # Start system dbus daemon
    sudo dbus-daemon --system --fork

    # Wait for socket to be created
    sleep 1

    # Verify dbus is running
    if [ -S /run/dbus/system_bus_socket ] || [ -S /var/run/dbus/system_bus_socket ]; then
        echo "System dbus daemon started successfully"
    else
        echo "Warning: dbus daemon started but socket not found"
    fi
else
    echo "System dbus daemon already running"
fi

# Create X11 unix directory with proper permissions for Xvfb
sudo mkdir -p /tmp/.X11-unix
sudo chmod 1777 /tmp/.X11-unix
echo "X11 directory prepared for headless testing"

# Configure git to use .githooks directory for hooks
echo "Configuring git hooks path..."
git config core.hooksPath .githooks
echo "Git hooks path set to .githooks"

# Bring up Tailscale in userspace-networking mode (no NET_ADMIN / /dev/net/tun).
echo "Setting up Tailscale..."
TAILSCALE_STATE_DIR="${HOME}/.local/share/tailscale"
TAILSCALE_SOCKET="${TAILSCALE_STATE_DIR}/tailscaled.sock"

if ! command -v tailscaled >/dev/null 2>&1; then
    echo "⚠ tailscaled not found — install the tailscale package in the Dockerfile"
else
    # State lives under the host-backed /home/node bind mount, so it persists
    # across rebuilds. Ensure the dir exists and is node-owned before tailscaled
    # writes to it (the host mount may surface it root-owned on first creation).
    sudo mkdir -p "$TAILSCALE_STATE_DIR"
    sudo chown -R node:node "$TAILSCALE_STATE_DIR"

    if ! pgrep -x tailscaled > /dev/null; then
        echo "Starting tailscaled (userspace-networking mode)..."
        nohup tailscaled \
            --tun=userspace-networking \
            --statedir="$TAILSCALE_STATE_DIR" \
            --socket="$TAILSCALE_SOCKET" >/dev/null 2>&1 &
        disown
        for _ in $(seq 1 10); do
            if [ -S "$TAILSCALE_SOCKET" ]; then break; fi
            sleep 0.5
        done
    fi

    if tailscale --socket="$TAILSCALE_SOCKET" status >/dev/null 2>&1; then
        echo "✓ Already joined tailnet"
        tailscale --socket="$TAILSCALE_SOCKET" up \
            --hostname="$TS_HOSTNAME" \
            --advertise-tags=tag:devcontainer \
            --accept-routes
    else
        echo "Joining tailnet as $TS_HOSTNAME..."
        tailscale --socket="$TAILSCALE_SOCKET" up \
            --authkey="$TS_AUTHKEY" \
            --hostname="$TS_HOSTNAME" \
            --advertise-tags=tag:devcontainer \
            --accept-routes
    fi
fi

# Shared runtime setup from the base image: Rust (latest stable) + clippy/rustfmt,
# uv, Antigravity, the zsh theme, and a rootless sshd — all installed into the
# persisted /home/node.
/usr/local/share/devcontainer/post-create-common.sh
