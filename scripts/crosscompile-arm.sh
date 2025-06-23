#!/bin/bash
nix develop .#cross-aarch64 -c \
  cargo build \
    --release \
    --package sensei \
    --target aarch64-unknown-linux-musl \
    -Z build-std=std,panic_abort \
    -Z build-std-features=panic_immediate_abort \
    -Z build-std-features=optimize_for_size && \
    upx --best --lzma ./target/aarch64-unknown-linux-musl/release/sensei # compress the binary

#filesize
du -sh ./target/aarch64-unknown-linux-musl/release/sensei
