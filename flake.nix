{
  description = "crank - Local merge queue CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crane, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system:
          let
            overlays = [ (import rust-overlay) ];
            pkgs = import nixpkgs { inherit system overlays; };
            rustToolchain = pkgs.rust-bin.stable.latest.default.override {
              extensions = [ "rust-src" "rust-analyzer" ];
            };
            craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

            commonArgs = {
              src = craneLib.cleanCargoSource ./.;
              strictDeps = true;
              buildInputs = with pkgs; [
                openssl
                pkg-config
              ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin (with pkgs.darwin.apple_sdk.frameworks; [
                Security
                SystemConfiguration
              ]);
              nativeBuildInputs = with pkgs; [ pkg-config ];
            };

            cargoArtifacts = craneLib.buildDepsOnly commonArgs;
            crank = craneLib.buildPackage (commonArgs // { inherit cargoArtifacts; });
          in
          f { inherit pkgs system rustToolchain craneLib crank; }
        );
    in
    {
      packages = forAllSystems ({ crank, ... }: {
        default = crank;
        crank = crank;
      });

      devShells = forAllSystems ({ pkgs, rustToolchain, ... }: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            rustToolchain
            pkg-config
            openssl
            cargo-watch
            just
            git
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin (with pkgs.darwin.apple_sdk.frameworks; [
            Security
            SystemConfiguration
          ]);
          shellHook = ''
            export IN_NIX_SHELL=1
          '';
        };
      });
    };
}
