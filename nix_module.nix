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
      package = lib.mkPackageOption pkgs "dark-sorter" { };
      photo-group = mkOption {
        type = types.str;
        description = ''
            Group for all the files and links created by dark-sorter. Needs to
          	have read access to the raw and xmp files and be able to
            write to the target directory.'';
      };
      # TODO modify clap so we can generate this? gotta think about that for a
      # bit... Like arg groups gotta generate attribute sets.
      immich = {
        url = mkOption {
          type = types.str;
        };
        api-key = mkOption {
          type = types.str;
        };
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
    # pkgs.dark-sorter.debug = true; // TODO make this work

    systemd.services.dark-sorter = {
      description = "Maintains a sibling folder structure of symlinks";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        Type = "simple";
        ExecStart = ''
                              ${lib.getExe cfg.package} \
                              --source-dir ${cfg.source-dir} \
                              --target-dir ${cfg.target-dir} \
                              --user ${cfg.user} \
                              ${
                                lib.optionalString (cfg.photo-group != null) "--photo-group ${cfg.photo-group}"
                              } \
          					${
                 lib.optionalString (
                   cfg.immich != null
                 ) "
					  --immich-url ${cfg.immich.url} \
					  --immich-api-key ${cfg.immich.api-key} \
				  "
               }
                              --daemon
        '';
      };
    };
  };
}
