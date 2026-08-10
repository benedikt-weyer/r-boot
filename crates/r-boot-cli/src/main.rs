//! Inspects and edits `r-boot.toml`, the menu config r-boot's UEFI menu
//! reads at boot time. Changes made here persist until the next
//! `nixos-rebuild switch` regenerates the file via `r-boot-conf-builder`.

use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

mod args;
mod config;

use args::{Args, Command};
use config::Config;

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
        eprintln!("r-boot-cli: {err}");
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<(), Box<dyn Error>> {
    let config_path = args.esp.join("boot").join("r-boot.toml");

    match &args.command {
        Command::Show => show(&config_path),
        Command::SetDefault(id) => set_default(&config_path, id),
        Command::SetTimeout(seconds) => set_timeout(&config_path, *seconds),
        Command::SetSpinner(mode) => set_spinner(&config_path, mode),
        Command::SetLogo(visible) => set_logo(&config_path, *visible),
        Command::Remove { id, purge } => remove(&args.esp, &config_path, id, *purge),
    }
}

fn show(config_path: &Path) -> Result<(), Box<dyn Error>> {
    let config = load(config_path)?;

    println!("config:   {}", config_path.display());
    println!(
        "default:  {}",
        config
            .default
            .as_deref()
            .unwrap_or("(unset, boots first entry)")
    );
    match config.timeout {
        Some(0) => println!("timeout:  0 (boots default immediately)"),
        Some(seconds) => println!("timeout:  {seconds}s"),
        None => println!("timeout:  (unset, defaults to 5s)"),
    }
    println!(
        "spinner:  {}",
        config.spinner.as_deref().unwrap_or("graphical")
    );
    println!(
        "logo:     {}",
        if config.logo.unwrap_or(true) {
            "on"
        } else {
            "off"
        }
    );
    println!();

    if config.entries.is_empty() {
        println!("no boot entries found");
        return Ok(());
    }

    println!("entries:");
    for entry in &config.entries {
        let marker = if config.default.as_deref() == Some(entry.id.as_str()) {
            '*'
        } else {
            ' '
        };
        let title = entry.title.as_deref().unwrap_or(&entry.id);
        println!("  {marker} {} ({})", title, entry.id);
    }

    Ok(())
}

fn set_default(config_path: &Path, id: &str) -> Result<(), Box<dyn Error>> {
    let mut config = load(config_path)?;

    if !config.entries.iter().any(|entry| entry.id == id) {
        let known: Vec<&str> = config.entries.iter().map(|e| e.id.as_str()).collect();
        return Err(format!(
            "no entry with id \"{id}\" (known ids: {})",
            known.join(", ")
        )
        .into());
    }

    config.default = Some(id.to_string());
    save(config_path, &config)?;
    println!("default entry set to \"{id}\"");
    Ok(())
}

fn set_timeout(config_path: &Path, seconds: u32) -> Result<(), Box<dyn Error>> {
    let mut config = load(config_path)?;
    config.timeout = Some(seconds);
    save(config_path, &config)?;
    println!("timeout set to {seconds}s");
    Ok(())
}

fn set_spinner(config_path: &Path, mode: &str) -> Result<(), Box<dyn Error>> {
    let mut config = load(config_path)?;
    config.spinner = Some(mode.to_string());
    save(config_path, &config)?;
    println!("spinner set to {mode}");
    Ok(())
}

fn set_logo(config_path: &Path, visible: bool) -> Result<(), Box<dyn Error>> {
    let mut config = load(config_path)?;
    config.logo = Some(visible);
    save(config_path, &config)?;
    println!(
        "firmware logo {}",
        if visible { "enabled" } else { "disabled" }
    );
    Ok(())
}

fn remove(esp: &Path, config_path: &Path, id: &str, purge: bool) -> Result<(), Box<dyn Error>> {
    let mut config = load(config_path)?;

    let Some(index) = config.entries.iter().position(|entry| entry.id == id) else {
        let known: Vec<&str> = config.entries.iter().map(|e| e.id.as_str()).collect();
        return Err(format!(
            "no entry with id \"{id}\" (known ids: {})",
            known.join(", ")
        )
        .into());
    };
    let entry = config.entries.remove(index);

    if config.default.as_deref() == Some(id) {
        config.default = None;
    }

    save(config_path, &config)?;
    println!("removed entry \"{id}\"");

    if purge {
        let mut files: Vec<&str> = entry.initrd.iter().map(String::as_str).collect();
        if let Some(linux) = &entry.linux {
            files.push(linux);
        }
        if let Some(efi) = &entry.efi {
            files.push(efi);
        }
        for file in files {
            let path = resolve_esp_path(esp, file);
            if let Err(err) = remove_boot_file(&path) {
                eprintln!("r-boot-cli: cannot remove {}: {err}", path.display());
            } else {
                println!("removed {}", path.display());
            }
        }
    }

    Ok(())
}

/// Resolves a config-file path like `/boot/nixos/kernel-foo` (rooted at the
/// ESP, matching r-boot's own path handling) to its real filesystem location.
fn resolve_esp_path(esp: &Path, entry_path: &str) -> PathBuf {
    esp.join(entry_path.trim_start_matches('/'))
}

/// Kernels/initrds are installed read-only (see r-boot-conf-builder); undo
/// that before removing so the unlink doesn't get rejected.
fn remove_boot_file(path: &Path) -> Result<(), Box<dyn Error>> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o200);
    fs::set_permissions(path, perms)?;
    fs::remove_file(path)?;
    Ok(())
}

fn load(config_path: &Path) -> Result<Config, Box<dyn Error>> {
    let contents = fs::read_to_string(config_path)
        .map_err(|e| format!("cannot read {}: {e}", config_path.display()))?;
    Ok(Config::parse(&contents))
}

fn save(config_path: &Path, config: &Config) -> Result<(), Box<dyn Error>> {
    let tmp_path: PathBuf = config_path.with_extension(format!("toml.tmp.{}", std::process::id()));
    fs::write(&tmp_path, config.render())?;
    fs::rename(&tmp_path, config_path)?;
    Ok(())
}
