pub mod cell;
pub mod grid;

use crate::image::{self, DecodedImage};
use cell::{CellFlags, Rgb};
use grid::Grid;
use vte::{Params, Parser, Perform};

/// ANSI 16-color palette (Alacritty-ish "dark" defaults). Overridable via config.
const PALETTE: [Rgb; 16] = [
    Rgb::new(0x1e, 0x1e, 0x1e), // black
    Rgb::new(0xcc, 0x66, 0x66), // red
    Rgb::new(0xb5, 0xbd, 0x68), // green
    Rgb::new(0xf0, 0xc6, 0x74), // yellow
    Rgb::new(0x81, 0xa2, 0xbe), // blue
    Rgb::new(0xb2, 0x94, 0xbb), // magenta
    Rgb::new(0x8a, 0xbe, 0xb7), // cyan
    Rgb::new(0xc5, 0xc8, 0xc6), // white
    Rgb::new(0x66, 0x66, 0x66), // bright black
    Rgb::new(0xd5, 0x4e, 0x53), // bright red
    Rgb::new(0xb9, 0xca, 0x4a), // bright green
    Rgb::new(0xe7, 0xc5, 0x47), // bright yellow
    Rgb::new(0x7a, 0xa6, 0xda), // bright blue
    Rgb::new(0xc3, 0x97, 0xd8), // bright magenta
    Rgb::new(0x70, 0xc0, 0xb1), // bright cyan
    Rgb::new(0xea, 0xea, 0xea), // bright white
];

/// Which DCS payload is currently being buffered via `Perform::hook/put`.
enum DcsKind {
    Sixel,
}

pub struct Term {
    pub grid: Grid,
    parser: Parser,
    default_fg: Rgb,
    default_bg: Rgb,
    /// Set true by OSC 0/2 (window title change) so the UI can update the tab label.
    pub title: Option<String>,
    /// Bell requested (BEL char) — UI can flash the tab or play a sound.
    pub bell: bool,
    /// Images decoded from Sixel/Kitty escape sequences, in arrival order.
    /// Not yet drawn by the renderer (textured-quad upload is a follow-up) —
    /// this is just the decode-and-collect half of the pipeline.
    pub images: Vec<DecodedImage>,
    dcs_kind: Option<DcsKind>,
    dcs_buf: Vec<u8>,
    /// Cursor stashed by DECSC (ESC 7) / SCP (CSI s), restored by their
    /// counterparts. Shells use this around prompt redraws.
    saved_cursor: Option<grid::CursorPos>,
}

impl Term {
    pub fn new(cols: usize, rows: usize, scrollback_limit: usize) -> Self {
        let default_fg = Rgb::new(0xd8, 0xd8, 0xd8);
        let default_bg = Rgb::new(0x1e, 0x1e, 0x1e);
        Self {
            grid: Grid::new(cols, rows, scrollback_limit),
            parser: Parser::new(),
            default_fg,
            default_bg,
            title: None,
            bell: false,
            images: Vec::new(),
            dcs_kind: None,
            dcs_buf: Vec::new(),
            saved_cursor: None,
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }

    /// Feed raw bytes read from the PTY into the VT100/ANSI state machine.
    ///
    /// Kitty graphics (APC `ESC _ G ... ST`) is intercepted here rather than
    /// through `vte::Perform`: this vte version's state machine silently
    /// discards APC string content (no callback exists for it), so we pull
    /// the sequence out of the raw stream ourselves before it ever reaches
    /// the parser. Everything else still goes through `parser_advance`
    /// byte-by-byte as before.
    pub fn advance(&mut self, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'_') && bytes.get(i + 2) == Some(&b'G') {
                if let Some(st_offset) = find_st(&bytes[i..]) {
                    let payload = &bytes[i + 3..i + st_offset];
                    self.handle_kitty_apc(payload);
                    i += st_offset + 2; // skip past the ST (ESC \)
                    continue;
                }
                // ST not in this chunk yet (rare: split across PTY reads).
                // Fall through to normal per-byte feed; vte will just
                // silently ignore it and we miss this one image.
            }
            self.parser_advance(bytes[i]);
            i += 1;
        }
    }

    fn handle_kitty_apc(&mut self, payload: &[u8]) {
        let Ok(s) = std::str::from_utf8(payload) else { return };
        let (control, data) = s.split_once(';').unwrap_or((s, ""));
        match image::decode_kitty(control, data, self.grid.cursor.col, self.grid.cursor.row) {
            Ok(img) => self.images.push(img),
            Err(e) => log::warn!("gagal decode gambar Kitty: {e}"),
        }
    }

    fn parser_advance(&mut self, byte: u8) {
        // Work around needing `&mut self.parser` and `&mut self` (as Perform)
        // simultaneously by taking the parser out temporarily.
        let mut parser = std::mem::replace(&mut self.parser, Parser::new());
        let mut performer = TermPerformer { term: self };
        parser.advance(&mut performer, byte);
        self.parser = parser;
    }
}

/// Find the offset of the first `ESC \` (String Terminator) in `data`,
/// relative to the start of `data`. Only the 7-bit two-byte form is
/// recognized — real-world senders (kitty's own `icat`, etc.) always emit
/// this form rather than the 8-bit C1 `0x9c`.
fn find_st(data: &[u8]) -> Option<usize> {
    data.windows(2).position(|w| w[0] == 0x1b && w[1] == b'\\')
}

struct TermPerformer<'a> {
    term: &'a mut Term,
}

impl<'a> Perform for TermPerformer<'a> {
    fn print(&mut self, c: char) {
        self.term.grid.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.term.grid.linefeed(),
            b'\r' => self.term.grid.carriage_return(),
            0x08 => self.term.grid.backspace(),
            0x07 => self.term.bell = true, // BEL
            0x09 => {
                // Tab: advance to next multiple of 8.
                let next = (self.term.grid.cursor.col / 8 + 1) * 8;
                self.term.grid.cursor.col = next.min(self.term.grid.cols.saturating_sub(1));
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, c: char) {
        let p = |i: usize, default: u16| -> u16 {
            params
                .iter()
                .nth(i)
                .and_then(|p| p.first().copied())
                .filter(|&v| v != 0)
                .unwrap_or(default)
        };

        match c {
            'A' => self.term.grid.cursor.row = self.term.grid.cursor.row.saturating_sub(p(0, 1) as usize),
            'B' => {
                self.term.grid.cursor.row =
                    (self.term.grid.cursor.row + p(0, 1) as usize).min(self.term.grid.rows - 1)
            }
            'C' => {
                self.term.grid.cursor.col =
                    (self.term.grid.cursor.col + p(0, 1) as usize).min(self.term.grid.cols - 1)
            }
            'D' => self.term.grid.cursor.col = self.term.grid.cursor.col.saturating_sub(p(0, 1) as usize),
            'H' | 'f' => {
                let row = p(0, 1).saturating_sub(1) as usize;
                let col = p(1, 1).saturating_sub(1) as usize;
                self.term.grid.cursor.row = row.min(self.term.grid.rows.saturating_sub(1));
                self.term.grid.cursor.col = col.min(self.term.grid.cols.saturating_sub(1));
            }
            'G' => {
                let col = p(0, 1).saturating_sub(1) as usize;
                self.term.grid.cursor.col = col.min(self.term.grid.cols.saturating_sub(1));
            }
            'd' => {
                let row = p(0, 1).saturating_sub(1) as usize;
                self.term.grid.cursor.row = row.min(self.term.grid.rows.saturating_sub(1));
            }
            'E' => {
                self.term.grid.cursor.col = 0;
                self.term.grid.cursor.row =
                    (self.term.grid.cursor.row + p(0, 1) as usize).min(self.term.grid.rows - 1);
            }
            'F' => {
                self.term.grid.cursor.col = 0;
                self.term.grid.cursor.row = self.term.grid.cursor.row.saturating_sub(p(0, 1) as usize);
            }
            'J' => self.term.grid.erase_in_display(p(0, 0)),
            'K' => self.term.grid.erase_in_line(p(0, 0)),
            '@' => self.term.grid.insert_chars(p(0, 1) as usize),
            'P' => self.term.grid.delete_chars(p(0, 1) as usize),
            'X' => self.term.grid.erase_chars(p(0, 1) as usize),
            'L' => self.term.grid.insert_lines(p(0, 1) as usize),
            'M' => self.term.grid.delete_lines(p(0, 1) as usize),
            's' => self.term.saved_cursor = Some(self.term.grid.cursor),
            'u' => {
                if let Some(pos) = self.term.saved_cursor {
                    self.term.grid.cursor = pos;
                }
            }
            // DECSET/DECRST. Only DECTCEM (?25, cursor visibility) is acted
            // on; the rest are accepted and ignored so they don't fall
            // through and print as garbage.
            'h' | 'l' if intermediates.first() == Some(&b'?') => {
                if p(0, 0) == 25 {
                    self.term.grid.cursor_visible = c == 'h';
                }
            }
            'm' => self.sgr(params),
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'M' => self.term.grid.reverse_index(), // RI
            b'7' => self.term.saved_cursor = Some(self.term.grid.cursor),
            b'8' => {
                if let Some(pos) = self.term.saved_cursor {
                    self.term.grid.cursor = pos;
                }
            }
            _ => {}
        }
    }

    /// DCS introducer. Sixel graphics arrive as `ESC P ... q <data> ESC \`
    /// — the final action character of the DCS header is `q`; everything
    /// else (device attribute requests, etc.) we don't handle, so it's
    /// dropped rather than buffered.
    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        self.term.dcs_buf.clear();
        self.term.dcs_kind = if action == 'q' { Some(DcsKind::Sixel) } else { None };
    }

    fn put(&mut self, byte: u8) {
        if self.term.dcs_kind.is_some() {
            self.term.dcs_buf.push(byte);
        }
    }

    fn unhook(&mut self) {
        if let Some(DcsKind::Sixel) = self.term.dcs_kind.take() {
            let (col, row) = (self.term.grid.cursor.col, self.term.grid.cursor.row);
            match image::decode_sixel(&self.term.dcs_buf, col, row) {
                Ok(img) => self.term.images.push(img),
                Err(e) => log::warn!("gagal decode gambar Sixel: {e}"),
            }
        }
        self.term.dcs_buf.clear();
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 0 / 2: set window title.
        if let [id, title @ ..] = params {
            if *id == b"0" || *id == b"2" {
                let joined: Vec<u8> = title.concat();
                if let Ok(s) = String::from_utf8(joined) {
                    self.term.title = Some(s);
                }
            }
        }
    }
}

impl<'a> TermPerformer<'a> {
    fn sgr(&mut self, params: &Params) {
        let grid = &mut self.term.grid;
        let mut iter = params.iter();
        while let Some(p) = iter.next() {
            let code = p.first().copied().unwrap_or(0);
            match code {
                0 => {
                    grid.fg = self.term.default_fg;
                    grid.bg = self.term.default_bg;
                    grid.flags = CellFlags::empty();
                }
                1 => grid.flags |= CellFlags::BOLD,
                3 => grid.flags |= CellFlags::ITALIC,
                4 => grid.flags |= CellFlags::UNDERLINE,
                7 => grid.flags |= CellFlags::INVERSE,
                9 => grid.flags |= CellFlags::STRIKEOUT,
                22 => grid.flags.remove(CellFlags::BOLD | CellFlags::DIM),
                23 => grid.flags.remove(CellFlags::ITALIC),
                24 => grid.flags.remove(CellFlags::UNDERLINE),
                27 => grid.flags.remove(CellFlags::INVERSE),
                30..=37 => grid.fg = PALETTE[(code - 30) as usize],
                38 => {
                    // Extended fg color: 38;5;N (256) or 38;2;R;G;B (truecolor)
                    if let Some(next) = iter.next() {
                        match next.first().copied().unwrap_or(0) {
                            2 => {
                                let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                grid.fg = Rgb::new(r as u8, g as u8, b as u8);
                            }
                            5 => {
                                let idx = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                grid.fg = palette_256(idx);
                            }
                            _ => {}
                        }
                    }
                }
                39 => grid.fg = self.term.default_fg,
                40..=47 => grid.bg = PALETTE[(code - 40) as usize],
                48 => {
                    if let Some(next) = iter.next() {
                        match next.first().copied().unwrap_or(0) {
                            2 => {
                                let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                grid.bg = Rgb::new(r as u8, g as u8, b as u8);
                            }
                            5 => {
                                let idx = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                                grid.bg = palette_256(idx);
                            }
                            _ => {}
                        }
                    }
                }
                49 => grid.bg = self.term.default_bg,
                90..=97 => grid.fg = PALETTE[(code - 90 + 8) as usize],
                100..=107 => grid.bg = PALETTE[(code - 100 + 8) as usize],
                _ => {}
            }
        }
    }
}

fn palette_256(idx: u16) -> Rgb {
    if idx < 16 {
        return PALETTE[idx as usize];
    }
    if idx < 232 {
        let idx = idx - 16;
        let r = idx / 36;
        let g = (idx % 36) / 6;
        let b = idx % 6;
        let scale = |v: u16| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
        return Rgb::new(scale(r), scale(g), scale(b));
    }
    let level = (idx - 232) * 10 + 8;
    Rgb::new(level as u8, level as u8, level as u8)
}
