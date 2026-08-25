//! PTY handling. `portable-pty` picks ConPTY on Windows automatically —
//! that's what gives us real ANSI/VT100 behavior for cmd/PowerShell/WSL
//! without shelling out to winpty or writing our own console host.

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct PtyHandle {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    pub output_rx: Receiver<Vec<u8>>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
}

fn detect_shell(configured: &str) -> String {
    if !configured.is_empty() {
        return configured.to_string();
    }
    #[cfg(windows)]
    {
        // Prefer PowerShell 7 (pwsh) > Windows PowerShell > cmd.
        for candidate in ["pwsh.exe", "powershell.exe"] {
            if which_windows(candidate) {
                return candidate.to_string();
            }
        }
        "cmd.exe".to_string()
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
}

#[cfg(windows)]
fn which_windows(exe: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|p| p.join(exe).exists())
        })
        .unwrap_or(false)
}

impl PtyHandle {
    pub fn spawn(cols: u16, rows: u16, shell: &str) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("gagal buka PTY (ConPTY)")?;

        let shell = detect_shell(shell);
        let cmd = CommandBuilder::new(shell);

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("gagal spawn shell")?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("gagal clone PTY reader")?;
        let writer = pair.master.take_writer().context("gagal ambil PTY writer")?;

        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = crossbeam_channel::unbounded();

        // Reader thread: blocks on PTY output and forwards to the render
        // thread via a channel, keeping the GPU/event loop thread free to
        // stay at 60/120/144Hz regardless of shell output volume.
        thread::Builder::new()
            .name("rterm-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 32 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("gagal spawn thread reader PTY")?;

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            master: Arc::new(Mutex::new(pair.master)),
            output_rx: rx,
            child: Arc::new(Mutex::new(child)),
        })
    }

    pub fn write_input(&self, bytes: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master.lock().unwrap().resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    pub fn is_alive(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(None))
    }
}
