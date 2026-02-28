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
            crank = craneLib.buildPackage (commonArgs // {
              inherit cargoArtifacts;
              # Tests use $HOME/.crank which doesn't work in nix sandbox
              # Run tests via `just test` before committing instead
              doCheck = false;
            });
            mkCrankAlertBadge = { xcodeBaseDir ? "/Applications/Xcode.app" }:
              let
                xcodeWrapper = pkgs.xcodeenv.composeXcodeWrapper {
                  inherit xcodeBaseDir;
                };
              in
              pkgs.stdenvNoCC.mkDerivation {
                pname = "crank-alert-badge";
                version = "0.1.0";
                src = ./apps/crank-alert-badge;
                nativeBuildInputs = [ xcodeWrapper ];
                __noChroot = true;

                buildPhase = ''
                  export HOME="$TMPDIR"
                  export DEVELOPER_DIR="${xcodeBaseDir}/Contents/Developer"
                  export MACOSX_DEPLOYMENT_TARGET=13.0
                  ${xcodeWrapper}/bin/xcrun --sdk macosx swiftc \
                    -O \
                    -framework AppKit \
                    -o CrankAlertBadge \
                    Sources/main.swift
                '';

                installPhase = ''
                  app="$out/Applications/CrankAlertBadge.app"
                  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
                  cp CrankAlertBadge "$app/Contents/MacOS/"

                  if [ -f AppIcon.icns ]; then
                    cp AppIcon.icns "$app/Contents/Resources/"
                  fi

                  cat > "$app/Contents/Info.plist" << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>CrankAlertBadge</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>com.crank.alert-badge</string>
    <key>CFBundleName</key>
    <string>CrankAlertBadge</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
EOF

                  ${xcodeWrapper}/bin/codesign --force --deep --sign - "$app"
                '';

                meta = with pkgs.lib; {
                  description = "Crank alert badge menu bar app";
                  platforms = platforms.darwin;
                  hydraPlatforms = [];
                };
              };
            crankAlertBadge =
              if pkgs.stdenv.isDarwin then mkCrankAlertBadge { } else null;
          in
          f {
            inherit pkgs system rustToolchain craneLib crank crankAlertBadge mkCrankAlertBadge;
          }
        );
    in
    {
      packages = forAllSystems ({ crank, crankAlertBadge, pkgs, ... }: {
        default = crank;
        crank = crank;
      } // (pkgs.lib.optionalAttrs pkgs.stdenv.isDarwin {
        crank-alert-badge = crankAlertBadge;
      }));

      lib = forAllSystems ({ mkCrankAlertBadge, ... }: {
        mkCrankAlertBadge = mkCrankAlertBadge;
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
