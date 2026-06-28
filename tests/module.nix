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
      imports = [ nixosModule.default ];
      services.dark-sorter = {
        enable = true;
        source-dir = "/source";
        target-dir = "/target";
        photo-group = "photos";
      };
      system.stateVersion = "25.11";
    };
  # Methods available on machine objects:
  # https://nixos.org/manual/nixos/stable/index.html#ssec-machine-objects
  testScript = ''
from time import sleep
from pathlib import Path

# setup
machine.succeed("mkdir /source --mode 770")
machine.succeed("mkdir /target --mode 770")
machine.succeed("chgrp photos /source")
machine.succeed("chgrp photos /target")

# rated file already in place at start (scan should detect this one)
machine.copy_from_host("${./assets/small_raw.NEF}", "/source/a.NEF")
machine.copy_from_host("${./assets/rated.NEF.xmp}", "/source/a.NEF.xmp")
machine.succeed("sed -i 's/<FILENAME>/a/' /source/a.NEF.xmp")

# rated file that will be moved in (new files).
machine.copy_from_host("${./assets/small_raw.NEF}", "/b.NEF")
machine.copy_from_host("${./assets/rated.NEF.xmp}", "/b.NEF.xmp")
machine.succeed("sed -i 's/<FILENAME>/b/' /b.NEF.xmp")

# unrated file already in place at start (scan should skip)
machine.copy_from_host("${./assets/small_raw.NEF}", "/source/c.NEF")
machine.copy_from_host("${./assets/unrated.NEF.xmp}", "/source/c.NEF.xmp")
machine.succeed("sed -i 's/<FILENAME>/c/' /source/c.NEF.xmp")


machine.wait_for_unit("default.target")


# # TEST 1: scan creates needed links and previews
# machine.wait_for_file("/source/a.jpg", 20)
# symlink = machine.wait_until_succeeds("realpath /target/a.jpg", 2)
# assert symlink == "/source/a.jpg"
sleep(5)


# TEST 2: watcher notices new files
machine.succeed("mv /b.NEF /source/b.NEF")
machine.succeed("mv /b.NEF.xmp /source/b.NEF.xmp")
machine.wait_for_file("/source/b.jpg", 20)
symlink = machine.wait_until_succeeds("realpath /target/b.jpg", 2)
assert symlink == "/source/b.jpg"


# TEST 3: watcher notices rating changing
assert not Path("/target/c.jpg").exists()
machine.succeed("sed -i 's/xmp:Rating=\"0\"/xmp:Rating=\"4\"/' /source/c.NEF.xmp")
machine.wait_for_file("/source/c.jpg", 20)
symlink = machine.wait_until_succeeds("realpath /target/c.jpg", 2)
assert symlink == "/source/c.jpg"
'';
    }
