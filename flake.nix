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
            # `nomodeset`: without it, the `bochs` KMS driver takes over the
            # console from the EFI framebuffer mid-boot with a real mode-set
            # against QEMU's "std" VGA device, and some QEMU GTK/SDL builds
            # fail to repaint their window after that handover (the pixel
            # data itself is fine; only the GTK/SDL widget stops updating).
            # Staying on the EFI framebuffer for the whole boot avoids it.
            boot.kernelParams = [ "console=tty0" "console=ttyS0" "nomodeset" ];

            # Lets the smoke test (and anyone booting the image) inspect and
            # tweak the running system's r-boot menu with `r-boot-cli`.
            environment.systemPackages = [ self.packages.${system}.r-boot-cli ];

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

        # `r-boot-cli` for inspecting/editing a running system's r-boot menu
        # (`crates/r-boot-cli`), split out of the `r-boot` derivation's
        # `$out/bin` so it can be depended on independently of the
        # bootloader package.
        r-boot-cli = pkgs.runCommand "r-boot-cli" { } ''
          install -D ${r-boot}/bin/r-boot-cli $out/bin/r-boot-cli
        '';

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
