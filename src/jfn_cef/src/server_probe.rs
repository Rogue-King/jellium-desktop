//! Two-phase server probe over `CefURLRequest`.
//!
//! HEAD with redirect-follow finds the canonical base URL, then GET
//! `{base}/System/Info/Public` confirms a Jellyfin server. Travelling over
//! CEF keeps Chromium's proxy and TLS-trust configuration applied to it.

use cef::*;
use parking_lot::Mutex;
use std::sync::Arc;

use cef::rc::Rc;
use cef::{ThreadId, post_task, wrap_task};

use crate::connection_error::{ConnectionError, validate_server_response};
use jfn_jellyfin::{extract_base_url, normalize_input};

pub type ProbeCallback = Box<dyn FnOnce(Result<String, ConnectionError>) + Send + Sync>;

#[derive(Copy, Clone, PartialEq)]
enum Phase {
    Head,
    Get,
}

struct ProbeState {
    _owner: crossbeam_channel::Sender<std::convert::Infallible>,
    url: String,
    phase: Phase,
    base: String,
    body: Vec<u8>,
    callback: Option<ProbeCallback>,
    active: Option<Urlrequest>,
    cancelled: bool,
    session: Arc<crate::runtime::Session>,
}

pub(crate) struct Probe {
    state: Arc<Mutex<ProbeState>>,
}

impl Probe {
    /// Answers with the canonical base URL of the Jellyfin server `url`
    /// resolves to, or a failure safe to display, on the CEF UI thread.
    ///
    /// Callable from any thread: `CefURLRequest::Create` is UI-thread-only
    /// under a multi-threaded message loop, so the request is built inside a
    /// posted TID_UI task and its handle is published back into the returned
    /// `Probe`.
    pub(crate) fn start(
        session: Arc<crate::runtime::Session>,
        url: &str,
        on_done: ProbeCallback,
    ) -> Probe {
        let (owner, disconnected) = crossbeam_channel::unbounded();
        let state = Arc::new(Mutex::new(ProbeState {
            _owner: owner,
            url: normalize_input(url),
            phase: Phase::Head,
            base: String::new(),
            body: Vec::new(),
            callback: Some(on_done),
            active: None,
            cancelled: false,
            session: Arc::clone(&session),
        }));
        let accepted = session
            .dispatch(|| {
                session.track_producer(disconnected);
                let mut task = StartTask::new(Arc::clone(&state));
                post_task(ThreadId::UI, Some(&mut task)) == 1
            })
            .unwrap_or(false);
        if !accepted {
            state.lock().cancelled = true;
            let callback = state.lock().callback.take();
            if let Some(callback) = callback {
                session.dispatch(|| callback(Err(ConnectionError::Unavailable)));
            }
        }
        Probe { state }
    }

    /// Aborts the in-flight request on TID_UI; `on_done` never fires
    /// afterwards, including when the request had not been built yet.
    /// Called by the session's UI drain barrier, before releasing native CEF.
    pub(crate) fn cancel_on_ui(self) {
        {
            let mut state = self.state.lock();
            state.cancelled = true;
            state.callback = None;
        }
        cancel_on_ui(&self.state);
    }
}

/// Builds the HEAD request on TID_UI and publishes its handle.
fn start_on_ui(state: &Arc<Mutex<ProbeState>>) {
    let head_url = {
        let st = state.lock();
        if st.cancelled || !st.session.is_active() {
            return;
        }
        st.url.clone()
    };
    if !head_url.starts_with("http://") && !head_url.starts_with("https://") {
        finish(state, Err(ConnectionError::InvalidUrl));
        return;
    }
    let client = JfnServerProbeClient::new(Arc::clone(state));
    let request = make_request("HEAD", &head_url, client);
    let mut st = state.lock();
    if st.cancelled || !st.session.is_active() {
        if let Some(r) = request {
            r.cancel();
        }
        return;
    }
    match request {
        Some(r) => st.active = Some(r),
        None => {
            jfn_logging::log(
                jfn_logging::Category::Cef,
                jfn_logging::Level::Warn,
                "server probe: CefURLRequest::Create returned null",
            );
            if let Some(f) = st.callback.take() {
                drop(st);
                f(Err(ConnectionError::Unavailable));
            }
        }
    }
}

fn cancel_on_ui(state: &Arc<Mutex<ProbeState>>) {
    let request = state.lock().active.take();
    if let Some(r) = request {
        r.cancel();
    }
}

wrap_task! {
    struct StartTask {
        state: Arc<Mutex<ProbeState>>,
    }
    impl Task {
        fn execute(&self) {
            let session = Arc::clone(&self.state.lock().session);
            session.dispatch(|| start_on_ui(&self.state));
        }
    }
}

fn finish(state: &Arc<Mutex<ProbeState>>, result: Result<String, ConnectionError>) {
    let callback = {
        let mut state = state.lock();
        state.active = None;
        state.callback.take()
    };
    if let Some(callback) = callback {
        callback(result);
    }
}

fn on_complete(state: &Arc<Mutex<ProbeState>>, request: &Urlrequest) {
    if !state.lock().session.is_active() {
        cancel_on_ui(state);
        return;
    }
    if request.request_status().as_ref() != &sys::cef_urlrequest_status_t::UR_SUCCESS {
        let code: sys::cef_errorcode_t = request.request_error().into();
        finish(state, Err(ConnectionError::from_cef(code as i32)));
        return;
    }
    let head = {
        let mut st = state.lock();
        if st.callback.is_none() {
            return;
        }
        if st.phase == Phase::Head {
            let resolved = request
                .response()
                .map(|resp| {
                    let url = resp.url();
                    let url: CefString = (&url).into();
                    url.to_string()
                })
                .filter(|url| !url.is_empty())
                .unwrap_or_else(|| st.url.clone());
            st.base = extract_base_url(&resolved).to_string();
            st.phase = Phase::Get;
            Some(format!("{}/System/Info/Public", st.base))
        } else {
            None
        }
    };
    if let Some(url) = head {
        // A server may reject HEAD while accepting GET; only transport failures abort the first phase.
        let client = JfnServerProbeClient::new(Arc::clone(state));
        if let Some(next) = make_request("GET", &url, client) {
            state.lock().active = Some(next);
        } else {
            finish(state, Err(ConnectionError::Unavailable));
        }
        return;
    }
    let result = {
        let st = state.lock();
        match request.response() {
            Some(response) => {
                validate_server_response(response.status(), &st.body).map(|()| st.base.clone())
            }
            None => Err(ConnectionError::Unavailable),
        }
    };
    finish(state, result);
}

fn make_request(method: &str, url: &str, client: UrlrequestClient) -> Option<Urlrequest> {
    let req: Request = request_create()?;
    req.set_url(Some(&CefString::from(url)));
    req.set_method(Some(&CefString::from(method)));
    let mut req_arg = req;
    let mut client_arg = client;
    urlrequest_create(Some(&mut req_arg), Some(&mut client_arg), None)
}

cef::wrap_urlrequest_client! {
    struct JfnServerProbeClient {
        state: Arc<Mutex<ProbeState>>,
    }

    impl UrlrequestClient {
        fn on_request_complete(&self, request: Option<&mut Urlrequest>) {
            let Some(req) = request else { return };
            let session = Arc::clone(&self.state.lock().session);
            session.dispatch(|| on_complete(&self.state, req));
        }
        fn on_download_data(
            &self,
            _request: Option<&mut Urlrequest>,
            data: *const u8,
            data_length: usize,
        ) {
            let mut st = self.state.lock();
            if st.phase == Phase::Get && !data.is_null() && data_length > 0 {
                let slice = unsafe { std::slice::from_raw_parts(data, data_length) };
                st.body.extend_from_slice(slice);
            }
        }
    }
}
