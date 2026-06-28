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
      user = lib.mkOption {
        type = types.str;
        default = "dark-sorter";
        description = "The user dark-sorter creates files as";
      };
      photo-group = mkOption {
        type = types.str;
        description = ''
          			Group for all the files and links created by dark-sorter. Needs to
          			have access to the raw and xmp files as well.
          		'';
      };
    };
  };

  config = mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      useDefaultShell = true; # TODO remove
      description = "User to run darktable under";
      group = "${cfg.photo-group}";
    };
    users.groups.${cfg.photo-group} = { };
    systemd.services.dark-sorter = {
      description = "Maintains a sibling folder structure of symlinks";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];
      environment = {
        RUST_BACKTRACE = "full";
		RUST_LOG = "debug";
      };

      serviceConfig = {
        Type = "simple";
        ExecStart = ''
                              ${pkgs.dark-sorter}/bin/dark-sorter \
                              --source-dir ${cfg.source-dir} \
                              --target-dir ${cfg.target-dir} \
          					--user ${cfg.user} \
          					--photo-group ${cfg.photo-group} \
                    		  --daemon
        '';
      };
    };
  };
}
