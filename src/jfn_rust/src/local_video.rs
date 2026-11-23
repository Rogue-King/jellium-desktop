//! Standalone local playback, isolated from Jellyfin's browser and mpv handle.

use std::{io, path::Path};

use clap::Parser;
use jfn_mpv::{Command, EndFileReason, Event, Handle};

use crate::app::StartupOptions;

// Match the dark Jellyfin OSD and the shell's Jellyfin blue accent without a web session.
const OFFLINE_OSC: &str = concat!(
    "osc-layout=bottombar,osc-seekbarstyle=knob,osc-timetotal=yes,",
    "osc-background_color=#101010,osc-timecode_color=#00a4dc,osc-title=${media-title}",
);

pub(crate) fn pick() {
    fn open(path: std::path::PathBuf) {
        if let Err(error) = launch(&path) {
            tracing::error!(target: "Main", "could not launch local player: {error}");
            #[cfg(target_os = "macos")]
            jfn_macos::show_local_player_error(error.to_string());
        }
    }
    #[cfg(target_os = "macos")]
    jfn_macos::pick_video_file(open);
    #[cfg(not(target_os = "macos"))]
    if let Err(error) = jfn_cef::web_overlay::pick_video_file(open) {
        tracing::error!(target: "Main", "could not open file picker: {error}");
    }
}

fn launch(path: &Path) -> io::Result<()> {
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .arg("--open-file")
        .arg(path)
        .arg("--config-dir")
        .arg(jfn_paths::config_dir())
        .spawn()?;
    // Wait off the UI thread so the completed player doesn't become a zombie.
    std::thread::spawn(move || {
        if let Err(error) = child.wait() {
            tracing::warn!(target: "Main", "could not reap local player: {error}");
        }
    });
    Ok(())
}

/// Skip CEF and single-instance dispatch for the standalone player process.
pub(crate) fn run_if_requested() -> Option<i32> {
    let requested = std::env::args_os()
        .skip(1)
        .take_while(|arg| arg != "--")
        .any(|arg| arg == "--open-file" || arg.as_encoded_bytes().starts_with(b"--open-file="));
    if !requested {
        return None;
    }
    let mut cli = crate::cli::Cli::parse();
    let path = cli.open_file.take()?;
    if let Some(dir) = &cli.config_dir {
        jfn_paths::set_config_dir_override(dir.into());
    }
    crate::app::load_client_settings();
    Some(run(&path, crate::app::resolve_startup_options(&cli)))
}

fn run(path: &Path, options: StartupOptions) -> i32 {
    #[cfg(target_os = "macos")]
    {
        let path = path.to_owned();
        jfn_macos::run_local_player(move || play_path(&path, &options))
    }
    #[cfg(not(target_os = "macos"))]
    match play_path(path, &options) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Could not play the local file: {error}");
            1
        }
    }
}

fn play_path(path: &Path, options: &StartupOptions) -> Result<(), String> {
    let command = load_command(path).map_err(|e| e.to_string())?;
    let mpv_dir = jfn_paths::mpv_home();
    let mpv_dir = mpv_dir
        .to_str()
        .ok_or("mpv config directory is not valid UTF-8")?;
    play(&command, mpv_dir, options).map_err(|e| e.to_string())
}

fn load_command(path: &Path) -> io::Result<Command> {
    let path = std::path::absolute(path)?;
    if !path.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "choose a video file",
        ));
    }
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let bytes = path
        .to_str()
        .map(str::as_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsupported file name"))?;
    Command::new([b"loadfile".as_slice(), bytes])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn play(command: &Command, mpv_dir: &str, options: &StartupOptions) -> Result<(), jfn_mpv::Error> {
    let player = Handle::create()?;
    // Normal playback reads Jellium's mpv.conf, including any HDR output settings.
    for (option, value) in [
        ("config", "yes"),
        ("config-dir", mpv_dir),
        ("hwdec", options.hwdec.as_str()),
        ("script-opts", OFFLINE_OSC),
        ("load-scripts", "no"),
        ("input-default-bindings", "yes"),
        ("input-vo-keyboard", "yes"),
        ("osc", "yes"),
        ("ytdl", "no"),
        ("autoload-files", "no"),
        ("access-references", "no"),
        ("drag-and-drop", "no"),
        ("demuxer-lavf-o", "protocol_whitelist=file"),
        ("title", "Jellium Desktop — ${filename}"),
    ] {
        player.set_option_string(option, value)?;
    }
    if !options.audio_passthrough.is_empty() {
        player.set_option_string("audio-spdif", &options.audio_passthrough)?;
    }
    if !options.audio_channels.is_empty() {
        player.set_option_string("audio-channels", &options.audio_channels)?;
    }
    if options.audio_exclusive {
        player.set_option_flag("audio-exclusive", true)?;
    }
    player.initialize()?;
    player.command_async(1, command)?;
    loop {
        match player.wait_event(-1.0) {
            Event::CommandReply { error, .. } if error < 0 => {
                return Err(jfn_mpv::Error::new(error));
            }
            Event::EndFile(EndFileReason::Error(error)) => return Err(error),
            Event::EndFile(EndFileReason::Redirect) => {
                return Err(jfn_mpv::Error::new(
                    jfn_mpv::sys::mpv_error::MPV_ERROR_LOADING_FAILED.0,
                ));
            }
            Event::EndFile(_) | Event::Shutdown => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_existing_regular_files_become_load_commands() {
        assert!(load_command(Path::new(env!("CARGO_MANIFEST_DIR"))).is_err());
        assert!(load_command(Path::new("https://example.invalid/movie.mp4")).is_err());
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(load_command(&file).is_ok_and(|cmd| cmd.len() == 2));
    }
}
