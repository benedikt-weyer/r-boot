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

  # `-b` names the boot EFI r-boot-conf-builder copies into the ESP. When
  # signing, that's the freshly-signed copy the script below produces at
  # activation time ($BOOT_EFI); otherwise it's cfg.package's binary
  # directly. Either way it's a runtime shell variable, not a Nix-time
  # store path, so the unsigned case sets $BOOT_EFI too.
  builderArgs =
    ''-t ${timeoutStr} -d '${efi.efiSysMountPoint}' -b "$BOOT_EFI"''
    + " -g ${toString cfg.configurationLimit}"
    + lib.optionalString efi.canTouchEfiVariables " -e";

  # sbsign needs a certificate *file*; sign-cert may instead be inline PEM
  # content, so at runtime treat it as a path only if one actually exists
  # on disk, and fall back to spilling the string to a temp file otherwise.
  signScript = lib.optionalString cfg.sign-bootloader ''
    CLEANUP_FILES=""
    cleanup() { [ -n "$CLEANUP_FILES" ] && ${pkgs.coreutils}/bin/rm -f $CLEANUP_FILES; }
    trap cleanup EXIT

    if [ ! -f ${lib.escapeShellArg cfg.sign-pk} ]; then
      echo "r-boot: sign-bootloader is enabled but ${cfg.sign-pk} (sign-pk) is missing;" >&2
      echo "r-boot: generate it with 'r-boot-cli sign-key create' (as root) first" >&2
      exit 1
    fi

    CERT_FILE=${lib.escapeShellArg cfg.sign-cert}
    if [ ! -f "$CERT_FILE" ]; then
      CERT_FILE="$(${pkgs.coreutils}/bin/mktemp)"
      CLEANUP_FILES="$CLEANUP_FILES $CERT_FILE"
      printf '%s' ${lib.escapeShellArg cfg.sign-cert} > "$CERT_FILE"
    fi

    SIGNED_EFI="$(${pkgs.coreutils}/bin/mktemp)"
    CLEANUP_FILES="$CLEANUP_FILES $SIGNED_EFI"
    ${pkgs.sbsigntool}/bin/sbsign \
      --key ${lib.escapeShellArg cfg.sign-pk} \
      --cert "$CERT_FILE" \
      --output "$SIGNED_EFI" \
      "$BOOT_EFI"
    BOOT_EFI="$SIGNED_EFI"
  '';

  installBootLoader = pkgs.writeShellScript "install-r-boot.sh" ''
    set -e
    BOOT_EFI="${cfg.package}/EFI/BOOT/BOOTX64.EFI"
    ${signScript}
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

    sign-bootloader = lib.mkEnableOption ''
      signing r-boot.efi with sign-cert/sign-pk (Authenticode, via sbsign)
      before it's installed to the ESP, for UEFI secure boot
    '';

    sign-cert = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/r-boot/pki/db.pem";
      description = ''
        Certificate used to sign `r-boot.efi` when `sign-bootloader` is
        enabled: either the PEM-encoded certificate content directly, or a
        path to a certificate file readable at activation time. Defaults to
        the certificate `r-boot-cli sign-key create` generates.
      '';
    };

    sign-pk = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/r-boot/pki/db.key";
      description = ''
        Path to the private key used to sign `r-boot.efi` when
        `sign-bootloader` is enabled. Must stay root-owned and root-only
        readable, and must never be a Nix store path (the store is
        world-readable). Defaults to the private key `r-boot-cli sign-key
        create` generates.
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
      {
        assertion = !cfg.sign-bootloader || !lib.hasPrefix builtins.storeDir (toString cfg.sign-pk);
        message = ''
          boot.loader.r-boot.sign-pk points into the Nix store, which is
          world-readable; the secure boot private key would leak to every
          local user. Point it at a root-only path outside the store
          (default: /var/lib/r-boot/pki/db.key, from `r-boot-cli sign-key
          create`).
        '';
      }
    ];

    boot.loader.supportsInitrdSecrets = false;
    boot.loader.grub.enable = lib.mkDefault false;
    boot.loader.systemd-boot.enable = lib.mkDefault false;

    system.build.installBootLoader = installBootLoader;
    system.boot.loader.id = "r-boot";
  };
}
