{
  pkgs,
  nixosModule,
}:

pkgs.testers.runNixOSTest {
  name = "Watcher";
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
      systemd.tmpfiles.rules = [
        "d /source 770 root photos - -"
        "d /target 770 root photos - -"

        "C /rated.NEF 770 root photos - ${raw}"
        "C /rated.NEF.xmp 770 root photos - ${xmp "rated" 4}"
        "C /starts_unrated.NEF 770 root photos - ${raw}"
        "C /starts_unrated.NEF.xmp 770 root photos - ${xmp "starts_unrated" 0}"
      ];
      system.stateVersion = "25.11";
    };
  # Methods available on machine objects:
  # https://nixos.org/manual/nixos/stable/index.html#ssec-machine-objects
  testScript = ''
    from time import sleep

    # setup
    machine.wait_for_unit("default.target")

    # TEST 1: watcher notices new files
    machine.succeed("mv /rated.NEF /source/rated.NEF")
    machine.succeed("mv /rated.NEF.xmp /source/rated.NEF.xmp")
    machine.wait_for_file("/target/rated.jpg", 20)

    # TEST 2: watcher notices file getting rated
    machine.succeed("mv /starts_unrated.NEF /source/starts_unrated.NEF")
    machine.succeed("mv /starts_unrated.NEF.xmp /source/starts_unrated.NEF.xmp")
    machine.succeed("sed -i 's/xmp:Rating=\"0\"/xmp:Rating=\"4\"/' /source/starts_unrated.NEF.xmp")
    machine.wait_for_file("/target/starts_unrated.jpg", 20)

    # TEST 3: watcher notices file rating getting removed
    machine.succeed("sed -i 's/xmp:Rating=\"4\"/xmp:Rating=\"0\"/' /source/starts_unrated.NEF.xmp")
    sleep(1)
    machine.fail("test -f /target/starts_unrated.jpg")
  '';
}
