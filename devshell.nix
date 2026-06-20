{ pkgs, package }:
pkgs.mkShell {
  name = "dark-sorter";
  # Takes a derivation, adds the build dependencies from it
  inputsFrom = [ package ];
  # RUST_SRC_PATH = "${rustPlatform.rustLibSrc}";
  CARGO_TERM_COLOR = "always";
}
