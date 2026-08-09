#![no_main]
#![no_std]

extern crate alloc;

mod bgrt;
mod elf;
mod limine;
mod linux;
mod menu;
mod paging;
mod protocol;
mod spinner;

use alloc::vec::Vec;
use core::convert::Infallible;

use protocol::BootProtocol;
use spinner::Spinner;
use uefi::boot::{self, AllocateType, LoadImageSource, MemoryType};
use uefi::cstr16;
use uefi::fs::{FileSystem, Path};
use uefi::prelude::*;

const HHDM_OFFSET: u64 = 0xffff_8000_0000_0000;

#[entry]
fn main() -> Status {
    uefi::helpers::init().expect("UEFI helpers must initialize");
    log::info!("r-boot: selecting boot protocol");

    match boot_kernel() {
        Ok(never) => match never {},
        Err(error) => {
            log::error!("r-boot: {error}");
            Status::LOAD_ERROR
        }
    }
}

fn boot_kernel() -> Result<Infallible, &'static str> {
    let image = boot::image_handle();
    let fs = boot::get_image_file_system(image).map_err(|_| "cannot open boot volume")?;
    let mut fs = FileSystem::new(fs);
    let mut spinner = Spinner::new(menu::configured_spinner_mode(&mut fs));
    spinner.tick("Searching for boot entries...");
    let mut menu = menu::Menu::load(&mut fs);
    spinner.clear();
    spinner.set_mode(menu.spinner_mode());
    if !menu.entries.is_empty() {
        let selected = menu.select(&mut fs)?;
        spinner.set_mode(menu.spinner_mode());
        let entry = menu.entries.swap_remove(selected);
        return match entry.kind {
            menu::Kind::Linux => {
                let kernel_path = menu::path(&entry.kernel)?;
                let kernel = read_with_spinner(
                    &mut fs,
                    &kernel_path,
                    &mut spinner,
                    "cannot read selected Linux kernel",
                )?;
                let mut initramfs = Vec::new();
                for path in entry.initrds {
                    let path = menu::path(&path)?;
                    let bytes = read_with_spinner(
                        &mut fs,
                        &path,
                        &mut spinner,
                        "cannot read selected initrd",
                    )?;
                    initramfs.extend_from_slice(&bytes);
                }
                drop(fs);
                linux::handover(image, &kernel, &initramfs, entry.options.as_deref())
            }
            menu::Kind::Limine => {
                let kernel_path = menu::path(&entry.kernel)?;
                let bytes = read_with_spinner(
                    &mut fs,
                    &kernel_path,
                    &mut spinner,
                    "cannot read selected Limine kernel",
                )?;
                drop(fs);
                boot_limine(&bytes)
            }
            menu::Kind::Efi => {
                let image_path = menu::path(&entry.kernel)?;
                let bytes = read_with_spinner(
                    &mut fs,
                    &image_path,
                    &mut spinner,
                    "cannot read selected EFI image",
                )?;
                drop(fs);
                let child = boot::load_image(
                    image,
                    LoadImageSource::FromBuffer {
                        buffer: &bytes,
                        file_path: None,
                    },
                )
                .map_err(|_| "firmware rejected selected EFI image")?;
                boot::start_image(child).map_err(|_| "selected EFI image returned an error")?;
                Err("selected EFI image returned")
            }
        };
    }

    // Preserve the original single-image layout when no menu configuration is
    // present, so existing ESPs remain bootable.
    if let Ok(kernel) = read_with_spinner(
        &mut fs,
        Path::new(cstr16!("\\boot\\vmlinuz")),
        &mut spinner,
        "cannot read \\boot\\vmlinuz",
    ) {
        let initramfs = read_with_spinner(
            &mut fs,
            Path::new(cstr16!("\\boot\\initramfs")),
            &mut spinner,
            "cannot read \\boot\\initramfs",
        )?;
        drop(fs);
        return linux::handover(image, &kernel, &initramfs, None);
    }
    let bytes = read_with_spinner(
        &mut fs,
        Path::new(cstr16!("\\boot\\kernel.elf")),
        &mut spinner,
        "cannot read \\boot\\kernel.elf",
    )?;
    drop(fs);
    boot_limine(&bytes)
}

/// Reads one boot payload while keeping the user informed of disk activity.
///
/// Firmware file reads are synchronous, so the frame is drawn immediately
/// before each read. Successful reads leave the frame visible while the next
/// boot stage is prepared; failures clear it before returning the error.
fn read_with_spinner(
    fs: &mut FileSystem,
    path: &Path,
    spinner: &mut Spinner,
    error: &'static str,
) -> Result<Vec<u8>, &'static str> {
    spinner.tick("Loading boot payload...");
    let bytes = fs.read(path).map_err(|_| error);
    if bytes.is_err() {
        spinner.clear();
    }
    bytes
}

fn boot_limine(bytes: &[u8]) -> Result<Infallible, &'static str> {
    let image = elf::Image::parse(&bytes)?;
    let loaded = load_segments(&bytes, &image)?;
    limine::Limine.prepare(&bytes, &image, &loaded, HHDM_OFFSET)?;

    // This initial map keeps the firmware's low address space available while
    // mapping the kernel at its requested Limine virtual addresses.
    let mut tables = paging::PageTables::new()?;
    tables.map_identity_first_4g()?;
    tables.map_hhdm_first_4g(HHDM_OFFSET)?;
    for segment in &loaded {
        tables.map_range(
            segment.virtual_address,
            segment.physical_address,
            segment.length,
            segment.flags,
        )?;
    }

    let entry = image.entry;
    log::debug!("r-boot: entering Limine kernel at {entry:#x}");

    // No Rust allocation or UEFI service may happen after this point.
    unsafe {
        let _memory_map = boot::exit_boot_services(None);
        tables.activate();
        let kernel: extern "sysv64" fn() -> ! = core::mem::transmute(entry as usize);
        kernel()
    }
}

fn load_segments(
    bytes: &[u8],
    image: &elf::Image,
) -> Result<Vec<elf::LoadedSegment>, &'static str> {
    let mut loaded = Vec::new();

    for segment in image.load_segments()? {
        let pages = elf::pages_for(segment.memory_size)?;
        let memory = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages)
            .map_err(|_| "cannot allocate kernel segment")?;
        unsafe {
            core::ptr::write_bytes(memory.as_ptr(), 0, pages * elf::PAGE_SIZE);
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr().add(segment.file_offset),
                memory.as_ptr(),
                segment.file_size,
            );
        }
        loaded.push(elf::LoadedSegment {
            virtual_address: segment.virtual_address,
            physical_address: memory.as_ptr() as u64,
            length: segment.memory_size,
            flags: segment.flags,
            file_offset: segment.file_offset,
            file_size: segment.file_size,
        });
    }

    if loaded.is_empty() {
        return Err("ELF has no PT_LOAD segments");
    }
    Ok(loaded)
}
