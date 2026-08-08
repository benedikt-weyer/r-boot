//! The initial Limine request backend.
//!
//! It intentionally supports a small useful subset: base revision 0,
//! bootloader-info, firmware-type, HHDM, and executable-address.  Unknown
//! requests are left unanswered, as required by the protocol.

use alloc::boxed::Box;

use crate::elf::{Image, LoadedSegment};
use crate::protocol::BootProtocol;

const COMMON_MAGIC: [u64; 2] = [0xc7b1_dd30_df4c_8b88, 0x0a82_e883_a194_f07b];
const BASE_MAGIC: [u64; 2] = [0xf956_2b2d_5c95_a6c8, 0x6a7b_3849_4453_6bdc];
const BOOTLOADER_INFO: [u64; 2] = [0xf550_38d8_e2a1_202f, 0x2794_26fc_f5f5_9740];
const FIRMWARE_TYPE: [u64; 2] = [0x8c2f_75d9_0bef_28a8, 0x7045_a468_8eac_00c3];
const HHDM: [u64; 2] = [0x48dc_f1cb_8ad2_b852, 0x6398_4e95_9a98_244b];
const EXECUTABLE_ADDRESS: [u64; 2] = [0x71ba_7686_3cc5_5f63, 0xb264_4a48_c516_a487];
const REQUEST_SIZE: usize = 48;

#[repr(C)]
struct BootloaderInfoResponse {
    revision: u64,
    name: u64,
    version: u64,
}
#[repr(C)]
struct FirmwareTypeResponse {
    revision: u64,
    firmware_type: u64,
}
#[repr(C)]
struct HhdmResponse {
    revision: u64,
    offset: u64,
}
#[repr(C)]
struct ExecutableAddressResponse {
    revision: u64,
    physical_base: u64,
    virtual_base: u64,
}

static NAME: &[u8] = b"r-boot\0";
static VERSION: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();

pub struct Limine;

impl BootProtocol for Limine {
    fn prepare(
        &self,
        bytes: &[u8],
        image: &Image<'_>,
        segments: &[LoadedSegment],
        hhdm_offset: u64,
    ) -> Result<(), &'static str> {
        write_responses(bytes, image, segments, hhdm_offset)
    }
}

fn write_responses(
    bytes: &[u8],
    _image: &Image<'_>,
    segments: &[LoadedSegment],
    hhdm_offset: u64,
) -> Result<(), &'static str> {
    if !segments
        .iter()
        .all(|segment| segment.physical_address < 0x1_0000_0000)
    {
        return Err("this basic backend currently requires kernel allocations below 4 GiB");
    }

    let physical_base = segments
        .iter()
        .map(|segment| segment.physical_address)
        .min()
        .unwrap();
    let virtual_base = segments
        .iter()
        .map(|segment| segment.virtual_address)
        .min()
        .unwrap();

    for offset in (0..bytes.len().saturating_sub(24)).step_by(8) {
        let first = u64_at(bytes, offset)?;
        let second = u64_at(bytes, offset + 8)?;
        if [first, second] == BASE_MAGIC {
            // A value of zero means base revision 0 is supported.
            write_at_offset(segments, offset + 16, 0)?;
            continue;
        }
        if [first, second] != COMMON_MAGIC || offset + REQUEST_SIZE > bytes.len() {
            continue;
        }
        let id = [u64_at(bytes, offset + 16)?, u64_at(bytes, offset + 24)?];
        let response = if id == BOOTLOADER_INFO {
            let response = Box::new(BootloaderInfoResponse {
                revision: 0,
                name: HHDM_OFFSET_OF_STATIC(NAME.as_ptr(), hhdm_offset),
                version: HHDM_OFFSET_OF_STATIC(VERSION.as_ptr(), hhdm_offset),
            });
            hhdm_of_box(response, hhdm_offset)
        } else if id == FIRMWARE_TYPE {
            hhdm_of_box(
                Box::new(FirmwareTypeResponse {
                    revision: 0,
                    firmware_type: 2,
                }),
                hhdm_offset,
            )
        } else if id == HHDM {
            hhdm_of_box(
                Box::new(HhdmResponse {
                    revision: 0,
                    offset: hhdm_offset,
                }),
                hhdm_offset,
            )
        } else if id == EXECUTABLE_ADDRESS {
            hhdm_of_box(
                Box::new(ExecutableAddressResponse {
                    revision: 0,
                    physical_base,
                    virtual_base,
                }),
                hhdm_offset,
            )
        } else {
            continue;
        };
        write_at_offset(segments, offset + 40, response)?;
    }
    Ok(())
}

#[allow(non_snake_case)]
fn HHDM_OFFSET_OF_STATIC(pointer: *const u8, offset: u64) -> u64 {
    // Rust statics are already mapped by the firmware identity map.  The
    // post-exit page tables map only RAM below 4 GiB, which includes this
    // QEMU-targeted loader image.
    pointer as u64 + offset
}

fn hhdm_of_box<T>(value: Box<T>, offset: u64) -> u64 {
    let physical = Box::into_raw(value) as u64;
    physical + offset
}

fn write_at_offset(
    segments: &[LoadedSegment],
    file_offset: usize,
    value: u64,
) -> Result<(), &'static str> {
    let segment = segments
        .iter()
        .find(|segment| {
            file_offset >= segment.file_offset
                && file_offset + 8 <= segment.file_offset + segment.file_size
        })
        .ok_or("Limine request is not in a loadable segment")?;
    let target = (segment.physical_address as usize)
        .checked_add(file_offset - segment.file_offset)
        .ok_or("request address overflow")? as *mut u64;
    unsafe { target.write_unaligned(value.to_le()) };
    Ok(())
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, &'static str> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or("truncated Limine request")?
            .try_into()
            .unwrap(),
    ))
}
