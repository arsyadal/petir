# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Petir — a lightweight, GPU-accelerated terminal emulator for Windows, written in Rust. Built as a direct answer to two Alacritty gaps: no built-in split panes, and no plain `Ctrl+C`/`Ctrl+V` copy-paste. Uses ConPTY (via `portable-pty`), the `vte` crate for VT100/ANSI parsing (same crate Alacritty uses), and `wgpu` + `glyphon`/`cosmic-text` for GPU text rendering.

**Windows-only in practice.** This code was written in a Linux sandbox with no GPU/display, so it has only been validated with `cargo check`, never actually run. Building and running on real Windows is the first thing to verify when picking up work here.

## Commands

```powershell
cargo build --release      # first build is slow (wgpu, LTO); incremental after
cargo check                 # fast compile-only check, works cross-platform
.\target\release\petir.exe  # run
```

No test suite exists yet. There's no `cargo build` (debug) workflow documented — release is what's been used because `[profile.release]` in `Cargo.toml` has `lto = "fat"`, `panic = "abort"`, `codegen-units = 1`, which meaningfully changes runtime behavior vs. debug builds (in particular `panic = "abort"` — don't assume debug-build panic/unwind semantics apply).

## Architecture

**Event loop (`src/main.rs`)**: a single `App` struct owns everything — GPU state, tab/pane tree, clipboard, search state — and is driven by a `winit` event loop. Per-frame flow: `App::frame()` drains PTY output into each pane's `Term` (`Pane::pump`), flattens the active tab's split-tree layout into pixel rects, and hands `(grid, rect, is_active)` tuples to the renderer. Keybindings are hardcoded in `App::handle_key` (not config-driven yet) — check there first when adding shortcuts, mindful of the three-tier dispatch order: `Ctrl+Shift+<key>` app-level shortcuts, then `Ctrl+<key>` (which can fall through to sending the raw byte, e.g. smart-copy-or-SIGINT on `Ctrl+C`), then plain key-to-PTY-bytes.

**PTY layer (`src/pty.rs`)**: `portable-pty` auto-selects ConPTY on Windows. Each pane spawns a dedicated reader thread that blocks on PTY output and forwards bytes over an unbounded `crossbeam_channel`, keeping the GPU/event-loop thread free regardless of shell output volume — never read the PTY synchronously from the render path.

**Terminal state (`src/term/`)**: `Term` wraps a `vte::Parser` and implements `vte::Perform` (`mod.rs`) to mutate a `Grid` (`grid.rs`) of `Cell`s (`cell.rs`) — cursor movement, SGR color/attribute codes, erase, OSC window-title. The parser is swapped out of `self` via `mem::replace` during `advance()` to satisfy the borrow checker (needs `&mut self.parser` and `&mut self` as `Perform` simultaneously) — a pattern to preserve if touching that function. `Grid` stores `visible` (viewport) and `scrollback` as separate `VecDeque<Vec<Cell>>` for O(1) push/pop scrolling; resize is pad/truncate, not proper reflow (Alacritty-style reflow is unimplemented by design, noted as a known simplification).

**Panes/tabs (`src/pane.rs`)**: each `Tab` holds a `Vec<Pane>` plus a `Layout` binary tree (`Layout::Leaf(idx)` / `Layout::Split { dir, ratio, first, second }`) describing how panes tile the viewport. `Layout::compute_rects` (mirrored by `renderer::flatten_layout`) walks this tree to produce pixel rects — this same flattened list drives both rendering and mouse hit-testing, so keep the two in sync if the layout logic changes.

**Renderer (`src/renderer/mod.rs` + `quad.rs`)**: `GpuState` owns the wgpu device/surface, a custom instanced-quad renderer (`quad.rs`, for cell backgrounds and the cursor), and `glyphon` for text. Per frame it rebuilds a full `glyphon::Buffer` per pane from scratch (`grid_to_text` + shape) — there is no caching of unchanged rows yet; this is the known next perf target along with damage-tracking, per the README. Cell size (`cell_w`/`cell_h`) is derived once at startup by shaping a probe `"M"` glyph, not guessed from font size.

**Config (`src/config.rs`)**: TOML at `%APPDATA%\petir\petir.toml`, written with defaults on first run if missing, falls back silently to `Config::default()` on parse errors. All `*Config` structs use `#[serde(default)]` so partial user TOMLs work. Config governs font/window/clipboard/scroll/colors/shell — but not keybindings (see `main.rs` note above).

**Not yet wired up** (modules exist, logic works in isolation, but not connected to the main VT100 pipeline):
- `src/search.rs` — scrollback search; `search_active` toggle state already exists in `App` (`Ctrl+Shift+F`), but there's no overlay UI or actual search-and-highlight wired in yet.
- `src/image.rs` — Sixel and Kitty graphics protocol decoders (raw RGBA/RGB only, no PNG); not hooked into `Perform::hook/put/unhook` (DCS, for Sixel) or APC (for Kitty) in `src/term/mod.rs`.

When extending the VT100 handling in `src/term/mod.rs`, note `csi_dispatch` and the `sgr` helper only implement a subset of codes (cursor movement, basic/256/truecolor SGR, erase, no scroll-region/DECSTBM, no alternate screen buffer, no mouse reporting) — check what's missing before assuming a given escape sequence is handled.
