{
  pkgs,
  nixosModule,
}:
pkgs.testers.runNixOSTest {
  name = "Immich-integration";
  enableDebugHook = true;
  sshBackdoor.enable = true;
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
        "C /source/unrated.NEF 770 root photos - ${raw}"
        "C /source/unrated.NEF.xmp 770 root photos - ${xmp "unrated" 0}"
      ];
      system.stateVersion = "25.11";
    };
  # Methods available on machine objects:
  # https://nixos.org/manual/nixos/stable/index.html#ssec-machine-objects
  testScript = ''
# import time
import json

machine.wait_for_open_port(2283) # immich is ready
machine.wait_until_succeeds("test -f /target/rated.jpg", timeout=60)
libs = machine.wait_until_succeeds("curl --fail --silent http://localhost:2283/api/libraries -H x-api-key:magic_api_key_for_dark_sorter_testing", timeout=20)
libs = json.loads(libs)

import_path = libs[0]["importPaths"][0]
machine.log(import_path)
print(f"**************************** {import_path}")
assert import_path == "/target"
machine.log("test done")
print("******************************")
machine.shutdown()
#
# machine.shell_interact()

# # scan should create preview for rated file
# machine.wait_until_succeeds("test -f /target/rated.jpg", 20)
#
# # scan should not create preview for unrated file
# machine.fail("test -f /target/unrated.jpg")
'';
}
