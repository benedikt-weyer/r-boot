//! Discovery and chainloading of other UEFI bootloaders found on attached
//! volumes, reachable from the boot menu via the `b` key.

use alloc::vec::Vec;

use uefi::boot::{self, LoadImageSource};
use uefi::fs::{FileSystem, Path};
use uefi::proto::console::text::{Key, ScanCode};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{CStr16, Handle, cstr16, system};

/// A bootloader EFI application found on some attached volume.
pub struct Bootloader {
    title: &'static str,
    device: Handle,
    path: &'static CStr16,
}

/// EFI application paths recognized as other bootloaders, checked on every
/// attached volume. Vendor directories (e.g. Microsoft's) don't match the OS
/// name, and shim/GRUB naming varies by distribution, so this is a fixed
/// list rather than a directory scan.
const KNOWN: &[(&CStr16, &str)] = &[
    (
        cstr16!("\\EFI\\Microsoft\\Boot\\bootmgfw.efi"),
        "Windows Boot Manager",
    ),
    (cstr16!("\\EFI\\ubuntu\\shimx64.efi"), "Ubuntu"),
    (cstr16!("\\EFI\\ubuntu\\grubx64.efi"), "Ubuntu (GRUB)"),
    (cstr16!("\\EFI\\fedora\\shimx64.efi"), "Fedora"),
    (cstr16!("\\EFI\\debian\\shimx64.efi"), "Debian"),
    (cstr16!("\\EFI\\opensuse\\shimx64.efi"), "openSUSE"),
    (
        cstr16!("\\EFI\\systemd\\systemd-bootx64.efi"),
        "systemd-boot",
    ),
    (cstr16!("\\EFI\\BOOT\\BOOTX64.EFI"), "Fallback bootloader"),
];

/// Lets the user pick another bootloader found on any attached volume and
/// hands off execution to it. Returns `Ok(())` if the user cancelled or no
/// other bootloader was found; only a failed handoff is an error.
pub fn run(fs: &mut FileSystem) -> Result<(), &'static str> {
    let own_device = own_device_handle();
    let bootloaders = discover(fs, own_device);
    if bootloaders.is_empty() {
        notify("No other bootloaders found.");
        return Ok(());
    }
    let Some(index) = select(&bootloaders) else {
        return Ok(());
    };
    let bootloader = &bootloaders[index];
    let bytes = read(fs, bootloader, own_device)?;
    let image = boot::image_handle();
    let child = boot::load_image(
        image,
        LoadImageSource::FromBuffer {
            buffer: &bytes,
            file_path: None,
        },
    )
    .map_err(|_| "firmware rejected selected bootloader image")?;
    boot::start_image(child).map_err(|_| "selected bootloader image returned an error")?;
    Err("selected bootloader image returned")
}

/// The device handle r-boot itself was loaded from, used to skip the
/// fallback path on r-boot's own volume (that is where r-boot is installed).
fn own_device_handle() -> Option<Handle> {
    let loaded_image = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()).ok()?;
    loaded_image.device()
}

/// Finds other bootloaders on the current boot volume (`fs`, already open)
/// and any other attached volume.
fn discover(fs: &mut FileSystem, own_device: Option<Handle>) -> Vec<Bootloader> {
    let mut found = Vec::new();
    if let Some(device) = own_device {
        scan(fs, device, own_device, &mut found);
    }
    let Ok(handles) = boot::find_handles::<SimpleFileSystem>() else {
        return found;
    };
    for handle in handles {
        if Some(handle) == own_device {
            continue;
        }
        let Ok(protocol) = boot::open_protocol_exclusive::<SimpleFileSystem>(handle) else {
            continue;
        };
        let mut other_fs = FileSystem::new(protocol);
        scan(&mut other_fs, handle, own_device, &mut found);
    }
    found
}

fn scan(
    fs: &mut FileSystem,
    device: Handle,
    own_device: Option<Handle>,
    found: &mut Vec<Bootloader>,
) {
    for &(path, title) in KNOWN {
        if title == "Fallback bootloader" && Some(device) == own_device {
            continue;
        }
        if fs.try_exists(Path::new(path)).unwrap_or(false) {
            found.push(Bootloader {
                title,
                device,
                path,
            });
        }
    }
}

/// Reads a bootloader's bytes, reusing `fs` if it lives on r-boot's own
/// volume, or opening its volume otherwise.
fn read(
    fs: &mut FileSystem,
    bootloader: &Bootloader,
    own_device: Option<Handle>,
) -> Result<alloc::vec::Vec<u8>, &'static str> {
    if Some(bootloader.device) == own_device {
        return fs
            .read(Path::new(bootloader.path))
            .map_err(|_| "cannot read selected bootloader");
    }
    let protocol = boot::open_protocol_exclusive::<SimpleFileSystem>(bootloader.device)
        .map_err(|_| "cannot open selected bootloader's volume")?;
    let mut other_fs = FileSystem::new(protocol);
    other_fs
        .read(Path::new(bootloader.path))
        .map_err(|_| "cannot read selected bootloader")
}

/// Presents `bootloaders` in a simple list menu and returns the chosen
/// index, or `None` if the user pressed Escape.
fn select(bootloaders: &[Bootloader]) -> Option<usize> {
    let mut selected = 0;
    draw(bootloaders, selected);
    loop {
        let key_event = system::with_stdin(|input| input.wait_for_key_event()).ok()?;
        let mut events = unsafe { [key_event.unsafe_clone()] };
        boot::wait_for_event(&mut events).ok()?;
        let key = system::with_stdin(|input| input.read_key()).ok()?;
        match key {
            Some(Key::Special(ScanCode::UP)) => {
                selected = selected.checked_sub(1).unwrap_or(bootloaders.len() - 1);
                draw(bootloaders, selected);
            }
            Some(Key::Special(ScanCode::DOWN)) => {
                selected = (selected + 1) % bootloaders.len();
                draw(bootloaders, selected);
            }
            Some(Key::Printable(character)) if character == '\r' => return Some(selected),
            Some(Key::Special(ScanCode::ESCAPE)) => return None,
            _ => continue,
        }
    }
}

fn draw(bootloaders: &[Bootloader], selected: usize) {
    system::with_stdout(|output| {
        let _ = output.clear();
    });
    uefi::println!("Chainload another bootloader");
    uefi::println!();
    for (index, bootloader) in bootloaders.iter().enumerate() {
        let marker = if index == selected { '>' } else { ' ' };
        uefi::println!("{marker} {}", bootloader.title);
    }
    uefi::println!();
    uefi::println!("Use Up/Down and Enter to boot. Esc cancels.");
}

/// Shows a message and waits for a keypress before returning.
fn notify(message: &str) {
    system::with_stdout(|output| {
        let _ = output.clear();
    });
    uefi::println!("{message}");
    uefi::println!();
    uefi::println!("Press any key to continue.");
    let _ = system::with_stdin(|input| input.wait_for_key_event())
        .ok()
        .map(|key_event| {
            let mut events = unsafe { [key_event.unsafe_clone()] };
            let _ = boot::wait_for_event(&mut events);
            let _ = system::with_stdin(|input| input.read_key());
        });
}
