//! Parses and renders `r-boot.toml`, the menu config r-boot reads at boot
//! time (see `r-boot/src/menu.rs`) and `r-boot-conf-builder` regenerates on
//! every `nixos-rebuild switch`. Edits made here survive until the next
//! rebuild, same as `bootctl set-default` for systemd-boot.

use std::fmt::Write as _;

#[derive(Debug, Default, Clone)]
pub struct Entry {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub linux: Option<String>,
    pub efi: Option<String>,
    pub initrd: Vec<String>,
    pub options: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Config {
    pub default: Option<String>,
    pub timeout: Option<u32>,
    pub spinner: Option<String>,
    pub entries: Vec<Entry>,
}

impl Config {
    pub fn parse(contents: &str) -> Self {
        let mut config = Config::default();
        let mut entry: Option<Entry> = None;

        for line in contents.lines() {
            let line = strip_comment(line).trim();
            if line.is_empty() {
                continue;
            }
            if line == "[[entries]]" {
                if let Some(entry) = entry.take() {
                    config.entries.push(entry);
                }
                entry = Some(Entry::default());
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = unquote(value.trim());
            match entry.as_mut() {
                Some(entry) => match key {
                    "id" => entry.id = value.to_string(),
                    "title" => entry.title = Some(value.to_string()),
                    "kind" => entry.kind = Some(value.to_string()),
                    "linux" => entry.linux = Some(value.to_string()),
                    "efi" => entry.efi = Some(value.to_string()),
                    "initrd" => entry.initrd.push(value.to_string()),
                    "options" => entry.options = Some(value.to_string()),
                    _ => {}
                },
                None => match key {
                    "default" => config.default = Some(value.to_string()),
                    "timeout" => config.timeout = value.parse().ok(),
                    "spinner" => config.spinner = Some(value.to_string()),
                    _ => {}
                },
            }
        }
        if let Some(entry) = entry {
            config.entries.push(entry);
        }
        config
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# Generated file, all changes will be lost on nixos-rebuild!\n");
        if let Some(default) = &self.default {
            let _ = writeln!(out, "default = \"{default}\"");
        }
        if let Some(timeout) = self.timeout {
            let _ = writeln!(out, "timeout = {timeout}");
        }
        if let Some(spinner) = &self.spinner {
            let _ = writeln!(out, "spinner = \"{spinner}\"");
        }
        for entry in &self.entries {
            out.push('\n');
            out.push_str("[[entries]]\n");
            let _ = writeln!(out, "id = \"{}\"", entry.id);
            if let Some(title) = &entry.title {
                let _ = writeln!(out, "title = \"{title}\"");
            }
            if let Some(kind) = &entry.kind {
                let _ = writeln!(out, "kind = \"{kind}\"");
            }
            if let Some(linux) = &entry.linux {
                let _ = writeln!(out, "linux = \"{linux}\"");
            }
            if let Some(efi) = &entry.efi {
                let _ = writeln!(out, "efi = \"{efi}\"");
            }
            for initrd in &entry.initrd {
                let _ = writeln!(out, "initrd = \"{initrd}\"");
            }
            if let Some(options) = &entry.options {
                let _ = writeln!(out, "options = \"{options}\"");
            }
        }
        out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_typical_config() {
        let contents = "\
# Generated file, all changes will be lost on nixos-rebuild!
default = \"nixos-default\"
timeout = 5
spinner = \"graphical\"

[[entries]]
id = \"nixos-default\"
title = \"NixOS (24.11, nixos-default)\"
kind = \"linux\"
linux = \"/boot/nixos/kernel\"
initrd = \"/boot/nixos/initrd\"
options = \"init=/nix/store/foo/init\"
";
        let config = Config::parse(contents);
        assert_eq!(config.default.as_deref(), Some("nixos-default"));
        assert_eq!(config.timeout, Some(5));
        assert_eq!(config.spinner.as_deref(), Some("graphical"));
        assert_eq!(config.entries.len(), 1);
        assert_eq!(config.entries[0].id, "nixos-default");
        assert_eq!(
            config.entries[0].linux.as_deref(),
            Some("/boot/nixos/kernel")
        );

        let reparsed = Config::parse(&config.render());
        assert_eq!(reparsed.default, config.default);
        assert_eq!(reparsed.timeout, config.timeout);
        assert_eq!(reparsed.spinner, config.spinner);
        assert_eq!(reparsed.entries.len(), config.entries.len());
    }
}
