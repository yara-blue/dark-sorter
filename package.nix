{
  pkgs,
  rust-overlay,
  ...
}:

let
  # pkgs = pkgs2.extend rust-overlay;
  src = ./.;
  cargoTOML = pkgs.lib.importTOML "${src}/Cargo.toml";
  rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
  rust = pkgs.makeRustPlatform {
    cargo = rustToolchain;
    rustc = rustToolchain;
  };
in
rust.buildRustPackage {
  pname = cargoTOML.package.name;
  version = cargoTOML.package.version;

  inherit src;

  nativeBuildInputs = [ pkgs.makeWrapper ];

  cargoLock = {
    lockFile = "${src}/Cargo.lock";
    outputHashes = {
      "fanotify-fid-0.4.1" = "sha256-bJPs8bt/HDZVo6OTPg+zlwmo3GgIGSXb7KReKWz1DBQ=";
    };
  };

  meta = {
    inherit (cargoTOML.package) description homepage;
    maintainers = cargoTOML.package.authors;
  };

  postInstall = ''
    wrapProgram $out/bin/dark-sorter --prefix PATH: ${
      pkgs.lib.makeBinPath [
        pkgs.darktable # for darktable-cli
        pkgs.coreutils # for nice
      ]
    }
  '';
}
