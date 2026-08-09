{ rBootPackage }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.boot.loader.r-boot;
  efi = config.boot.loader.efi;

  timeoutStr = if config.boot.loader.timeout == null then "0" else toString config.boot.loader.timeout;

  builderArgs =
    "-t ${timeoutStr} -d '${efi.efiSysMountPoint}' -b '${cfg.package}/EFI/BOOT/BOOTX64.EFI'"
    + " -g ${toString cfg.configurationLimit}"
    + lib.optionalString efi.canTouchEfiVariables " -e";

  installBootLoader = pkgs.writeShellScript "install-r-boot.sh" ''
    set -e
    ${cfg.package}/bin/r-boot-conf-builder ${builderArgs} -c "$@"
  '';
in
{
  options.boot.loader.r-boot = {
    enable = lib.mkEnableOption "the r-boot UEFI bootloader";

    package = lib.mkOption {
      type = lib.types.package;
      default = rBootPackage;
      defaultText = lib.literalExpression "r-boot's flake `packages.<system>.default`";
      description = ''
        The r-boot package providing `EFI/BOOT/BOOTX64.EFI`, installed to the
        EFI system partition.
      '';
    };

    configurationLimit = lib.mkOption {
      type = lib.types.int;
      default = 20;
      example = 10;
      description = ''
        Maximum number of older generations listed in the r-boot menu, in
        addition to the current one. Older boot files beyond this limit are
        removed from the EFI system partition on activation.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = pkgs.stdenv.hostPlatform.isx86_64;
        message = "r-boot only supports x86_64 UEFI systems.";
      }
      {
        assertion = !config.boot.loader.grub.enable && !config.boot.loader.systemd-boot.enable;
        message = "boot.loader.r-boot cannot be used together with GRUB or systemd-boot.";
      }
    ];

    boot.loader.supportsInitrdSecrets = false;
    boot.loader.grub.enable = lib.mkDefault false;
    boot.loader.systemd-boot.enable = lib.mkDefault false;

    system.build.installBootLoader = installBootLoader;
    system.boot.loader.id = "r-boot";
  };
}
