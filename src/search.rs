//! Scrollback search (Ctrl+Shift+F). v1 is plain case-insensitive substring
//! search across scrollback + the visible viewport; regex is a natural
//! follow-up (swap `contains` for the `regex` crate) once this ships.

use crate::term::grid::Grid;

#[derive(Clone, Copy, Debug)]
pub struct Match {
    /// Row index; negative-space scrollback rows are indexed before 0,
    /// visible rows start at 0 — see `SearchIndex` below for how callers
    /// map this back to something drawable.
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
}

pub struct SearchState {
    pub query: String,
    pub matches: Vec<Match>,
    pub current: usize,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        }
    }

    /// Re-run the search. `scrollback_offset` lets callers treat scrollback
    /// rows as coming before row 0 of the visible grid, i.e. row indices are
    /// `0..scrollback.len()` for history then `scrollback.len()..` for the
    /// visible viewport.
    pub fn run(&mut self, grid: &Grid) {
        self.matches.clear();
        self.current = 0;
        if self.query.is_empty() {
            return;
        }
        let needle = self.query.to_lowercase();

        for (i, line) in grid.scrollback.iter().enumerate() {
            let text: String = line.iter().map(|c| c.c).collect();
            find_in_line(&text, &needle, i, &mut self.matches);
        }
        let base = grid.scrollback.len();
        for (i, line) in grid.visible.iter().enumerate() {
            let text: String = line.iter().map(|c| c.c).collect();
            find_in_line(&text, &needle, base + i, &mut self.matches);
        }
    }

    pub fn next_match(&mut self) -> Option<Match> {
        if self.matches.is_empty() {
            return None;
        }
        self.current = (self.current + 1) % self.matches.len();
        Some(self.matches[self.current])
    }

    pub fn prev_match(&mut self) -> Option<Match> {
        if self.matches.is_empty() {
            return None;
        }
        self.current = if self.current == 0 {
            self.matches.len() - 1
        } else {
            self.current - 1
        };
        Some(self.matches[self.current])
    }
}

fn find_in_line(text: &str, needle: &str, row: usize, out: &mut Vec<Match>) {
    let lower = text.to_lowercase();
    let mut start = 0;
    while let Some(pos) = lower[start..].find(needle) {
        let abs = start + pos;
        out.push(Match {
            row,
            col_start: abs,
            col_end: abs + needle.chars().count(),
        });
        start = abs + needle.len().max(1);
        if start >= lower.len() {
            break;
        }
    }
}
