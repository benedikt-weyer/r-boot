//! Boot-entry discovery, configuration parsing, and the UEFI text menu.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use core::time::Duration;

use uefi::boot::{self, EventType, TimerTrigger, Tpl};
use uefi::fs::{FileSystem, Path, PathBuf};
use uefi::proto::console::text::{Key, ScanCode};
use uefi::{CString16, cstr16, system};

use crate::spinner::Mode as SpinnerMode;
use crate::splash::Image as SplashImage;

const ENTRIES_PER_PAGE: usize = 10;
/// Row where the first entry is printed by `draw`; used to target dirty
/// updates without redrawing the whole screen.
const ENTRY_LIST_ROW: usize = 3;
/// Row of the status line ("Select an entry..."), updated in place each
/// countdown tick instead of redrawing the whole screen.
const STATUS_ROW: usize = 1;

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
    pub image: Option<SplashImage>,
}

#[derive(Debug)]
pub struct Menu {
    pub entries: Vec<Entry>,
    default: Option<String>,
    timeout: Option<u64>,
    spinner_mode: SpinnerMode,
    logo_visible: bool,
}

impl Menu {
    pub fn load(fs: &mut FileSystem) -> Self {
        let mut menu = Self {
            entries: Vec::new(),
            default: None,
            timeout: None,
            spinner_mode: SpinnerMode::Graphical,
            logo_visible: true,
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

    pub fn spinner_mode(&self) -> SpinnerMode {
        self.spinner_mode
    }

    pub fn logo_visible(&self) -> bool {
        self.logo_visible
    }

    /// Removes the menu before boot progress is displayed.
    pub fn clear(&self) {
        system::with_stdout(|output| {
            let _ = output.clear();
        });
        crate::spinner::clear_screen();
    }

    pub fn select(&mut self, fs: &mut FileSystem) -> Result<usize, &'static str> {
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
        self.draw(selected, remaining);
        loop {
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
                        self.update_status(remaining);
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
                self.update_status(remaining);
            }
            match key {
                Some(Key::Special(ScanCode::UP)) => {
                    let previous = selected;
                    selected = selected.checked_sub(1).unwrap_or(self.entries.len() - 1);
                    self.move_selection(previous, selected, remaining);
                }
                Some(Key::Special(ScanCode::DOWN)) => {
                    let previous = selected;
                    selected = (selected + 1) % self.entries.len();
                    self.move_selection(previous, selected, remaining);
                }
                Some(Key::Special(ScanCode::LEFT)) => {
                    let page = page_for(selected).saturating_sub(1);
                    selected = entry_on_page(selected, self.entries.len(), page);
                    self.draw(selected, remaining);
                }
                Some(Key::Special(ScanCode::RIGHT)) => {
                    let last_page = page_count(self.entries.len()) - 1;
                    let page = core::cmp::min(page_for(selected) + 1, last_page);
                    selected = entry_on_page(selected, self.entries.len(), page);
                    self.draw(selected, remaining);
                }
                Some(Key::Printable(character)) if character == '\r' => {
                    let _ = boot::close_event(timer);
                    return Ok(selected);
                }
                Some(Key::Printable(character)) if character == 'c' || character == 'C' => {
                    if self.edit_config(fs, selected)? {
                        timeout = self.timeout.unwrap_or(5);
                        if timeout > 0 {
                            boot::set_timer(&timer, TimerTrigger::Periodic(Duration::from_secs(1)))
                                .map_err(|_| "cannot restart boot-menu timeout")?;
                            remaining = Some(timeout);
                        } else {
                            remaining = None;
                        }
                    }
                    self.draw(selected, remaining);
                }
                _ => continue,
            }
        }
    }

    /// Moves the selection marker between two rows on the same page without
    /// redrawing the rest of the screen. Falls back to a full redraw if the
    /// move crosses a page boundary.
    fn move_selection(&self, previous: usize, selected: usize, remaining: Option<u64>) {
        if page_for(previous) != page_for(selected) {
            self.draw(selected, remaining);
            return;
        }
        let first = page_for(selected) * ENTRIES_PER_PAGE;
        self.set_marker(previous - first, ' ');
        self.set_marker(selected - first, '>');
    }

    /// Rewrites a single entry row's marker column in place.
    fn set_marker(&self, row_in_page: usize, marker: char) {
        system::with_stdout(|output| {
            let _ = output.set_cursor_position(0, ENTRY_LIST_ROW + row_in_page);
            let _ = write!(output, "{marker}");
        });
    }

    /// Rewrites the countdown status line in place.
    fn update_status(&self, remaining: Option<u64>) {
        system::with_stdout(|output| {
            let _ = output.set_cursor_position(0, STATUS_ROW);
            let _ = match remaining {
                Some(seconds) => write!(output, "Select an entry (boots in {seconds}s): "),
                None => write!(output, "Select an entry:                          "),
            };
        });
    }

    /// Lets the user persist boot-menu preferences to `boot/r-boot.toml`.
    fn edit_config(
        &mut self,
        fs: &mut FileSystem,
        entry_index: usize,
    ) -> Result<bool, &'static str> {
        let mut timeout = self.timeout.unwrap_or(5);
        let mut spinner_mode = self.spinner_mode;
        let mut logo_visible = self.logo_visible;
        let mut image = self.entries[entry_index].image;
        let mut selected = 0;
        loop {
            self.draw_config(timeout, spinner_mode, logo_visible, image, selected);
            let key_event = system::with_stdin(|input| input.wait_for_key_event())
                .map_err(|_| "keyboard input is unavailable")?;
            let mut events = unsafe { [key_event.unsafe_clone()] };
            boot::wait_for_event(&mut events).map_err(|_| "cannot wait for keyboard input")?;
            let key = system::with_stdin(|input| input.read_key())
                .map_err(|_| "cannot read keyboard input")?;
            match key {
                Some(Key::Special(ScanCode::UP)) => selected = selected.saturating_sub(1),
                Some(Key::Special(ScanCode::DOWN)) => selected = (selected + 1) % 4,
                Some(Key::Special(ScanCode::LEFT)) => match selected {
                    0 => timeout = timeout.saturating_sub(1),
                    1 => spinner_mode = spinner_mode.previous(),
                    2 => logo_visible = !logo_visible,
                    3 => image = image.and_then(SplashImage::previous),
                    _ => unreachable!(),
                },
                Some(Key::Special(ScanCode::RIGHT)) => match selected {
                    0 => timeout = timeout.saturating_add(1),
                    1 => spinner_mode = spinner_mode.next(),
                    2 => logo_visible = !logo_visible,
                    3 => {
                        image = match image {
                            None => Some(SplashImage::Nixos),
                            Some(image) => image.next(),
                        }
                    }
                    _ => unreachable!(),
                },
                Some(Key::Printable(character)) if character == '\r' => {
                    persist_settings(
                        fs,
                        timeout,
                        spinner_mode,
                        logo_visible,
                        &self.entries[entry_index].id,
                        image,
                    )?;
                    self.timeout = Some(timeout);
                    self.spinner_mode = spinner_mode;
                    self.logo_visible = logo_visible;
                    self.entries[entry_index].image = image;
                    return Ok(true);
                }
                Some(Key::Special(ScanCode::ESCAPE)) => return Ok(false),
                _ => continue,
            }
        }
    }

    fn draw_config(
        &self,
        timeout: u64,
        spinner_mode: SpinnerMode,
        logo_visible: bool,
        image: Option<SplashImage>,
        selected: usize,
    ) {
        self.clear();
        uefi::println!("r-boot configuration");
        uefi::println!();
        let timeout_marker = if selected == 0 { '>' } else { ' ' };
        let spinner_marker = if selected == 1 { '>' } else { ' ' };
        let logo_marker = if selected == 2 { '>' } else { ' ' };
        let image_marker = if selected == 3 { '>' } else { ' ' };
        uefi::println!("{timeout_marker} Timeout: {timeout}s");
        uefi::println!("{spinner_marker} Spinner: {}", spinner_mode.as_str());
        uefi::println!(
            "{logo_marker} Firmware logo: {}",
            if logo_visible { "on" } else { "off" }
        );
        uefi::println!(
            "{image_marker} Selected entry image: {}",
            image.map(SplashImage::as_str).unwrap_or("none")
        );
        uefi::println!();
        uefi::println!("Use Up/Down to select, Left/Right to change.");
        uefi::println!("Enter saves to boot/r-boot.toml. Esc cancels.");
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
        self.clear();
        uefi::println!("r-boot");
        match remaining {
            Some(seconds) => uefi::println!("Select an entry (boots in {seconds}s):"),
            None => uefi::println!("Select an entry:"),
        }
        uefi::println!();
        let page = page_for(selected);
        let pages = page_count(self.entries.len());
        let first = page * ENTRIES_PER_PAGE;
        let last = core::cmp::min(first + ENTRIES_PER_PAGE, self.entries.len());
        for (index, entry) in self.entries[first..last].iter().enumerate() {
            let index = first + index;
            let marker = if index == selected { '>' } else { ' ' };
            uefi::println!("{marker} {}", entry.title);
        }
        uefi::println!();
        if pages > 1 {
            uefi::println!("Page {} of {pages}", page + 1);
        }
        uefi::println!("Use Up/Down and Enter. Left/Right changes pages. Press c to configure.");
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
                    "spinner" => {
                        if let Some(mode) = SpinnerMode::parse(value) {
                            self.spinner_mode = mode;
                        }
                    }
                    "logo" => self.logo_visible = value.parse().unwrap_or(self.logo_visible),
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
            image: raw.image.as_deref().and_then(|value| {
                let image = SplashImage::parse(value);
                if image.is_none() {
                    log::warn!("r-boot: ignoring unknown splash image");
                }
                image
            }),
        });
    }
}

fn page_count(entry_count: usize) -> usize {
    entry_count.div_ceil(ENTRIES_PER_PAGE)
}

fn page_for(entry_index: usize) -> usize {
    entry_index / ENTRIES_PER_PAGE
}

/// Moves to the requested page while preserving the selected row when possible.
fn entry_on_page(selected: usize, entry_count: usize, page: usize) -> usize {
    let first = page * ENTRIES_PER_PAGE;
    let last = core::cmp::min(first + ENTRIES_PER_PAGE, entry_count);
    first + core::cmp::min(selected % ENTRIES_PER_PAGE, last - first - 1)
}

/// Reads the spinner preference early, before full boot-entry discovery starts.
/// Invalid or missing settings retain the graphical default.
pub fn configured_spinner_mode(fs: &mut FileSystem) -> SpinnerMode {
    let Ok(contents) = fs.read_to_string(Path::new(cstr16!("\\boot\\r-boot.toml"))) else {
        return SpinnerMode::default();
    };
    for line in contents.lines() {
        let line = strip_comment(line).trim();
        if line == "[[entries]]" {
            break;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "spinner" {
                if let Some(mode) = SpinnerMode::parse(unquote(value.trim())) {
                    return mode;
                }
            }
        }
    }
    SpinnerMode::default()
}

/// Reads the firmware-logo preference early, before boot-entry discovery.
pub fn configured_logo_visible(fs: &mut FileSystem) -> bool {
    let Ok(contents) = fs.read_to_string(Path::new(cstr16!("\\boot\\r-boot.toml"))) else {
        return true;
    };
    for line in contents.lines() {
        let line = strip_comment(line).trim();
        if line == "[[entries]]" {
            break;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "logo" {
                return value.trim().parse().unwrap_or(true);
            }
        }
    }
    true
}

#[derive(Default)]
struct RawEntry {
    id: Option<String>,
    title: Option<String>,
    kind: Option<String>,
    kernel: Option<String>,
    initrds: Vec<String>,
    options: Option<String>,
    image: Option<String>,
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
            "image" => self.image = Some(value.to_string()),
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

/// Updates r-boot's settings and the selected entry's splash image.
fn persist_settings(
    fs: &mut FileSystem,
    timeout: u64,
    spinner_mode: SpinnerMode,
    logo_visible: bool,
    entry_id: &str,
    image: Option<SplashImage>,
) -> Result<(), &'static str> {
    let path = Path::new(cstr16!("\\boot\\r-boot.toml"));
    let contents = fs.read_to_string(path).unwrap_or_default();
    let mut updated = String::from("timeout = ");
    updated.push_str(&timeout.to_string());
    updated.push_str("\nspinner = \"");
    updated.push_str(spinner_mode.as_str());
    updated.push_str("\"\nlogo = ");
    updated.push_str(if logo_visible { "true\n" } else { "false\n" });
    let mut entries_started = false;
    let mut entry_lines = Vec::new();

    for line in contents.lines() {
        let trimmed = strip_comment(line).trim();
        if trimmed == "[[entries]]" {
            append_entry(&mut updated, &entry_lines, entry_id, image);
            entry_lines.clear();
            entries_started = true;
        }
        if entries_started {
            entry_lines.push(line);
            continue;
        }
        let key = trimmed.split_once('=').map(|(key, _)| key.trim());
        if matches!(key, Some("timeout" | "spinner" | "logo")) {
            continue;
        }
        updated.push_str(line);
        updated.push('\n');
    }
    append_entry(&mut updated, &entry_lines, entry_id, image);

    fs.write(path, updated.as_bytes())
        .map_err(|_| "cannot save boot configuration")
}

fn append_entry(updated: &mut String, lines: &[&str], entry_id: &str, image: Option<SplashImage>) {
    let is_selected = lines.iter().any(|line| {
        let trimmed = strip_comment(line).trim();
        trimmed
            .split_once('=')
            .is_some_and(|(key, value)| key.trim() == "id" && unquote(value.trim()) == entry_id)
    });
    let has_image = lines.iter().any(|line| {
        strip_comment(line)
            .trim()
            .split_once('=')
            .is_some_and(|(key, _)| key.trim() == "image")
    });

    for line in lines {
        let key = strip_comment(line)
            .trim()
            .split_once('=')
            .map(|(key, _)| key.trim());
        if is_selected && key == Some("image") {
            if let Some(image) = image {
                updated.push_str("image = \"");
                updated.push_str(image.as_str());
                updated.push_str("\"\n");
            }
            continue;
        }
        updated.push_str(line);
        updated.push('\n');
        if is_selected && !has_image && key == Some("id") {
            if let Some(image) = image {
                updated.push_str("image = \"");
                updated.push_str(image.as_str());
                updated.push_str("\"\n");
            }
        }
    }
}

pub fn path(value: &str) -> Result<PathBuf, &'static str> {
    let value = CString16::try_from(value).map_err(|_| "boot path is not valid UTF-16")?;
    Ok(PathBuf::from(value))
}
