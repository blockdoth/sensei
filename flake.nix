{
  description = "The Sensei dev flake";

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
            config.allowBroken = true;
          };

          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./toolchain.toml;
          target = "aarch64-unknown-linux-musl";
          isLinux = pkgs.stdenv.isLinux;
          crossPkgs = pkgs.pkgsCross.aarch64-multiplatform-musl;
          linker = "${crossPkgs.stdenv.cc}/bin/aarch64-unknown-linux-musl-gcc";
          muslLib = "${crossPkgs.musl}/lib";
          # gccLib = "${crossPkgs.stdenv.cc.libc}/lib";
          gccLibDir = builtins.head (
            builtins.attrNames (
              builtins.readDir "${pkgs.pkgsCross.aarch64-multiplatform.stdenv.cc.cc}/lib/gcc/aarch64-unknown-linux-gnu"
            )
          );
          gccLibPath = "${pkgs.pkgsCross.aarch64-multiplatform.stdenv.cc.cc}/lib/gcc/aarch64-unknown-linux-gnu/${gccLibDir}";

          libunwindMuslStatic = crossPkgs.stdenv.mkDerivation rec {
            pname = "libunwind";
            version = "1.8.2";

            src = pkgs.fetchFromGitHub {
              owner = "libunwind";
              repo = "libunwind";
              rev = "v${version}";
              sha256 = "sha256-MsUReXFHlj15SgEZHOYhdSfAbSeVVl8LCi4NnUwvhpw=";
            };

            nativeBuildInputs = [
              pkgs.autoconf
              pkgs.automake
              pkgs.libtool
              pkgs.pkg-config
            ];

            # cross-compilation flags
            configureFlags = [
              "--enable-static"
              "--disable-shared"
              "--enable-cxx-exceptions"
              "--prefix=$out"
              "--host=aarch64-unknown-linux-musl"
            ];

            buildPhase = ''
              autoreconf -i
              ./configure ${pkgs.lib.concatStringsSep " " configureFlags}
              make
              make -C src
            '';

            installPhase = ''
              mkdir -p $out/lib $out/include
              cp src/.libs/*.a $out/lib/
              cp -r include/libunwind* $out/include/
            '';

            doCheck = false;

            meta = with pkgs.lib; {
              description = "libunwind library with static libs cross-compiled for aarch64-musl";
              license = licenses.bsd3;
              platforms = platforms.all;
              maintainers = with maintainers; [ ];
            };
          };
          libunwindMusl = "${libunwindMuslStatic}/lib";
          muslGcc = "${crossPkgs.stdenv.cc}";
        in
        {
          devShells.default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                python3
                toolchain
                pkgs.gcc
                pkgs.glibc
                ruff
                shellcheck
                nixfmt-rfc-style
                rust-analyzer-unwrapped
                mprocs
                pkg-config
              ]
              ++ lib.optionals isLinux [
                udev
                valgrind
                llvmPackages_latest.llvm
                cargo-llvm-cov
              ];

            RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
            CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER = "${linker}";
            RUSTFLAGS = "";
            # For coverage tools
            LLVM_COV = "${pkgs.llvmPackages_latest.llvm}/bin/llvm-cov";
            LLVM_PROFDATA = "${pkgs.llvmPackages_latest.llvm}/bin/llvm-profdata";
          };

          packages.cross-aarch64 = lib.mkIf (system == "x86_64-linux" || system == "aarch64-linux") (
            pkgs.rustPlatform.buildRustPackage {
              pname = "sensei";
              version = "0.1.0";
              src = ./.;
              cargoLock = {
                lockFile = ./Cargo.lock;
              };
              cargoToml = ./Cargo.toml;

              packages = with pkgs; [
                toolchain
                pkgs.gcc
                pkgs.glibc
                pkgs.pkg-config
                pkgs.pkgsCross.aarch64-multiplatform-musl.stdenv
                libunwindMuslStatic
              ];

              nativeBuildInputs = [
                toolchain
                pkgs.gcc
                pkgs.glibc
                pkgs.pkg-config
                pkgs.pkgsCross.aarch64-multiplatform-musl.stdenv  
                libunwindMuslStatic
              ];              

              cargoBuildFlags = [
                "--package" "sensei"
                "--target" "aarch64-unknown-linux-musl"
                "-Z" "build-std=std,panic_abort"
                "-Z" "build-std-features=panic_immediate_abort"
                "-Z" "build-std-features=optimize_for_size"
                ];

              RUSTFLAGS = "-C linker=${linker}
              -C link-arg=-nostartfiles
              -L${muslLib}
              -L${gccLibPath}
              -L${libunwindMusl}
              -C link-arg=-nostdlib
              -C link-arg=-static
              -C link-arg=-Wl,--start-group
              -C link-arg=-lc
              -C link-arg=-lm
              -C link-arg=-lgcc
              -C link-arg=-lunwind
              -C link-arg=-Wl,--end-group
              -C panic=abort
              -C force-unwind-tables=yes
              -C opt-level=z
              -Zlocation-detail=none
              -Zfmt-debug=none
              ";
              doCheck = false;

              MUSL_LIB_PATH = "${muslLib}";
              MUSL_GCC_LIB_PATH = "${gccLibPath}";
              MUSL_GCC_PATH = "${muslGcc}";
              MUSL_UNWIND_PATH = "${libunwindMusl}";
            }
          );

          # Default native build
          packages.default = pkgs.rustPlatform.buildRustPackage {
            pname = "sensei";
            version = "0.1.0";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };
            cargoToml = ./Cargo.toml;
            buildInputs = lib.optionals isLinux [ pkgs.udev ];
            nativeBuildInputs = with pkgs; [
              toolchain
              pkg-config
            ];
          };
        };
    };
}
