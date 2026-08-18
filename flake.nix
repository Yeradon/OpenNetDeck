{
  description = "OpenNetDeck: Open-source Elgato Network Dock reimplementation in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "opennetdeck";
          version = "0.1.0";
          src = ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.libusb1 ]
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.udev ]
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin (
              if pkgs ? darwin && pkgs.darwin ? apple_sdk then
                [
                  pkgs.darwin.apple_sdk.frameworks.IOKit
                  pkgs.darwin.apple_sdk.frameworks.CoreFoundation
                  pkgs.darwin.apple_sdk.frameworks.AppKit
                ]
              else
                [ ]
            );
        };

        checks = {
          build = self.packages.${system}.default;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            pkg-config
            libusb1
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.udev ]
          ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin (
            if pkgs ? darwin && pkgs.darwin ? apple_sdk then
              [
                pkgs.darwin.apple_sdk.frameworks.IOKit
                pkgs.darwin.apple_sdk.frameworks.CoreFoundation
                pkgs.darwin.apple_sdk.frameworks.AppKit
              ]
            else
              [ ]
          );
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      }
    );
}
