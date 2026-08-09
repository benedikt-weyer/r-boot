use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

/// Copies a file from the Nix store into `target/nixos`, deduplicated by
/// name, and returns its path relative to the ESP root (the form r-boot's
/// config expects).
fn copy_to_kernels_dir(
    target: &Path,
    src: &Path,
    copied: &mut HashSet<PathBuf>,
) -> Result<String, Box<dyn Error>> {
    let src = fs::canonicalize(src)?;
    let name = clean_name(&src);
    let dst = target.join("nixos").join(&name);

    if !dst.exists() {
        let tmp = target
            .join("nixos")
            .join(format!("{name}.tmp.{}", std::process::id()));
        fs::copy(&src, &tmp)?;
        fs::rename(&tmp, &dst)?;
    }
    copied.insert(dst);

    Ok(format!("/boot/nixos/{name}"))
}

/// Converts a Nix store path such as `/nix/store/<hash>-<name>/file` to
/// `<hash>-<name>-file`.
fn clean_name(path: &Path) -> String {
    let path = path.to_string_lossy();
    let path = path.strip_prefix("/nix/store/").unwrap_or(&path);
    path.replace('/', "-")
}

/// Builds the `[[entries]]` table for one generation, copying its kernel
/// and initrd into the ESP. Returns `None` (and adds nothing) if the
/// generation doesn't look like a bootable NixOS system.
pub fn add_entry(
    target: &Path,
    generation: &Path,
    id: &str,
    copied: &mut HashSet<PathBuf>,
) -> Result<Option<String>, Box<dyn Error>> {
    let path = fs::canonicalize(generation)?;

    if !path.join("kernel").exists() || !path.join("initrd").exists() {
        return Ok(None);
    }

    let kernel = copy_to_kernels_dir(target, &path.join("kernel"), copied)?;
    let initrd = copy_to_kernels_dir(target, &path.join("initrd"), copied)?;

    let nixos_label = read_trimmed(&path.join("nixos-version")).unwrap_or_else(|| "unknown".into());
    let extra_params = read_trimmed(&path.join("kernel-params")).unwrap_or_default();

    let init = path.join("init");
    let options = format!("init={} {extra_params}", init.display());

    Ok(Some(format!(
        "\n[[entries]]\nid = \"{id}\"\ntitle = \"NixOS ({nixos_label}, {id})\"\nkind = \"linux\"\nlinux = \"{kernel}\"\ninitrd = \"{initrd}\"\noptions = \"{options}\"\n"
    )))
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim_end().to_string())
}
