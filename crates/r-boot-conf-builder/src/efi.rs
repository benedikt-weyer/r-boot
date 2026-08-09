use std::fs;
use std::path::Path;
use std::process::Command;

/// Registers a UEFI boot entry for r-boot via `efibootmgr`, unless one
/// already exists. Best-effort: failures are silently ignored, matching
/// `nixos-rebuild`'s tolerance for boot managers that can't touch NVRAM
/// (e.g. inside a VM without an EFI variable store).
pub fn ensure_boot_entry(esp: &Path) {
    let Some(entries) = run_capture("efibootmgr", &[]) else {
        return;
    };
    if entries.contains("r-boot") {
        return;
    }

    let Some(source) = run_capture(
        "findmnt",
        &["-n", "-o", "SOURCE", "--target", &esp.to_string_lossy()],
    ) else {
        return;
    };

    let Some(part_name) = Path::new(&source).file_name() else {
        return;
    };
    let part_name = part_name.to_string_lossy();

    let Some(disk_name) = run_capture("lsblk", &["-no", "pkname", &source])
        .and_then(|out| out.lines().next().map(str::to_string))
        .filter(|name| !name.is_empty())
    else {
        return;
    };

    let Some(part_num) = fs::read_to_string(format!("/sys/class/block/{part_name}/partition"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return;
    };

    let _ = Command::new("efibootmgr")
        .args([
            "--create",
            "--disk",
            &format!("/dev/{disk_name}"),
            "--part",
            &part_num,
            "--label",
            "r-boot",
            "--loader",
            r"\EFI\BOOT\BOOTX64.EFI",
        ])
        .output();
}

/// Runs `program`, returning its trimmed stdout on success.
fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
