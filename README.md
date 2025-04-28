# Sensei





# Building and development
Sensei uses nix flakes for setting up a declarative development environment and achieving reproducible builds.   

## Installing Nix

### Install Nix on Linux/macOS

Run the following command in your terminal to install Nix:

```bash
curl -L https://nixos.org/nix/install | sh
```

### Verify the Installation

After the installation completes, verify Nix is correctly installed by running:
```bash
nix --version
```

### Enable required nix features
This project uses the experimental, but widely adopted flakes and nix-command features. To avoid having to manually enable these features with flags each time you can run the following.

```
mkdir $HOME/.config/nix/
echo "experimental-features = nix-command flakes" > "$HOME/.config/nix/nix.conf"
```

## Using Nix
The 3 most important nix features for this project are:

### `nix build` 
Reproducibly builds the project and produces an executable.  
### `nix run` 
Reproducibly builds and immediately runs the project. 
### `nix develop`
Reproducibly creates a development environment containing all tools and dependencies specified in the flake.

## CI/CD pipeline
The pipeline is based on the nix flake. Since the flake is fully reproducible you can be completely sure that if the `nix build` / `nix run` works locally it will work in the pipeline and on other people's machines.

However at the linting and formatting stage is not directly integrated into the flake. Therefore you need to run the following command to run all code checks.
```nix
nix develop --command ./scripts/check.sh
```
IMPORTANT: Run this script before every push and fix any issues it brings up. Otherwise the CI will likely fail.

Using the `nix` cli directly guarantees the project will compile in any environment. However during active development it will be a lot faster to enter the dev shell environment using `nix develop` and use `cargo` directly for compiling. This is mainly because nix does not cache compiled crates and instead reevaluates from scratch.

