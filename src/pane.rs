//! Tabs + split panes. Fixes the other Alacritty gap the user called out:
//! Alacritty has no built-in split; here a tab holds a binary tree of panes
//! so you can split horizontally/vertically and resize, like Windows
//! Terminal / kitty / tmux.

use crate::config::Config;
use crate::pty::PtyHandle;
use crate::selection::Selection;
use crate::term::Term;
use anyhow::Result;

pub struct Pane {
    pub term: Term,
    pub pty: PtyHandle,
    pub selection: Option<Selection>,
    pub cols: u16,
    pub rows: u16,
}

impl Pane {
    pub fn new(cols: u16, rows: u16, config: &Config) -> Result<Self> {
        let pty = PtyHandle::spawn(cols, rows, &config.shell)?;
        let term = Term::new(cols as usize, rows as usize, config.scroll.history_lines);
        Ok(Self {
            term,
            pty,
            selection: None,
            cols,
            rows,
        })
    }

    /// Drain whatever the PTY has produced since the last frame and feed it
    /// to the VT100 parser. Called once per frame from the render loop.
    pub fn pump(&mut self) {
        while let Ok(bytes) = self.pty.output_rx.try_recv() {
            self.term.advance(&bytes);
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.resize(cols as usize, rows as usize);
        let _ = self.pty.resize(cols, rows);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    Horizontal, // side by side
    Vertical,   // stacked
}

/// A binary tree describing how panes tile a tab's viewport.
pub enum Layout {
    Leaf(usize), // index into Tab::panes
    Split {
        dir: SplitDir,
        /// Fraction (0.0-1.0) of space given to `first`.
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

impl Layout {
    /// Compute pixel rects for every leaf pane given the tab's total area.
    pub fn compute_rects(&self, x: f32, y: f32, w: f32, h: f32, out: &mut Vec<(usize, [f32; 4])>) {
        match self {
            Layout::Leaf(idx) => out.push((*idx, [x, y, w, h])),
            Layout::Split { dir, ratio, first, second } => match dir {
                SplitDir::Horizontal => {
                    let w1 = w * ratio;
                    first.compute_rects(x, y, w1, h, out);
                    second.compute_rects(x + w1, y, w - w1, h, out);
                }
                SplitDir::Vertical => {
                    let h1 = h * ratio;
                    first.compute_rects(x, y, w, h1, out);
                    second.compute_rects(x, y + h1, w, h - h1, out);
                }
            },
        }
    }

    fn replace_leaf(&mut self, target: usize, new_leaf: usize, dir: SplitDir) -> bool {
        match self {
            Layout::Leaf(idx) if *idx == target => {
                let old = Layout::Leaf(target);
                let new = Layout::Leaf(new_leaf);
                *self = Layout::Split {
                    dir,
                    ratio: 0.5,
                    first: Box::new(old),
                    second: Box::new(new),
                };
                true
            }
            Layout::Leaf(_) => false,
            Layout::Split { first, second, .. } => {
                first.replace_leaf(target, new_leaf, dir) || second.replace_leaf(target, new_leaf, dir)
            }
        }
    }
}

pub struct Tab {
    pub title: String,
    pub panes: Vec<Pane>,
    pub layout: Layout,
    pub active_pane: usize,
}

impl Tab {
    pub fn new(cols: u16, rows: u16, config: &Config) -> Result<Self> {
        let pane = Pane::new(cols, rows, config)?;
        Ok(Self {
            title: "rterm".to_string(),
            panes: vec![pane],
            layout: Layout::Leaf(0),
            active_pane: 0,
        })
    }

    /// Split the currently active pane. `total_{cols,rows}` is the full tab
    /// viewport in cells, used to size the freshly created pane before the
    /// next real resize event arrives.
    pub fn split_active(&mut self, dir: SplitDir, config: &Config, cols: u16, rows: u16) -> Result<()> {
        let (new_cols, new_rows) = match dir {
            SplitDir::Horizontal => (cols / 2, rows),
            SplitDir::Vertical => (cols, rows / 2),
        };
        let pane = Pane::new(new_cols.max(1), new_rows.max(1), config)?;
        let new_idx = self.panes.len();
        self.panes.push(pane);
        self.layout.replace_leaf(self.active_pane, new_idx, dir);
        self.active_pane = new_idx;
        Ok(())
    }

    pub fn active_pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.active_pane]
    }

    /// Move focus to the next pane in creation order (simple, predictable
    /// cycling — directional pane navigation is a follow-up).
    pub fn focus_next_pane(&mut self) {
        if self.panes.is_empty() {
            return;
        }
        self.active_pane = (self.active_pane + 1) % self.panes.len();
    }
}

pub struct TabBar {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}

impl TabBar {
    pub fn new(cols: u16, rows: u16, config: &Config) -> Result<Self> {
        Ok(Self {
            tabs: vec![Tab::new(cols, rows, config)?],
            active_tab: 0,
        })
    }

    pub fn new_tab(&mut self, cols: u16, rows: u16, config: &Config) -> Result<()> {
        self.tabs.push(Tab::new(cols, rows, config)?);
        self.active_tab = self.tabs.len() - 1;
        Ok(())
    }

    pub fn close_active_tab(&mut self) {
        if self.tabs.len() <= 1 {
            return;
        }
        self.tabs.remove(self.active_tab);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }
}
