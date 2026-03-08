#!/bin/bash
# Run tests for muxox

set -e

echo "Running tests..."
cargo test -p muxox
echo "Done."
