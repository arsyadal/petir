//! A single terminal grid cell: character + style.
//! Kept small and `Copy` on purpose — the grid can hold hundreds of
//! thousands of these in scrollback, so cache-friendliness matters for speed.

use bitflags::bitflags;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn to_wgpu(self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            1.0,
        ]
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct CellFlags: u16 {
        const BOLD          = 0b0000_0000_0001;
        const ITALIC        = 0b0000_0000_0010;
        const UNDERLINE     = 0b0000_0000_0100;
        const STRIKEOUT     = 0b0000_0000_1000;
        const INVERSE       = 0b0000_0001_0000;
        const HIDDEN        = 0b0000_0010_0000;
        const DIM           = 0b0000_0100_0000;
        const WIDE_CHAR     = 0b0000_1000_0000; // occupies 2 columns (CJK/emoji)
        const WIDE_SPACER   = 0b0001_0000_0000; // trailing half of a wide char
        const CURSOR        = 0b0010_0000_0000; // rendering hint only
        const SELECTED      = 0b0100_0000_0000; // rendering hint only
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub c: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub flags: CellFlags,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: Rgb::new(0xd8, 0xd8, 0xd8),
            bg: Rgb::new(0x1e, 0x1e, 0x1e),
            flags: CellFlags::empty(),
        }
    }
}

impl Cell {
    pub fn is_empty(&self) -> bool {
        self.c == ' ' && self.flags.is_empty()
    }
}
