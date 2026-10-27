mod app;

fn main() {
    eframe::run_native(
        "Bilibili Anime Downloader",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(app::App))),
    )
    .ok();
}
