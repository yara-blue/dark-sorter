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
        # TODO do some validation to make sure only one of these is present
        url = mkOption {
          type = types.nullOr types.str;
		  default = null;
		};
        url-path = mkOption {
          type = types.nullOr types.path;
		  default = null;
		};
        api-key = mkOption {
          type = types.nullOr types.str;
          default = null;
        };
		# TODO use the _secret for file thing that the immich module itself has
        api-key-path = mkOption { 
          type = types.nullOr types.path;
          default = null;
        };
      };
    };
  };

  config = mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      description = "User to run darktable under";
      group = "${cfg.photo-group}";
    };
    users.groups.${cfg.photo-group} = { };

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
          ${lib.optionalString (cfg.photo-group != null) "--photo-group ${cfg.photo-group}"} \
          ${
            lib.optionalString (
              cfg.immich != null && cfg.immich.url != null
            ) "--immich-url ${cfg.immich.url}"
          } \
          ${
            lib.optionalString (
              cfg.immich != null && cfg.immich.url-path != null
            ) "--immich-url-path ${cfg.immich.url-path}"
          } \
          ${
            lib.optionalString (
              cfg.immich != null && cfg.immich.api-key != null
            ) "--immich-api-key ${cfg.immich.api-key}"
          } \
          ${
            lib.optionalString (
              cfg.immich != null && cfg.immich.api-key-path != null
            ) "--immich-api-key-path ${cfg.immich.api-key-path}"
          } \
          --daemon
        '';
      };
    };
  };
}
