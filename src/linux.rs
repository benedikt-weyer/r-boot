//! Linux x86 EFI handover protocol (64-bit entry).
//!
//! The kernel's EFI stub remains in control of UEFI boot services. This avoids
//! the normal Linux boot protocol's 32-bit transition and lets the kernel
//! construct its own final memory map.

use core::convert::Infallible;
use core::ffi::c_void;

use uefi::Handle;
use uefi::boot::{self, AllocateType, LoadImageSource, MemoryType};
use uefi::proto::loaded_image::LoadedImage;
use uefi::table;

const SETUP_HEADER: usize = 0x1f1;
const HEADER_MAGIC: u32 = 0x5372_6448; // "HdrS"
const MIN_PROTOCOL_VERSION: u16 = 0x020b;
const XLF_EFI_HANDOVER_64: u16 = 1 << 3;
const BOOT_PARAMS_SIZE: usize = 4096;
const PAGE_SIZE: usize = 4096;
const CMDLINE: &[u8] = b"console=ttyS0\0";

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

pub fn handover(
    parent: Handle,
    kernel: &[u8],
    initramfs: &[u8],
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
    log::info!(
        "r-boot: Linux EFI image loaded at {:#x}",
        image_base as usize
    );

    let params = allocate_and_zero(BOOT_PARAMS_SIZE)?;
    let initrd = allocate_copy(initramfs)?;
    let command_line = allocate_copy(CMDLINE)?;
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
        write_u32(
            params.as_ptr(),
            RAMDISK_IMAGE,
            initrd.as_ptr() as usize as u32,
        );
        write_u32(params.as_ptr(), RAMDISK_SIZE, initramfs.len() as u32);
        write_u32(
            params.as_ptr(),
            CMD_LINE_PTR,
            command_line.as_ptr() as usize as u32,
        );
        write_u32(params.as_ptr(), CMDLINE_SIZE, CMDLINE.len() as u32);
    }

    let entry = (image_base as usize)
        .checked_add(header.setup_bytes)
        .and_then(|address| address.checked_add(header.handover_offset))
        .and_then(|address| address.checked_add(0x200))
        .ok_or("Linux handover address overflow")?;
    let system_table = table::system_table_raw()
        .ok_or("UEFI system table is unavailable")?
        .as_ptr()
        .cast::<c_void>();

    log::info!("r-boot: Linux EFI 64-bit handover at {entry:#x}");
    log::info!(
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

unsafe fn write_u8(base: *mut u8, offset: usize, value: u8) {
    unsafe { base.add(offset).write(value) };
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
