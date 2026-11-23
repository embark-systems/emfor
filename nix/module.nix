# NixOS module for the emfor daemon. The flake exposes it as nixosModules.default
# and fills in `package`; the unit follows packages/common/emfor.service.
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.emfor;
  toml = pkgs.formats.toml { };
  configFile = toml.generate "emfor.toml" cfg.settings;
  etcPath = "emfor/emfor.toml";
  configPath = "/etc/${etcPath}";
in
{
  options.services.emfor = {
    enable = lib.mkEnableOption "the emfor health-check daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.emfor;
      defaultText = lib.literalExpression "emfor.packages.\${system}.emfor";
      description = "The emfor package that provides the daemon.";
    };

    settings = lib.mkOption {
      type = toml.type;
      default = { };
      example = lib.literalExpression ''
        {
          site = "dc1";
          dns = {
            listen = [ "127.0.0.1:5300" ];
            zone = "svc.emfor";
          };
          services."api.svc.emfor" = {
            check = "tcp";
            backends = [ "10.0.0.1:8080" ];
          };
        }
      '';
      description = ''
        Contents of emfor.toml. See the emfor manual, chapter 5, for the keys. The file is
        written to the Nix store and linked to ${configPath}; `emfor check-config` runs
        against it when the unit starts and when it reloads.

        Secrets (a peer secret, a check password) do not belong here, since the store is
        readable by every user. Point the options that read a file at one outside the store.
      '';
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "emfor";
      description = ''
        Group of the daemon. With the default `[store] file_mode` of 0640, the users of
        applications that use an emfor client have to be in this group.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.emfor = {
      isSystemUser = true;
      group = cfg.group;
      description = "emfor health-check daemon";
    };
    users.groups.${cfg.group} = { };

    # A link to the store file: root-owned, mode 0444, and /etc/emfor itself is a
    # root-owned directory, which is what emfor demands of its config.
    environment.etc.${etcPath}.source = configFile;
    environment.systemPackages = [ cfg.package ];

    systemd.services.emfor = {
      description = "emfor health-check daemon";
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      # Reload when the config changes instead of restarting.
      reloadTriggers = [ configFile ];
      unitConfig = {
        StartLimitIntervalSec = 60;
        StartLimitBurst = 5;
      };
      serviceConfig = {
        Type = "exec";
        User = "emfor";
        Group = cfg.group;
        ExecStartPre = "${cfg.package}/bin/emfor check-config --config ${configPath}";
        ExecStart = "${cfg.package}/bin/emfor server --config ${configPath}";
        ExecReload = [
          "${cfg.package}/bin/emfor check-config --config ${configPath}"
          "${pkgs.coreutils}/bin/kill -HUP $MAINPID"
        ];
        # 1 (address in use, store held by another emfor) and 2 (invalid config)
        # do not go away by trying again.
        Restart = "on-failure";
        RestartSec = "2s";
        RestartPreventExitStatus = "1 2";
        LimitNOFILE = 65536;
        # /var/lib/emfor, mode 0755 so clients can reach the store. Not DynamicUser=:
        # that puts the directory under /var/lib/private (mode 0700).
        StateDirectory = "emfor";
        StateDirectoryMode = "0755";
        NoNewPrivileges = true;
        UMask = "0027";
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectProc = "invisible";
        ProcSubset = "pid";
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        ProtectHostname = true;
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        RestrictAddressFamilies = "AF_UNIX AF_INET AF_INET6";
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" ];
        CapabilityBoundingSet = "";
      };
    };
  };
}
