//! mdwriter: a desktop editor for a markdown blog kept in a GitHub repository.
//! The UI is Rune (a GPU-rendered Rust UI framework); git and GitHub work is in `backend` and `sync`.

mod app;
mod backend;
mod demo;
mod frontmatter;
mod keymap;
mod linediff;
mod md;
mod pages;
mod sync;
mod ui;

use rune::{AppSpec, Chrome};

fn init() -> app::App {
    // A headless scene never touches the real data dir or the network.
    if std::env::args().any(|a| a == "--scene") {
        let mut s = app::App::new(backend::AppHandle::new(std::env::temp_dir().join("mdwriter-scene")));
        s.inline = true;
        return s;
    }
    match backend::AppHandle::for_user() {
        Ok(be) => {
            let mut s = app::App::new(be);
            app::start(&mut s);
            s
        }
        Err(e) => {
            let mut s = app::App::new(backend::AppHandle::new(std::env::temp_dir()));
            s.fatal = Some(e);
            s
        }
    }
}

fn main() {
    let spec = AppSpec {
        min_size: Some((760.0, 480.0)),
        scenes: demo::SCENES,
        setup: Some(app::setup),
        poll: Some(app::poll),
        on_key: Some(app::on_key),
        ..AppSpec::new("mdwriter", (1100.0, 720.0), init, ui::view)
    };
    rune::App::new(spec)
        .chrome(Chrome { full_size_content: true, traffic_lights: Some((14.0, 16.0)), drag_regions: true })
        .standard_menus()
        // Closing the window or quitting writes the open draft first.
        .on_close(app::on_quit)
        .on_quit(app::on_quit)
        .run();
}
