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
