//! Durable ranges, kept separate until the complete file can be published.

use futures_util::future::try_join_all;
use reqwest::header::{
    ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, ETAG, IF_RANGE, LAST_MODIFIED, RANGE,
};
use reqwest::{Client, RequestBuilder, Response, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use url::Url;

pub(super) const PREFIX: &str = ".jellium-download-";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Job {
    pub endpoint: String,
    pub name: String,
    pub length: u64,
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
struct Layout {
    validator: Option<String>,
    parts: u8,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    if !path.symlink_metadata()?.is_file() {
        return Err(invalid("not a download metadata file"));
    }
    serde_json::from_reader(File::open(path)?).map_err(io::Error::from)
}

fn write_json(dir: &Path, name: &str, value: &impl Serialize) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, value)?;
    file.as_file().sync_all()?;
    file.persist(dir.join(name)).map_err(|err| err.error)?;
    Ok(())
}

impl Job {
    pub fn create(&self, root: &Path) -> io::Result<PathBuf> {
        let mut builder = tempfile::Builder::new();
        builder.prefix(PREFIX);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        let dir = builder.tempdir_in(root)?;
        write_json(dir.path(), "job.json", self)?;
        Ok(dir.keep())
    }

    pub fn load(dir: &Path) -> io::Result<Self> {
        read_json(&dir.join("job.json"))
    }

    pub fn saved_bytes(&self, dir: &Path) -> u64 {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix("part-"))
                    .and_then(|part| part.parse::<u8>().ok())
                    .is_some()
            })
            .filter_map(|entry| entry.path().symlink_metadata().ok())
            .filter(|meta| meta.is_file())
            .fold(0u64, |total, meta| total.saturating_add(meta.len()))
            .min(self.length)
    }
}

pub(super) async fn download(
    job: &Job,
    dir: &Path,
    destination: &Path,
    url: Url,
    connections: u8,
    progress: impl Fn(u64),
) -> io::Result<()> {
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "A file with that name already exists.",
        ));
    }
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .map_err(http_error)?;
    let request = || client.get(url.clone()).header(ACCEPT_ENCODING, "identity");
    let probe = send(request().header(RANGE, "bytes=0-0")).await?;
    let old = read_json::<Layout>(&dir.join("layout.json")).ok();
    let layout = if probe.status() == StatusCode::OK {
        Layout {
            validator: None,
            parts: 1,
        }
    } else {
        check_range(&probe, 0, 0, job.length)?;
        let validator = validator(&probe);
        let parts = if validator.is_some() {
            old.as_ref()
                .filter(|old| {
                    old.validator == validator
                        && (1..=16).contains(&old.parts)
                        && u64::from(old.parts) <= job.length
                })
                .map_or(u64::from(connections).min(job.length) as u8, |old| {
                    old.parts
                })
        } else {
            1
        };
        Layout { validator, parts }
    };
    if layout.validator.is_none() || old.as_ref() != Some(&layout) {
        clear_parts(dir)?;
        write_json(dir, "layout.json", &layout)?;
    }
    let received = AtomicU64::new(job.saved_bytes(dir));
    progress(received.load(Ordering::Relaxed));
    let advance = |bytes| progress(received.fetch_add(bytes, Ordering::Relaxed) + bytes);
    if let Some(validator) = layout.validator.as_deref() {
        drop(probe);
        try_join_all((0..layout.parts).map(|index| {
            let (start, end) = part(u64::from(index), u64::from(layout.parts), job.length);
            download_range(
                request(),
                dir.join(format!("part-{index}")),
                start..=end,
                job.length,
                validator,
                &advance,
            )
        }))
        .await?;
    } else {
        let response = if probe.status() == StatusCode::OK {
            probe
        } else {
            drop(probe);
            send(request()).await?
        };
        if response.status() != StatusCode::OK {
            return Err(invalid("Unexpected response for a full download."));
        }
        copy_exact(
            response,
            &mut open_part(&dir.join("part-0"))?,
            job.length,
            &advance,
        )
        .await?;
    }
    publish(dir, destination, &layout, job.length)?;
    Ok(())
}

fn clear_parts(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|s| s.starts_with("part-"))
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn open_part(path: &Path) -> io::Result<File> {
    match OpenOptions::new().append(true).create_new(true).open(path) {
        Err(err)
            if err.kind() == io::ErrorKind::AlreadyExists && path.symlink_metadata()?.is_file() =>
        {
            OpenOptions::new().append(true).open(path)
        }
        result => result,
    }
}

async fn download_range(
    request: RequestBuilder,
    path: PathBuf,
    range: std::ops::RangeInclusive<u64>,
    length: u64,
    validator: &str,
    advance: &impl Fn(u64),
) -> io::Result<()> {
    let mut file = open_part(&path)?;
    let size = range.end() - range.start() + 1;
    let saved = file.metadata()?.len();
    if saved > size {
        return Err(invalid("Saved download segment is too large."));
    }
    if saved == size {
        return Ok(());
    }
    let (start, end) = (range.start() + saved, *range.end());
    let response = send(
        request
            .header(RANGE, format!("bytes={start}-{end}"))
            .header(IF_RANGE, validator),
    )
    .await?;
    check_range(&response, start, end, length)?;
    copy_exact(response, &mut file, size - saved, advance).await
}

fn publish(dir: &Path, destination: &Path, layout: &Layout, length: u64) -> io::Result<()> {
    if layout.parts == 1 {
        return fs::hard_link(dir.join("part-0"), destination);
    }
    let mut complete = tempfile::NamedTempFile::new_in(dir)?;
    for index in 0..layout.parts {
        let (start, end) = part(u64::from(index), u64::from(layout.parts), length);
        let mut input = File::open(dir.join(format!("part-{index}")))?;
        if io::copy(&mut input, &mut complete)? != end - start + 1 {
            return Err(invalid("Saved download segment size changed."));
        }
    }
    complete.as_file().sync_all()?;
    complete
        .persist_noclobber(destination)
        .map_err(|err| err.error)?;
    Ok(())
}

fn part(index: u64, streams: u64, length: u64) -> (u64, u64) {
    let (size, extra) = (length / streams, length % streams);
    let start = index * size + index.min(extra);
    (start, start + size - u64::from(index >= extra))
}

fn validator(response: &Response) -> Option<String> {
    let headers = response.headers();
    headers
        .get(ETAG)
        .and_then(|h| h.to_str().ok())
        .filter(|v| v.starts_with('"') && v.ends_with('"'))
        .or_else(|| {
            headers
                .get(LAST_MODIFIED)
                .and_then(|h| h.to_str().ok())
                .filter(|v| !v.is_empty())
        })
        .map(str::to_owned)
}

fn http_error(error: reqwest::Error) -> io::Error {
    // The URL contains the Jellyfin API key. Do not log any request error's URL or source chain.
    let message = error.status().map_or_else(
        || "HTTP transfer failed.".to_owned(),
        |status| format!("Jellyfin returned HTTP {status}."),
    );
    io::Error::other(message)
}

async fn send(request: RequestBuilder) -> io::Result<Response> {
    request
        .send()
        .await
        .and_then(Response::error_for_status)
        .map_err(http_error)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn check_range(response: &Response, start: u64, end: u64, length: u64) -> io::Result<()> {
    let expected = format!("bytes {start}-{end}/{length}");
    if response.status() != StatusCode::PARTIAL_CONTENT
        || !response
            .headers()
            .get(CONTENT_RANGE)
            .is_some_and(|value| *value == expected.as_str())
    {
        return Err(invalid(
            "Server returned the wrong byte range. Try resuming again.",
        ));
    }
    Ok(())
}

async fn copy_exact(
    mut response: Response,
    output: &mut File,
    length: u64,
    advance: &impl Fn(u64),
) -> io::Result<()> {
    if response
        .headers()
        .get(CONTENT_ENCODING)
        .is_some_and(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err(invalid("Server returned encoded download data."));
    }
    let mut remaining = length;
    while let Some(bytes) = response.chunk().await.map_err(http_error)? {
        let size = bytes.len() as u64;
        remaining = remaining
            .checked_sub(size)
            .ok_or_else(|| invalid("Server sent more download data than expected."))?;
        output.write_all(&bytes)?;
        advance(size);
    }
    if remaining != 0 {
        return Err(invalid("Server ended the download early."));
    }
    output.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumes_only_the_missing_bytes_and_publishes_once() -> io::Result<()> {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}/download", listener.local_addr()?);
        let server = std::thread::spawn(move || -> io::Result<()> {
            for (expected, headers, body) in [
                ("range: bytes=0-0", "bytes 0-0/8", "a"),
                ("range: bytes=6-7", "bytes 6-7/8", "gh"),
            ] {
                let (mut stream, _) = listener.accept()?;
                let request = BufReader::new(&stream)
                    .lines()
                    .take_while(|line| line.as_ref().is_ok_and(|line| !line.is_empty()))
                    .collect::<io::Result<Vec<_>>>()?;
                assert!(request.iter().any(|line| line == expected));
                if body == "gh" {
                    assert!(request.iter().any(|line| line == "if-range: \"same\""));
                }
                write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nConnection: close\r\nContent-Range: {headers}\r\nETag: \"same\"\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )?;
            }
            Ok(())
        });
        let root = tempfile::tempdir()?;
        let job = Job {
            endpoint: endpoint.clone(),
            name: "video.mp4".into(),
            length: 8,
        };
        let dir = job.create(root.path())?;
        write_json(
            &dir,
            "layout.json",
            &Layout {
                validator: Some("\"same\"".into()),
                parts: 2,
            },
        )?;
        fs::write(dir.join("part-0"), b"abcd")?;
        fs::write(dir.join("part-1"), b"ef")?;
        let reloaded = Job::load(&dir)?;
        assert_eq!(reloaded.saved_bytes(&dir), 6);
        let destination = root.path().join(&job.name);
        let url = Url::parse(&format!("{endpoint}?ApiKey=fixture")).map_err(io::Error::other)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let progress = std::cell::RefCell::new(Vec::new());
        runtime.block_on(download(&reloaded, &dir, &destination, url, 1, |n| {
            progress.borrow_mut().push(n)
        }))?;
        server
            .join()
            .map_err(|_| io::Error::other("fixture failed"))??;
        assert_eq!(fs::read(&destination)?, b"abcdefgh");
        assert_eq!(progress.into_inner(), [6, 8]);
        assert!(!fs::read_to_string(dir.join("job.json"))?.contains("fixture"));
        assert!(
            publish(
                &dir,
                &destination,
                &Layout {
                    validator: None,
                    parts: 2
                },
                8
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn range_parts_cover_even_and_uneven_files() {
        assert_eq!(
            (0..3).map(|i| part(i, 3, 7)).collect::<Vec<_>>(),
            [(0, 2), (3, 4), (5, 6)]
        );
        assert_eq!(
            (0..2).map(|i| part(i, 2, 4)).collect::<Vec<_>>(),
            [(0, 1), (2, 3)]
        );
    }
}
