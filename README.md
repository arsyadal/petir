# Petir ⚡

Terminal emulator ringan & super cepat untuk Windows. Ditulis di Rust, render lewat GPU (wgpu → DirectX 12/Vulkan), pakai ConPTY asli lewat `portable-pty`.

Dibangun sebagai jawaban atas dua keluhan umum soal Alacritty:

- **Split terminal bawaan** — `Ctrl+Shift+E` (split horizontal) / `Ctrl+Shift+D` (split vertical), `Ctrl+Tab` pindah pane.
- **Copy-paste normal** — `Ctrl+C` men-copy kalau ada teks yang diseleksi (kalau tidak ada seleksi, tetap kirim SIGINT seperti biasa), `Ctrl+V` langsung paste. Tidak perlu `Ctrl+Shift+C/V`.

## Status proyek (penting, baca dulu)

Ini v0.1 — arsitektur inti sudah solid dan **compile bersih** (`cargo check` lolos tanpa error), tapi ini proyek besar yang wajar dikerjakan bertahap. Yang sudah jalan penuh:

- PTY + ConPTY (spawn shell, resize, read/write)
- Parser ANSI/VT100 (pakai crate `vte`, sama seperti Alacritty) — cursor movement, warna 16/256/truecolor, erase line/display, dst.
- Render GPU penuh: background per-cell via instanced quad (wgpu), teks via `glyphon`/`cosmic-text` dengan shaping (ligatures on by default)
- Config TOML dengan hot-load pertama kali jalan (`%APPDATA%\petir\petir.toml`)
- Smart clipboard (`Ctrl+C`/`Ctrl+V` seperti dijelaskan di atas)
- Tabs (`Ctrl+Shift+T` baru, `Ctrl+Shift+W` tutup) + split pane (tree-based, bisa split berulang)
- Icon & branding (lightning bolt, lihat `assets/logo.svg`)

Yang sudah ada modulnya (logic teruji secara terisolasi) tapi **belum disambungkan** ke pipeline VT100 utama — ini pekerjaan lanjutan yang paling jelas buat siapa pun (termasuk Claude Code di sesi lokal kamu) yang lanjutin:

- `src/search.rs` — cari di scrollback, tinggal disambung ke toggle `Ctrl+Shift+F` yang sudah ada state-nya (`search_active`) di `main.rs`, plus render overlay kotak pencarian.
- `src/image.rs` — decoder Sixel & Kitty graphics protocol (raw RGBA/RGB, PNG belum). Belum di-hook ke `Perform::hook/put/unhook` (DCS, buat Sixel) dan APC (buat Kitty) di `src/term/mod.rs`.

Kenapa belum: saya (Claude) nulis kode ini di sandbox Linux cloud, tanpa GPU/display Windows untuk build & test end-to-end. Semua yang di atas "sudah jalan" itu lolos `cargo check`, tapi belum pernah benar-benar di-run & dilihat di layar Windows asli — itu langkah pertama yang perlu kamu lakukan.

## Build di Windows

1. Install Rust: https://rustup.rs (pilih toolchain MSVC)
2. Buka folder ini di terminal / tab **Code** di Claude Desktop
3. Build:

   ```powershell
   cargo build --release
   ```

4. Jalankan:

   ```powershell
   .\target\release\petir.exe
   ```

Build pertama akan lama (compile wgpu, dsb.) — beberapa menit. Build berikutnya jauh lebih cepat (incremental).

## Keybinding

| Aksi | Shortcut |
|---|---|
| Copy (kalau ada seleksi) / SIGINT (kalau tidak) | `Ctrl+C` |
| Paste | `Ctrl+V` |
| Copy paksa (fallback ala Alacritty) | `Ctrl+Shift+C` |
| Paste paksa | `Ctrl+Shift+V` |
| Tab baru | `Ctrl+Shift+T` |
| Tutup tab | `Ctrl+Shift+W` |
| Split horizontal | `Ctrl+Shift+E` |
| Split vertical | `Ctrl+Shift+D` |
| Pindah pane | `Ctrl+Tab` |
| Clear screen | `Ctrl+L` |
| Toggle search (belum ada UI overlay) | `Ctrl+Shift+F` |

Semua bisa diubah lewat `%APPDATA%\petir\petir.toml` (dibuat otomatis saat pertama kali run) — kecuali keybinding, yang untuk sekarang masih hardcoded di `src/main.rs::handle_key` (config-able keybinding adalah follow-up alami).

## Benchmark vs Alacritty

Belum ada angka — ini harus diukur di Windows asli. Cara yang disarankan:

- **Startup time**: `hyperfine "petir.exe -e exit" "alacritty.exe -e exit"` (perlu tambah flag `-e`/one-shot-exit kalau belum ada, atau ukur manual dengan stopwatch/Process Monitor untuk waktu window pertama muncul)
- **Throughput render** (seberapa cepat nampilin output banyak): `vtebench` (https://github.com/alacritty/vtebench) — jalankan skenario `scrolling`, `dense_cells`, dsb. di kedua terminal, bandingkan waktu.
- **Input latency**: ada alat seperti `typometer` atau ukur manual dengan kamera 240fps (cara yang Alacritty sendiri pakai untuk klaim performanya).

Kalau hasil awal ternyata Petir belum lebih cepat dari Alacritty di beberapa skenario, itu ekspektasi wajar untuk v0.1 — Alacritty sudah dioptimasi bertahun-tahun. Area yang paling mungkin butuh tuning lanjutan: rebuild `TextBuffer` per-frame di `renderer/mod.rs` (saat ini semua baris di-reshape ulang tiap frame; caching baris yang tidak berubah adalah optimasi besar berikutnya), dan damage-tracking (cuma re-render region yang berubah, bukan seluruh grid).

## Struktur kode

```
src/
  main.rs        - event loop winit, input handling, keybinding, app state
  config.rs       - TOML config + default
  pty.rs          - spawn shell via ConPTY (portable-pty)
  pane.rs         - tab + split-pane tree
  selection.rs    - seleksi teks mouse + smart clipboard
  search.rs       - scrollback search (belum disambung ke UI)
  image.rs        - decoder Sixel & Kitty graphics protocol (belum disambung)
  term/
    mod.rs        - VT100/ANSI parser (vte::Perform) → grid
    grid.rs        - grid sel + scrollback ring buffer
    cell.rs        - struct Cell/Rgb/CellFlags
  renderer/
    mod.rs        - setup wgpu, glyphon text, render loop
    quad.rs        - instanced solid-color quad renderer (background/cursor)
assets/
  logo.svg        - source vector logo
  icon.ico        - Windows app icon (di-embed ke .exe lewat build.rs)
```

## Lisensi

MIT — pakai, ubah, republish bebas.
