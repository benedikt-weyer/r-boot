#!/usr/bin/env bash
set -euo pipefail

cat <<'EOF'
r-boot developer commands

  nix develop                 enter the reproducible toolchain
  direnv allow                enable the dev shell automatically
  cargo fmt --check           check Rust formatting
  cargo clippy --target x86_64-unknown-uefi -- -D warnings
                              lint the UEFI application
  cargo build --release --target x86_64-unknown-uefi
                              build EFI/BOOT/BOOTX64.EFI source image
  RBOOT_KERNEL=/path/kernel.elf ./run-qemu.sh
                              build an ESP and boot a Limine-compatible ELF
  RBOOT_KERNEL_URL=https://example/kernel.elf ./run-qemu.sh
                              download a compatible ELF then boot it

This is a Limine boot-protocol loader. Ordinary Linux bzImage, vmlinuz, and
distribution ISO images use the Linux boot protocol and will be rejected.
EOF
