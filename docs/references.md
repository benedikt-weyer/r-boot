# Protocol and UEFI references

These are the authoritative references used by r-boot's protocol backends.

## Limine

- [Limine boot protocol specification](https://github.com/Limine-Bootloader/limine-protocol/blob/trunk/PROTOCOL.md)
- [Limine C/C++ API definitions (`limine.h`)](https://github.com/Limine-Bootloader/limine-protocol/blob/trunk/include/limine.h)
- [Limine reference bootloader](https://github.com/Limine-Bootloader/Limine)
- [Rust `limine` bindings](https://docs.rs/limine/latest/limine/)

## Linux x86 boot protocol

- [Linux/x86 boot protocol documentation](https://docs.kernel.org/arch/x86/boot.html)
- [Linux EFI handover protocol](https://docs.kernel.org/arch/x86/boot.html#efi-handover-protocol)
- [Linux `boot_params` and setup-header definitions](https://github.com/torvalds/linux/blob/master/arch/x86/include/uapi/asm/bootparam.h)
- [Linux EFI stub source](https://github.com/torvalds/linux/tree/master/drivers/firmware/efi/libstub)
- [Linux x86 EFI stub source](https://github.com/torvalds/linux/tree/master/arch/x86/boot/compressed)

The EFI handover entry is deprecated upstream in favor of directly executing
the kernel's PE/COFF EFI entry point, but remains implemented by r-boot for
explicit 64-bit `boot_params` handoff.

## UEFI and EFI APIs

- [UEFI specification](https://uefi.org/specifications)
- [UEFI 2.11 PDF](https://uefi.org/specs/UEFI/2.11/)
- [UEFI Device Path specification](https://uefi.org/specs/UEFI/2.11/10_Protocols_Device_Path_Protocol.html)
- [EDK II reference implementation](https://github.com/tianocore/edk2)
- [EDK II UEFI API headers](https://github.com/tianocore/edk2/tree/master/MdePkg/Include)
- [`uefi-rs` API documentation](https://docs.rs/uefi/latest/uefi/)
- [`uefi-raw` Rust API definitions](https://docs.rs/uefi-raw/latest/uefi_raw/)
- [Rust UEFI target documentation](https://doc.rust-lang.org/rustc/platform-support/unknown-uefi.html)
