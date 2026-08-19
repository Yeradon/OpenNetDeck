{
  description = "OpenNetDeck: Open-source Elgato Network Dock reimplementation in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
    in
    flake-utils.lib.eachSystem supportedSystems (system:
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
          buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
            pkgs.apple-sdk
          ];
          postFixup = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
            for bin in $out/bin/*; do
              if [ -f "$bin" ]; then
                for dylib in $(otool -L "$bin" 2>/dev/null | awk 'NR>1 {print $1}' | grep libiconv); do
                  install_name_tool -change "$dylib" "/usr/lib/libiconv.2.dylib" "$bin"
                done
              fi
            done
          '';
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
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.apple-sdk ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      }
    );
}
