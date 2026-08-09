//! Minimal, deliberately strict ELF64 loader for the first protocol backend.

use alloc::vec::Vec;

pub const PAGE_SIZE: usize = 4096;
const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const PT_LOAD: u32 = 1;
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;

#[derive(Clone, Copy)]
pub struct Segment {
    pub virtual_address: u64,
    pub file_offset: usize,
    pub file_size: usize,
    pub memory_size: usize,
    pub flags: u32,
}

pub struct LoadedSegment {
    pub virtual_address: u64,
    pub physical_address: u64,
    pub length: usize,
    pub flags: u32,
    pub file_offset: usize,
    pub file_size: usize,
}

pub struct Image<'a> {
    bytes: &'a [u8],
    pub entry: u64,
    program_headers_offset: usize,
    program_headers_count: usize,
    program_header_size: usize,
}

impl<'a> Image<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, &'static str> {
        if bytes.len() < ELF_HEADER_SIZE || &bytes[..4] != b"\x7fELF" {
            return Err("kernel is not an ELF file");
        }
        if bytes[4] != 2 || bytes[5] != 1 {
            return Err("kernel must be little-endian ELF64");
        }
        if read_u16(bytes, 16)? != ET_EXEC || read_u16(bytes, 18)? != EM_X86_64 {
            return Err("kernel must be an x86_64 ET_EXEC ELF");
        }
        let program_headers_offset =
            usize::try_from(read_u64(bytes, 32)?).map_err(|_| "ELF offsets exceed usize")?;
        let program_header_size = read_u16(bytes, 54)? as usize;
        let program_headers_count = read_u16(bytes, 56)? as usize;
        if program_header_size != PROGRAM_HEADER_SIZE {
            return Err("unexpected ELF program header size");
        }
        let table_size = program_header_size
            .checked_mul(program_headers_count)
            .ok_or("ELF table overflow")?;
        bytes
            .get(
                program_headers_offset
                    ..program_headers_offset
                        .checked_add(table_size)
                        .ok_or("ELF table overflow")?,
            )
            .ok_or("truncated ELF program header table")?;

        Ok(Self {
            bytes,
            entry: read_u64(bytes, 24)?,
            program_headers_offset,
            program_headers_count,
            program_header_size,
        })
    }

    pub fn load_segments(&self) -> Result<Vec<Segment>, &'static str> {
        let mut segments = Vec::new();
        for index in 0..self.program_headers_count {
            let offset = self.program_headers_offset + index * self.program_header_size;
            if read_u32(self.bytes, offset)? != PT_LOAD {
                continue;
            }
            let file_offset = usize::try_from(read_u64(self.bytes, offset + 8)?)
                .map_err(|_| "ELF offset exceeds usize")?;
            let virtual_address = read_u64(self.bytes, offset + 16)?;
            let file_size = usize::try_from(read_u64(self.bytes, offset + 32)?)
                .map_err(|_| "ELF size exceeds usize")?;
            let memory_size = usize::try_from(read_u64(self.bytes, offset + 40)?)
                .map_err(|_| "ELF size exceeds usize")?;
            if memory_size < file_size || virtual_address < 0xffff_8000_0000_0000 {
                return Err("PT_LOAD segment is not a valid higher-half Limine segment");
            }
            if virtual_address as usize & (PAGE_SIZE - 1) != 0 || file_offset & (PAGE_SIZE - 1) != 0
            {
                return Err("PT_LOAD segments must be page aligned");
            }
            self.bytes
                .get(
                    file_offset
                        ..file_offset
                            .checked_add(file_size)
                            .ok_or("ELF segment overflow")?,
                )
                .ok_or("truncated ELF segment")?;
            segments.push(Segment {
                virtual_address,
                file_offset,
                file_size,
                memory_size,
                flags: read_u32(self.bytes, offset + 4)?,
            });
        }
        Ok(segments)
    }
}

pub fn pages_for(bytes: usize) -> Result<usize, &'static str> {
    bytes
        .checked_add(PAGE_SIZE - 1)
        .map(|value| value / PAGE_SIZE)
        .filter(|pages| *pages > 0)
        .ok_or("invalid empty or oversized ELF segment")
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or("truncated ELF header")?
            .try_into()
            .unwrap(),
    ))
}
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("truncated ELF header")?
            .try_into()
            .unwrap(),
    ))
}
fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, &'static str> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or("truncated ELF header")?
            .try_into()
            .unwrap(),
    ))
}
