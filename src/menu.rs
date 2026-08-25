//! Right-click context menu. Exists mostly for discoverability: every entry
//! here is also a keybinding, and the menu shows that binding next to the
//! label so people learn the shortcut instead of reaching for the mouse
//! again next time.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Copy,
    Paste,
    SplitHorizontal,
    SplitVertical,
    NewTab,
    CloseTab,
    Search,
    Clear,
}

pub struct MenuItem {
    pub label: &'static str,
    /// Shown right-aligned; empty string renders as a separator row.
    pub shortcut: &'static str,
    pub action: Option<MenuAction>,
}

const fn item(label: &'static str, shortcut: &'static str, action: MenuAction) -> MenuItem {
    MenuItem { label, shortcut, action: Some(action) }
}

const SEPARATOR: MenuItem = MenuItem { label: "", shortcut: "", action: None };

pub struct ContextMenu {
    /// Top-left corner, in physical pixels.
    pub x: f32,
    pub y: f32,
    pub items: Vec<MenuItem>,
    pub hovered: Option<usize>,
}

impl ContextMenu {
    pub const WIDTH: f32 = 260.0;
    pub const ITEM_H: f32 = 26.0;
    pub const SEPARATOR_H: f32 = 9.0;
    pub const PAD: f32 = 8.0;

    pub fn new(x: f32, y: f32, window_w: f32, window_h: f32) -> Self {
        let items = vec![
            item("Copy", "Ctrl+C", MenuAction::Copy),
            item("Paste", "Ctrl+V", MenuAction::Paste),
            SEPARATOR,
            item("Split horizontally", "Ctrl+Shift+E", MenuAction::SplitHorizontal),
            item("Split vertically", "Ctrl+Shift+D", MenuAction::SplitVertical),
            SEPARATOR,
            item("New tab", "Ctrl+Shift+T", MenuAction::NewTab),
            item("Close tab", "Ctrl+Shift+W", MenuAction::CloseTab),
            SEPARATOR,
            item("Find", "Ctrl+Shift+F", MenuAction::Search),
            item("Clear screen", "Ctrl+L", MenuAction::Clear),
        ];

        let mut menu = Self { x, y, items, hovered: None };
        // Flip the menu back inside the window when opened near an edge,
        // rather than letting it render half off-screen.
        let h = menu.height();
        if menu.x + Self::WIDTH > window_w {
            menu.x = (window_w - Self::WIDTH).max(0.0);
        }
        if menu.y + h > window_h {
            menu.y = (window_h - h).max(0.0);
        }
        menu
    }

    pub fn height(&self) -> f32 {
        self.items
            .iter()
            .map(|i| if i.action.is_some() { Self::ITEM_H } else { Self::SEPARATOR_H })
            .sum::<f32>()
            + Self::PAD * 2.0
    }

    /// Y offset of each row's top edge, relative to the menu's top.
    pub fn row_offsets(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.items.len());
        let mut y = Self::PAD;
        for i in &self.items {
            out.push(y);
            y += if i.action.is_some() { Self::ITEM_H } else { Self::SEPARATOR_H };
        }
        out
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + Self::WIDTH && py >= self.y && py <= self.y + self.height()
    }

    /// Index of the selectable row under the cursor, if any.
    pub fn hit(&self, px: f32, py: f32) -> Option<usize> {
        if !self.contains(px, py) {
            return None;
        }
        let local_y = py - self.y;
        for (idx, (item, top)) in self.items.iter().zip(self.row_offsets()).enumerate() {
            let h = if item.action.is_some() { Self::ITEM_H } else { Self::SEPARATOR_H };
            if local_y >= top && local_y < top + h {
                return item.action.map(|_| idx);
            }
        }
        None
    }
}
