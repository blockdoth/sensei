#!/bin/bash

# This build script depends on the cross-aarch64 build env of the nix flake.
# Online mode is required for building the arm binary, and therefore the standard build process cannot be used.
# See the nix flake for more details.

nix develop .#cross-aarch64 -c bash -c '
  cargo clean && \
  cargo build \
    --release \
    --package sensei \
    --target aarch64-unknown-linux-musl \
    --no-default-features \
    -Z build-std=std,panic_abort \
    -Z build-std-features=panic_immediate_abort \
    -Z build-std-features=optimize_for_size && \
    upx --best --lzma ./target/aarch64-unknown-linux-musl/release/sensei # compresses the binary
'
du -sh ./target/aarch64-unknown-linux-musl/release/sensei
