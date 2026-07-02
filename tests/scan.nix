{
  pkgs,
  nixosModule,
}:
pkgs.testers.runNixOSTest {
  name = "Scan";
  enableDebugHook = true;
  sshBackdoor.enable = true;
  nodes.machine =
    { ... }:
    let
      helpers = import ./helpers.nix;
      raw = helpers.raw;
      xmp = helpers.xmp;
    in
    {
      imports = [ nixosModule.default ];
      services.dark-sorter = {
        enable = true;
        source-dir = "/source";
        target-dir = "/target";
        photo-group = "photos";
        package = pkgs.dark-sorter-debug;
      };
      systemd.services.dark-sorter.environment = {
        RUST_BACKTRACE = "1";
        RUST_LOG = "debug";
      };
      # We need these files to be present BEFORE dark-sorter starts running
      # or we'll just be testing the watcher
      # See man tmpfiles.d(5) for the syntax
      systemd.tmpfiles.rules = [
        "d /source 770 root photos - -"
        "d /target 770 root photos - -"

        "C /source/rated.NEF 770 root photos - ${raw}"
        "C /source/rated.NEF.xmp 770 root photos - ${xmp "rated" 4}"
        "C /source/unrated.NEF 770 root photos - ${raw}"
        "C /source/unrated.NEF.xmp 770 root photos - ${xmp "unrated" 0}"
      ];
      system.stateVersion = "25.11";
    };
  # Methods available on machine objects:
  # https://nixos.org/manual/nixos/stable/index.html#ssec-machine-objects
  testScript = ''
    machine.wait_for_unit("default.target")

    # scan should create preview for rated file
    machine.wait_until_succeeds("test -f /target/rated.jpg", 20)

    # scan should not create preview for unrated file
    machine.fail("test -f /target/unrated.jpg")
  '';
}
