//! The initial Limine request backend.
//!
//! It intentionally supports a small useful subset: base revision 0,
//! bootloader-info, firmware-type, HHDM, and executable-address.  Unknown
//! requests are left unanswered, as required by the protocol.

use alloc::{boxed::Box, vec::Vec};
use crate::elf::{Image, LoadedSegment};
use uefi::boot::{self, AllocateType, MemoryType};
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};

const COMMON_MAGIC: [u64; 2] = [0xc7b1_dd30_df4c_8b88, 0x0a82_e883_a194_f07b];
const BASE_MAGIC: [u64; 2] = [0xf956_2b2d_5c95_a6c8, 0x6a7b_3849_4453_6bdc];
const BOOTLOADER_INFO: [u64; 2] = [0xf550_38d8_e2a1_202f, 0x2794_26fc_f5f5_9740];
const FIRMWARE_TYPE: [u64; 2] = [0x8c2f_75d9_0bef_28a8, 0x7045_a468_8eac_00c3];
const HHDM: [u64; 2] = [0x48dc_f1cb_8ad2_b852, 0x6398_4e95_9a98_244b];
const EXECUTABLE_ADDRESS: [u64; 2] = [0x71ba_7686_3cc5_5f63, 0xb264_4a48_c516_a487];
const FRAMEBUFFER: [u64; 2] = [0x9d58_27dc_d881_dd75, 0xa314_8604_f6fa_b11b];
const MEMORY_MAP: [u64; 2] = [0x67cf_3d9d_378a_806f, 0xe304_acdf_c50c_3c62];
const MODULES: [u64; 2] = [0x3e7e_2797_02be_32af, 0xca1c_4f3b_d128_0cee];
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
#[repr(C)]
struct FramebufferResponse {
    revision: u64,
    framebuffer_count: u64,
    framebuffers: u64,
}
#[repr(C)]
struct RawFramebuffer {
    address: u64,
    width: u64,
    height: u64,
    pitch: u64,
    bpp: u16,
    memory_model: u8,
    red_mask_size: u8,
    red_mask_shift: u8,
    green_mask_size: u8,
    green_mask_shift: u8,
    blue_mask_size: u8,
    blue_mask_shift: u8,
    unused: [u8; 7],
    edid_size: u64,
    edid: u64,
}
#[repr(C)]
struct MemoryMapResponse {
    revision: u64,
    entry_count: u64,
    entries: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryMapEntry {
    base: u64,
    length: u64,
    kind: u64,
}
#[repr(C)]
struct ModuleResponse {
    revision: u64,
    module_count: u64,
    modules: u64,
}
#[repr(C)]
struct File {
    revision: u64,
    address: u64,
    size: u64,
    path: u64,
    string: u64,
    media_type: u32,
    unused: u32,
    tftp_ip: u32,
    tftp_port: u32,
    partition_index: u32,
    mbr_disk_id: u32,
    gpt_disk_id: [u8; 16],
    gpt_partition_id: [u8; 16],
    partition_uuid: [u8; 16],
}

static NAME: &[u8] = b"r-boot\0";
static VERSION: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();

pub struct Limine;

pub struct Module {
    pub name: &'static str,
    pub bytes: Vec<u8>,
}

pub struct LoadedModule {
    pub physical_address: u64,
    pub length: usize,
    pub name: u64,
}

pub struct Prepared {
    memory_map: MemoryMapStorage,
}

impl Prepared {
    pub fn populate_memory_map(&mut self, memory_map: &impl MemoryMap) {
        self.memory_map.populate(memory_map);
    }

    pub fn hhdm_physical_end(&self) -> u64 {
        self.memory_map.physical_end
    }
}

impl Limine {
    pub fn prepare(
        bytes: &[u8],
        image: &Image<'_>,
        segments: &[LoadedSegment],
        modules: &[Module],
        hhdm_offset: u64,
    ) -> Result<Prepared, &'static str> {
        let modules = load_modules(modules, hhdm_offset)?;
        let module_response = module_response(&modules, hhdm_offset)?;
        let (framebuffer_response, framebuffer_range) = framebuffer_response(hhdm_offset);
        let mut ranges = Vec::new();
        for segment in segments {
            ranges.push(MemoryRange {
                base: segment.physical_address,
                length: segment.length as u64,
                kind: 6,
            });
        }
        for module in &modules {
            ranges.push(MemoryRange {
                base: module.physical_address,
                length: module.length as u64,
                kind: 6,
            });
        }
        if let Some(range) = framebuffer_range {
            ranges.push(range);
        }
        let memory_map = MemoryMapStorage::new(hhdm_offset, ranges)?;
        write_responses(
            bytes,
            image,
            segments,
            module_response,
            memory_map.response_address(hhdm_offset),
            framebuffer_response,
            hhdm_offset,
        )?;
        Ok(Prepared { memory_map })
    }
}

fn write_responses(
    bytes: &[u8],
    _image: &Image<'_>,
    segments: &[LoadedSegment],
    module_response: u64,
    memory_map_response: u64,
    framebuffer_response: u64,
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
        } else if id == FRAMEBUFFER {
            framebuffer_response
        } else if id == MEMORY_MAP {
            memory_map_response
        } else if id == MODULES {
            module_response
        } else {
            continue;
        };
        write_at_offset(segments, offset + 40, response)?;
    }
    Ok(())
}

fn load_modules(modules: &[Module], hhdm_offset: u64) -> Result<Vec<LoadedModule>, &'static str> {
    let mut loaded = Vec::new();
    for module in modules {
        let pages = module.bytes.len().div_ceil(4096);
        let memory = boot::allocate_pages(
            AllocateType::MaxAddress(0xffff_f000),
            MemoryType::LOADER_DATA,
            pages,
        )
        .map_err(|_| "cannot allocate Limine module below 4 GiB")?;
        unsafe {
            core::ptr::copy_nonoverlapping(module.bytes.as_ptr(), memory.as_ptr(), module.bytes.len());
        }
        let mut name = module.name.as_bytes().to_vec();
        name.push(0);
        let name = Box::leak(name.into_boxed_slice());
        loaded.push(LoadedModule {
            physical_address: memory.as_ptr() as u64,
            length: module.bytes.len(),
            name: hhdm_offset + name.as_ptr() as u64,
        });
    }
    Ok(loaded)
}

fn module_response(modules: &[LoadedModule], hhdm_offset: u64) -> Result<u64, &'static str> {
    let mut files = Vec::new();
    for module in modules {
        files.push(hhdm_of_box(
            Box::new(File {
                revision: 0,
                address: hhdm_offset + module.physical_address,
                size: module.length as u64,
                path: module.name,
                string: module.name,
                media_type: 0,
                unused: 0,
                tftp_ip: 0,
                tftp_port: 0,
                partition_index: 0,
                mbr_disk_id: 0,
                gpt_disk_id: [0; 16],
                gpt_partition_id: [0; 16],
                partition_uuid: [0; 16],
            }),
            hhdm_offset,
        ));
    }
    let files = Box::leak(files.into_boxed_slice());
    let response = ModuleResponse {
        revision: 0,
        module_count: files.len() as u64,
        modules: hhdm_offset + files.as_ptr() as u64,
    };
    Ok(hhdm_of_box(Box::new(response), hhdm_offset))
}

struct MemoryRange {
    base: u64,
    length: u64,
    kind: u64,
}

struct MemoryMapStorage {
    entries: Box<[MemoryMapEntry]>,
    pointers: Box<[u64]>,
    response: Box<MemoryMapResponse>,
    ranges: Vec<MemoryRange>,
    physical_end: u64,
}

impl MemoryMapStorage {
    fn new(hhdm_offset: u64, ranges: Vec<MemoryRange>) -> Result<Self, &'static str> {
        let memory_map =
            boot::memory_map(MemoryType::LOADER_DATA).map_err(|_| "cannot reserve Limine memory map")?;
        let count = memory_map.entries().count();
        let physical_end = memory_map
            .entries()
            .map(|entry| entry.phys_start.saturating_add(entry.page_count.saturating_mul(4096)))
            .max()
            .unwrap_or(0);
        // Every special range can split one UEFI descriptor into three pieces.
        let capacity = count + ranges.len() * 2 + 32;
        let entries = alloc::vec![
            MemoryMapEntry {
                base: 0,
                length: 0,
                kind: 1,
            };
            capacity
        ]
        .into_boxed_slice();
        let pointers = entries
            .iter()
            .map(|entry| hhdm_offset + (entry as *const MemoryMapEntry as u64))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let response = Box::new(MemoryMapResponse {
            revision: 0,
            entry_count: 0,
            entries: hhdm_offset + pointers.as_ptr() as u64,
        });
        Ok(Self {
            entries,
            pointers,
            response,
            ranges,
            physical_end,
        })
    }

    fn response_address(&self, hhdm_offset: u64) -> u64 {
        hhdm_offset + (&*self.response as *const MemoryMapResponse as u64)
    }

    fn populate(&mut self, memory_map: &impl MemoryMap) {
        let mut count = 0;
        for descriptor in memory_map.entries() {
            let mut cursor = descriptor.phys_start;
            let end = cursor.saturating_add(descriptor.page_count.saturating_mul(4096));
            while cursor < end && count < self.entries.len() {
                let mut next = end;
                let mut kind = uefi_memory_kind(descriptor.ty);
                for range in &self.ranges {
                    let range_end = range.base.saturating_add(range.length);
                    if cursor >= range.base && cursor < range_end {
                        kind = range.kind;
                        next = next.min(range_end);
                    } else if range.base > cursor {
                        next = next.min(range.base);
                    }
                }
                self.entries[count] = MemoryMapEntry {
                    base: cursor,
                    length: next.saturating_sub(cursor),
                    kind,
                };
                count += 1;
                cursor = next;
            }
        }
        self.response.entry_count = count as u64;
        let _ = &self.pointers;
    }
}

fn uefi_memory_kind(ty: MemoryType) -> u64 {
    match ty {
        MemoryType::CONVENTIONAL => 0,
        MemoryType::ACPI_RECLAIM => 2,
        MemoryType::ACPI_NON_VOLATILE => 3,
        MemoryType::UNUSABLE => 4,
        MemoryType::BOOT_SERVICES_CODE | MemoryType::BOOT_SERVICES_DATA => 5,
        _ => 1,
    }
}

fn framebuffer_response(hhdm_offset: u64) -> (u64, Option<MemoryRange>) {
    let Ok(handle) = boot::get_handle_for_protocol::<GraphicsOutput>() else {
        return (0, None);
    };
    let Ok(mut gop) = boot::open_protocol_exclusive::<GraphicsOutput>(handle) else {
        return (0, None);
    };
    let info = gop.current_mode_info();
    let (width, height) = info.resolution();
    let (red_shift, green_shift, blue_shift) = match info.pixel_format() {
        PixelFormat::Rgb => (0, 8, 16),
        PixelFormat::Bgr => (16, 8, 0),
        _ => return (0, None),
    };
    let framebuffer = hhdm_of_box(
        Box::new(RawFramebuffer {
            address: hhdm_offset + gop.frame_buffer().as_mut_ptr() as u64,
            width: width as u64,
            height: height as u64,
            pitch: (info.stride() * 4) as u64,
            bpp: 32,
            memory_model: 1,
            red_mask_size: 8,
            red_mask_shift: red_shift,
            green_mask_size: 8,
            green_mask_shift: green_shift,
            blue_mask_size: 8,
            blue_mask_shift: blue_shift,
            unused: [0; 7],
            edid_size: 0,
            edid: 0,
        }),
        hhdm_offset,
    );
    let framebuffers = Box::leak(alloc::vec![framebuffer].into_boxed_slice());
    let response = hhdm_of_box(
        Box::new(FramebufferResponse {
            revision: 0,
            framebuffer_count: 1,
            framebuffers: hhdm_offset + framebuffers.as_ptr() as u64,
        }),
        hhdm_offset,
    );
    (
        response,
        Some(MemoryRange {
            base: gop.frame_buffer().as_mut_ptr() as u64,
            length: (info.stride() * height * 4) as u64,
            kind: 7,
        }),
    )
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
