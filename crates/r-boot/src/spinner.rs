//! A compact GOP spinner for showing progress during blocking boot work.
//!
//! UEFI boot services are single-threaded and cooperative, so this is driven
//! by explicit `tick()` calls between synchronous operations rather than a
//! timer. Each frame is rendered in RAM and copied as one small dirty
//! rectangle through the Graphics Output Protocol.

use alloc::vec;
use alloc::vec::Vec;

use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams};
use uefi::proto::console::gop::{BltOp, BltPixel, BltRegion, GraphicsOutput};

const SIZE: usize = 48;
const DOT_RADIUS: i32 = 3;
const DOTS: [(i32, i32); 12] = [
    (0, -16),
    (8, -14),
    (14, -8),
    (16, 0),
    (14, 8),
    (8, 14),
    (0, 16),
    (-8, 14),
    (-14, 8),
    (-16, 0),
    (-14, -8),
    (-8, -14),
];
const BACKGROUND: BltPixel = BltPixel::new(0, 0, 0);
const INACTIVE: BltPixel = BltPixel::new(48, 48, 48);
const ACTIVE: BltPixel = BltPixel::new(255, 255, 255);
const TEXT_WIDTH: usize = 60;
const BLANK_TEXT: &str = "                                                            ";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    Off,
    Text,
    #[default]
    Graphical,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "text" => Some(Self::Text),
            "graphical" => Some(Self::Graphical),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Text => "text",
            Self::Graphical => "graphical",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Off => Self::Text,
            Self::Text => Self::Graphical,
            Self::Graphical => Self::Off,
        }
    }

    pub const fn previous(self) -> Self {
        match self {
            Self::Off => Self::Graphical,
            Self::Text => Self::Off,
            Self::Graphical => Self::Text,
        }
    }
}

pub struct Spinner {
    mode: Mode,
    index: usize,
    buffer: Vec<BltPixel>,
    last_position: Option<(usize, usize)>,
}

impl Spinner {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            index: 0,
            buffer: vec![BACKGROUND; SIZE * SIZE],
            last_position: None,
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.index = 0;
        self.last_position = None;
    }

    /// Advances one frame using the configured output mode.
    pub fn tick(&mut self, label: &str) {
        match self.mode {
            Mode::Off => {}
            Mode::Text => {
                const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
                uefi::print!(
                    "\r{}\r{} {label}",
                    &BLANK_TEXT[..TEXT_WIDTH],
                    FRAMES[self.index]
                );
                self.index = (self.index + 1) % FRAMES.len();
            }
            Mode::Graphical => self.tick_graphical(),
        }
    }

    fn tick_graphical(&mut self) {
        let Ok(mut gop) = open_gop() else {
            return;
        };
        let (width, height) = gop.current_mode_info().resolution();
        if width < SIZE || height < SIZE {
            return;
        }
        let position = ((width - SIZE) / 2, (height - SIZE) / 2);
        self.render_frame();
        if gop
            .blt(BltOp::BufferToVideo {
                buffer: &self.buffer,
                src: BltRegion::Full,
                dest: position,
                dims: (SIZE, SIZE),
            })
            .is_ok()
        {
            self.last_position = Some(position);
            self.index = (self.index + 1) % DOTS.len();
        }
    }

    /// Erases the spinner's dirty rectangle without reading the framebuffer.
    pub fn clear(&mut self) {
        if self.mode == Mode::Text {
            uefi::print!("\r{}\r", &BLANK_TEXT[..TEXT_WIDTH]);
            return;
        }
        let Some(position) = self.last_position else {
            return;
        };
        for pixel in &mut self.buffer {
            *pixel = BACKGROUND;
        }
        let Ok(mut gop) = open_gop() else {
            return;
        };
        let _ = gop.blt(BltOp::BufferToVideo {
            buffer: &self.buffer,
            src: BltRegion::Full,
            dest: position,
            dims: (SIZE, SIZE),
        });
    }

    fn render_frame(&mut self) {
        for pixel in &mut self.buffer {
            *pixel = BACKGROUND;
        }
        for (index, (x, y)) in DOTS.iter().enumerate() {
            let color = if index == self.index {
                ACTIVE
            } else {
                INACTIVE
            };
            self.draw_dot(SIZE as i32 / 2 + x, SIZE as i32 / 2 + y, color);
        }
    }

    fn draw_dot(&mut self, center_x: i32, center_y: i32, color: BltPixel) {
        for y in -DOT_RADIUS..=DOT_RADIUS {
            for x in -DOT_RADIUS..=DOT_RADIUS {
                if x * x + y * y <= DOT_RADIUS * DOT_RADIUS {
                    let x = (center_x + x) as usize;
                    let y = (center_y + y) as usize;
                    self.buffer[y * SIZE + x] = color;
                }
            }
        }
    }
}

fn open_gop() -> Result<uefi::boot::ScopedProtocol<GraphicsOutput>, uefi::Error> {
    let handle = boot::get_handle_for_protocol::<GraphicsOutput>()?;
    let params = OpenProtocolParams {
        handle,
        agent: boot::image_handle(),
        controller: None,
    };
    // SAFETY: this is a read-only, non-exclusive protocol open. GOP blits are
    // coordinated by firmware and do not invalidate the text console driver.
    unsafe { boot::open_protocol::<GraphicsOutput>(params, OpenProtocolAttributes::GetProtocol) }
}
