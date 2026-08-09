{ lib, pkgs }:

pkgs.replaceVarsWith {
  src = ./r-boot-conf-builder.sh;
  isExecutable = true;
  replacements = {
    path = lib.makeBinPath [
      pkgs.coreutils
      pkgs.gnused
      pkgs.gnugrep
      pkgs.util-linux
      pkgs.efibootmgr
    ];
    inherit (pkgs) bash;
  };
}
