use super::{eligible_url, is_filename, transfer};
use parking_lot::Mutex;
use std::{
    cell::Cell,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use tokio::sync::oneshot;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadState {
    Downloading,
    Stopping,
    Paused,
    Complete,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct DownloadRow {
    pub id: usize,
    pub name: String,
    pub received: u64,
    pub total: u64,
    pub bits_per_second: Option<f64>,
    pub state: DownloadState,
}

struct Entry {
    job: transfer::Job,
    dir: PathBuf,
    received: u64,
    bits_per_second: Option<f64>,
    state: DownloadState,
    stop: Option<oneshot::Sender<()>>,
}

pub(super) struct Downloads {
    pub root: PathBuf,
    connections: u8,
    entries: Mutex<Vec<Entry>>,
    changed: fn(),
}

impl Downloads {
    pub(super) fn new(root: PathBuf, connections: u8, changed: fn()) -> io::Result<Self> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(&root)?.filter_map(Result::ok) {
            let dir = entry.path();
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with(transfer::PREFIX)
                || !entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                continue;
            }
            let Ok(job) = transfer::Job::load(&dir) else {
                continue;
            };
            if !is_filename(&job.name) || job.length == 0 {
                continue;
            }
            let received = job.saved_bytes(&dir);
            entries.push(Entry {
                job,
                dir,
                received,
                bits_per_second: None,
                state: DownloadState::Paused,
                stop: None,
            });
        }
        Ok(Self {
            root,
            connections,
            entries: Mutex::new(entries),
            changed,
        })
    }

    pub(super) fn rows(&self) -> Vec<DownloadRow> {
        self.entries
            .lock()
            .iter()
            .enumerate()
            .map(|(id, entry)| DownloadRow {
                id,
                name: entry.job.name.clone(),
                received: entry.received,
                total: entry.job.length,
                bits_per_second: entry.bits_per_second,
                state: entry.state.clone(),
            })
            .rev()
            .collect()
    }

    pub(super) fn add(self: &Arc<Self>, url: Url, name: String, length: u64) -> io::Result<()> {
        let mut endpoint = url.clone();
        endpoint.set_query(None);
        let mut entries = self.entries.lock();
        let id = if let Some(id) = entries.iter().position(|entry| {
            entry.job.endpoint == endpoint.as_str()
                && entry.job.name == name
                && entry.job.length == length
        }) {
            id
        } else {
            let job = transfer::Job {
                endpoint: endpoint.into(),
                name,
                length,
            };
            let dir = job.create(&self.root)?;
            let id = entries.len();
            entries.push(Entry {
                job,
                dir,
                received: 0,
                bits_per_second: None,
                state: DownloadState::Paused,
                stop: None,
            });
            id
        };
        drop(entries);
        self.start(id, url);
        Ok(())
    }

    pub(super) fn endpoint(&self, id: usize) -> Option<Url> {
        Url::parse(&self.entries.lock().get(id)?.job.endpoint).ok()
    }

    pub(super) fn authorize(self: &Arc<Self>, id: usize, page: &str, token: &str) {
        let Some(mut url) = self.endpoint(id) else {
            return;
        };
        if !token.is_empty() && url.query().is_none() {
            url.query_pairs_mut().append_pair("ApiKey", token);
            if let Some(url) =
                eligible_url(&jfn_config::server_url(), page, url.as_str(), url.as_str())
            {
                self.start(id, url);
                return;
            }
        }
        self.fail_to_start(
            id,
            "Sign in to the Jellyfin server that owns this download, then try again.",
        );
    }

    pub(super) fn fail_to_start(&self, id: usize, message: &str) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.get_mut(id)
            && matches!(
                entry.state,
                DownloadState::Paused | DownloadState::Failed(_)
            )
        {
            entry.state = DownloadState::Failed(message.into());
            drop(entries);
            (self.changed)();
        }
    }

    pub(super) fn stop(&self, id: usize) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.get_mut(id)
            && let Some(stop) = entry.stop.take()
        {
            entry.state = DownloadState::Stopping;
            entry.bits_per_second = None;
            let _ = stop.send(());
            drop(entries);
            (self.changed)();
        }
    }

    fn start(self: &Arc<Self>, id: usize, url: Url) {
        let mut entries = self.entries.lock();
        let Some(entry) = entries.get_mut(id) else {
            return;
        };
        if !matches!(
            entry.state,
            DownloadState::Paused | DownloadState::Failed(_)
        ) {
            return;
        }
        let (stop, cancelled) = oneshot::channel();
        entry.stop = Some(stop);
        entry.state = DownloadState::Downloading;
        let (job, dir) = (entry.job.clone(), entry.dir.clone());
        drop(entries);
        (self.changed)();
        let queue = Arc::clone(self);
        if let Err(error) =
            std::thread::Builder::new().spawn(move || queue.run(id, &job, &dir, url, cancelled))
        {
            self.finish(id, DownloadState::Failed(error.to_string()));
        }
    }

    fn run(
        &self,
        id: usize,
        job: &transfer::Job,
        dir: &Path,
        url: Url,
        cancelled: oneshot::Receiver<()>,
    ) {
        let destination = self.root.join(&job.name);
        let baseline = Cell::new(None);
        let progress = |received: u64| {
            let now = Instant::now();
            // The first progress report includes bytes already on disk, not network traffic.
            let rate = if let Some((start, saved)) = baseline.get() {
                let elapsed = now.duration_since(start).as_secs_f64();
                (elapsed > 0.0).then(|| received.saturating_sub(saved) as f64 * 8.0 / elapsed)
            } else {
                baseline.set(Some((now, received)));
                None
            };
            self.progress(id, received, rate);
        };
        let result = tokio::runtime::Builder::new_current_thread().enable_all().build().map(|runtime| runtime.block_on(async {
            tokio::select! {
                biased;
                _ = cancelled => None,
                result = transfer::download(job, dir, &destination, url, self.connections, progress) => Some(result),
            }
        }));
        let state = match result {
            Ok(Some(Ok(()))) => {
                if let Err(error) = fs::remove_dir_all(dir) {
                    jfn_logging::log(
                        jfn_logging::Category::Cef,
                        jfn_logging::Level::Warn,
                        &format!("Could not clean completed download: {error}"),
                    );
                }
                DownloadState::Complete
            }
            Ok(None) => DownloadState::Paused,
            Ok(Some(Err(error))) | Err(error) => DownloadState::Failed(error.to_string()),
        };
        self.finish(id, state);
    }

    fn progress(&self, id: usize, received: u64, bits_per_second: Option<f64>) {
        let mut entries = self.entries.lock();
        let Some(entry) = entries.get_mut(id) else {
            return;
        };
        let percent = |bytes| u128::from(bytes) * 100 / u128::from(entry.job.length);
        let show_rate = entry.state == DownloadState::Downloading;
        let rate = bits_per_second.filter(|_| show_rate);
        let changed = percent(entry.received) != percent(received)
            || entry.bits_per_second.is_some() != rate.is_some();
        entry.received = received;
        entry.bits_per_second = rate;
        drop(entries);
        if changed {
            (self.changed)();
        }
    }

    fn finish(&self, id: usize, state: DownloadState) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.get_mut(id) {
            entry.stop = None;
            entry.bits_per_second = None;
            entry.received = if state == DownloadState::Complete {
                entry.job.length
            } else {
                entry.job.saved_bytes(&entry.dir)
            };
            entry.state = state;
        }
        drop(entries);
        (self.changed)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopping_keeps_partial_bytes_for_the_next_session() -> io::Result<()> {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::{OnceLock, mpsc};

        static EVENTS: OnceLock<mpsc::Sender<()>> = OnceLock::new();
        let (events, changes) = mpsc::channel();
        EVENTS
            .set(events)
            .map_err(|_| io::Error::other("test observer already installed"))?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}/download", listener.local_addr()?);
        let (release, released) = mpsc::channel::<()>();
        let server = std::thread::spawn(move || -> io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            for line in BufReader::new(&stream).lines() {
                if line?.is_empty() {
                    break;
                }
            }
            stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nabcd",
            )?;
            // Leave the response unfinished until the queue has stopped it.
            let _ = released.recv();
            Ok(())
        });
        let root = tempfile::tempdir()?;
        let queue = Arc::new(Downloads::new(root.path().into(), 1, || {
            if let Some(events) = EVENTS.get() {
                let _ = events.send(());
            }
        })?);
        let url = Url::parse(&format!("{endpoint}?ApiKey=fixture")).map_err(io::Error::other)?;
        queue.add(url, "video.mp4".into(), 8)?;
        while changes.recv().is_ok() {
            let row = &queue.rows()[0];
            assert_eq!(row.state, DownloadState::Downloading);
            if row.received == 4 {
                assert!(row.bits_per_second.is_some_and(|rate| rate > 0.0));
                break;
            }
        }
        queue.stop(0);
        while changes.recv().is_ok() {
            let row = &queue.rows()[0];
            if row.state == DownloadState::Paused {
                assert!(row.bits_per_second.is_none());
                break;
            }
            assert_eq!(row.state, DownloadState::Stopping);
        }
        drop(release);
        server
            .join()
            .map_err(|_| io::Error::other("fixture failed"))??;
        let restarted = Downloads::new(root.path().into(), 1, || {})?;
        let row = &restarted.rows()[0];
        assert_eq!(row.state, DownloadState::Paused);
        assert_eq!(row.received, 4);
        assert!(!root.path().join("video.mp4").exists());
        Ok(())
    }
}
