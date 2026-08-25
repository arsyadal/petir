//! Mouse text selection + clipboard. This is where the "Ctrl+C should just
//! work" fix lives: `smart_copy` is what the main input handler calls on
//! Ctrl+C instead of always sending the byte 0x03 (SIGINT) to the shell.

use crate::term::grid::Grid;
use anyhow::Result;
use arboard::Clipboard;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridPos {
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub anchor: GridPos,
    pub active: GridPos,
}

impl Selection {
    pub fn new(at: GridPos) -> Self {
        Self { anchor: at, active: at }
    }

    pub fn extend(&mut self, to: GridPos) {
        self.active = to;
    }

    fn ordered(&self) -> (GridPos, GridPos) {
        if (self.anchor.row, self.anchor.col) <= (self.active.row, self.active.col) {
            (self.anchor, self.active)
        } else {
            (self.active, self.anchor)
        }
    }

    /// Extract the selected text out of the visible grid (row-major,
    /// linear/"stream" selection — box/rectangular selection is a nice
    /// follow-up but not needed for the common case).
    pub fn extract_text(&self, grid: &Grid) -> String {
        let (start, end) = self.ordered();
        let mut out = String::new();
        for row in start.row..=end.row {
            let line = grid.row_text(row);
            let chars: Vec<char> = line.chars().collect();
            let col_start = if row == start.row { start.col } else { 0 };
            let col_end = if row == end.row {
                end.col.min(chars.len().saturating_sub(1))
            } else {
                chars.len().saturating_sub(1)
            };
            if col_start <= col_end {
                out.push_str(&chars[col_start..=col_end.min(chars.len().saturating_sub(1))].iter().collect::<String>());
            }
            if row != end.row {
                out.push('\n');
            }
        }
        // Trim trailing spaces per line is nice-to-have; keep v1 simple.
        out
    }
}

pub struct ClipboardManager {
    inner: Option<Clipboard>,
}

impl ClipboardManager {
    pub fn new() -> Self {
        Self {
            inner: Clipboard::new().ok(),
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) -> Result<()> {
        if let Some(cb) = self.inner.as_mut() {
            cb.set_text(text.into())?;
        }
        Ok(())
    }

    pub fn get_text(&mut self) -> Result<String> {
        Ok(self.inner.as_mut().map(|c| c.get_text().unwrap_or_default()).unwrap_or_default())
    }
}

/// Called on Ctrl+C. Returns `true` if it consumed the event as "copy"
/// (selection existed) so the caller must NOT also forward 0x03 to the PTY.
/// Returns `false` if there was no selection, meaning the caller should
/// send SIGINT (0x03) to the shell like every other terminal does on plain
/// Ctrl+C.
pub fn smart_copy(
    selection: &Option<Selection>,
    grid: &Grid,
    clipboard: &mut ClipboardManager,
) -> bool {
    match selection {
        Some(sel) => {
            let text = sel.extract_text(grid);
            if text.trim().is_empty() {
                false
            } else {
                let _ = clipboard.set_text(text);
                true
            }
        }
        None => false,
    }
}
