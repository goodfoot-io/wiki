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

# Bring up Tailscale in TUN mode (MagicDNS) — shared routine from the base image.
# Reads $TS_HOSTNAME / $TS_AUTHKEY from .devcontainer/.env.
/usr/local/share/devcontainer/tailscale-up.sh

# Shared runtime setup from the base image: Rust (latest stable) + clippy/rustfmt,
# uv, Antigravity, the zsh theme, and a rootless sshd — all installed into the
# persisted /home/node.
/usr/local/share/devcontainer/post-create-common.sh
