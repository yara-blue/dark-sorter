{
  description = "Symlink rated photos from darktable into another folder";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        dark-sorter =
          with pkgs;
          let
            src = ./.;

            cargoTOML = lib.importTOML "${src}/Cargo.toml";
            rustToolchain = rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
            rust = makeRustPlatform {
              cargo = rustToolchain;
              rustc = rustToolchain;
            };
          in
          rust.buildRustPackage {
            pname = cargoTOML.package.name;
            version = cargoTOML.package.version;

            inherit src;

            nativeBuildInputs = [ makeWrapper ];

            cargoLock = {
              lockFile = "${src}/Cargo.lock";
            };

            meta = {
              inherit (cargoTOML.package) description homepage;
              maintainers = cargoTOML.package.authors;
            };

            postInstall = ''
              wrapProgram $out/bin/dark-sorter --prefix PATH: ${
                lib.makeBinPath [
                  darktable # for darktable-cli
                  coreutils # for nice
                ]
              }
            '';
          };

        devShell =
          with pkgs;
          mkShell {
            name = "dark-sorter";
            inputsFrom = [ dark-sorter ];
            RUST_SRC_PATH = "${rustPlatform.rustLibSrc}";
            CARGO_TERM_COLOR = "always";
          };
      in
      {
        devShells.default = devShell;
        defaultPackage = dark-sorter;
      }
    )
    // {
      overlays.default = _: prev: {
        dark-sorter = self.defaultPackage.${prev.system};
      };
      nixosModules.dark-sorter = ./nix_module.nix;
    };
}
