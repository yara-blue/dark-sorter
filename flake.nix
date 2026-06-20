{
  description = "Symlink rated photos from darktable into another folder";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      self,
    }:
    # TODO use flake parts?
    # https://nixos-and-flakes.thiscute.world/other-usage-of-flakes/outputs
    let
      pkgs = nixpkgs.legacyPackages.x86_64-linux.extend rust-overlay.overlays.default;
    in
    {
      packages.x86_64-linux.default = (import ./package.nix) {
        pkgs = pkgs;
        rust-overlay = rust-overlay;
      };
      devShells.x86_64-linux.default = (import ./devshell.nix) {
        pkgs = pkgs;
        package = self.packages.x86_64-linux.default;
      };
      checks.x86_64-linux.default = (import ./tests.nix) {
        pkgs = pkgs.extend self.overlays.default;
        nixosModule = self.nixosModule;
      };
      overlays.default = final: prev: {
        dark-sorter = self.packages.x86_64-linux.default;
      };
      nixosModule = import ./nix_module.nix;
    };
}
