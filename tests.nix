# https://nix.dev/tutorials/nixos/integration-testing-using-virtual-machines.html

# run intreactively using:
# `nix run .#checks.x86_64-linux.default.driverInteractive`
{
  pkgs,
  nixosModule,
}:
rec {
  # module = import ./tests/module.nix {
  #   pkgs = pkgs;
  #   nixosModule = nixosModule;
  # };
  watcher = import ./tests/watcher.nix {
    pkgs = pkgs;
    nixosModule = nixosModule;
  };
  # default = module;
}
