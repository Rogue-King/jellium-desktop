//! In-process downloads for Jellyfin's built-in original-file downloads.

use cef::{rc::Rc, *};
use jfn_logging::{Category, Level, log};
use std::{
    io,
    path::Path,
    sync::{Arc, OnceLock},
};
use url::Url;

use crate::cef_string::userfree_to_string;

mod queue;
mod transfer;

use queue::Downloads;
pub use queue::{DownloadRow, DownloadState};

static DOWNLOADS: OnceLock<DownloadRuntime> = OnceLock::new();

struct DownloadRuntime {
    queue: Arc<Downloads>,
    pending: parking_lot::Mutex<std::collections::HashSet<usize>>,
}

pub fn enable_downloads(directory: &Path, connections: u8, changed: fn()) -> io::Result<()> {
    let directory = directory.canonicalize()?;
    let queue = Downloads::new(
        directory,
        connections.clamp(
            jfn_config::DEFAULT_DOWNLOAD_CONNECTIONS,
            jfn_config::MAX_DOWNLOAD_CONNECTIONS,
        ),
        changed,
    )?;
    let _ = DOWNLOADS.set(DownloadRuntime {
        queue: Arc::new(queue),
        pending: Default::default(),
    });
    Ok(())
}

pub fn download_rows() -> Vec<DownloadRow> {
    DOWNLOADS
        .get()
        .map_or_else(Vec::new, |downloads| downloads.queue.rows())
}

pub fn download_directory() -> Option<&'static Path> {
    DOWNLOADS
        .get()
        .map(|downloads| downloads.queue.root.as_path())
}

pub fn stop_download(id: usize) {
    if let Some(downloads) = DOWNLOADS.get() {
        downloads.pending.lock().remove(&id);
        downloads.queue.stop(id);
    }
}

pub fn resume_download(id: usize) {
    let Some(downloads) = DOWNLOADS.get() else {
        return;
    };
    let Some(endpoint) = downloads.queue.endpoint(id) else {
        return;
    };
    let origin = endpoint.origin().ascii_serialization();
    let Ok(origin) = serde_json::to_string(&origin) else {
        return;
    };
    // The connected web app already holds the login. Don't persist it with the partial file.
    let script = format!(
        r#"(() => {{
        let token = '';
        try {{
            if (location.origin === {origin}) {{
                const client = window.ApiClient;
                token = client?.accessToken?.() || '';
            }}
        }} finally {{
            window.jmpNative?.resumeDownloadAuth('{id}', token);
        }}
    }})()"#
    );
    downloads.pending.lock().insert(id);
    if crate::web_overlay::with_current_client(|client| client.exec_js(&script)).is_none() {
        downloads.pending.lock().remove(&id);
        downloads
            .queue
            .fail_to_start(id, "Connect to Jellyfin before resuming.");
    }
}

pub(super) fn handle_resume_auth(frame: Option<&Frame>, args: Option<&ListValue>) {
    let (Some(downloads), Some(frame), Some(args)) = (DOWNLOADS.get(), frame, args) else {
        return;
    };
    if frame.is_main() == 0 {
        return;
    }
    let Ok(id) = userfree_to_string(&args.string(0)).parse::<usize>() else {
        return;
    };
    if downloads.pending.lock().remove(&id) {
        downloads.queue.authorize(
            id,
            &userfree_to_string(&frame.url()),
            &userfree_to_string(&args.string(1)),
        );
    }
}

pub(super) fn handler() -> Option<DownloadHandler> {
    DOWNLOADS
        .get()
        .map(|downloads| OriginalDownloadHandler::new(Arc::clone(&downloads.queue)))
}

/// NativeShell.downloadFile opens a popup rather than a Chromium download.
/// Route eligible popups through CEF so the handler still gets the server's filename and length.
pub(super) fn start_from_popup(browser: &Browser, frame: &Frame, target: &CefString) -> bool {
    if DOWNLOADS.get().is_none() {
        return false;
    }
    let page = userfree_to_string(&frame.url());
    let url = target.to_string();
    if eligible_url(&jfn_config::server_url(), &page, &url, &url).is_none() {
        return false;
    }
    let Some(host) = browser.host() else {
        return false;
    };
    host.start_download(Some(target));
    true
}

wrap_download_handler! {
    struct OriginalDownloadHandler { downloads: Arc<Downloads> }
    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _request_method: Option<&CefString>) -> i32 {
            1
        }

        fn on_before_download(
            &self,
            browser: Option<&mut Browser>,
            item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            _callback: Option<&mut BeforeDownloadCallback>,
        ) -> i32 {
            let (Some(frame), Some(item), Some(name)) = (browser.and_then(|b| b.main_frame()), item, suggested_name) else {
                return 0;
            };
            let current = userfree_to_string(&item.url());
            let original = userfree_to_string(&item.original_url());
            let page = userfree_to_string(&frame.url());
            let Some(url) = eligible_url(&jfn_config::server_url(), &page, &original, &current) else {
                return 0;
            };
            let name = name.to_string();
            let Ok(length) = u64::try_from(item.total_bytes()) else { return 0 };
            if length == 0 || !is_filename(&name) {
                return 0;
            }
            if let Err(error) = self.downloads.add(url, name, length) {
                log(Category::Cef, Level::Error, &format!("Could not queue Jellyfin download: {error}"));
                return 0;
            }
            // Do not continue Chromium's duplicate download.
            1
        }
    }
}

fn eligible_url(server: &str, page: &str, original: &str, current: &str) -> Option<Url> {
    let server = Url::parse(server).ok()?;
    let page = Url::parse(page).ok()?;
    let url = Url::parse(current).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url != Url::parse(original).ok()?
        || url.origin() != server.origin()
        || url.origin() != page.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url
        .path()
        .strip_prefix(server.path().trim_end_matches('/'))?;
    let mut segments = path.strip_prefix('/')?.split('/');
    let (items, id, download) = (segments.next()?, segments.next()?, segments.next()?);
    if !items.eq_ignore_ascii_case("items")
        || !download.eq_ignore_ascii_case("download")
        || segments.next().is_some()
        || !valid_item_id(id)
        || !url.query_pairs().any(|(key, value)| {
            (key.eq_ignore_ascii_case("apikey") || key.eq_ignore_ascii_case("api_key"))
                && !value.is_empty()
        })
    {
        return None;
    }
    Some(url)
}

fn valid_item_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    match bytes.len() {
        32 => bytes.iter().all(u8::is_ascii_hexdigit),
        36 => bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        }),
        _ => false,
    }
}

fn is_filename(name: &str) -> bool {
    Path::new(name).file_name().is_some_and(|part| part == name)
        && !name.ends_with(['.', ' '])
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_authenticated_original_downloads_on_the_connected_server_are_eligible() {
        let server = "https://jellyfin.example/library/";
        let page = "https://jellyfin.example/library/web/index.html";
        let url = "https://jellyfin.example/library/Items/00112233445566778899aabbccddeeff/Download?ApiKey=test";
        assert!(eligible_url(server, page, url, url).is_some());
        assert!(eligible_url(server, "https://other.example/web/", url, url).is_none());
        assert!(eligible_url("https://jellyfin.example/other/", page, url, url).is_none());
        let plugin = url.replace("/Items/", "/Plugin/Items/");
        assert!(eligible_url(server, page, &plugin, &plugin).is_none());
        assert!(eligible_url(server, page, url, &plugin).is_none());
        let unauthenticated = url.trim_end_matches("?ApiKey=test");
        assert!(eligible_url(server, page, unauthenticated, unauthenticated).is_none());
    }

    #[test]
    fn filename_cannot_escape_the_download_directory() {
        assert!(is_filename("A movie.mp4"));
        for name in [
            "",
            "..",
            "../movie.mp4",
            "a\\movie.mp4",
            "C:movie.mp4",
            "movie\n.mp4",
            "movie.mp4/",
        ] {
            assert!(!is_filename(name));
        }
    }
}
