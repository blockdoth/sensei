{
  description = "A Nix flake template for a rust development environment";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    { self, nixpkgs, rust-overlay, }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      # Imports the correct version of nixpkgs for each system architecture
      forEachSupportedSystem =
        f: nixpkgs.lib.genAttrs supportedSystems (system: f { pkgs = import nixpkgs { 
          inherit system; 
          overlays = [rust-overlay.overlays.default];
        }; });
      in
    {
      devShells = forEachSupportedSystem (
        { pkgs }:
        let 
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./toolchain.toml;
        in        
        {
          default = pkgs.mkShell {
            shellHook = " echo 'Entering a rust shell template'";
            RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
            packages = [
              toolchain
              pkgs.rust-analyzer-unwrapped
            ];
            
          };
        }
      );
      packages = forEachSupportedSystem ({ pkgs }: 
        let
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./toolchain.toml;
        in {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "my-rust-project";
            version = "0.1.0";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };
            cargoToml = ./Cargo.toml;
            nativeBuildInputs = [ toolchain ];
          };
        });      
    };
}
