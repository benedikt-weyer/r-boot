//! Built-in operating-system splash images for graphical boot progress.

use crate::bgrt::{self, Logo};

#[derive(Clone, Copy, Debug)]
pub enum Image {
    Nixos,
    Linux,
}

impl Image {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "nixos" => Some(Self::Nixos),
            "linux" => Some(Self::Linux),
            _ => None,
        }
    }

    pub fn render(self) -> Option<Logo> {
        match self {
            Self::Nixos => {
                bgrt::decode_bmp_bytes(include_bytes!("../../../assets/splash/nixos.bmp"))
            }
            Self::Linux => {
                bgrt::decode_bmp_bytes(include_bytes!("../../../assets/splash/linux.bmp"))
            }
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nixos => "nixos",
            Self::Linux => "linux",
        }
    }

    pub const fn next(self) -> Option<Self> {
        match self {
            Self::Nixos => Some(Self::Linux),
            Self::Linux => None,
        }
    }

    pub const fn previous(self) -> Option<Self> {
        match self {
            Self::Nixos => None,
            Self::Linux => Some(Self::Nixos),
        }
    }
}
