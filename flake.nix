{
  description = "crank - unattended governor for long-running coding plans";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
  }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        src = pkgs.lib.cleanSource ./.;

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "rustfmt"
            "clippy"
          ];
        };

        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };

        cargoDeps = rustPlatform.importCargoLock {
          lockFile = ./Cargo.lock;
        };

        mkCargoCheck =
          name: cmd:
          pkgs.stdenv.mkDerivation {
            inherit name src;
            inherit cargoDeps;
            nativeBuildInputs = [
              rustToolchain
              rustPlatform.cargoSetupHook
            ];
            dontConfigure = true;
            buildPhase = ''
              runHook preBuild
              export HOME="$PWD/.home"
              export CARGO_HOME="$PWD/.cargo-home"
              export CARGO_TARGET_DIR="$PWD/target"
              mkdir -p "$HOME" "$CARGO_HOME" "$CARGO_TARGET_DIR"
              ${cmd}
              runHook postBuild
            '';
            installPhase = ''
              mkdir -p "$out"
            '';
          };

        crankPkg = rustPlatform.buildRustPackage {
          pname = "crank";
          version = "0.1.0";
          inherit src;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
        };
      in
      {
        packages.default = crankPkg;

        checks = {
          build = crankPkg;
          test = mkCargoCheck "crank-test" "cargo test --frozen --locked";
          fmt = mkCargoCheck "crank-fmt" "cargo fmt --all -- --check";
          clippy = mkCargoCheck "crank-clippy" "cargo clippy --frozen --all-targets";
        };

        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
            pkgs.just
            pkgs.git
            pkgs.jq
          ];

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

          shellHook = ''
            export CARGO_TERM_COLOR=always
            export RUST_BACKTRACE=1
          '';
        };

        formatter = pkgs.nixfmt;
      }
    );
}
