//! A minimal interactive shell for browsing the boot filesystem, reachable
//! from the boot menu via the `t` key. Supports `ls`, `lsblk`, `cd`, `pwd`,
//! `clear`, and `help`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams};
use uefi::fs::{FileSystem, Path, PathBuf, SEPARATOR_STR};
use uefi::proto::console::text::{Key, ScanCode};
use uefi::proto::device_path::text::{AllowShortcuts, DisplayOnly};
use uefi::proto::device_path::DevicePath;
use uefi::proto::media::block::BlockIO;
use uefi::{system, CString16};

/// Command names completed at the start of a line, before the first space.
const COMMANDS: [&str; 8] = ["cd", "clear", "exit", "help", "ls", "lsblk", "pwd", "quit"];

/// Runs the shell until the user types `exit` or presses Escape.
pub fn run(fs: &mut FileSystem) {
    let mut cwd = PathBuf::from(SEPARATOR_STR);
    system::with_stdout(|output| {
        let _ = output.clear();
    });
    uefi::println!("r-boot console. Type `help` for a list of commands. Tab completes.");
    loop {
        let prompt = alloc::format!("{}> ", display_path(&cwd));
        uefi::print!("{prompt}");
        let Some(line) = read_line(fs, &cwd, &prompt) else {
            break;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (command, argument) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let argument = argument.trim();
        match command {
            "ls" => list(fs, &cwd, argument),
            "lsblk" => list_block_devices(),
            "cd" => change_dir(fs, &mut cwd, argument),
            "pwd" => uefi::println!("{}", display_path(&cwd)),
            "clear" => {
                system::with_stdout(|output| {
                    let _ = output.clear();
                });
            }
            "help" => print_help(),
            "exit" | "quit" => break,
            _ => uefi::println!("unknown command: {command}"),
        }
    }
}

/// Prints a summary of the available commands.
fn print_help() {
    uefi::println!("Available commands:");
    uefi::println!("  ls [path]   list directory contents");
    uefi::println!("  lsblk       list attached block devices (disks, partitions, USB sticks)");
    uefi::println!("  cd <path>   change the current directory");
    uefi::println!("  pwd         print the current directory");
    uefi::println!("  clear       clear the screen");
    uefi::println!("  help        show this message");
    uefi::println!("  exit, quit  leave the console");
}

/// Reads one line of input, echoing keystrokes and handling backspace and
/// Tab completion. Returns `None` if the user pressed Escape.
fn read_line(fs: &mut FileSystem, cwd: &Path, prompt: &str) -> Option<String> {
    let mut line = String::new();
    loop {
        let key_event = system::with_stdin(|input| input.wait_for_key_event()).ok()?;
        let mut events = unsafe { [key_event.unsafe_clone()] };
        boot::wait_for_event(&mut events).ok()?;
        let key = system::with_stdin(|input| input.read_key()).ok()?;
        match key {
            Some(Key::Printable(character)) if character == '\r' => {
                uefi::println!();
                return Some(line);
            }
            Some(Key::Printable(character)) if character == '\u{8}' => {
                if line.pop().is_some() {
                    uefi::print!("\u{8} \u{8}");
                }
            }
            Some(Key::Printable(character)) if character == '\t' => {
                complete(fs, cwd, &mut line, prompt);
            }
            Some(Key::Printable(character)) => {
                let character = char::from(character);
                if !character.is_control() {
                    line.push(character);
                    uefi::print!("{character}");
                }
            }
            Some(Key::Special(ScanCode::ESCAPE)) => return None,
            _ => continue,
        }
    }
}

/// Completes the command name or, once a command has been typed, the path
/// argument following it.
fn complete(fs: &mut FileSystem, cwd: &Path, line: &mut String, prompt: &str) {
    match line.find(char::is_whitespace) {
        None => {
            let candidates = COMMANDS
                .iter()
                .map(|name| (name.to_string(), Some(' ')))
                .collect();
            apply_completion(line, 0, candidates, prompt);
        }
        Some(space) => {
            let command = &line[..space];
            if command == "ls" || command == "cd" {
                complete_path(fs, cwd, line, space + 1, prompt);
            }
        }
    }
}

/// Completes the path argument starting at `arg_start`, listing whichever
/// directory the argument's own path prefix (if any) resolves to.
fn complete_path(
    fs: &mut FileSystem,
    cwd: &Path,
    line: &mut String,
    arg_start: usize,
    prompt: &str,
) {
    let argument = &line[arg_start..];
    let (dir_part, prefix_start) = match argument.rfind(['\\', '/']) {
        Some(index) => (&argument[..=index], index + 1),
        None => ("", 0),
    };
    let directory = if dir_part.is_empty() {
        Some(cwd.to_path_buf())
    } else {
        resolve(cwd, dir_part)
    };
    let Some(directory) = directory else {
        return;
    };
    let Ok(entries) = fs.read_dir(&directory) else {
        return;
    };
    let candidates = entries
        .flatten()
        .map(|info| {
            let trailing = if info.is_directory() { '/' } else { ' ' };
            (info.file_name().to_string(), Some(trailing))
        })
        .collect();
    apply_completion(line, arg_start + prefix_start, candidates, prompt);
}

/// Filters `candidates` against the word already typed at `word_start` and
/// either fills in an unambiguous match, extends the word to the longest
/// common prefix of the remaining matches, or lists them all.
fn apply_completion(
    line: &mut String,
    word_start: usize,
    mut candidates: Vec<(String, Option<char>)>,
    prompt: &str,
) {
    let typed = line[word_start..].to_string();
    candidates.retain(|(name, _)| name.starts_with(typed.as_str()));
    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [] => {}
        [(name, trailing)] => {
            let completion = &name[typed.len()..];
            line.push_str(completion);
            uefi::print!("{completion}");
            if let Some(trailing) = trailing {
                line.push(*trailing);
                uefi::print!("{trailing}");
            }
        }
        _ => {
            let names: Vec<&str> = candidates.iter().map(|(name, _)| name.as_str()).collect();
            let common = longest_common_prefix(&names);
            if common.len() > typed.len() {
                let completion = &common[typed.len()..];
                line.push_str(completion);
                uefi::print!("{completion}");
                return;
            }
            uefi::println!();
            for name in &names {
                uefi::print!("{name}  ");
            }
            uefi::println!();
            uefi::print!("{prompt}{line}");
        }
    }
}

/// Renders a path for display with `/` separators instead of the `\`
/// separators the underlying UEFI filesystem paths actually use.
fn display_path(path: &Path) -> String {
    path.to_string().replace('\\', "/")
}

/// Longest common leading substring shared by every string in `strings`.
fn longest_common_prefix(strings: &[&str]) -> String {
    let mut chars: Vec<char> = match strings.first() {
        Some(first) => first.chars().collect(),
        None => return String::new(),
    };
    for other in &strings[1..] {
        let other: Vec<char> = other.chars().collect();
        let common_len = chars
            .iter()
            .zip(other.iter())
            .take_while(|(a, b)| a == b)
            .count();
        chars.truncate(common_len);
    }
    chars.into_iter().collect()
}

fn list(fs: &mut FileSystem, cwd: &Path, argument: &str) {
    let target = if argument.is_empty() {
        cwd.to_path_buf()
    } else {
        let Some(target) = resolve(cwd, argument) else {
            uefi::println!("ls: invalid path");
            return;
        };
        target
    };
    match fs.read_dir(&target) {
        Ok(entries) => {
            let mut names: Vec<(String, bool)> = entries
                .flatten()
                .map(|info| (info.file_name().to_string(), info.is_directory()))
                .collect();
            names.sort();
            if names.is_empty() {
                uefi::println!("(empty)");
            }
            for (name, is_directory) in names {
                if is_directory {
                    uefi::println!("{name}/");
                } else {
                    uefi::println!("{name}");
                }
            }
        }
        Err(_) => uefi::println!(
            "ls: cannot access {}: no such directory",
            display_path(&target)
        ),
    }
}

/// Lists attached block devices, both whole disks and individual partitions
/// (including the boot partition), noting which ones are removable media
/// such as USB sticks.
fn list_block_devices() {
    let Ok(handles) = boot::find_handles::<BlockIO>() else {
        uefi::println!("no block devices found");
        return;
    };
    let mut printed = false;
    for handle in handles {
        let params = OpenProtocolParams {
            handle,
            agent: boot::image_handle(),
            controller: None,
        };
        // SAFETY: opened with `GetProtocol`, which (unlike
        // `open_protocol_exclusive`) does not disconnect the firmware's own
        // disk and filesystem drivers from the handle. The interfaces are
        // only read from within this loop iteration, never mutated or held
        // past it.
        let Ok(block_io) =
            (unsafe { boot::open_protocol::<BlockIO>(params, OpenProtocolAttributes::GetProtocol) })
        else {
            continue;
        };
        let media = block_io.media();
        let name = (unsafe {
            boot::open_protocol::<DevicePath>(params, OpenProtocolAttributes::GetProtocol)
        })
        .ok()
        .and_then(|path| {
            path.to_string16(DisplayOnly(true), AllowShortcuts(true))
                .ok()
        })
        .map(|name| name.to_string())
        .unwrap_or_else(|| "unknown device".to_string());
        let scope = if media.is_logical_partition() {
            "partition"
        } else {
            "disk"
        };
        let kind = if media.is_removable_media() {
            "removable"
        } else {
            "fixed"
        };
        let size = format_size(
            (media.last_block() + 1).saturating_mul(u64::from(media.block_size())),
        );
        uefi::println!("{name}  {scope}, {kind}, {size}");
        printed = true;
    }
    if !printed {
        uefi::println!("no block devices found");
    }
}

/// Formats a byte count as a human-readable size using binary (1024-based)
/// units, e.g. `15.3 GiB`.
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        alloc::format!("{bytes} {}", UNITS[unit])
    } else {
        alloc::format!("{size:.1} {}", UNITS[unit])
    }
}

fn change_dir(fs: &mut FileSystem, cwd: &mut PathBuf, argument: &str) {
    if argument.is_empty() {
        uefi::println!("usage: cd <path>");
        return;
    }
    let Some(target) = resolve(cwd, argument) else {
        uefi::println!("cd: invalid path");
        return;
    };
    match fs.metadata(&target) {
        Ok(info) if info.is_directory() => *cwd = target,
        Ok(_) => uefi::println!("cd: {}: not a directory", display_path(&target)),
        Err(_) => uefi::println!("cd: {}: no such directory", display_path(&target)),
    }
}

/// Resolves a user-typed path (absolute or relative, `/`- or `\`-separated,
/// with `.`/`..` components) against the current working directory.
///
/// Builds the result as a plain string rather than via [`PathBuf::push`],
/// since that method miscomputes whether a separator is already present
/// (it inspects the string's null terminator instead of its last character)
/// and ends up doubling the separator whenever `cwd` is the root path.
fn resolve(cwd: &Path, argument: &str) -> Option<PathBuf> {
    let cwd_string = cwd.to_string();
    let mut components: Vec<&str> = if argument.starts_with(['\\', '/']) {
        Vec::new()
    } else {
        cwd_string
            .split(['\\', '/'])
            .filter(|component| !component.is_empty())
            .collect()
    };
    for component in argument.split(['\\', '/']) {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            name => components.push(name),
        }
    }
    let mut result = SEPARATOR_STR.to_string();
    result.push_str(&components.join("\\"));
    let result = CString16::try_from(result.as_str()).ok()?;
    Some(PathBuf::from(result))
}
