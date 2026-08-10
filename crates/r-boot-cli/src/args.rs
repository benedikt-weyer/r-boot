use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "r-boot-cli",
    about = "Inspect and edit r-boot's boot menu configuration"
)]
pub struct Cli {
    /// EFI system partition mount point (matches r-boot-conf-builder's `-d`).
    #[arg(
        short = 'd',
        long = "esp",
        value_name = "esp-mount-point",
        default_value = "/boot",
        global = true
    )]
    pub esp: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Print the current default entry, timeout, and menu entries.
    Show,
    /// Change the default boot entry.
    SetDefault {
        /// Id of the entry to make the default.
        id: String,
    },
    /// Change the menu timeout, in seconds.
    SetTimeout {
        /// Seconds to wait on the menu before booting the default entry.
        seconds: u32,
    },
    /// Change spinner output to off, text, or graphical.
    SetSpinner {
        /// Spinner mode.
        mode: SpinnerMode,
    },
    /// Show or hide the firmware logo while using the graphical spinner.
    SetLogo {
        /// Whether the firmware logo should be shown.
        visible: OnOff,
    },
    /// Change fastboot: skip menu interaction, the spinner, and splash art
    /// to boot the default entry as quickly as possible.
    SetFastboot {
        /// Fastboot mode.
        mode: FastbootMode,
    },
    /// Remove a boot entry.
    Remove {
        /// Id of the entry to remove.
        id: String,
        /// Also delete the entry's kernel/initramfs/EFI files from the ESP.
        #[arg(long)]
        purge: bool,
    },
    /// Scan the ESP and list kernel/initramfs files found.
    ListFiles,
    /// Print a shell completion script to stdout.
    #[command(hide = true)]
    Completions {
        /// Shell to generate a completion script for.
        shell: clap_complete::Shell,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum SpinnerMode {
    Off,
    Text,
    Graphical,
}

impl SpinnerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SpinnerMode::Off => "off",
            SpinnerMode::Text => "text",
            SpinnerMode::Graphical => "graphical",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum FastbootMode {
    Off,
    /// Fast boot exactly once, then automatically revert to `off`.
    NextBoot,
    On,
}

impl FastbootMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FastbootMode::Off => "off",
            FastbootMode::NextBoot => "next-boot",
            FastbootMode::On => "on",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

impl OnOff {
    pub fn as_bool(self) -> bool {
        matches!(self, OnOff::On)
    }
}
