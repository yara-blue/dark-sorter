{
  pkgs,
  nixosModule,
}:
pkgs.testers.runNixOSTest {
  name = "Immich-integration";
  enableDebugHook = true;
  sshBackdoor.enable = true;
  extraPythonPackages = p: [ p.retrying ];
  nodes.machine =
    { pkgs, ... }:
    let
      helpers = import ./helpers.nix;
      raw = helpers.raw;
      xmp = helpers.xmp;
    in
    {
      virtualisation.graphics = false;
      virtualisation.memorySize = 8192;
      imports = [ nixosModule.default ];
      services.dark-sorter = {
        enable = true;
        source-dir = "/source";
        target-dir = "/target";
        photo-group = "photos";
        package = pkgs.dark-sorter-debug;
        immich = {
          url = "http://localhost:2283";
          api-key = "magic_api_key_for_dark_sorter_testing";
        };
      };
      # systemd.services.dark-sorter.enable = false;
      systemd.services.dark-sorter.environment = {
        RUST_BACKTRACE = "1";
        RUST_LOG = "info,dark_sorter=debug";
      };
      services.immich = {
        enable = true;
        group = "photos";
        settings = {
          library.scan.enabled = false;
          library.watch.enabled = false;
          machineLearning.enabled = false;
          # logging.enabled = false;
          # logging.level = "Warn";
        };
        machine-learning.enable = false;
        package = pkgs.immich.overrideAttrs (old: {
          #TODO use this instead https://github.com/isabelroses/nixpkgs/blob/95d5f3106884fe743b38613b0d75b098a75c1266/nixos/tests/web-apps/immich.nix#L49
          patches = (old.patches or [ ]) ++ [
            ./add_hardcoded_admin_and_api_key.patch
          ];
        });
      };
      networking.firewall.enable = false;
      # We need these files to be present BEFORE dark-sorter starts running
      # or we'll just be testing the watcher
      # See man tmpfiles.d(5) for the syntax
      systemd.tmpfiles.rules = [
        "d /source 770 root photos - -"
        "d /target 770 root photos - -"

        "C /source/rated.NEF 770 root photos - ${raw}"
        "C /source/rated.NEF.xmp 770 root photos - ${xmp "rated" 4}"
        "C /rated2.NEF 770 root photos - ${raw}"
        "C /rated2.NEF.xmp 770 root photos - ${xmp "rated2" 4}"
        "C /source/unrated.NEF 770 root photos - ${raw}"
        "C /source/unrated.NEF.xmp 770 root photos - ${xmp "unrated" 0}"
      ];
      system.stateVersion = "25.11";
    };
  # Methods available on machine objects:
  # https://nixos.org/manual/nixos/stable/index.html#ssec-machine-objects
  testScript = ''
    from typing import Any
    import json

    def wait_until_immich_ready():
        machine.wait_for_open_port(2283) # immich is ready
        machine.wait_until_succeeds("curl --fail --silent http://localhost:2283/api/libraries -H x-api-key:magic_api_key_for_dark_sorter_testing", timeout=20)

    def get_one_or_more_libs_from_immich() -> list[dict[str, Any]]:
        cmd = r"""
            while true; do
                libs="$(curl --fail --silent \
                    http://localhost:2283/api/libraries \
                    -H x-api-key:magic_api_key_for_dark_sorter_testing)" 

                if [ $? -ne 0 ]; then
                    sleep 0.1
                    continue
                fi

                if [ "$libs" == "[]" ]; then
                    sleep 0.1
                    continue
                fi

                echo "$libs"
                break
            done
        """

        libs = machine.succeed(cmd, timeout=20)
        libs = json.loads(libs)
        return libs

    def get_zero_libs_from_immich():
        cmd = r"""
            while true; do
                libs="$(curl --fail --silent \
                    http://localhost:2283/api/libraries \
                    -H x-api-key:magic_api_key_for_dark_sorter_testing)" 

                if [ $? -ne 0 ]; then
                    sleep 0.1
                    continue
                fi

                if [ "$libs" != "[]" ]; then
                    sleep 0.1
                    continue
                fi

                break
            done
        """

        machine.succeed(cmd, timeout=20)
        return

    print("SETUP 1 #########################################################")
    wait_until_immich_ready()
    machine.wait_until_succeeds("test -f /target/rated.jpg", timeout=60)


    # TEST 1: should create an immich library
    print("TEST 1 ##########################################################")
    libs = get_one_or_more_libs_from_immich()
    import_path = libs[0]["importPaths"][0]
    assert import_path == "/target"


    # TEST 2: should remove the library as it got emptied
    print("TEST 2 ##########################################################")
    machine.succeed("sed -i 's/xmp:Rating=\"4\"/xmp:Rating=\"0\"/' /source/rated.NEF.xmp")
    get_zero_libs_from_immich()


    # TEST 3: add a library pointing to a subfolder
    print("TEST 3 ##########################################################")
    machine.succeed("mkdir /source/subdir")
    machine.succeed("chgrp photos /source/subdir")
    machine.succeed("sudo cp -p /rated2.NEF /source/subdir/")
    machine.succeed("sudo cp -p /rated2.NEF.xmp /source/subdir/")

    machine.wait_until_succeeds("test -f /target/subdir/rated2.jpg", timeout=60)
    libs = get_one_or_more_libs_from_immich()
    import_path = libs[0]["importPaths"][0]
    print(f"************************************ pathhhhhh issss {import_path}")
    assert import_path == "/target/subdir"
  '';
}
