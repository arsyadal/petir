fn main() {
    // Embeds assets/icon.ico as the .exe's icon (taskbar, File Explorer,
    // Alt-Tab). Only runs when building on/for Windows — a no-op elsewhere.
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=gagal embed icon.ico ke exe: {e}");
        }
    }
}
