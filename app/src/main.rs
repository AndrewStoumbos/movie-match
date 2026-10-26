//! Movie Match desktop app: import your Letterboxd ratings, then see how much you're
//! likely to enjoy upcoming movies, and why.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod jobs;
mod store;

use eframe::egui;

fn main() -> eframe::Result {
    // Lets developers keep using a `.env` file; the app's own setting takes precedence.
    dotenvy::dotenv().ok();
    // A ratings file passed on the command line (or via "Open with") is imported on start.
    let import = std::env::args_os().nth(1).map(std::path::PathBuf::from);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Movie Match")
        .with_inner_size([1040.0, 760.0])
        .with_min_inner_size([680.0, 480.0]);
    // Title bar and taskbar icon. Regenerate with assets/make-icon.ps1.
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Movie Match",
        options,
        Box::new(|cc| Ok(Box::new(app::MovieMatchApp::new(cc, import)?))),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_icon_decodes() {
        let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
            .expect("assets/icon.png is a valid PNG");
        assert_eq!((icon.width, icon.height), (256, 256));
    }
}
