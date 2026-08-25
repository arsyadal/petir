# Petir ⚡

*[Bahasa Indonesia](README.id.md)*

A lightweight, blazing-fast terminal emulator for Windows. Written in Rust, GPU-rendered (wgpu → DirectX 12/Vulkan), backed by real ConPTY via `portable-pty`.

Built as an answer to two common Alacritty complaints:

- **Built-in split panes** — `Ctrl+Shift+E` (split horizontal) / `Ctrl+Shift+D` (split vertical), `Ctrl+Tab` to switch panes.
- **Normal copy-paste** — `Ctrl+C` copies when there's a selection (falls through to SIGINT as usual when there isn't), `Ctrl+V` pastes directly. No `Ctrl+Shift+C/V` needed.

## Project status (important, read first)

This is v0.1 — the core architecture is solid and **compiles clean** (`cargo check` passes with no errors), but it's a big project that's reasonably built in stages. What's fully working:

- PTY + ConPTY (spawn shell, resize, read/write)
- ANSI/VT100 parser (using the `vte` crate, same as Alacritty) — cursor movement, 16/256/truecolor, erase line/display, etc.
- Full GPU rendering: per-cell background via instanced quads (wgpu), text via `glyphon`/`cosmic-text` with shaping (ligatures on by default)
- TOML config with hot-load on first run (`%APPDATA%\petir\petir.toml`)
- Smart clipboard (`Ctrl+C`/`Ctrl+V` as described above)
- Tabs (`Ctrl+Shift+T` new, `Ctrl+Shift+W` close) + split panes (tree-based, splittable repeatedly)
- Icon & branding (lightning bolt, see `assets/logo.svg`)

What has a module (logic tested in isolation) but is **not yet wired up** to the main VT100 pipeline — the clearest follow-up work for anyone (including Claude Code in your local session) continuing this:

- `src/search.rs` — scrollback search, just needs wiring to the `Ctrl+Shift+F` toggle that already has state (`search_active`) in `main.rs`, plus a search-box overlay to render.
- `src/image.rs` — Sixel & Kitty graphics protocol decoder (raw RGBA/RGB, no PNG yet). Not hooked into `Perform::hook/put/unhook` (DCS, for Sixel) or APC (for Kitty) in `src/term/mod.rs`.

Why: this code was written by Claude in a Linux cloud sandbox, with no Windows GPU/display for end-to-end build & test. Everything marked "fully working" above passes `cargo check`, but has never actually been run and seen on a real Windows screen — that's the first step you need to take.

## Building on Windows

1. Install Rust: https://rustup.rs (pick the MSVC toolchain)
2. Open this folder in a terminal / the **Code** tab in Claude Desktop
3. Build:

   ```powershell
   cargo build --release
   ```

4. Run:

   ```powershell
   .\target\release\petir.exe
   ```

The first build will be slow (compiling wgpu, etc.) — a few minutes. Subsequent builds are much faster (incremental).

## Keybindings

| Action | Shortcut |
|---|---|
| Copy (if there's a selection) / SIGINT (otherwise) | `Ctrl+C` |
| Paste | `Ctrl+V` |
| Delete previous word | `Ctrl+Backspace` |
| Force copy (Alacritty-style fallback) | `Ctrl+Shift+C` |
| Force paste | `Ctrl+Shift+V` |
| New tab | `Ctrl+Shift+T` |
| Close tab | `Ctrl+Shift+W` |
| Split horizontal | `Ctrl+Shift+E` |
| Split vertical | `Ctrl+Shift+D` |
| Switch pane | `Ctrl+Tab` |
| Clear screen | `Ctrl+L` |
| Toggle search (no overlay UI yet) | `Ctrl+Shift+F` |

Everything can be changed via `%APPDATA%\petir\petir.toml` (created automatically on first run) — except keybindings, which are currently hardcoded in `src/main.rs::handle_key` (configurable keybindings is a natural follow-up).

## Benchmark vs Alacritty

No numbers yet — this needs to be measured on real Windows. Suggested approach:

- **Startup time**: `hyperfine "petir.exe -e exit" "alacritty.exe -e exit"` (needs a `-e`/one-shot-exit flag added if missing, or measure manually with a stopwatch/Process Monitor for time-to-first-window)
- **Render throughput** (how fast it can display large amounts of output): `vtebench` (https://github.com/alacritty/vtebench) — run the `scrolling`, `dense_cells`, etc. scenarios on both terminals, compare times.
- **Input latency**: tools like `typometer`, or measure manually with a 240fps camera (the method Alacritty itself uses for its performance claims).

If early results show Petir isn't yet faster than Alacritty in some scenarios, that's expected for v0.1 — Alacritty has years of optimization behind it. The most likely areas needing further tuning: rebuilding the `TextBuffer` every frame in `renderer/mod.rs` (currently every line is reshaped every frame; caching unchanged lines is the next big optimization), and damage-tracking (only re-rendering the region that changed, not the whole grid).

## Code structure

```
src/
  main.rs        - winit event loop, input handling, keybindings, app state
  config.rs       - TOML config + defaults
  pty.rs          - spawn shell via ConPTY (portable-pty)
  pane.rs         - tabs + split-pane tree
  selection.rs    - mouse text selection + smart clipboard
  search.rs       - scrollback search (not wired to UI yet)
  image.rs        - Sixel & Kitty graphics protocol decoder (not wired up yet)
  term/
    mod.rs        - VT100/ANSI parser (vte::Perform) → grid
    grid.rs        - cell grid + scrollback ring buffer
    cell.rs        - Cell/Rgb/CellFlags structs
  renderer/
    mod.rs        - wgpu setup, glyphon text, render loop
    quad.rs        - instanced solid-color quad renderer (background/cursor)
assets/
  logo.svg        - source vector logo
  icon.ico        - Windows app icon (embedded into the .exe via build.rs)
```

## License

MIT — use, modify, republish freely.
