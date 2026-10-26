//! Embeds the app icon in movie-match.exe on Windows, so Explorer, shortcuts and the
//! taskbar show it. Regenerate the icon with assets/make-icon.ps1.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icon.ico");
        resource.compile()?;
    }
    Ok(())
}
