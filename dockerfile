FROM nixos/nix:2.28.2


WORKDIR /sensei
COPY flake.nix flake.lock toolchain.toml ./

ENV NIX_CONFIG="experimental-features = nix-command flakes"


RUN nix develop
# RUN nix build
