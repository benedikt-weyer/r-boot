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

      # Throwaway RSA keypair + self-signed cert, generated at build time
      # (not by `r-boot-cli sign-key`, which requires real root) purely to
      # exercise `boot.loader.r-boot.sign-bootloader` end to end in the
      # r-boot-qemu smoke-test image: seeded into that image's
      # /var/lib/r-boot/pki/ (see `nixos-image`'s `contents` below) and
      # enrolled as PK/KEK/db into the secure-boot-enforcing OVMF vars this
      # image boots under (see `nixos-image-vars`).
      qemuTestPki = pkgs.runCommand "r-boot-qemu-test-pki" { nativeBuildInputs = [ pkgs.openssl ]; } ''
        mkdir -p $out
        openssl req -x509 -newkey rsa:2048 -noenc \
          -keyout $out/db.key -out $out/db.pem \
          -days 3650 -subj "/CN=r-boot-qemu test db/"
      '';

      # secureBoot=true gets us OVMF built with the SMM/AuthVariable
      # services real Secure Boot enforcement needs (systemManagementModeRequired
      # then follows automatically in make-disk-image.nix below).
      qemuTestOVMF = pkgs.OVMFFull.fd;

      # Pre-enroll qemuTestPki's cert as PK/KEK/db and flip SecureBootEnable
      # on, offline, into a copy of OVMF's blank vars template: booting a
      # NixOS VM interactively to click through firmware Setup Mode isn't
      # practical for a disk image built (and meant to be booted) headless.
      qemuTestVars = pkgs.runCommand "r-boot-qemu-test-vars" {
        nativeBuildInputs = [ pkgs.python3Packages.virt-firmware ];
      } ''
        virt-fw-vars -i ${qemuTestOVMF.variables} \
          --set-pk 61dfe48b-ca93-d211-aa0d-00e098032b8c ${qemuTestPki}/db.pem \
          --add-kek 61dfe48b-ca93-d211-aa0d-00e098032b8c ${qemuTestPki}/db.pem \
          --add-db 61dfe48b-ca93-d211-aa0d-00e098032b8c ${qemuTestPki}/db.pem \
          --secure-boot \
          -o $out
      '';
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
            # Exercises boot.loader.r-boot.sign-bootloader end to end: the
            # pki-bundle this signs with is seeded into /var/lib/r-boot/pki
            # via `nixos-image`'s `contents` below (sign-cert/sign-pk keep
            # their defaults, which point there), and the image boots under
            # OVMF with that same cert enrolled into db and secure boot
            # enforced (see qemuTestVars above).
            boot.loader.r-boot.sign-bootloader = true;
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
          mkdir -p $out/share
          cp -r ${r-boot}/share/. $out/share/
        '';

        # `nix build .#nixos-image` (wrapped by `scripts/build-nixos-image`)
        # produces a qcow2 disk image with r-boot installed to its ESP,
        # signed with qemuTestPki per boot.loader.r-boot.sign-bootloader
        # above. touchEFIVars/OVMF stay at their plain defaults here: that
        # firmware only boots make-disk-image.nix's own internal helper VM
        # (a NixOS installer environment, not r-boot, and not something
        # qemuTestPki has any business signing) to run switch-to-configuration
        # boot; overriding it to the secure-boot-enforcing firmware makes
        # *that* fail to boot, well before r-boot ever enters the picture.
        # The actual secure-boot-under-OVMF exercise happens afterwards, in
        # scripts/run-nixos-image-qemu, using nixos-image-ovmf/-vars below.
        nixos-image = import "${pkgs.path}/nixos/lib/make-disk-image.nix" {
          inherit pkgs;
          inherit (pkgs) lib;
          config = self.nixosConfigurations.r-boot-qemu.config;
          format = "qcow2";
          partitionTableType = "efi";
          touchEFIVars = false;
          contents = [
            {
              source = "${qemuTestPki}/db.key";
              target = "/var/lib/r-boot/pki/db.key";
              mode = "0600";
              user = "root";
              group = "root";
            }
            {
              source = "${qemuTestPki}/db.pem";
              target = "/var/lib/r-boot/pki/db.pem";
              mode = "0644";
              user = "root";
              group = "root";
            }
          ];
        };

        # scripts/run-nixos-image-qemu boots `nixos-image` under this
        # secure-boot-capable firmware, with qemuTestPki's cert enrolled as
        # PK/KEK/db (see nixos-image-vars) so an unsigned or wrongly-signed
        # r-boot.efi actually fails to boot there.
        nixos-image-ovmf = qemuTestOVMF;
        nixos-image-vars = qemuTestVars;
      };

      # Import into a NixOS system flake as `inputs.r-boot.nixosModules.default`
      # and set `boot.loader.r-boot.enable = true;` to boot that system with
      # r-boot instead of GRUB or systemd-boot.
      nixosModules.default = import ./nix/module.nix { rBootPackage = r-boot; };

      devShells.${system}.default = pkgs.mkShell {
        packages = [ rust pkgs.qemu pkgs.OVMF pkgs.mtools pkgs.dosfstools pkgs.curl pkgs.gnumake pkgs.xorriso ];
        OVMF_CODE = "${pkgs.OVMF.fd}/FV/OVMF_CODE.fd";
      };
    };
}
