#!/bin/bash
# Run clippy and format checks

set -e

echo "Checking formatting..."
cargo fmt --all -- --check

echo "Running clippy..."
cargo clippy --all-targets --all-features -- -D warnings

echo "Done."
