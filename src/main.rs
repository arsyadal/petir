mod config;
mod image;
mod pane;
mod pty;
mod renderer;
mod search;
mod selection;
mod term;

use config::Config;
use pane::{SplitDir, TabBar};
use renderer::GpuState;
use search::SearchState;
use selection::{ClipboardManager, GridPos, Selection};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::{
    dpi::PhysicalPosition,
    event::{ElementState, KeyEvent, MouseButton, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
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
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let config = Config::load();

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let window_icon = load_icon();
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Petir")
            .with_inner_size(winit::dpi::LogicalSize::new(1000.0, 650.0))
            .with_window_icon(window_icon)
            .build(&event_loop)?,
    );

    let gpu = pollster::block_on(GpuState::new(window.clone(), config.font.size))?;
    let (cols, rows) = gpu.cols_rows_for(
        gpu.size.width as f32 - config.window.padding_x as f32 * 2.0,
        gpu.size.height as f32 - config.window.padding_y as f32 * 2.0,
    );
    let tabs = TabBar::new(cols, rows, &config)?;

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
    };

    let mut cursor_pos = PhysicalPosition::new(0.0, 0.0);

    event_loop.run(move |event, elwt| match event {
        winit::event::Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
            WindowEvent::CloseRequested => elwt.exit(),
            WindowEvent::Resized(size) => {
                app.gpu.resize(size);
                app.resize_panes();
            }
            WindowEvent::ModifiersChanged(mods) => app.modifiers = mods.state(),
            WindowEvent::CursorMoved { position, .. } => {
                cursor_pos = position;
                if app.mouse_down {
                    app.extend_selection(cursor_pos);
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => match state {
                ElementState::Pressed => {
                    app.mouse_down = true;
                    app.begin_selection(cursor_pos);
                }
                ElementState::Released => {
                    app.mouse_down = false;
                    if app.config.clipboard.copy_on_select {
                        app.copy_current_selection();
                    }
                }
            },
            WindowEvent::KeyboardInput { event, .. } => app.handle_key(event, elwt),
            WindowEvent::RedrawRequested => {
                app.frame();
                window.request_redraw();
            }
            _ => {}
        },
        winit::event::Event::AboutToWait => {
            // Cap redraw rate if configured; otherwise request every loop
            // iteration for the lowest possible input-to-photon latency.
            let target = app.config.window.max_fps;
            if target > 0 {
                let frame_time = Duration::from_secs_f64(1.0 / target as f64);
                if app.last_frame.elapsed() >= frame_time {
                    app.last_frame = Instant::now();
                    window.request_redraw();
                }
            } else {
                window.request_redraw();
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

    fn frame(&mut self) {
        let tab = self.tabs.active_tab_mut();
        for pane in tab.panes.iter_mut() {
            pane.pump();
        }

        let pad = (self.config.window.padding_x as f32, self.config.window.padding_y as f32);
        let w = self.gpu.size.width as f32 - pad.0 * 2.0;
        let h = self.gpu.size.height as f32 - pad.1 * 2.0;
        let rects = renderer::flatten_layout(&tab.layout, 0.0, 0.0, w, h);

        let panes: Vec<(&term::grid::Grid, [f32; 4], bool)> = rects
            .iter()
            .map(|(idx, rect)| (&tab.panes[*idx].term.grid, *rect, *idx == tab.active_pane))
            .collect();

        if let Err(e) = self.gpu.render_frame(&panes, pad, self.config.font.ligatures) {
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

    fn handle_key(&mut self, event: KeyEvent, elwt: &winit::event_loop::EventLoopWindowTarget<()>) {
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
                    let _ = self.tabs.new_tab(cols, rows, &self.config);
                    return;
                }
                Key::Character(s) if s.as_str() == "W" || s.as_str() == "w" => {
                    self.tabs.close_active_tab();
                    return;
                }
                Key::Character(s) if s.as_str() == "E" || s.as_str() == "e" => {
                    let (cols, rows) = self.current_cols_rows();
                    let tab = self.tabs.active_tab_mut();
                    let _ = tab.split_active(SplitDir::Horizontal, &self.config, cols, rows);
                    self.resize_panes();
                    return;
                }
                Key::Character(s) if s.as_str() == "D" || s.as_str() == "d" => {
                    let (cols, rows) = self.current_cols_rows();
                    let tab = self.tabs.active_tab_mut();
                    let _ = tab.split_active(SplitDir::Vertical, &self.config, cols, rows);
                    self.resize_panes();
                    return;
                }
                Key::Character(s) if s.as_str() == "F" || s.as_str() == "f" => {
                    self.search_active = !self.search_active;
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
                _ => {}
            }
        }

        if event.logical_key == Key::Named(NamedKey::Escape) {
            elwt.set_control_flow(ControlFlow::Poll); // no-op, keeps app responsive
            if self.search_active {
                self.search_active = false;
                return;
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
