Given a file tree with raw files creates and maintains file tree containing
up-to-date jpeg exports of those raw files that are rated. Only edits made by
_Darktable_ are supported, though both _Darktable_ and _Lightroom_ ratings
_should_ work given they both use `XMP` files. 

Optionally integrates Immich creating external libraries for each folder in the
mirror directory structure and triggering selective rescans when needed.

## Why I build this
I too many pictures each time I go out with my camera. The raws all move to our
NAS when I get home to _at some point_ get rated. Those I really like I'll edit.
What I dread though is exporting the rated/edited pictures and importing them to
jpeg. I am lazy to the point that I spend (too) many hours automating that.
Let's hope this stays working for the next few decades or I'll never get my time
back... :3

## Limitations
I have a Nikon camera so this only works for `.NEF` files right now.

# Usage
Note that this needs to run as root when using `--daemon` for more options see
`--help` or the options in the NixOS module.

When using the daemon it is recommended to have the photos on their own file
system. This makes file watching more efficient.

# Install
## NixOS flakes
1. Add as input
```nix
inputs = {
  # long list of existing modules
  dark-sorter.url = "github:yara-blue/dark-sorter";
};
```
2. Pass the module to nixosSystem
```nix
# somewhere in your config
lib.nixosSystem {
  specialArgs = { inherit inputs };
  system = system;
  modules = [
    ... # big list of existing modules
    inputs.dark-sorter.nixosModules.default
  ];
```
3. Configure the service
```
# somewhere in your config
services.dark-sorter = {
    enable = true;
    source-dir = /srv/photos/raws;
    target-dir = /srv/photos/jpgs;
    user = "dark-sorter"; # this is also the default
    # has read access to raws and read+write to jpgs
    photo-group = "photos"; 
    # optional
    immich = {
        url-path = <>;
        api-key-path = <>;
    };
};
```


## Other
Good luck! you should only need `darktable-cli` and `nice` available on your
path. Then compile with Cargo build --release.

## Future work
- sync ratings between xmp's and immich
- sync nondestructive edits from immich to xmp's
