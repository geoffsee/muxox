#!/bin/bash
# Build the muxox binary

set -e

echo "Building muxox..."
cargo build -p muxox
echo "Done."
