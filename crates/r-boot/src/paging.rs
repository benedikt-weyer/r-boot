//! x86_64 four-level page tables. The CR3 write is the sole assembly in r-boot:
//! Rust has no stable intrinsic for this privileged instruction.

use core::ptr::NonNull;

use uefi::boot::{self, AllocateType, MemoryType};

use crate::elf::PAGE_SIZE;

const PRESENT: u64 = 1;
const WRITABLE: u64 = 1 << 1;
const HUGE: u64 = 1 << 7;
const EXECUTE_DISABLE: u64 = 1 << 63;
const PAGE_2M: u64 = 2 * 1024 * 1024;

pub struct PageTables {
    root: NonNull<u64>,
}

impl PageTables {
    pub fn new() -> Result<Self, &'static str> {
        Ok(Self {
            root: allocate_table()?,
        })
    }

    pub fn map_identity_first_4g(&mut self) -> Result<(), &'static str> {
        self.map_2m_window(0)
    }

    pub fn map_hhdm_first_4g(&mut self, offset: u64) -> Result<(), &'static str> {
        self.map_2m_window(offset)
    }

    fn map_2m_window(&mut self, virtual_base: u64) -> Result<(), &'static str> {
        for gigabyte in 0..4u64 {
            let pdpt =
                self.child_table(self.root, index_pml4(virtual_base + gigabyte * (1 << 30)))?;
            let pd = allocate_table()?;
            unsafe {
                pdpt.as_ptr()
                    .add(index_pdpt(virtual_base + gigabyte * (1 << 30)))
                    .write((pd.as_ptr() as u64) | PRESENT | WRITABLE)
            };
            for entry in 0..512u64 {
                let physical = gigabyte * (1 << 30) + entry * PAGE_2M;
                unsafe {
                    pd.as_ptr()
                        .add(entry as usize)
                        .write(physical | PRESENT | WRITABLE | HUGE)
                };
            }
        }
        Ok(())
    }

    pub fn map_range(
        &mut self,
        virtual_address: u64,
        physical_address: u64,
        length: usize,
        elf_flags: u32,
    ) -> Result<(), &'static str> {
        let pages = length
            .checked_add(PAGE_SIZE - 1)
            .ok_or("mapping length overflow")?
            / PAGE_SIZE;
        for page in 0..pages {
            let virtual_page = virtual_address + (page * PAGE_SIZE) as u64;
            let physical_page = physical_address + (page * PAGE_SIZE) as u64;
            let pdpt = self.child_table(self.root, index_pml4(virtual_page))?;
            let pd = self.child_table(pdpt, index_pdpt(virtual_page))?;
            let pt = self.child_table(pd, index_pd(virtual_page))?;
            let mut flags = PRESENT;
            if elf_flags & 2 != 0 {
                flags |= WRITABLE;
            }
            if elf_flags & 1 == 0 {
                flags |= EXECUTE_DISABLE;
            }
            unsafe {
                pt.as_ptr()
                    .add(index_pt(virtual_page))
                    .write(physical_page | flags)
            };
        }
        Ok(())
    }

    fn child_table(
        &mut self,
        parent: NonNull<u64>,
        index: usize,
    ) -> Result<NonNull<u64>, &'static str> {
        let entry = unsafe { parent.as_ptr().add(index).read() };
        if entry & PRESENT != 0 {
            return NonNull::new((entry & 0x000f_ffff_ffff_f000) as *mut u64)
                .ok_or("invalid page table entry");
        }
        let table = allocate_table()?;
        unsafe {
            parent
                .as_ptr()
                .add(index)
                .write((table.as_ptr() as u64) | PRESENT | WRITABLE)
        };
        Ok(table)
    }

    pub unsafe fn activate(&self) {
        // SAFETY: `root` is a page-aligned, initialized PML4 whose physical
        // address is identity mapped by the current UEFI page tables.
        unsafe {
            core::arch::asm!(
                "mov cr3, {}",
                in(reg) self.root.as_ptr() as u64,
                options(nostack, preserves_flags),
            );
        }
    }
}

fn allocate_table() -> Result<NonNull<u64>, &'static str> {
    let page = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
        .map_err(|_| "cannot allocate page table")?;
    unsafe { core::ptr::write_bytes(page.as_ptr(), 0, PAGE_SIZE) };
    Ok(page.cast())
}

fn index_pml4(address: u64) -> usize {
    ((address >> 39) & 0x1ff) as usize
}
fn index_pdpt(address: u64) -> usize {
    ((address >> 30) & 0x1ff) as usize
}
fn index_pd(address: u64) -> usize {
    ((address >> 21) & 0x1ff) as usize
}
fn index_pt(address: u64) -> usize {
    ((address >> 12) & 0x1ff) as usize
}
