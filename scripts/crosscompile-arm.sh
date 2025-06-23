#!/bin/bash

rustup target add aarch64-unknown-linux-musl
rustup install nightly
rustup override set nightly

echo building testapp

cargo build \
    --release \
    --package testapp \
    --target aarch64-unknown-linux-musl \
    -Z build-std=std,panic_abort \
    -Z build-std-features=panic_immediate_abort
