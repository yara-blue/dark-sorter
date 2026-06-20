# https://nix.dev/tutorials/nixos/integration-testing-using-virtual-machines.html

# run intreactively using:
# `nix run .#checks.x86_64-linux.default.driverInteractive`

{
  pkgs,
  nixosModule,
}:
pkgs.testers.runNixOSTest {
  name = "test-name";
  enableDebugHook = true;
  sshBackdoor.enable = true;
  nodes.machine =
    { ... }:
    {
      imports = [ nixosModule ];
      users.users.alice = {
        isNormalUser = true;
        extraGroups = [ "wheel" ];
      };
      services.dark-sorter = {
        enable = true;
        source-dir = "/source";
        target-dir = "/target";
      };
      system.stateVersion = "25.11";
    };
  # Methods available on machine objects:
  # https://nixos.org/manual/nixos/stable/index.html#ssec-machine-objects
  testScript = ''
    machine.succeed("mkdir /source")
    machine.succeed("mkdir /target")
    machine.copy_from_host("${./test-assets/RAW_NIKON_D1.NEF}", "/source/RAW_NIKON_D1.NEF")
    machine.copy_from_host("${./test-assets/RAW_NIKON_D1.NEF.xmp}", "/source/RAW_NIKON_D1.NEF.xmp")
    machine.wait_for_unit("default.target")
    machine.wait_for_file("/source/RAW_NIKON_D1.jpg", 20)
    symlink = machine.wait_until_succeeds("realpath /target/RAW_NIKON_D1.jpg", 2)
    assert symlink == "/source/RAW_NIKON_D1.jpg"
  '';
}
