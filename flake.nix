{
  description = "Rust-only UEFI Limine-protocol bootloader development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      system = "x86_64-linux";
      overlays = [ (import rust-overlay) ];
      pkgs = import nixpkgs { inherit system overlays; };
      rust = pkgs.rust-bin.stable.latest.default.override {
        targets = [ "x86_64-unknown-uefi" ];
      };
      r-boot = pkgs.callPackage ./nix/package.nix { inherit rust; };
    in {
      packages.${system}.default = r-boot;

      # Import into a NixOS system flake as `inputs.r-boot.nixosModules.default`
      # and set `boot.loader.r-boot.enable = true;` to boot that system with
      # r-boot instead of GRUB or systemd-boot.
      nixosModules.default = import ./nix/module.nix { rBootPackage = r-boot; };

      devShells.${system}.default = pkgs.mkShell {
        packages = [ rust pkgs.qemu pkgs.OVMF pkgs.mtools pkgs.dosfstools pkgs.curl pkgs.gnumake ];
        OVMF_CODE = "${pkgs.OVMF.fd}/FV/OVMF_CODE.fd";
      };
    };
}
