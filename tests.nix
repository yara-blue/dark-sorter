# https://nix.dev/tutorials/nixos/integration-testing-using-virtual-machines.html

# run intreactively using:
# `nix run .#checks.x86_64-linux.default.driverInteractive`
# instead of `default` also try `watcher`
{
  pkgs,
  nixosModule,
}:
rec {
  module = import ./tests/scan.nix {
    pkgs = pkgs;
    nixosModule = nixosModule;
  };
  watcher = import ./tests/watcher.nix {
    pkgs = pkgs;
    nixosModule = nixosModule;
  };
  # immich_integration = import ./tests/watcher.nix {
  #   pkgs = pkgs;
  #   nixosModule = nixosModule;
  # };
  default = module;
}
