# r-boot

`r-boot` is a Rust UEFI bootloader. It implements the Limine boot protocol for
x86_64 higher-half, `ET_EXEC` ELF kernels and the Linux x86 EFI 64-bit handover
protocol for standard `vmlinuz` kernels. The protocol
boundary is [`src/protocol.rs`](src/protocol.rs); later backends can implement
the same `BootProtocol` trait without changing the UEFI file-loading or paging
code.

There is no C source and the only assembly is one inline `mov cr3` needed to
install page tables before entering a higher-half kernel. Rust has no stable
replacement for that privileged instruction.

## Current Limine subset

The initial backend is intentionally small. It supports base revision 0 and
the bootloader-info, firmware-type (UEFI x86_64), HHDM, and executable-address
requests. Unsupported requests are left with a null response as the Limine
protocol requires. It maps the first 4 GiB both identity-mapped and at the
HHDM offset, which is sufficient for the default 256 MiB QEMU test machine.

## Linux EFI handover

When `boot/vmlinuz` and `boot/initramfs` are present, r-boot uses the Linux
x86 64-bit EFI handover entry. It uses UEFI `LoadImage` to honor the kernel
PE/COFF layout and relocations, then passes the kernel handle, UEFI system
table, and populated `boot_params` structure to the EFI stub. The kernel owns
the eventual `ExitBootServices` transition.

The handover protocol is deprecated upstream in favor of executing the Linux
EFI PE entry point directly, but it is implemented here as requested and
required for explicit 64-bit boot-parameter handoff.

## Boot menu

`r-boot` combines entries from both of these sources on the ESP:

- `boot/r-boot.toml`, its native TOML configuration.
- `loader/entries/*.conf`, systemd-boot's Boot Loader Specification Type #1
  entry format. `loader/loader.conf` supplies its `default` and `timeout`
  settings when the TOML file has not set them.
- `boot/grub/grub.cfg` and `grub/grub.cfg`, using GRUB's static Linux
  `menuentry` format.

Use Up/Down and Enter to select an entry. The selected default boots after
five seconds unless `timeout` is configured; `timeout = 0` boots immediately.
When neither source contains entries, the original fixed
`boot/vmlinuz` + `boot/initramfs` or `boot/kernel.elf` layouts remain supported.

The native format deliberately uses a small TOML subset: quoted strings,
integer `timeout`, and repeated `[[entries]]` tables.

```toml
default = "alpine"
timeout = 5

[[entries]]
id = "alpine"
title = "Alpine Linux"
kind = "linux"
linux = "/boot/vmlinuz-virt"
initrd = "/boot/initramfs-virt"
options = "console=ttyS0 quiet"

[[entries]]
title = "My Limine kernel"
kind = "limine"
kernel = "/boot/kernel.elf"
```

For systemd-boot compatibility, `title`, `linux`, repeated `initrd`, `options`,
and `efi` are accepted. `efi` entries are started directly as UEFI images;
Linux entries use r-boot's EFI handover implementation.

GRUB entries support `menuentry`, `--id=`, `linux`/`linuxefi`, and
`initrd`/`initrdefi`; top-level `set default=` and `set timeout=` are also
read. GRUB scripts with variable expansion, conditionals, generated
submenus, chainloading, or non-Linux commands are intentionally ignored.

## NixOS

This repository is a flake exposing `packages.x86_64-linux.default` (the
`BOOTX64.EFI` binary) and `nixosModules.default`, a `boot.loader.r-boot`
implementation. Add it as an input to a NixOS system flake:

```nix
{
  inputs.r-boot.url = "github:benedikt-weyer/r-boot";

  outputs = { nixpkgs, r-boot, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        r-boot.nixosModules.default
        {
          boot.loader.r-boot.enable = true;
          boot.loader.efi.canTouchEfiVariables = true;
        }
      ];
    };
  };
}
```

`boot.loader.r-boot` cannot be combined with `boot.loader.grub` or
`boot.loader.systemd-boot`. On activation it writes `boot/r-boot.toml`
listing the current generation plus up to
`boot.loader.r-boot.configurationLimit` older ones (default 20), copies
their kernels/initrds into `boot/nixos` on the ESP, and installs the r-boot
binary to `EFI/BOOT/BOOTX64.EFI`. With `boot.loader.efi.canTouchEfiVariables`
it also registers a `r-boot` NVRAM boot entry via `efibootmgr`.

`flake.nix` also defines `nixosConfigurations.r-boot-qemu`, a minimal NixOS
system with `boot.loader.r-boot` enabled, and a `nixos-image` package that
turns it into a qcow2 disk image via nixpkgs' `make-disk-image.nix`. Build
and boot it to exercise r-boot as a real installed bootloader, rather than
the fixed kernel/initrd layouts used by `run-linux-qemu`/`run-nixos-qemu`:

```sh
./scripts/build-nixos-image
./scripts/run-nixos-image-qemu
```

## References

Authoritative Limine, Linux boot-protocol, UEFI, EFI API, and Rust UEFI links
are collected in [docs/references.md](docs/references.md).

## Development

```sh
direnv allow
nix develop
./help.sh
```

Build the UEFI executable:

```sh
cargo build --release --target x86_64-unknown-uefi
```

Run it in QEMU with a Limine-compatible kernel:

```sh
RBOOT_KERNEL=/path/to/kernel.elf ./run-qemu.sh
# or a direct artifact URL:
RBOOT_KERNEL_URL=https://example.invalid/kernel.elf ./run-qemu.sh
```

The runner builds a FAT ESP containing `EFI/BOOT/BOOTX64.EFI` and
`boot/kernel.elf`, starts OVMF, and exposes serial/debug-console output.

Test standard Linux handoff with Alpine's small `virt` netboot kernel and
initramfs:

```sh
./run-linux-qemu.sh
```

Test it against nix-community/nixos-images' rolling minimal netboot build:

```sh
./run-nixos-qemu.sh
```
