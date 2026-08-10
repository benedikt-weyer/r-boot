//! A minimal interactive shell for browsing the boot filesystem, reachable
//! from the boot menu via the `t` key. Supports `ls` and `cd`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use uefi::boot;
use uefi::fs::{FileSystem, Path, PathBuf, SEPARATOR_STR};
use uefi::proto::console::text::{Key, ScanCode};
use uefi::{CString16, system};

/// Runs the shell until the user types `exit` or presses Escape.
pub fn run(fs: &mut FileSystem) {
    let mut cwd = PathBuf::from(SEPARATOR_STR);
    system::with_stdout(|output| {
        let _ = output.clear();
    });
    uefi::println!("r-boot console. Commands: ls [path], cd <path>, exit.");
    loop {
        uefi::print!("{cwd}> ");
        let Some(line) = read_line() else {
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
            "cd" => change_dir(fs, &mut cwd, argument),
            "exit" | "quit" => break,
            _ => uefi::println!("unknown command: {command}"),
        }
    }
}

/// Reads one line of input, echoing keystrokes and handling backspace.
/// Returns `None` if the user pressed Escape.
fn read_line() -> Option<String> {
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
                    uefi::println!("{name}\\");
                } else {
                    uefi::println!("{name}");
                }
            }
        }
        Err(_) => uefi::println!("ls: cannot access {target}: no such directory"),
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
        Ok(_) => uefi::println!("cd: {target}: not a directory"),
        Err(_) => uefi::println!("cd: {target}: no such directory"),
    }
}

/// Resolves a user-typed path (absolute or relative, `/`- or `\`-separated,
/// with `.`/`..` components) against the current working directory.
fn resolve(cwd: &Path, argument: &str) -> Option<PathBuf> {
    let mut result = if argument.starts_with(['\\', '/']) {
        PathBuf::from(SEPARATOR_STR)
    } else {
        cwd.to_path_buf()
    };
    for component in argument.split(['\\', '/']) {
        match component {
            "" | "." => {}
            ".." => result = result.parent().unwrap_or_else(|| PathBuf::from(SEPARATOR_STR)),
            name => {
                let name = CString16::try_from(name).ok()?;
                result.push(Path::new(&name));
            }
        }
    }
    Some(result)
}
