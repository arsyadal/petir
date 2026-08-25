//! The terminal grid: a fixed-size visible viewport backed by a scrollback
//! ring buffer. Rows are `VecDeque<Vec<Cell>>` so scrolling is O(1) (push/pop
//! at either end) instead of copying the whole grid every line feed — this is
//! one of the bigger wins for "feels instant" scroll performance.

use super::cell::{Cell, CellFlags, Rgb};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Default)]
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
}

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    pub scrollback_limit: usize,
    /// Lines that have scrolled off the top of the viewport, oldest first.
    pub scrollback: VecDeque<Vec<Cell>>,
    /// The visible viewport, `rows` long.
    pub visible: VecDeque<Vec<Cell>>,
    pub cursor: CursorPos,
    pub cursor_visible: bool,
    pub fg: Rgb,
    pub bg: Rgb,
    pub flags: CellFlags,
    /// Offset into scrollback when the user has scrolled up (0 = pinned to bottom).
    pub scroll_offset: usize,
}

impl Grid {
    pub fn new(cols: usize, rows: usize, scrollback_limit: usize) -> Self {
        let mut visible = VecDeque::with_capacity(rows);
        for _ in 0..rows {
            visible.push_back(vec![Cell::default(); cols]);
        }
        Self {
            cols,
            rows,
            scrollback_limit,
            scrollback: VecDeque::with_capacity(scrollback_limit.min(4096)),
            visible,
            cursor: CursorPos::default(),
            cursor_visible: true,
            fg: Rgb::new(0xd8, 0xd8, 0xd8),
            bg: Rgb::new(0x1e, 0x1e, 0x1e),
            flags: CellFlags::empty(),
            scroll_offset: 0,
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        // Reflow is a whole feature on its own (Alacritty does it properly).
        // For v1 we do the simple, fast thing: pad/truncate rows and
        // add/remove rows from the bottom. Good enough for correctness and
        // keeps resize O(rows) instead of O(rows*cols) reflow.
        for row in self.visible.iter_mut() {
            row.resize(cols, Cell::default());
        }
        while self.visible.len() < rows {
            self.visible.push_back(vec![Cell::default(); cols]);
        }
        while self.visible.len() > rows {
            let line = self.visible.pop_front().unwrap();
            self.push_scrollback(line);
        }
        self.cols = cols;
        self.rows = rows;
        self.cursor.row = self.cursor.row.min(rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(cols.saturating_sub(1));
    }

    fn push_scrollback(&mut self, line: Vec<Cell>) {
        self.scrollback.push_back(line);
        if self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
        }
    }

    pub fn newline(&mut self) {
        let top = self.visible.pop_front().unwrap();
        self.push_scrollback(top);
        self.visible.push_back(vec![Cell::default(); self.cols]);
    }

    pub fn put_char(&mut self, c: char) {
        let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1).max(1);

        if self.cursor.col + width > self.cols {
            self.carriage_return();
            self.linefeed();
        }

        let row = self.cursor.row;
        let col = self.cursor.col;
        if let Some(line) = self.visible.get_mut(row) {
            if col < line.len() {
                line[col] = Cell {
                    c,
                    fg: self.fg,
                    bg: self.bg,
                    flags: if width == 2 {
                        self.flags | CellFlags::WIDE_CHAR
                    } else {
                        self.flags
                    },
                };
                if width == 2 && col + 1 < line.len() {
                    line[col + 1] = Cell {
                        c: ' ',
                        fg: self.fg,
                        bg: self.bg,
                        flags: CellFlags::WIDE_SPACER,
                    };
                }
            }
        }
        self.cursor.col += width;
    }

    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
    }

    pub fn linefeed(&mut self) {
        if self.cursor.row + 1 >= self.rows {
            self.newline();
        } else {
            self.cursor.row += 1;
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
    }

    pub fn erase_in_display(&mut self, mode: u16) {
        match mode {
            0 => {
                // cursor to end of screen
                if let Some(line) = self.visible.get_mut(self.cursor.row) {
                    for cell in line.iter_mut().skip(self.cursor.col) {
                        *cell = Cell::default();
                    }
                }
                for row in (self.cursor.row + 1)..self.rows {
                    if let Some(line) = self.visible.get_mut(row) {
                        line.iter_mut().for_each(|c| *c = Cell::default());
                    }
                }
            }
            1 => {
                for row in 0..self.cursor.row {
                    if let Some(line) = self.visible.get_mut(row) {
                        line.iter_mut().for_each(|c| *c = Cell::default());
                    }
                }
                if let Some(line) = self.visible.get_mut(self.cursor.row) {
                    for cell in line.iter_mut().take(self.cursor.col + 1) {
                        *cell = Cell::default();
                    }
                }
            }
            2 | 3 => {
                for row in self.visible.iter_mut() {
                    row.iter_mut().for_each(|c| *c = Cell::default());
                }
            }
            _ => {}
        }
    }

    pub fn erase_in_line(&mut self, mode: u16) {
        if let Some(line) = self.visible.get_mut(self.cursor.row) {
            match mode {
                0 => {
                    for cell in line.iter_mut().skip(self.cursor.col) {
                        *cell = Cell::default();
                    }
                }
                1 => {
                    for cell in line.iter_mut().take(self.cursor.col + 1) {
                        *cell = Cell::default();
                    }
                }
                2 => line.iter_mut().for_each(|c| *c = Cell::default()),
                _ => {}
            }
        }
    }

    /// Text of a visible row, used by search & selection copy.
    pub fn row_text(&self, row: usize) -> String {
        self.visible
            .get(row)
            .map(|line| line.iter().map(|c| c.c).collect::<String>())
            .unwrap_or_default()
    }
}
