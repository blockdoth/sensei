{
  description = "A flake template for a rust and python project";

  inputs = {
    flake-parts.url = "github:hercules-ci/flake-parts";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    inputs@{ self, flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      perSystem =
        {
          config,
          self',
          inputs',
          pkgs,
          system,
          lib,
          ...
        }:
        let
          pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ inputs.rust-overlay.overlays.default ];
          };
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./toolchain.toml;
        in
        {
          devShells.default = pkgs.mkShell {
            packages = with pkgs; [
              toolchain
              ruff
              shellcheck
              nixfmt-rfc-style
              rust-analyzer-unwrapped
              mprocs
            ];
            RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
          };

          # Example package definition
          packages.default = pkgs.rustPlatform.buildRustPackage {
            pname = "sensei";
            version = "0.1.0";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };
            cargoToml = ./Cargo.toml;
            buildInputs = lib.optionals pkgs.stdenv.isLinux [ pkgs.udev ];
            nativeBuildInputs = with pkgs; [
              toolchain
              pkg-config
            ];
          };

          # broken because clippy doesnt work in the sandboxed nix env
          # checks = {
          #   style =
          #     pkgs.runCommand "pre-push"
          #       {
          #         nativeBuildInputs = [
          #           toolchain
          #           pkgs.ruff
          #           pkgs.shellcheck
          #           pkgs.nixfmt-rfc-style
          #         ];
          #       }
          #       ''
          #         export HOME=$(mktemp -d)
          #         cd ${self.outPath}
          #         ${builtins.readFile ./scripts/check.sh}
          #         touch $out
          #       '';
          #   cargo-test =
          #     pkgs.runCommand "cargo-test"
          #       {
          #         nativeBuildInputs = [ toolchain ];
          #       }
          #       ''
          #         # Ensure the Rust package is built and then run tests
          #         cargo test
          #         touch $out
          #       '';
          # };
        };
    };
}
