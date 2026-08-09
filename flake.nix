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
      # A minimal NixOS system used to smoke-test the `boot.loader.r-boot`
      # module end to end: `scripts/build-nixos-image` turns this into a disk
      # image via nixpkgs' make-disk-image.nix, and `scripts/run-nixos-image-qemu`
      # boots that image under OVMF.
      nixosConfigurations.r-boot-qemu = nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [
          self.nixosModules.default
          ({ modulesPath, ... }: {
            imports = [ "${modulesPath}/profiles/qemu-guest.nix" ];

            boot.loader.r-boot.enable = true;
            boot.loader.timeout = 5;
            boot.kernelParams = [ "console=ttyS0" ];

            fileSystems."/" = {
              device = "/dev/disk/by-label/nixos";
              fsType = "ext4";
            };
            fileSystems."/boot" = {
              device = "/dev/disk/by-label/ESP";
              fsType = "vfat";
            };

            networking.hostName = "r-boot-qemu";
            users.users.root.initialPassword = "root";
            services.getty.autologinUser = "root";
            documentation.enable = false;

            system.stateVersion = pkgs.lib.trivial.release;
          })
        ];
      };

      packages.${system} = {
        default = r-boot;

        # `nix build .#nixos-image` (wrapped by `scripts/build-nixos-image`)
        # produces a qcow2 disk image with r-boot installed to its ESP.
        nixos-image = import "${pkgs.path}/nixos/lib/make-disk-image.nix" {
          inherit pkgs;
          inherit (pkgs) lib;
          config = self.nixosConfigurations.r-boot-qemu.config;
          format = "qcow2";
          partitionTableType = "efi";
          touchEFIVars = false;
        };
      };

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
