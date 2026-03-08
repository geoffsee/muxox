#!/bin/bash
# Run muxox with the example configuration

set -e

CONFIG_FILE="muxox.example.toml"

if [ ! -f "$CONFIG_FILE" ]; then
    echo "Error: $CONFIG_FILE not found."
    exit 1
fi

echo "Starting muxox with $CONFIG_FILE..."
# Ensure we are in the project root so relative paths in the config work as expected
cargo run -p muxox -- --config "$CONFIG_FILE" "$@"
