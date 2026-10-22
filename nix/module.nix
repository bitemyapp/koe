{ self }:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.aizome.koe;
  settingsFile = (pkgs.formats.toml { }).generate "koe.toml" cfg.settings;
  args = lib.optionalString (cfg.settings != { }) "--config ${settingsFile} ";
in {
  options.programs.aizome.koe = {
    enable = lib.mkEnableOption "Koe voice dictation in the Aizome session";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.koe;
      description = "Koe package to run.";
    };
    settings = lib.mkOption {
      type = (pkgs.formats.toml { }).type;
      default = { };
      description = ''
        Declarative Koe configuration. Empty uses the user's
        ~/.config/aizome/koe.toml; nonempty replaces it for the service.
        Never put credentials here: Nix store files are world-readable.
        Use engine.http.api_key_env and environmentFile instead.
      '';
    };
    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "%h/.config/aizome/koe.env";
      description = "Optional user-private systemd environment file for HTTP credentials.";
    };
  };
  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    systemd.user.services.aizome-koe = {
      description = "Koe voice dictation for Aizome";
      partOf = [ "aizome-session.target" ];
      after = [ "aizome.service" "pipewire.service" ];
      wantedBy = [ "aizome-session.target" ];
      serviceConfig = {
        Type = "exec";
        ExecStart = "${cfg.package}/bin/koe ${args}daemon";
        Restart = "on-failure";
        RestartSec = 2;
        Slice = "session.slice";
        UMask = "0077";
        LimitCORE = 0;
        NoNewPrivileges = true;
        PrivateTmp = false;
        TimeoutStopSec = 10;
        EnvironmentFile = lib.optional (cfg.environmentFile != null) cfg.environmentFile;
      };
    };
  };
}
