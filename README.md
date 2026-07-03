Given a file tree with raw files creates and maintains file tree containing
up-to-date jpeg exports of those raw files that are rated. Only edits made by
_Darktable_ are supported, though both _Darktable_ and _Lightroom_ ratings
_should_ work given they both use `XMP` files. 

Optionally integrates Immich creating external libraries for each folder in the
mirror directory structure and triggering selective rescans when needed.

# Usage
See `--help` or the options in the nixos module.
