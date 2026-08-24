#!/usr/bin/env sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$SCRIPT_DIR"

command -v pnpm >/dev/null 2>&1 || { echo "[ERROR] pnpm is required (see packageManager in package.json)."; exit 1; }
command -v cargo >/dev/null 2>&1 || { echo "[ERROR] Rust/Cargo is required (see rust-version in Cargo.toml)."; exit 1; }

echo "[Ruvyxa] Installing workspace dependencies..."
pnpm install --frozen-lockfile
echo "[Ruvyxa] Building workspace packages..."
pnpm -r build
echo "[Ruvyxa] Compiling the Ruvyxa CLI..."
cargo build --locked -p ruvyxa_cli

echo ""
echo "Setup complete. Start developing with:"
echo "  cd examples/demo"
echo "  pnpm dev"
