//! Boot-entry discovery, configuration parsing, and the UEFI text menu.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::time::Duration;

use uefi::boot::{self, EventType, TimerTrigger, Tpl};
use uefi::fs::{FileSystem, Path, PathBuf};
use uefi::proto::console::text::{Key, ScanCode};
use uefi::{CString16, cstr16, system};

#[derive(Debug)]
pub enum Kind {
    Linux,
    Limine,
    Efi,
}

#[derive(Debug)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub kind: Kind,
    pub kernel: String,
    pub initrds: Vec<String>,
    pub options: Option<String>,
}

#[derive(Debug)]
pub struct Menu {
    pub entries: Vec<Entry>,
    default: Option<String>,
    timeout: Option<u64>,
}

impl Menu {
    pub fn load(fs: &mut FileSystem) -> Self {
        let mut menu = Self {
            entries: Vec::new(),
            default: None,
            timeout: None,
        };

        if let Ok(contents) = fs.read_to_string(Path::new(cstr16!("\\boot\\r-boot.toml"))) {
            menu.parse_toml(&contents);
        }
        if let Ok(contents) = fs.read_to_string(Path::new(cstr16!("\\loader\\loader.conf"))) {
            menu.parse_loader_config(&contents);
        }
        menu.load_systemd_entries(fs);
        menu.load_grub_config(fs, Path::new(cstr16!("\\boot\\grub\\grub.cfg")));
        menu.load_grub_config(fs, Path::new(cstr16!("\\grub\\grub.cfg")));
        menu
    }

    pub fn select(&self) -> Result<usize, &'static str> {
        if self.entries.is_empty() {
            return Err("no boot entries found");
        }
        let mut selected = self.default_index();
        let mut timeout = self.timeout.unwrap_or(5);
        if timeout == 0 {
            return Ok(selected);
        }

        let timer = unsafe {
            boot::create_event(EventType::TIMER, Tpl::APPLICATION, None, None)
                .map_err(|_| "cannot create boot-menu timer")?
        };
        boot::set_timer(&timer, TimerTrigger::Periodic(Duration::from_secs(1)))
            .map_err(|_| "cannot set boot-menu timeout")?;

        let mut remaining = Some(timeout);
        loop {
            self.draw(selected, remaining);
            let key_event = system::with_stdin(|input| input.wait_for_key_event())
                .map_err(|_| "keyboard input is unavailable")?;
            match remaining {
                Some(left) => {
                    // `wait_for_event` takes owned handles although it does not
                    // close them. The aliases remain valid until `timer` is
                    // closed below.
                    let mut events = unsafe { [key_event.unsafe_clone(), timer.unsafe_clone()] };
                    let index = boot::wait_for_event(&mut events)
                        .map_err(|_| "cannot wait for keyboard input")?;
                    if index == 1 {
                        if left == 1 {
                            let _ = boot::close_event(timer);
                            return Ok(selected);
                        }
                        remaining = Some(left - 1);
                        continue;
                    }
                }
                None => {
                    let mut events = unsafe { [key_event.unsafe_clone()] };
                    boot::wait_for_event(&mut events)
                        .map_err(|_| "cannot wait for keyboard input")?;
                }
            }
            let key = system::with_stdin(|input| input.read_key())
                .map_err(|_| "cannot read keyboard input")?;
            if remaining.is_some() {
                let _ = boot::set_timer(&timer, TimerTrigger::Cancel);
                remaining = None;
            }
            match key {
                Some(Key::Special(ScanCode::UP)) => {
                    selected = selected.checked_sub(1).unwrap_or(self.entries.len() - 1)
                }
                Some(Key::Special(ScanCode::DOWN)) => {
                    selected = (selected + 1) % self.entries.len()
                }
                Some(Key::Printable(character)) if character == '\r' => {
                    let _ = boot::close_event(timer);
                    return Ok(selected);
                }
                Some(Key::Printable(character)) if character == 'c' || character == 'C' => {
                    if let Some(new_timeout) = self.edit_config(timeout)? {
                        timeout = new_timeout;
                        if timeout > 0 {
                            boot::set_timer(&timer, TimerTrigger::Periodic(Duration::from_secs(1)))
                                .map_err(|_| "cannot restart boot-menu timeout")?;
                            remaining = Some(timeout);
                        }
                    }
                }
                _ => continue,
            }
        }
    }

    /// Lets the user adjust the boot-menu timeout for the current boot.
    /// Returns the new timeout on save, or `None` if the edit was cancelled.
    fn edit_config(&self, current_timeout: u64) -> Result<Option<u64>, &'static str> {
        let mut value = current_timeout;
        loop {
            self.draw_config(value);
            let key_event = system::with_stdin(|input| input.wait_for_key_event())
                .map_err(|_| "keyboard input is unavailable")?;
            let mut events = unsafe { [key_event.unsafe_clone()] };
            boot::wait_for_event(&mut events).map_err(|_| "cannot wait for keyboard input")?;
            let key = system::with_stdin(|input| input.read_key())
                .map_err(|_| "cannot read keyboard input")?;
            match key {
                Some(Key::Special(ScanCode::UP)) => value = value.saturating_add(1),
                Some(Key::Special(ScanCode::DOWN)) => value = value.saturating_sub(1),
                Some(Key::Printable(character)) if character == '\r' => return Ok(Some(value)),
                Some(Key::Special(ScanCode::ESCAPE)) => return Ok(None),
                _ => continue,
            }
        }
    }

    fn draw_config(&self, timeout: u64) {
        system::with_stdout(|output| {
            let _ = output.clear();
        });
        uefi::println!("r-boot configuration");
        uefi::println!();
        uefi::println!("Timeout: {timeout}s");
        uefi::println!();
        uefi::println!("Use Up/Down to adjust, Enter to save, Esc to cancel.");
    }

    fn default_index(&self) -> usize {
        self.default
            .as_deref()
            .and_then(|default| {
                let prefix = default.strip_suffix('*');
                self.entries.iter().position(|entry| {
                    entry.title == default
                        || entry.id == default
                        || prefix.is_some_and(|prefix| entry.id.starts_with(prefix))
                })
            })
            .unwrap_or(0)
    }

    fn draw(&self, selected: usize, remaining: Option<u64>) {
        system::with_stdout(|output| {
            let _ = output.clear();
        });
        uefi::println!("r-boot");
        match remaining {
            Some(seconds) => uefi::println!("Select an entry (boots in {seconds}s):"),
            None => uefi::println!("Select an entry:"),
        }
        uefi::println!();
        for (index, entry) in self.entries.iter().enumerate() {
            let marker = if index == selected { '>' } else { ' ' };
            uefi::println!("{marker} {}", entry.title);
        }
        uefi::println!();
        uefi::println!("Use Up/Down and Enter. Press c to configure.");
    }

    fn parse_toml(&mut self, contents: &str) {
        let mut entry = None;
        for line in contents.lines() {
            let line = strip_comment(line).trim();
            if line == "[[entries]]" {
                self.push_entry(entry.take());
                entry = Some(RawEntry::default());
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = unquote(value.trim());
            if let Some(entry) = entry.as_mut() {
                entry.set(key, value);
            } else {
                match key {
                    "default" => self.default = Some(value.to_string()),
                    "timeout" => self.timeout = value.parse().ok(),
                    _ => {}
                }
            }
        }
        self.push_entry(entry);
    }

    fn parse_loader_config(&mut self, contents: &str) {
        for line in contents.lines() {
            let line = strip_comment(line).trim();
            let Some((key, value)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            match key {
                "default" if self.default.is_none() => {
                    self.default = Some(value.trim().to_string())
                }
                "timeout" if self.timeout.is_none() => self.timeout = value.trim().parse().ok(),
                _ => {}
            }
        }
    }

    fn load_systemd_entries(&mut self, fs: &mut FileSystem) {
        let entries_path = Path::new(cstr16!("\\loader\\entries"));
        let Ok(entries) = fs.read_dir(entries_path) else {
            return;
        };
        for info in entries.flatten() {
            if info.is_directory() {
                continue;
            }
            let name = info.file_name().to_string();
            if !name.ends_with(".conf") {
                continue;
            }
            let mut path = PathBuf::from(cstr16!("\\loader\\entries"));
            path.push(info.file_name());
            let Ok(contents) = fs.read_to_string(&path) else {
                continue;
            };
            let mut raw = RawEntry {
                title: Some(name.trim_end_matches(".conf").to_string()),
                id: Some(name.trim_end_matches(".conf").to_string()),
                ..RawEntry::default()
            };
            for line in contents.lines() {
                let line = strip_comment(line).trim();
                let Some((key, value)) = line.split_once(char::is_whitespace) else {
                    continue;
                };
                raw.set(key, value.trim());
            }
            self.push_entry(Some(raw));
        }
    }

    fn load_grub_config(&mut self, fs: &mut FileSystem, path: &Path) {
        let Ok(contents) = fs.read_to_string(path) else {
            return;
        };
        let mut entry = None;
        for line in contents.lines() {
            let line = strip_comment(line).trim();
            if let Some(menuentry) = line.strip_prefix("menuentry") {
                self.push_entry(entry.take());
                entry = parse_grub_menuentry(menuentry);
                continue;
            }
            if entry.is_none() {
                self.parse_grub_setting(line);
                continue;
            }
            let words = shell_words(line);
            let Some((command, arguments)) = words.split_first() else {
                continue;
            };
            if command == "}" {
                self.push_entry(entry.take());
                continue;
            }
            let entry = entry.as_mut().expect("entry was checked above");
            match command.as_str() {
                "linux" | "linuxefi" => {
                    if let Some((kernel, options)) = arguments.split_first() {
                        entry.kernel = Some(kernel.clone());
                        entry.options = Some(options.join(" "));
                    }
                }
                "initrd" | "initrdefi" => entry.initrds.extend_from_slice(arguments),
                _ => {}
            }
        }
        self.push_entry(entry);
    }

    fn parse_grub_setting(&mut self, line: &str) {
        let Some(setting) = line.strip_prefix("set ") else {
            return;
        };
        let Some((key, value)) = setting.split_once('=') else {
            return;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "default" if self.default.is_none() => self.default = Some(value.to_string()),
            "timeout" if self.timeout.is_none() => self.timeout = value.parse().ok(),
            _ => {}
        }
    }

    fn push_entry(&mut self, raw: Option<RawEntry>) {
        let Some(raw) = raw else {
            return;
        };
        let Some(kernel) = raw.kernel else {
            log::warn!("r-boot: ignoring menu entry without a kernel");
            return;
        };
        let kind = match raw.kind.as_deref() {
            Some("limine") => Kind::Limine,
            Some("linux") | None => Kind::Linux,
            Some("efi") => Kind::Efi,
            Some(_) => {
                log::warn!("r-boot: ignoring entry with unknown kind");
                return;
            }
        };
        self.entries.push(Entry {
            id: raw
                .id
                .unwrap_or_else(|| raw.title.clone().unwrap_or_else(|| kernel.clone())),
            title: raw.title.unwrap_or(kernel.clone()),
            kind,
            kernel,
            initrds: raw.initrds,
            options: raw.options,
        });
    }
}

#[derive(Default)]
struct RawEntry {
    id: Option<String>,
    title: Option<String>,
    kind: Option<String>,
    kernel: Option<String>,
    initrds: Vec<String>,
    options: Option<String>,
}

impl RawEntry {
    fn set(&mut self, key: &str, value: &str) {
        match key {
            "title" => self.title = Some(value.to_string()),
            "id" => self.id = Some(value.to_string()),
            "kind" => self.kind = Some(value.to_string()),
            "linux" | "kernel" => self.kernel = Some(value.to_string()),
            "efi" => {
                self.kind = Some("efi".to_string());
                self.kernel = Some(value.to_string());
            }
            "initrd" => self.initrds.push(value.to_string()),
            "options" => self.options = Some(value.to_string()),
            _ => {}
        }
    }
}

fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or("")
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn parse_grub_menuentry(value: &str) -> Option<RawEntry> {
    let words = shell_words(value);
    let (title, options) = words.split_first()?;
    let mut entry = RawEntry {
        title: Some(title.clone()),
        ..RawEntry::default()
    };
    for option in options {
        if let Some(id) = option.strip_prefix("--id=") {
            entry.id = Some(id.to_string());
        }
    }
    Some(entry)
}

/// Split the subset of GRUB command syntax needed for static menu entries.
/// Quotes group words and backslashes escape the following character.
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            word.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if Some(character) == quote {
            quote = None;
        } else if quote.is_none() && (character == '\'' || character == '"') {
            quote = Some(character);
        } else if quote.is_none() && character.is_whitespace() {
            if !word.is_empty() {
                words.push(core::mem::take(&mut word));
            }
        } else if character != '{' && character != ';' {
            word.push(character);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

pub fn path(value: &str) -> Result<PathBuf, &'static str> {
    let value = CString16::try_from(value).map_err(|_| "boot path is not valid UTF-16")?;
    Ok(PathBuf::from(value))
}
