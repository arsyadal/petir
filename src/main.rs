// Windows: no extra console window on launch. `windows_subsystem` only takes
// effect in release builds; debug builds keep the console so log output and
// panics stay visible while developing.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod image;
mod menu;
mod pane;
mod pty;
mod renderer;
mod search;
mod selection;
mod term;

use config::Config;
use menu::{ContextMenu, MenuAction};
use pane::{SplitDir, TabBar};
use pty::PtyWake;
use renderer::GpuState;
use search::SearchState;
use selection::{ClipboardManager, GridPos, Selection};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::{
    dpi::PhysicalPosition,
    event::{ElementState, KeyEvent, MouseButton, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::WindowBuilder,
};

struct App {
    config: Config,
    gpu: GpuState,
    tabs: TabBar,
    clipboard: ClipboardManager,
    search: SearchState,
    search_active: bool,
    modifiers: ModifiersState,
    mouse_down: bool,
    last_frame: Instant,
    /// Something changed since the last presented frame. The render loop is
    /// idle-driven: with this false there is nothing new to draw, so no frame
    /// is submitted at all. Without it the loop re-shapes and re-renders the
    /// whole grid continuously and pegs a core even on an idle prompt.
    dirty: bool,
    /// Handed to every PTY we spawn so its reader thread can wake the loop.
    proxy: EventLoopProxy<PtyWake>,
    menu: Option<ContextMenu>,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let config = Config::load();

    let event_loop = EventLoopBuilder::<PtyWake>::with_user_event().build()?;
    // Idle-driven: the loop sleeps until the OS delivers input or a PTY
    // reader thread wakes it with a PtyWake. Nothing is polled on a timer.
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let window_icon = load_icon();
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Petir")
            .with_inner_size(winit::dpi::LogicalSize::new(1000.0, 650.0))
            .with_window_icon(window_icon)
            .build(&event_loop)?,
    );

    let gpu = pollster::block_on(GpuState::new(window.clone(), config.font.size, config.window.vsync))?;
    let (cols, rows) = gpu.cols_rows_for(
        gpu.size.width as f32 - config.window.padding_x as f32 * 2.0,
        gpu.size.height as f32 - config.window.padding_y as f32 * 2.0,
    );
    let tabs = TabBar::new(cols, rows, &config, &proxy)?;

    let mut app = App {
        config,
        gpu,
        tabs,
        clipboard: ClipboardManager::new(),
        search: SearchState::new(),
        search_active: false,
        modifiers: ModifiersState::empty(),
        mouse_down: false,
        last_frame: Instant::now(),
        dirty: true,
        proxy: proxy.clone(),
        menu: None,
    };

    let mut cursor_pos = PhysicalPosition::new(0.0, 0.0);

    event_loop.run(move |event, elwt| match event {
        winit::event::Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
            WindowEvent::CloseRequested => elwt.exit(),
            WindowEvent::Resized(size) => {
                app.gpu.resize(size);
                app.resize_panes();
                app.dirty = true;
            }
            WindowEvent::ModifiersChanged(mods) => app.modifiers = mods.state(),
            WindowEvent::CursorMoved { position, .. } => {
                cursor_pos = position;
                if let Some(m) = app.menu.as_mut() {
                    let hovered = m.hit(position.x as f32, position.y as f32);
                    if hovered != m.hovered {
                        m.hovered = hovered;
                        app.dirty = true;
                    }
                } else if app.mouse_down {
                    app.extend_selection(cursor_pos);
                    app.dirty = true;
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => {
                if state == ElementState::Pressed {
                    app.menu = Some(ContextMenu::new(
                        cursor_pos.x as f32,
                        cursor_pos.y as f32,
                        app.gpu.size.width as f32,
                        app.gpu.size.height as f32,
                    ));
                    app.dirty = true;
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                match state {
                    ElementState::Pressed => {
                        // A click anywhere dismisses the menu; a click on a
                        // row runs it. Either way it must not also start a
                        // text selection underneath the menu.
                        if let Some(m) = app.menu.take() {
                            if let Some(idx) = m.hit(cursor_pos.x as f32, cursor_pos.y as f32) {
                                if let Some(action) = m.items[idx].action {
                                    app.run_menu_action(action);
                                }
                            }
                        } else {
                            app.mouse_down = true;
                            app.begin_selection(cursor_pos);
                        }
                    }
                    ElementState::Released => {
                        if app.mouse_down {
                            app.mouse_down = false;
                            if app.config.clipboard.copy_on_select {
                                app.copy_current_selection();
                            }
                        }
                    }
                }
                app.dirty = true;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                app.handle_key(event, elwt);
                app.dirty = true;
            }
            WindowEvent::RedrawRequested => app.frame(),
            _ => {}
        },
        winit::event::Event::UserEvent(PtyWake) => {
            if app.pump_ptys() {
                app.dirty = true;
            }
        }
        winit::event::Event::AboutToWait => {
            if !app.dirty {
                return;
            }
            // Rate-limit presentation: a burst of output (a build log, `cat`
            // of a big file) wakes us far more often than the display can
            // show, so coalesce those into at most one frame per slot and
            // sleep out the remainder.
            let frame_time = app.frame_time();
            let since = app.last_frame.elapsed();
            if since >= frame_time {
                app.last_frame = Instant::now();
                window.request_redraw();
                // Back to sleeping indefinitely; a stale WaitUntil deadline
                // left over from the branch below would busy-loop.
                elwt.set_control_flow(ControlFlow::Wait);
            } else {
                elwt.set_control_flow(ControlFlow::WaitUntil(Instant::now() + (frame_time - since)));
            }
        }
        _ => {}
    })?;

    Ok(())
}

impl App {
    fn resize_panes(&mut self) {
        let pad = (self.config.window.padding_x as f32, self.config.window.padding_y as f32);
        let w = self.gpu.size.width as f32 - pad.0 * 2.0;
        let h = self.gpu.size.height as f32 - pad.1 * 2.0;
        let tab = self.tabs.active_tab_mut();
        let rects = renderer::flatten_layout(&tab.layout, 0.0, 0.0, w, h);
        for (idx, rect) in rects {
            let (cols, rows) = self.gpu.cols_rows_for(rect[2], rect[3]);
            if let Some(pane) = tab.panes.get_mut(idx) {
                pane.resize(cols.max(1), rows.max(1));
            }
        }
    }

    /// Minimum time between presented frames, from `window.max_fps`
    /// (0 = uncapped, bounded here to a 1000 Hz poll so the loop still sleeps).
    fn frame_time(&self) -> Duration {
        let target = self.config.window.max_fps;
        if target > 0 {
            Duration::from_secs_f64(1.0 / target as f64)
        } else {
            Duration::from_millis(1)
        }
    }

    /// Drain PTY output for every pane of the active tab into its parser.
    /// Returns whether any bytes arrived, i.e. whether the screen changed.
    fn pump_ptys(&mut self) -> bool {
        let mut changed = false;
        for pane in self.tabs.active_tab_mut().panes.iter_mut() {
            if pane.pump() {
                changed = true;
            }
        }
        changed
    }

    fn frame(&mut self) {
        self.dirty = false;
        let tab = self.tabs.active_tab_mut();

        let pad = (self.config.window.padding_x as f32, self.config.window.padding_y as f32);
        let w = self.gpu.size.width as f32 - pad.0 * 2.0;
        let h = self.gpu.size.height as f32 - pad.1 * 2.0;
        let rects = renderer::flatten_layout(&tab.layout, 0.0, 0.0, w, h);

        let panes: Vec<(&term::grid::Grid, [f32; 4], bool)> = rects
            .iter()
            .map(|(idx, rect)| (&tab.panes[*idx].term.grid, *rect, *idx == tab.active_pane))
            .collect();

        let search_overlay = if self.search_active {
            let scrollback_len = tab.panes[tab.active_pane].term.grid.scrollback.len();
            Some((&self.search, scrollback_len))
        } else {
            None
        };

        if let Err(e) = self.gpu.render_frame(
            &panes,
            pad,
            self.config.font.ligatures,
            search_overlay,
            self.menu.as_ref(),
        ) {
            log::warn!("render error: {e}");
        }
    }

    fn active_grid_pos(&self, px: PhysicalPosition<f64>) -> GridPos {
        let pad = (self.config.window.padding_x as f32, self.config.window.padding_y as f32);
        let col = ((px.x as f32 - pad.0) / self.gpu.cell_w).floor().max(0.0) as usize;
        let row = ((px.y as f32 - pad.1) / self.gpu.cell_h).floor().max(0.0) as usize;
        GridPos { row, col }
    }

    fn begin_selection(&mut self, px: PhysicalPosition<f64>) {
        let pos = self.active_grid_pos(px);
        let pane = self.tabs.active_tab_mut().active_pane_mut();
        pane.selection = Some(Selection::new(pos));
    }

    fn extend_selection(&mut self, px: PhysicalPosition<f64>) {
        let pos = self.active_grid_pos(px);
        let pane = self.tabs.active_tab_mut().active_pane_mut();
        if let Some(sel) = pane.selection.as_mut() {
            sel.extend(pos);
        }
    }

    fn copy_current_selection(&mut self) {
        let pane = self.tabs.active_tab_mut().active_pane_mut();
        let _ = selection::smart_copy(&pane.selection, &pane.term.grid, &mut self.clipboard);
    }

    fn handle_key(&mut self, event: KeyEvent, elwt: &winit::event_loop::EventLoopWindowTarget<PtyWake>) {
        if event.state != ElementState::Pressed {
            return;
        }
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();

        // --- App-level shortcuts (tabs, splits, search) ---
        if ctrl && shift {
            match &event.logical_key {
                Key::Character(s) if s.as_str() == "T" || s.as_str() == "t" => {
                    let (cols, rows) = self.current_cols_rows();
                    let _ = self.tabs.new_tab(cols, rows, &self.config, &self.proxy);
                    return;
                }
                Key::Character(s) if s.as_str() == "W" || s.as_str() == "w" => {
                    self.tabs.close_active_tab();
                    return;
                }
                Key::Character(s) if s.as_str() == "E" || s.as_str() == "e" => {
                    let (cols, rows) = self.current_cols_rows();
                    let tab = self.tabs.active_tab_mut();
                    let _ = tab.split_active(SplitDir::Horizontal, &self.config, cols, rows, &self.proxy);
                    self.resize_panes();
                    return;
                }
                Key::Character(s) if s.as_str() == "D" || s.as_str() == "d" => {
                    let (cols, rows) = self.current_cols_rows();
                    let tab = self.tabs.active_tab_mut();
                    let _ = tab.split_active(SplitDir::Vertical, &self.config, cols, rows, &self.proxy);
                    self.resize_panes();
                    return;
                }
                Key::Character(s) if s.as_str() == "F" || s.as_str() == "f" => {
                    self.search_active = !self.search_active;
                    if self.search_active {
                        self.rerun_search();
                    }
                    return;
                }
                Key::Character(s) if s.as_str() == "C" || s.as_str() == "c" => {
                    // Alacritty-style fallback: always available even if
                    // smart_ctrl_c_ctrl_v is turned off in config.
                    self.copy_current_selection();
                    return;
                }
                Key::Character(s) if s.as_str() == "V" || s.as_str() == "v" => {
                    self.paste_clipboard();
                    return;
                }
                _ => {}
            }
        }

        if ctrl && !shift {
            match &event.logical_key {
                Key::Character(s) if s.eq_ignore_ascii_case("c") => {
                    if self.config.clipboard.smart_ctrl_c_ctrl_v {
                        let pane = self.tabs.active_tab_mut().active_pane_mut();
                        let copied = selection::smart_copy(&pane.selection, &pane.term.grid, &mut self.clipboard);
                        if copied {
                            pane.selection = None;
                            return; // consumed as copy, do NOT also send SIGINT
                        }
                        // fall through: no selection -> send SIGINT like normal
                    }
                    self.send_bytes(&[0x03]);
                    return;
                }
                Key::Character(s) if s.eq_ignore_ascii_case("v") => {
                    if self.config.clipboard.smart_ctrl_c_ctrl_v {
                        self.paste_clipboard();
                        return;
                    }
                    // fall through to normal Ctrl+V byte if smart paste disabled
                }
                Key::Character(s) if s.eq_ignore_ascii_case("l") => {
                    self.tabs.active_tab_mut().active_pane_mut().term.grid.erase_in_display(2);
                    return;
                }
                Key::Named(NamedKey::Tab) => {
                    self.tabs.active_tab_mut().focus_next_pane();
                    return;
                }
                Key::Named(NamedKey::Backspace) => {
                    // Delete the previous word. 0x08 is what Windows Terminal
                    // sends for Ctrl+Backspace, so PSReadLine maps it to
                    // BackwardDeleteWord; readline shells treat it the same
                    // way once they see it as Ctrl+H.
                    self.send_bytes(&[0x08]);
                    return;
                }
                _ => {}
            }
        }

        if event.logical_key == Key::Named(NamedKey::Escape) {
            if self.menu.take().is_some() {
                return;
            }
            if self.search_active {
                self.search_active = false;
                return;
            }
        }

        // --- Search box input (consumes keys instead of forwarding to the
        // shell while the search overlay is open) ---
        if self.search_active {
            match &event.logical_key {
                Key::Named(NamedKey::Backspace) => {
                    self.search.query.pop();
                    self.rerun_search();
                    return;
                }
                Key::Named(NamedKey::Enter) => {
                    if shift {
                        self.search.prev_match();
                    } else {
                        self.search.next_match();
                    }
                    return;
                }
                Key::Character(s) => {
                    self.search.query.push_str(s.as_str());
                    self.rerun_search();
                    return;
                }
                _ => return, // swallow everything else (e.g. arrows) while search is focused
            }
        }

        // --- Ordinary input forwarded to the shell ---
        let bytes = key_to_bytes(&event);
        if !bytes.is_empty() {
            self.send_bytes(&bytes);
        }
    }

    fn current_cols_rows(&self) -> (u16, u16) {
        let tab = &self.tabs.tabs[self.tabs.active_tab];
        let pane = &tab.panes[tab.active_pane];
        (pane.cols, pane.rows)
    }

    fn paste_clipboard(&mut self) {
        if let Ok(text) = self.clipboard.get_text() {
            self.send_bytes(text.as_bytes());
        }
    }

    fn send_bytes(&mut self, bytes: &[u8]) {
        let pane = self.tabs.active_tab_mut().active_pane_mut();
        let _ = pane.pty.write_input(bytes);
    }

    /// Run a context-menu entry. Each one routes to the same code path as
    /// its keyboard shortcut, so the two can't drift apart in behavior.
    fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Copy => self.copy_current_selection(),
            MenuAction::Paste => self.paste_clipboard(),
            MenuAction::SplitHorizontal | MenuAction::SplitVertical => {
                let dir = if action == MenuAction::SplitHorizontal {
                    SplitDir::Horizontal
                } else {
                    SplitDir::Vertical
                };
                let (cols, rows) = self.current_cols_rows();
                let tab = self.tabs.active_tab_mut();
                let _ = tab.split_active(dir, &self.config, cols, rows, &self.proxy);
                self.resize_panes();
            }
            MenuAction::NewTab => {
                let (cols, rows) = self.current_cols_rows();
                let _ = self.tabs.new_tab(cols, rows, &self.config, &self.proxy);
            }
            MenuAction::CloseTab => self.tabs.close_active_tab(),
            MenuAction::Search => {
                self.search_active = !self.search_active;
                if self.search_active {
                    self.rerun_search();
                }
            }
            MenuAction::Clear => {
                self.tabs.active_tab_mut().active_pane_mut().term.grid.erase_in_display(2);
            }
        }
    }

    /// Re-run scrollback search against the active pane's grid, e.g. after
    /// the query changes or the search box is (re-)opened.
    fn rerun_search(&mut self) {
        let pane = self.tabs.active_tab_mut().active_pane_mut();
        self.search.run(&pane.term.grid);
    }
}

/// App/window icon, baked into the binary so it works regardless of the
/// working directory the .exe is launched from.
fn load_icon() -> Option<winit::window::Icon> {
    let bytes = include_bytes!("../assets/icon_256.png");
    // Fully-qualified `::image` to disambiguate from our own `image` module
    // (src/image.rs, the Sixel/Kitty graphics protocol decoder).
    let img = ::image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    winit::window::Icon::from_rgba(img.into_raw(), w, h).ok()
}

fn key_to_bytes(event: &KeyEvent) -> Vec<u8> {
    match &event.logical_key {
        Key::Character(s) => s.as_bytes().to_vec(),
        Key::Named(NamedKey::Enter) => vec![b'\r'],
        Key::Named(NamedKey::Backspace) => vec![0x7f],
        Key::Named(NamedKey::Tab) => vec![b'\t'],
        Key::Named(NamedKey::Space) => vec![b' '],
        Key::Named(NamedKey::ArrowUp) => b"\x1b[A".to_vec(),
        Key::Named(NamedKey::ArrowDown) => b"\x1b[B".to_vec(),
        Key::Named(NamedKey::ArrowRight) => b"\x1b[C".to_vec(),
        Key::Named(NamedKey::ArrowLeft) => b"\x1b[D".to_vec(),
        Key::Named(NamedKey::Escape) => vec![0x1b],
        Key::Named(NamedKey::Delete) => b"\x1b[3~".to_vec(),
        Key::Named(NamedKey::Home) => b"\x1b[H".to_vec(),
        Key::Named(NamedKey::End) => b"\x1b[F".to_vec(),
        _ => Vec::new(),
    }
}
