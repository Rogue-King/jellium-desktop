# Jellium Desktop

An unofficial [Jellyfin](https://jellyfin.org) desktop client built on [CEF](https://github.com/chromiumembedded/cef) and [mpv](https://mpv.io/).

## Desktop features

- **Open File…** on the connect screen or app menu plays a downloaded video in a separate Jellium player window, without a Jellyfin connection. On macOS, use **File → Open File…** (⌘O). From a terminal, use `jellium-desktop --open-file /path/to/video.mp4`. Offline playback uses Jellyfin-style controls and the same Jellium mpv configuration and hardware-decoding setting as server playback. HDR output still depends on the file, display, and graphics backend. Local playback does not update your Jellyfin watch history.
- Jellyfin's playback information panel shows actual mpv codecs, hardware decoding, separate output/decoder frame drops, and buffering/cache estimates alongside the play method.
- **About → Copy diagnostics** copies runtime facts without server URLs, credentials, media titles, or local paths. The last playback failure and recovery guidance are also available in About.
- Connection errors explain DNS, HTTPS/certificate, HTTP, and invalid-server failures. **Retry** keeps the address; **Edit address** returns to the form.
- Native **Settings → Server** lets you switch between saved servers and edit their names. **Connect to another server** preserves bookmarks; forgetting the active server disconnects it. Switching stops playback. Existing browser logins are reused, not deleted or split into account profiles.

### Download original media

Use **Download** on a Jellyfin item, then open **Downloads** from Jellium’s app menu to see progress. **Stop** keeps the partial file. To resume after restarting Jellium, sign in to the same server and select **Resume**. Completed files go to your system Downloads folder by default.

In **Client Settings → Downloads**, set the **Download directory** and **Parallel connections** for each download, then restart Jellium. Leave the directory blank to use your system Downloads folder. Partial downloads stay in the folder where they started; switch back to that folder to resume them.

Command-line flags override the saved settings for that launch:

```sh
jellium-desktop --download-dir "$HOME/Downloads" --download-connections 4
```

On macOS, use `Jellium Desktop.app/Contents/MacOS/jellium-desktop` as the executable. No separate downloader or network share is needed. Existing files aren’t replaced.

Resume and parallel transfers require byte ranges and an ETag or Last-Modified header. If the server doesn’t provide them, Jellium restarts the transfer from the beginning. The Downloads panel covers original Jellyfin files, not plugin transcodes. The old `--aria2-downloads` and `--aria2-connections` flags still work as aliases.

## Downloads
### Linux
- AppImage
  - [x86_64](https://nightly.link/andrewrabert/jellium-desktop/workflows/build-linux-appimage/main/linux-appimage-x86_64.zip)
  - [aarch64](https://nightly.link/andrewrabert/jellium-desktop/workflows/build-linux-appimage/main/linux-appimage-aarch64.zip)
- Arch Linux (AUR): [jellium-desktop-git](https://aur.archlinux.org/packages/jellium-desktop-git)
- [Flatpak (non-Flathub bundle)](https://nightly.link/andrewrabert/jellium-desktop/workflows/build-linux-flatpak/main/linux-flatpak-x86_64.zip)

### macOS
- [Apple Silicon](https://nightly.link/andrewrabert/jellium-desktop/workflows/build-macos/main/macos-arm64.zip)
- [Intel](https://nightly.link/andrewrabert/jellium-desktop/workflows/build-macos/main/macos-x86_64.zip)

After installing, remove quarantine: 
```
sudo xattr -cr /Applications/Jellium\ Desktop.app
```

### Windows
- [x64](https://nightly.link/andrewrabert/jellium-desktop/workflows/build-windows/main/windows-x64.zip)
- [arm64](https://nightly.link/andrewrabert/jellium-desktop/workflows/build-windows/main/windows-arm64.zip)


## Development

This project uses [just](https://github.com/casey/just) as a command runner.

The injected-player JavaScript self-checks use Node's built-in runner (no npm install):

```sh
node --test src/web/tests/player.test.cjs
```

### macOS with Nix

With Nix flakes and Xcode Command Line Tools installed, enter the pinned development environment:

```sh
nix develop
just build
just test
just dmg
```

Alternatively, run `direnv allow` to load it automatically through `.envrc`.
The flake supports Apple Silicon and Intel Macs and supplies Rust, build tools, and native dependencies without Homebrew. It builds the project's mpv fork, not a system mpv. The app is staged in `build/output/Jellium Desktop.app` and the DMG in `dist/`.

```
Available recipes:
    [package]
    appimage ...    # [linux] build AppImage
    flatpak ...     # [linux] build Flatpak bundle
    dmg             # [macos] build Apple Disk Image (.dmg)

    [maintenance]
    outdated      # List outdated dependencies
    clean         # Remove build artifacts

    [test]
    test          # Run tests

    [lint]
    fmt           # Format workspace
    fmt-check     # Check formatting
    clippy        # Run clippy
    lint          # Lint workspace
    strict-lint   # Strict lint workspace

    [build]
    build         # Build the app

    [run]
    run *args     # Run the app
```
