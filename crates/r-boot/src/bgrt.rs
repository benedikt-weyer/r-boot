//! Locates the firmware's boot logo via the ACPI Boot Graphics Resource
//! Table (BGRT) and decodes the bitmap it points to.
//!
//! The BGRT (ACPI spec, "Boot Graphics Resource Table") is how firmware
//! tells the OS/bootloader where the splash image it already painted lives:
//! a physical address of an in-memory BMP file, plus the (x, y) it was
//! drawn at. Reading it here lets r-boot redraw the same logo after it
//! clears the screen to print the boot menu, without needing any
//! EDK2-specific protocol.

use alloc::vec::Vec;
use core::mem::size_of;

use uefi::proto::console::gop::{BltOp, BltPixel, BltRegion, GraphicsOutput};
use uefi::system;
use uefi::table::cfg::ConfigTableEntry;

/// A raster image ready to be blitted onto the GOP framebuffer.
pub struct Logo {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<BltPixel>,
}

/// Walks ACPI RSDP -> XSDT/RSDT -> BGRT, then decodes the BMP the BGRT
/// table points to. Returns `None` if any step is unsupported or missing,
/// which is common (not all firmware publishes a BGRT, and some use
/// compressed or non-bitmap logos).
pub fn locate() -> Option<Logo> {
    let rsdp_addr = system::with_config_table(|entries| {
        entries
            .iter()
            .find(|entry| entry.guid == ConfigTableEntry::ACPI2_GUID)
            .or_else(|| {
                entries
                    .iter()
                    .find(|entry| entry.guid == ConfigTableEntry::ACPI_GUID)
            })
            .map(|entry| entry.address as usize)
    })?;

    let Some(bgrt) = find_bgrt(rsdp_addr) else {
        log::debug!("r-boot: no BGRT table found");
        return None;
    };
    let image_address = bgrt.image_address;
    let offset_x = bgrt.offset_x;
    let offset_y = bgrt.offset_y;
    log::debug!(
        "r-boot: BGRT found image_address={image_address:#x} offset=({offset_x},{offset_y})"
    );
    let logo = decode_bmp(image_address as usize, offset_x as usize, offset_y as usize);
    match &logo {
        Some(logo) => log::debug!(
            "r-boot: decoded logo {}x{} at ({},{})",
            logo.width,
            logo.height,
            logo.x,
            logo.y
        ),
        None => log::debug!("r-boot: failed to decode BMP at BGRT image address"),
    }
    logo
}

/// Decodes a trusted, statically embedded 24- or 32-bit BMP.
pub fn decode_bmp_bytes(bytes: &[u8]) -> Option<Logo> {
    if bytes.len() < size_of::<BmpFileHeader>() + size_of::<BmpInfoHeader>() {
        return None;
    }
    // SAFETY: callers provide compile-time embedded BMP assets. `decode_bmp`
    // validates their format before allocating and decoding their pixels.
    decode_bmp(bytes.as_ptr() as usize, 0, 0)
}

/// Draws the firmware logo at the specified framebuffer position.
pub fn draw(gop: &mut GraphicsOutput, logo: &Logo, position: (usize, usize)) {
    let (width, height) = gop.current_mode_info().resolution();
    if position.0 + logo.width > width || position.1 + logo.height > height {
        return;
    }
    let _ = gop.blt(BltOp::BufferToVideo {
        buffer: &logo.pixels,
        src: BltRegion::Full,
        dest: position,
        dims: (logo.width, logo.height),
    });
}

#[allow(dead_code)]
#[repr(C, packed)]
struct Rsdp {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

#[allow(dead_code)]
#[repr(C, packed)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

#[allow(dead_code)]
#[repr(C, packed)]
struct Bgrt {
    header: SdtHeader,
    version: u16,
    status: u8,
    image_type: u8,
    image_address: u64,
    offset_x: u32,
    offset_y: u32,
}

/// # Safety
///
/// `rsdp_addr` must come from the firmware's ACPI configuration table entry.
/// UEFI firmware identity-maps this memory while boot services are active,
/// so it is safe to dereference here, before `ExitBootServices` is called.
fn find_bgrt(rsdp_addr: usize) -> Option<Bgrt> {
    // SAFETY: see function docs.
    let rsdp = unsafe { core::ptr::read_unaligned(rsdp_addr as *const Rsdp) };
    if rsdp.signature != *b"RSD PTR " {
        return None;
    }

    if rsdp.revision >= 2 && rsdp.xsdt_address != 0 {
        find_table(rsdp.xsdt_address as usize, size_of::<u64>())
    } else if rsdp.rsdt_address != 0 {
        find_table(rsdp.rsdt_address as usize, size_of::<u32>())
    } else {
        None
    }
}

/// Walks the entries of an RSDT (4-byte pointers) or XSDT (8-byte pointers)
/// looking for the BGRT table. `entry_size` selects which.
fn find_table(root_addr: usize, entry_size: usize) -> Option<Bgrt> {
    // SAFETY: `root_addr` was validated by the RSDP checksum-free signature
    // check in `find_bgrt`; ACPI table memory stays identity-mapped while
    // boot services are active.
    let header = unsafe { core::ptr::read_unaligned(root_addr as *const SdtHeader) };
    let table_len = (header.length as usize).saturating_sub(size_of::<SdtHeader>());
    let count = table_len / entry_size;
    let entries_addr = root_addr + size_of::<SdtHeader>();

    for index in 0..count {
        // SAFETY: `index` is bounded by `count`, computed from the SDT's own
        // declared length.
        let addr = unsafe {
            if entry_size == size_of::<u64>() {
                core::ptr::read_unaligned((entries_addr + index * entry_size) as *const u64)
                    as usize
            } else {
                core::ptr::read_unaligned((entries_addr + index * entry_size) as *const u32)
                    as usize
            }
        };
        if let Some(bgrt) = read_bgrt(addr) {
            return Some(bgrt);
        }
    }
    None
}

fn read_bgrt(addr: usize) -> Option<Bgrt> {
    if addr == 0 {
        return None;
    }
    // SAFETY: `addr` comes from an RSDT/XSDT entry, which the firmware
    // guarantees points to a valid ACPI table in identity-mapped memory.
    let header = unsafe { core::ptr::read_unaligned(addr as *const SdtHeader) };
    if header.signature != *b"BGRT" {
        return None;
    }
    // SAFETY: same as above; BGRT is a fixed-size table and its declared
    // signature was just checked.
    let bgrt = unsafe { core::ptr::read_unaligned(addr as *const Bgrt) };
    if bgrt.image_type != 0 || bgrt.image_address == 0 {
        // Only the "Bitmap" image type (0) is defined by the spec.
        return None;
    }
    Some(bgrt)
}

#[allow(dead_code)]
#[repr(C, packed)]
struct BmpFileHeader {
    signature: u16,
    file_size: u32,
    reserved: u32,
    data_offset: u32,
}

#[allow(dead_code)]
#[repr(C, packed)]
struct BmpInfoHeader {
    header_size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    image_size: u32,
    x_pixels_per_meter: i32,
    y_pixels_per_meter: i32,
    colors_used: u32,
    colors_important: u32,
}

/// Decodes an uncompressed 24- or 32-bit BI_RGB BMP at `addr` into a flat
/// row-major `BltPixel` buffer, ready for `GraphicsOutput::blt`.
///
/// # Safety
///
/// `addr` must point to a BMP image in identity-mapped memory, as given by
/// `BGRT.ImageAddress`.
fn decode_bmp(addr: usize, offset_x: usize, offset_y: usize) -> Option<Logo> {
    // SAFETY: see function docs.
    let file_header = unsafe { core::ptr::read_unaligned(addr as *const BmpFileHeader) };
    if file_header.signature != 0x4D42 {
        // "BM" stored little-endian.
        return None;
    }
    // SAFETY: `BmpInfoHeader` immediately follows `BmpFileHeader` in a BMP
    // file, per the format's fixed layout.
    let info = unsafe {
        core::ptr::read_unaligned((addr + size_of::<BmpFileHeader>()) as *const BmpInfoHeader)
    };
    if info.compression != 0 {
        // Only BI_RGB (uncompressed) is supported.
        return None;
    }
    let bytes_per_pixel = match info.bit_count {
        24 => 3,
        32 => 4,
        _ => return None,
    };
    let width = info.width as usize;
    if width == 0 || info.height == 0 {
        return None;
    }
    let top_down = info.height < 0;
    let height = info.height.unsigned_abs() as usize;

    // Rows are padded to a 4-byte boundary.
    let row_stride = (width * bytes_per_pixel + 3) & !3;
    let pixel_base = addr + file_header.data_offset as usize;

    let mut pixels = Vec::with_capacity(width * height);
    for row in 0..height {
        let src_row = if top_down { row } else { height - 1 - row };
        let row_addr = pixel_base + src_row * row_stride;
        for col in 0..width {
            let pixel_addr = row_addr + col * bytes_per_pixel;
            // SAFETY: `row` < `height` and `col` < `width`, both decoded
            // from the same BMP header that defines `row_stride`, so this
            // stays within the image's own pixel data.
            let blue = unsafe { core::ptr::read((pixel_addr) as *const u8) };
            // SAFETY: see above.
            let green = unsafe { core::ptr::read((pixel_addr + 1) as *const u8) };
            // SAFETY: see above.
            let red = unsafe { core::ptr::read((pixel_addr + 2) as *const u8) };
            pixels.push(BltPixel::new(red, green, blue));
        }
    }

    Some(Logo {
        x: offset_x,
        y: offset_y,
        width,
        height,
        pixels,
    })
}
