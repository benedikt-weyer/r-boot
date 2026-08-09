//! Protocol backends share this narrow pre-handoff interface.

use crate::elf::{Image, LoadedSegment};

pub trait BootProtocol {
    /// Populate protocol-owned data and request responses before boot services
    /// are exited. Implementations must not retain UEFI protocol handles.
    fn prepare(
        &self,
        file: &[u8],
        image: &Image<'_>,
        segments: &[LoadedSegment],
        hhdm_offset: u64,
    ) -> Result<(), &'static str>;
}
