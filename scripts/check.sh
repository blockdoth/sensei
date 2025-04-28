#!/usr/bin/env bash
set -euo pipefail

echo "Running clippy..."
cargo clippy --all-targets --all-features -- -D warnings

echo "Running rustfmt..."
cargo fmt --all -- 

echo "Running ruff..."
ruff format
ruff linter

echo "Running shellcheck..."
find . -name "*.sh" -print0 | xargs -0 shellcheck

echo "Running nixfmt..."
find . -name "*.nix" -exec nixfmt {} +

echo "All checks passed!"
