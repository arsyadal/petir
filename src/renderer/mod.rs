pub mod quad;

use crate::menu::ContextMenu;
use crate::pane::Layout;
use crate::search::SearchState;
use crate::term::cell::{srgb_to_linear, CellFlags};
use crate::term::grid::Grid;
use glyphon::{
    Attrs, Buffer as TextBuffer, Color as GlyphonColor, Family, FontSystem, Metrics, Resolution,
    Shaping, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
};
use quad::{QuadInstance, QuadRenderer};
use std::sync::Arc;
use winit::window::Window;

const SEARCH_BOX_W: f32 = 340.0;
const SEARCH_BOX_H: f32 = 30.0;

pub struct GpuState {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: winit::dpi::PhysicalSize<u32>,

    quad_renderer: QuadRenderer,
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    text_renderer: TextRenderer,

    pub cell_w: f32,
    pub cell_h: f32,
    pub font_size: f32,
}

impl GpuState {
    pub async fn new(window: Arc<Window>, font_size: f32, vsync: bool) -> anyhow::Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            // DX12 first on Windows: lower driver overhead than the GL/Vulkan
            // fallback path for most users, which is the whole point of
            // targeting wgpu instead of raw OpenGL like Alacritty does.
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| anyhow::anyhow!("tidak ada GPU adapter yang cocok ditemukan"))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("rterm-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: if vsync {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 1, // minimize input-to-photon latency
        };
        surface.configure(&device, &config);

        let quad_renderer = QuadRenderer::new(&device, format);

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let mut atlas = TextAtlas::new(&device, &queue, format);
        let text_renderer = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        // Measure monospace cell size from the shaped 'M' advance so the
        // grid lines up with actual glyph metrics instead of a guess.
        let metrics = Metrics::new(font_size, font_size * 1.2);
        let mut probe = TextBuffer::new(&mut font_system, metrics);
        probe.set_size(&mut font_system, 200.0, 200.0);
        probe.set_text(&mut font_system, "M", Attrs::new().family(Family::Monospace), Shaping::Advanced);
        probe.shape_until_scroll(&mut font_system);
        let cell_w = probe
            .layout_runs()
            .next()
            .and_then(|run| run.glyphs.first())
            .map(|g| g.w)
            .unwrap_or(font_size * 0.6);
        let cell_h = font_size * 1.2;

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            quad_renderer,
            font_system,
            swash_cache,
            atlas,
            text_renderer,
            cell_w,
            cell_h,
            font_size,
        })
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.size = size;
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn cols_rows_for(&self, px_w: f32, px_h: f32) -> (u16, u16) {
        let cols = (px_w / self.cell_w).floor().max(1.0) as u16;
        let rows = (px_h / self.cell_h).floor().max(1.0) as u16;
        (cols, rows)
    }

    /// Render every pane in `layout` inside its rect, plus tab bar chrome.
    /// `panes` is a slice of (grid, rect_px, is_active) tuples pre-resolved
    /// by the caller from the split-tree layout.
    pub fn render_frame(
        &mut self,
        panes: &[(&Grid, [f32; 4], bool)],
        padding: (f32, f32),
        ligatures: bool,
        search: Option<(&SearchState, usize)>,
        menu: Option<&ContextMenu>,
    ) -> anyhow::Result<()> {
        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        // --- Background quads (cell bg + cursor + selection) ---
        let mut instances: Vec<QuadInstance> = Vec::new();
        for (grid, rect, is_active) in panes {
            self.push_bg_instances(grid, *rect, padding, &mut instances);
            if *is_active && grid.cursor_visible {
                self.push_cursor_instance(grid, *rect, padding, &mut instances);
            }
        }

        // --- Search match highlights + search box chrome, active pane only ---
        let search_box = if let Some((state, scrollback_len)) = search {
            let active_rect = panes.iter().find(|(_, _, is_active)| *is_active).map(|(_, r, _)| *r);
            if let Some(rect) = active_rect {
                self.push_search_match_instances(state, scrollback_len, rect, padding, &mut instances);
            }
            Some(self.push_search_box_instances(state, &mut instances))
        } else {
            None
        };

        // --- Context menu chrome, drawn last so it sits above the grid ---
        let menu_text = menu.map(|m| self.push_menu_instances(m, &mut instances));

        // --- Text buffers, one per pane, built fresh each frame ---
        let shaping = if ligatures { Shaping::Advanced } else { Shaping::Basic };
        let mut buffers: Vec<TextBuffer> = Vec::with_capacity(panes.len() + 1);
        for (grid, rect, _) in panes {
            let mut buf = TextBuffer::new(&mut self.font_system, Metrics::new(self.font_size, self.cell_h));
            buf.set_size(&mut self.font_system, rect[2], rect[3]);
            let text = grid_to_text(grid);
            buf.set_text(&mut self.font_system, &text, Attrs::new().family(Family::Monospace), shaping);
            buf.shape_until_scroll(&mut self.font_system);
            buffers.push(buf);
        }
        let mut search_buf_idx = None;
        if let Some((_, _, label)) = &search_box {
            let mut buf = TextBuffer::new(&mut self.font_system, Metrics::new(self.font_size, self.cell_h));
            buf.set_size(&mut self.font_system, SEARCH_BOX_W, SEARCH_BOX_H);
            buf.set_text(&mut self.font_system, label, Attrs::new().family(Family::Monospace), Shaping::Basic);
            buf.shape_until_scroll(&mut self.font_system);
            search_buf_idx = Some(buffers.len());
            buffers.push(buf);
        }
        // One buffer per menu row: rows have different heights (items vs
        // separators), so a single multi-line buffer would drift out of
        // alignment with the highlight quads.
        let menu_buffers_start = buffers.len();
        if let Some(rows) = &menu_text {
            for (_, text) in rows {
                let mut buf = TextBuffer::new(
                    &mut self.font_system,
                    Metrics::new(self.font_size, ContextMenu::ITEM_H),
                );
                buf.set_size(&mut self.font_system, ContextMenu::WIDTH, ContextMenu::ITEM_H);
                buf.set_text(&mut self.font_system, text, Attrs::new().family(Family::Monospace), Shaping::Basic);
                buf.shape_until_scroll(&mut self.font_system);
                buffers.push(buf);
            }
        }

        let mut text_areas: Vec<TextArea> = panes
            .iter()
            .zip(buffers.iter())
            .map(|((_, rect, _), buf)| TextArea {
                buffer: buf,
                left: rect[0] + padding.0,
                top: rect[1] + padding.1,
                scale: 1.0,
                bounds: TextBounds {
                    left: rect[0] as i32,
                    top: rect[1] as i32,
                    right: (rect[0] + rect[2]) as i32,
                    bottom: (rect[1] + rect[3]) as i32,
                },
                default_color: GlyphonColor::rgb(0xd8, 0xd8, 0xd8),
            })
            .collect();
        if let (Some((x, y, _)), Some(idx)) = (&search_box, search_buf_idx) {
            let (x, y) = (*x, *y);
            text_areas.push(TextArea {
                buffer: &buffers[idx],
                left: x + 8.0,
                top: y + 6.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: x as i32,
                    top: y as i32,
                    right: (x + SEARCH_BOX_W) as i32,
                    bottom: (y + SEARCH_BOX_H) as i32,
                },
                default_color: GlyphonColor::rgb(0xff, 0xff, 0xff),
            });
        }

        if let (Some(m), Some(rows)) = (menu, &menu_text) {
            let offsets = m.row_offsets();
            let menu_bottom = (m.y + m.height()) as i32;
            for (buf_i, (item_idx, _)) in rows.iter().enumerate() {
                let top = m.y + offsets[*item_idx];
                text_areas.push(TextArea {
                    buffer: &buffers[menu_buffers_start + buf_i],
                    left: m.x + ContextMenu::PAD,
                    top: top + (ContextMenu::ITEM_H - self.cell_h) * 0.5,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: m.x as i32,
                        top: m.y as i32,
                        right: (m.x + ContextMenu::WIDTH) as i32,
                        bottom: menu_bottom,
                    },
                    default_color: GlyphonColor::rgb(0xe8, 0xe8, 0xe8),
                });
            }
        }

        let resolution = Resolution { width: self.config.width, height: self.config.height };
        self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            resolution,
            text_areas,
            &mut self.swash_cache,
        )?;

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: srgb_to_linear(0x1e) as f64,
                            g: srgb_to_linear(0x1e) as f64,
                            b: srgb_to_linear(0x1e) as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            self.quad_renderer.render(
                &self.device,
                &self.queue,
                &mut pass,
                self.config.width as f32,
                self.config.height as f32,
                &instances,
            );

            self.text_renderer.render(&self.atlas, &mut pass)?;
        }

        self.queue.submit(Some(encoder.finish()));
        output.present();
        self.atlas.trim();
        Ok(())
    }

    fn push_bg_instances(&self, grid: &Grid, rect: [f32; 4], padding: (f32, f32), out: &mut Vec<QuadInstance>) {
        for (row_idx, row) in grid.visible.iter().enumerate() {
            for (col_idx, cell) in row.iter().enumerate() {
                if cell.bg == grid.bg && !cell.flags.contains(CellFlags::INVERSE) {
                    continue; // skip default-bg cells; the pane clear color already covers them
                }
                let (fg, bg) = if cell.flags.contains(CellFlags::INVERSE) {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };
                let _ = fg;
                out.push(QuadInstance {
                    pos: [
                        rect[0] + padding.0 + col_idx as f32 * self.cell_w,
                        rect[1] + padding.1 + row_idx as f32 * self.cell_h,
                    ],
                    size: [self.cell_w, self.cell_h],
                    color: bg.to_wgpu(),
                });
            }
        }
    }

    /// Highlight rects for scrollback-search matches on the active pane.
    /// Only matches that fall within the visible viewport can be drawn —
    /// scrollback itself has no rendering path yet (see README), so matches
    /// with `row < scrollback_len` are silently skipped for now.
    fn push_search_match_instances(
        &self,
        state: &SearchState,
        scrollback_len: usize,
        rect: [f32; 4],
        padding: (f32, f32),
        out: &mut Vec<QuadInstance>,
    ) {
        if state.query.is_empty() {
            return;
        }
        for (i, m) in state.matches.iter().enumerate() {
            if m.row < scrollback_len {
                continue;
            }
            let visible_row = m.row - scrollback_len;
            let color = if i == state.current {
                // current match: brighter orange
                [srgb_to_linear(0xff), srgb_to_linear(0x8c), 0.0, 0.55]
            } else {
                // other matches: dim yellow
                [srgb_to_linear(0xff), srgb_to_linear(0xd9), srgb_to_linear(0x33), 0.35]
            };
            let width = m.col_end.saturating_sub(m.col_start).max(1) as f32 * self.cell_w;
            out.push(QuadInstance {
                pos: [
                    rect[0] + padding.0 + m.col_start as f32 * self.cell_w,
                    rect[1] + padding.1 + visible_row as f32 * self.cell_h,
                ],
                size: [width, self.cell_h],
                color,
            });
        }
    }

    /// Border + background chrome for the search box (top-right corner of
    /// the window). Returns its (x, y, label-text) so the caller can lay out
    /// a matching glyphon text area on top.
    fn push_search_box_instances(&self, state: &SearchState, out: &mut Vec<QuadInstance>) -> (f32, f32, String) {
        let x = self.config.width as f32 - SEARCH_BOX_W - 12.0;
        let y = 12.0;
        out.push(QuadInstance {
            pos: [x - 1.5, y - 1.5],
            size: [SEARCH_BOX_W + 3.0, SEARCH_BOX_H + 3.0],
            color: [srgb_to_linear(0xff), srgb_to_linear(0x8c), 0.0, 0.9],
        });
        out.push(QuadInstance {
            pos: [x, y],
            size: [SEARCH_BOX_W, SEARCH_BOX_H],
            color: [srgb_to_linear(0x14), srgb_to_linear(0x14), srgb_to_linear(0x14), 0.95],
        });
        let label = if state.query.is_empty() {
            "Search: (type to search)".to_string()
        } else if state.matches.is_empty() {
            format!("Search: {}  no matches", state.query)
        } else {
            format!("Search: {}  {}/{}", state.query, state.current + 1, state.matches.len())
        };
        (x, y, label)
    }

    /// Panel, hover highlight and separator hairlines for the context menu.
    /// Returns `(item_index, line_text)` for each selectable row, with the
    /// shortcut space-padded to sit flush against the right edge — the font
    /// is monospace, so column math is enough to right-align it.
    fn push_menu_instances(&self, m: &ContextMenu, out: &mut Vec<QuadInstance>) -> Vec<(usize, String)> {
        let h = m.height();

        // Border, then panel inset by 1px so the border reads as a hairline.
        out.push(QuadInstance {
            pos: [m.x - 1.0, m.y - 1.0],
            size: [ContextMenu::WIDTH + 2.0, h + 2.0],
            color: [srgb_to_linear(0x4a), srgb_to_linear(0x4a), srgb_to_linear(0x4a), 1.0],
        });
        out.push(QuadInstance {
            pos: [m.x, m.y],
            size: [ContextMenu::WIDTH, h],
            color: [srgb_to_linear(0x24), srgb_to_linear(0x24), srgb_to_linear(0x24), 1.0],
        });

        let offsets = m.row_offsets();
        let inner_w = ContextMenu::WIDTH - ContextMenu::PAD * 2.0;
        let cols = (inner_w / self.cell_w).floor().max(1.0) as usize;

        let mut rows = Vec::new();
        for (idx, item) in m.items.iter().enumerate() {
            let top = m.y + offsets[idx];
            if item.action.is_none() {
                out.push(QuadInstance {
                    pos: [m.x + ContextMenu::PAD, top + ContextMenu::SEPARATOR_H * 0.5],
                    size: [inner_w, 1.0],
                    color: [srgb_to_linear(0x40), srgb_to_linear(0x40), srgb_to_linear(0x40), 1.0],
                });
                continue;
            }

            if m.hovered == Some(idx) {
                out.push(QuadInstance {
                    pos: [m.x, top],
                    size: [ContextMenu::WIDTH, ContextMenu::ITEM_H],
                    color: [srgb_to_linear(0xff), srgb_to_linear(0x8c), 0.0, 0.28],
                });
            }

            let used = item.label.chars().count() + item.shortcut.chars().count();
            let gap = cols.saturating_sub(used).max(1);
            rows.push((idx, format!("{}{}{}", item.label, " ".repeat(gap), item.shortcut)));
        }
        rows
    }

    fn push_cursor_instance(&self, grid: &Grid, rect: [f32; 4], padding: (f32, f32), out: &mut Vec<QuadInstance>) {
        out.push(QuadInstance {
            pos: [
                rect[0] + padding.0 + grid.cursor.col as f32 * self.cell_w,
                rect[1] + padding.1 + grid.cursor.row as f32 * self.cell_h,
            ],
            size: [self.cell_w * 0.15, self.cell_h], // thin I-beam bar; block/underline are config follow-ups
            color: [1.0, 1.0, 1.0, 0.9],
        });
    }
}

fn grid_to_text(grid: &Grid) -> String {
    let mut out = String::with_capacity(grid.cols * grid.rows + grid.rows);
    for (i, row) in grid.visible.iter().enumerate() {
        for cell in row {
            out.push(cell.c);
        }
        if i + 1 != grid.visible.len() {
            out.push('\n');
        }
    }
    out
}

/// Flatten a tab's split-tree layout into (pane_index, pixel_rect) pairs,
/// used both for rendering and for hit-testing mouse clicks against panes.
pub fn flatten_layout(layout: &Layout, x: f32, y: f32, w: f32, h: f32) -> Vec<(usize, [f32; 4])> {
    let mut out = Vec::new();
    layout.compute_rects(x, y, w, h, &mut out);
    out
}
