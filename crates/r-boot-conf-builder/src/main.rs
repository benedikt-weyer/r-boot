//! Installs r-boot to a NixOS system's EFI system partition and generates
//! the `r-boot.toml` menu config from the system's boot generations.
//!
//! This is the Rust equivalent of `nixos-generate-config`-style installer
//! scripts used by other bootloaders (`systemd-boot`, GRUB): it is invoked
//! as `system.build.installBootLoader` on every `nixos-rebuild switch`.

use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

mod args;
mod efi;
mod menu;

use args::Args;

fn main() {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("{err}");
            Args::usage();
            std::process::exit(1);
        }
    };

    if let Err(err) = run(&args) {
        eprintln!("r-boot-conf-builder: {err}");
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<(), Box<dyn Error>> {
    let target = args.esp.join("boot");
    fs::create_dir_all(target.join("nixos"))?;

    let mut copied = HashSet::new();
    let mut toml = String::new();
    toml.push_str("# Generated file, all changes will be lost on nixos-rebuild!\n");
    toml.push_str("default = \"nixos-default\"\n");
    toml.push_str(&format!("timeout = {}\n", args.timeout));

    if let Some(entry) = menu::add_entry(&target, &args.default, "nixos-default", &mut copied)? {
        toml.push_str(&entry);
    }

    if args.num_generations > 0 {
        for (generation, path) in list_generations(args.num_generations)? {
            if let Some(entry) = menu::add_entry(
                &target,
                &path,
                &format!("nixos-generation-{generation}"),
                &mut copied,
            )? {
                toml.push_str(&entry);
            }
        }
    }

    let tmp_toml = target.join(format!("r-boot.toml.tmp.{}", std::process::id()));
    fs::write(&tmp_toml, &toml)?;
    fs::rename(&tmp_toml, target.join("r-boot.toml"))?;

    prune_unreferenced_boot_files(&target.join("nixos"), &copied)?;

    install_binary(&args.binary, &args.esp.join("EFI/BOOT/BOOTX64.EFI"))?;

    if args.touch_efi_vars {
        efi::ensure_boot_entry(&args.esp);
    }

    Ok(())
}

/// Older generations under `/nix/var/nix/profiles`, most recent first,
/// capped at `limit`.
fn list_generations(limit: u32) -> Result<Vec<(u64, PathBuf)>, Box<dyn Error>> {
    let profiles_dir = Path::new("/nix/var/nix/profiles");
    let mut generations = Vec::new();

    for entry in fs::read_dir(profiles_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(number) = name
            .strip_prefix("system-")
            .and_then(|rest| rest.strip_suffix("-link"))
        else {
            continue;
        };
        let Ok(number) = number.parse::<u64>() else {
            continue;
        };
        generations.push((number, profiles_dir.join(name)));
    }

    generations.sort_by_key(|(number, _)| std::cmp::Reverse(*number));
    generations.truncate(limit as usize);
    Ok(generations)
}

/// Removes kernels/initrds copied by a previous run that no longer belong
/// to any generation kept in this run's `r-boot.toml`.
fn prune_unreferenced_boot_files(
    kernels_dir: &Path,
    copied: &HashSet<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(kernels_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !copied.contains(&path) {
            println!("Removing no longer needed boot file: {}", path.display());
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(perms.mode() | 0o200);
            fs::set_permissions(&path, perms)?;
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn install_binary(src: &Path, dst: &Path) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dst)?;
    fs::set_permissions(dst, fs::Permissions::from_mode(0o755))?;
    Ok(())
}
