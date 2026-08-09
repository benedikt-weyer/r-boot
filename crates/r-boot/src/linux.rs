//! Linux x86 EFI handover protocol (64-bit entry).
//!
//! The kernel's EFI stub remains in control of UEFI boot services. This avoids
//! the normal Linux boot protocol's 32-bit transition and lets the kernel
//! construct its own final memory map.

use core::convert::Infallible;
use core::ffi::c_void;

use uefi::Handle;
use uefi::boot::{self, AllocateType, LoadImageSource, MemoryType};
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};
use uefi::proto::loaded_image::LoadedImage;
use uefi::table;

const SETUP_HEADER: usize = 0x1f1;
const HEADER_MAGIC: u32 = 0x5372_6448; // "HdrS"
const MIN_PROTOCOL_VERSION: u16 = 0x020b;
const XLF_EFI_HANDOVER_64: u16 = 1 << 3;
const BOOT_PARAMS_SIZE: usize = 4096;
const PAGE_SIZE: usize = 4096;
const DEFAULT_CMDLINE: &str = "console=ttyS0 modules=loop,squashfs ip=dhcp alpine_repo=http://dl-cdn.alpinelinux.org/alpine/latest-stable/main modloop=http://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/x86_64/netboot/modloop-virt";

// Offsets are from the start of the bzImage, and are therefore also offsets
// into the boot_params zero page after copying the setup header there.
const TYPE_OF_LOADER: usize = 0x210;
const CODE32_START: usize = 0x214;
const RAMDISK_IMAGE: usize = 0x218;
const RAMDISK_SIZE: usize = 0x21c;
const CMD_LINE_PTR: usize = 0x228;
const CMDLINE_SIZE: usize = 0x238;
const XLOADFLAGS: usize = 0x236;
const HANDOVER_OFFSET: usize = 0x264;

// `struct screen_info` (linux/screen_info.h), at offset 0 of boot_params.
// The kernel's own EFI stub fills this in from the GOP when it owns the
// UEFI handover; since r-boot jumps straight to the internal 64-bit
// handover entry (skipping that stub code), r-boot has to do it instead, or
// the kernel never initializes an efifb console and nothing after
// ExitBootServices reaches the display.
const ORIG_VIDEO_ISVGA: usize = 0x0f;
const LFB_WIDTH: usize = 0x12;
const LFB_HEIGHT: usize = 0x14;
const LFB_DEPTH: usize = 0x16;
const LFB_BASE: usize = 0x18;
const LFB_SIZE: usize = 0x1c;
const LFB_LINELENGTH: usize = 0x24;
const RED_SIZE: usize = 0x26;
const RED_POS: usize = 0x27;
const GREEN_SIZE: usize = 0x28;
const GREEN_POS: usize = 0x29;
const BLUE_SIZE: usize = 0x2a;
const BLUE_POS: usize = 0x2b;
const RSVD_SIZE: usize = 0x2c;
const RSVD_POS: usize = 0x2d;
const PAGES: usize = 0x32;
const CAPABILITIES: usize = 0x36;
const EXT_LFB_BASE: usize = 0x3a;

const VIDEO_TYPE_EFI: u8 = 0x70;
const VIDEO_CAPABILITY_SKIP_QUIRKS: u32 = 1 << 0;
const VIDEO_CAPABILITY_64BIT_BASE: u32 = 1 << 1;

pub fn handover(
    parent: Handle,
    kernel: &[u8],
    initramfs: &[u8],
    options: Option<&str>,
) -> Result<Infallible, &'static str> {
    let header = Header::parse(kernel)?;

    // UEFI LoadImage performs PE/COFF section placement and relocation. This
    // is mandatory for the EFI handover protocol; merely copying vmlinuz is
    // not sufficient.
    let kernel_handle = boot::load_image(
        parent,
        LoadImageSource::FromBuffer {
            buffer: kernel,
            file_path: None,
        },
    )
    .map_err(|_| "firmware rejected vmlinuz as a UEFI PE image")?;
    let loaded = boot::open_protocol_exclusive::<LoadedImage>(kernel_handle)
        .map_err(|_| "cannot inspect loaded Linux image")?;
    let (image_base, _) = loaded.info();
    drop(loaded);
    log::debug!(
        "r-boot: Linux EFI image loaded at {:#x}",
        image_base as usize
    );

    let params = allocate_and_zero(BOOT_PARAMS_SIZE)?;
    let initrd = if initramfs.is_empty() {
        None
    } else {
        Some(allocate_copy(initramfs)?)
    };
    let command_line_options = options.unwrap_or(DEFAULT_CMDLINE);
    let command_line = allocate_command_line(command_line_options)?;
    unsafe {
        // The setup header is part of struct boot_params at the same offset.
        core::ptr::copy_nonoverlapping(
            kernel.as_ptr().add(SETUP_HEADER),
            params.as_ptr().add(SETUP_HEADER),
            header.size,
        );
        write_u8(params.as_ptr(), TYPE_OF_LOADER, 0xff);
        write_u32(
            params.as_ptr(),
            CODE32_START,
            (image_base as usize)
                .checked_add(header.setup_bytes)
                .ok_or("Linux image address overflow")? as u32,
        );
        if let Some(initrd) = initrd {
            write_u32(
                params.as_ptr(),
                RAMDISK_IMAGE,
                initrd.as_ptr() as usize as u32,
            );
        }
        write_u32(params.as_ptr(), RAMDISK_SIZE, initramfs.len() as u32);
        write_u32(
            params.as_ptr(),
            CMD_LINE_PTR,
            command_line.as_ptr() as usize as u32,
        );
        write_u32(
            params.as_ptr(),
            CMDLINE_SIZE,
            command_line_options.len() as u32 + 1,
        );
    }
    setup_screen_info(params.as_ptr());

    let entry = (image_base as usize)
        .checked_add(header.setup_bytes)
        .and_then(|address| address.checked_add(header.handover_offset))
        .and_then(|address| address.checked_add(0x200))
        .ok_or("Linux handover address overflow")?;
    let system_table = table::system_table_raw()
        .ok_or("UEFI system table is unavailable")?
        .as_ptr()
        .cast::<c_void>();

    log::debug!("r-boot: Linux EFI 64-bit handover at {entry:#x}");
    log::debug!(
        "r-boot: Linux handover args handle={:#x} table={:#x} params={:#x}",
        kernel_handle.as_ptr() as usize,
        system_table as usize,
        params.as_ptr() as usize
    );
    // `handover_offset + 0x200` deliberately skips the PE entry's UEFI ABI
    // shim, so the internal 64-bit handover entry uses the System V ABI:
    // RDI=handle, RSI=system table, RDX=boot params.
    let handover: extern "sysv64" fn(Handle, *mut c_void, *mut c_void) -> u64 =
        unsafe { core::mem::transmute(entry) };
    let status = handover(kernel_handle, system_table, params.as_ptr().cast());
    Err(if status == 0 {
        "Linux EFI handover unexpectedly returned"
    } else {
        "Linux EFI handover returned an error"
    })
}

struct Header {
    setup_bytes: usize,
    handover_offset: usize,
    size: usize,
}

impl Header {
    fn parse(kernel: &[u8]) -> Result<Self, &'static str> {
        let magic = read_u32(kernel, 0x202)?;
        if magic != HEADER_MAGIC {
            return Err("vmlinuz does not contain a Linux setup header");
        }
        if read_u16(kernel, 0x206)? < MIN_PROTOCOL_VERSION {
            return Err("Linux kernel lacks EFI handover protocol support");
        }
        if read_u16(kernel, XLOADFLAGS)? & XLF_EFI_HANDOVER_64 == 0 {
            return Err("Linux kernel lacks 64-bit EFI handover support");
        }
        let setup_sectors = match *kernel.get(0x1f1).ok_or("truncated Linux header")? {
            0 => 4,
            count => count as usize,
        };
        let setup_bytes = (setup_sectors + 1) * 512;
        let handover_offset = read_u32(kernel, HANDOVER_OFFSET)? as usize;
        if handover_offset == 0 || setup_bytes > kernel.len() {
            return Err("Linux handover entry is invalid");
        }
        Ok(Self {
            setup_bytes,
            handover_offset,
            size: 0x290 - SETUP_HEADER,
        })
    }
}

/// Fills in `boot_params.screen_info` from the UEFI GOP, mirroring what the
/// kernel's own EFI stub does in `efi_setup_gop()`. Without this, the kernel
/// has no framebuffer to hand to `efifb`, and the display stays blank for
/// the whole Linux boot (kernel messages, initrd, ...) even though r-boot's
/// own menu was drawn to the same screen moments earlier.
fn setup_screen_info(params: *mut u8) {
    let Ok(handle) = boot::get_handle_for_protocol::<GraphicsOutput>() else {
        log::warn!("r-boot: no UEFI Graphics Output Protocol; Linux console will be text-only");
        return;
    };
    let Ok(mut gop) = boot::open_protocol_exclusive::<GraphicsOutput>(handle) else {
        log::warn!("r-boot: cannot open Graphics Output Protocol");
        return;
    };

    let info = gop.current_mode_info();
    let pixel_format = info.pixel_format();
    if pixel_format == PixelFormat::BltOnly {
        log::warn!("r-boot: GOP framebuffer is not memory-mapped; Linux console will be text-only");
        return;
    }
    let (width, height) = info.resolution();
    let pixels_per_scan_line = info.stride();
    let base = gop.frame_buffer().as_mut_ptr() as usize;

    let (red_pos, red_size, green_pos, green_size, blue_pos, blue_size, rsvd_pos, rsvd_size) =
        match pixel_format {
            PixelFormat::Rgb => (0u8, 8u8, 8u8, 8u8, 16u8, 8u8, 24u8, 8u8),
            PixelFormat::Bgr => (16u8, 8u8, 8u8, 8u8, 0u8, 8u8, 24u8, 8u8),
            PixelFormat::Bitmask => {
                let mask = info.pixel_bitmask().unwrap_or_default();
                let (rp, rs) = find_bits(mask.red);
                let (gp, gs) = find_bits(mask.green);
                let (bp, bs) = find_bits(mask.blue);
                let (xp, xs) = find_bits(mask.reserved);
                (rp, rs, gp, gs, bp, bs, xp, xs)
            }
            PixelFormat::BltOnly => unreachable!("returned above"),
        };
    let lfb_depth = red_size as u16 + green_size as u16 + blue_size as u16 + rsvd_size as u16;
    let lfb_linelength = (pixels_per_scan_line as u32 * lfb_depth as u32) / 8;
    let lfb_size = lfb_linelength * height as u32;

    unsafe {
        write_u8(params, ORIG_VIDEO_ISVGA, VIDEO_TYPE_EFI);
        write_u16(params, LFB_WIDTH, width as u16);
        write_u16(params, LFB_HEIGHT, height as u16);
        write_u16(params, LFB_DEPTH, lfb_depth);
        write_u32(params, LFB_BASE, base as u32);
        write_u32(params, LFB_SIZE, lfb_size);
        write_u16(params, LFB_LINELENGTH, lfb_linelength as u16);
        write_u8(params, RED_SIZE, red_size);
        write_u8(params, RED_POS, red_pos);
        write_u8(params, GREEN_SIZE, green_size);
        write_u8(params, GREEN_POS, green_pos);
        write_u8(params, BLUE_SIZE, blue_size);
        write_u8(params, BLUE_POS, blue_pos);
        write_u8(params, RSVD_SIZE, rsvd_size);
        write_u8(params, RSVD_POS, rsvd_pos);
        write_u16(params, PAGES, 1);
        let mut capabilities = VIDEO_CAPABILITY_SKIP_QUIRKS;
        let ext_base = base >> 32;
        if ext_base != 0 {
            capabilities |= VIDEO_CAPABILITY_64BIT_BASE;
            write_u32(params, EXT_LFB_BASE, ext_base as u32);
        }
        write_u32(params, CAPABILITIES, capabilities);
    }
}

/// UEFI guarantees the set bits of a GOP pixel bitmask are contiguous.
fn find_bits(mask: u32) -> (u8, u8) {
    if mask == 0 {
        return (0, 0);
    }
    let pos = mask.trailing_zeros() as u8;
    let size = (32 - mask.leading_zeros() as u8) - pos;
    (pos, size)
}

fn allocate_and_zero(size: usize) -> Result<core::ptr::NonNull<u8>, &'static str> {
    let pages = size
        .checked_add(PAGE_SIZE - 1)
        .map(|bytes| bytes / PAGE_SIZE)
        .ok_or("Linux allocation size overflow")?;
    let allocation = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages)
        .map_err(|_| "cannot allocate Linux boot data")?;
    unsafe { core::ptr::write_bytes(allocation.as_ptr(), 0, pages * PAGE_SIZE) };
    Ok(allocation)
}

fn allocate_copy(bytes: &[u8]) -> Result<core::ptr::NonNull<u8>, &'static str> {
    if bytes.len() > u32::MAX as usize {
        return Err("Linux file is too large");
    }
    let allocation = allocate_and_zero(bytes.len())?;
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), allocation.as_ptr(), bytes.len()) };
    Ok(allocation)
}

fn allocate_command_line(options: &str) -> Result<core::ptr::NonNull<u8>, &'static str> {
    let bytes = options.as_bytes();
    if bytes.contains(&0) {
        return Err("Linux command line contains NUL");
    }
    let allocation = allocate_and_zero(
        bytes
            .len()
            .checked_add(1)
            .ok_or("Linux command line is too large")?,
    )?;
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), allocation.as_ptr(), bytes.len()) };
    Ok(allocation)
}

unsafe fn write_u8(base: *mut u8, offset: usize, value: u8) {
    unsafe { base.add(offset).write(value) };
}

unsafe fn write_u16(base: *mut u8, offset: usize, value: u16) {
    unsafe { (base.add(offset) as *mut u16).write_unaligned(value.to_le()) };
}

unsafe fn write_u32(base: *mut u8, offset: usize, value: u32) {
    unsafe { (base.add(offset) as *mut u32).write_unaligned(value.to_le()) };
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or("truncated Linux header")?
            .try_into()
            .unwrap(),
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("truncated Linux header")?
            .try_into()
            .unwrap(),
    ))
}
