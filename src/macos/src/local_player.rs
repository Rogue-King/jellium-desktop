//! AppKit services used by standalone playback and the File menu.

use std::path::PathBuf;

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSModalResponseOK, NSOpenPanel,
};
use objc2_foundation::NSString;

pub fn pick_video_file(open: fn(PathBuf)) {
    crate::dispatch::post_to_main(move || {
        let Some(main) = MainThreadMarker::new() else {
            return;
        };
        let panel = NSOpenPanel::openPanel(main);
        panel.setTitle(Some(&NSString::from_str("Open Video")));
        panel.setAllowsMultipleSelection(false);
        panel.setCanChooseDirectories(false);
        if panel.runModal() == NSModalResponseOK
            && let Some(path) = panel.URL().and_then(|url| url.to_file_path())
        {
            open(path);
        }
    });
}

pub fn show_local_player_error(error: String) {
    crate::dispatch::post_to_main(move || {
        if let Some(main) = MainThreadMarker::new() {
            show_error(main, &error);
        }
    });
}

fn show_error(main: MainThreadMarker, error: &str) {
    let alert = NSAlert::new(main);
    alert.setMessageText(&NSString::from_str("Couldn’t play the local file"));
    alert.setInformativeText(&NSString::from_str(error));
    alert.runModal();
}

/// mpv's Cocoa video output dispatches to AppKit; run mpv on a worker so
/// both playback and handle destruction can complete while AppKit pumps.
pub fn run_local_player(play: impl FnOnce() -> Result<(), String> + Send + 'static) -> i32 {
    let Some(main) = MainThreadMarker::new() else {
        return 1;
    };
    let app = NSApplication::sharedApplication(main);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.activate();
    std::thread::spawn(move || {
        let result = play();
        crate::dispatch::post_to_main(move || {
            if let Err(error) = result {
                eprintln!("Could not play the local file: {error}");
                if let Some(main) = MainThreadMarker::new() {
                    show_error(main, &error);
                }
                std::process::exit(1);
            }
            std::process::exit(0);
        });
    });
    app.run();
    0
}
