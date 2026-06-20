# Placeholder copied from break-enforcer
{
  config,
  lib,
  pkgs,
  ...
}:

with lib;
with lib.types;
let
  cfg = config.services.dark-sorter;
in
{
  options = {
    services.dark-sorter = {
      enable = mkEnableOption "dark-sorter";
      source-dir = mkOption {
        type = types.path;
      };
      target-dir = mkOption {
        type = types.path;
      };
    };
  };

  config = mkIf cfg.enable {
    systemd.services.dark-sorter = {
      description = "Maintains a sibling folder structure of symlinks";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        Type = "simple";
        ExecStart = ''
          ${pkgs.dark-sorter}/bin/dark-sorter \
          --source-dir ${cfg.source-dir} \
          --target-dir ${cfg.target-dir} \
		  --daemon
        '';
      };
    };
  };
}
