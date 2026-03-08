#!/bin/bash
# Install git hooks for the muxox repository

set -e

REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOK_DIR="$REPO_ROOT/.git/hooks"

echo "Installing pre-commit hook..."

cat > "$HOOK_DIR/pre-commit" << 'EOF'
#!/bin/bash
set -e

echo "Running pre-commit checks..."

echo "Checking formatting..."
cargo fmt --all -- --check
if [ $? -ne 0 ]; then
    echo "Formatting check failed. Run 'cargo fmt --all' to fix."
    exit 1
fi

echo "Running clippy..."
cargo clippy --all-targets --all-features -- -D warnings
if [ $? -ne 0 ]; then
    echo "Clippy check failed. Fix the warnings above."
    exit 1
fi

echo "Pre-commit checks passed."
EOF

chmod +x "$HOOK_DIR/pre-commit"

echo "Git hooks installed successfully."