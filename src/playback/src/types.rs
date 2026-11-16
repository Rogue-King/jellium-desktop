//! Pure data types shared between the state machine and coordinator.
//!
//! These are internal Rust types. The FFI-facing shapes live in `ffi.rs`
//! and are populated from these at sink-delivery time.

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum MediaType {
    #[default]
    Unknown = 0,
    Audio = 1,
    Video = 2,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum PlayerPresence {
    #[default]
    None = 0,
    Present = 1,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum PlaybackPhase {
    Starting = 0,
    Playing = 1,
    Paused = 2,
    #[default]
    Stopped = 3,
}

impl PlaybackPhase {
    /// Whether media is loaded: every phase but `Stopped`.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Playing | Self::Paused)
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndReason {
    Eof = 0,
    Error = 1,
    Canceled = 2,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaMetadata {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_number: i32,
    pub duration_us: i64,
    pub art_url: String,
    pub art_data_uri: String,
    pub media_type: MediaType,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlaybackBufferedRange {
    pub start_ticks: i64,
    pub end_ticks: i64,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackDiagnostics {
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hardware_decoding: Option<String>,
    pub dropped_frames: Option<i64>,
    pub decoder_dropped_frames: Option<i64>,
    pub cache_duration: Option<f64>,
    pub cache_bytes: Option<i64>,
    pub input_bytes_per_second: Option<f64>,
}

#[derive(Clone, Debug)]
pub enum PlaybackDiagnostic {
    VideoCodec(Option<String>),
    AudioCodec(Option<String>),
    HardwareDecoding(Option<String>),
    DroppedFrames(Option<i64>),
    DecoderDroppedFrames(Option<i64>),
    Cache {
        duration: Option<f64>,
        bytes: Option<i64>,
        input_bytes_per_second: Option<f64>,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct PlaybackFailure {
    pub code: Option<i32>,
    pub message: String,
}

impl PlaybackFailure {
    pub fn details(&self) -> (&'static str, &'static str) {
        use jfn_mpv::sys::mpv_error as Code;
        match self.code {
            Some(code) if code == Code::MPV_ERROR_LOADING_FAILED.0 => (
                "NETWORK_ERROR",
                "The media stream could not be opened. Check server connectivity and access to this item, then retry.",
            ),
            Some(code)
                if code == Code::MPV_ERROR_UNKNOWN_FORMAT.0
                    || code == Code::MPV_ERROR_UNSUPPORTED.0 =>
            {
                (
                    "MEDIA_NOT_SUPPORTED",
                    "mpv could not play this format. Try enabling Force Transcoding in client settings.",
                )
            }
            Some(code) if code == Code::MPV_ERROR_AO_INIT_FAILED.0 => (
                "PLAYER_ERROR",
                "Audio output could not be initialized. Check the audio device and exclusive-output settings.",
            ),
            Some(code) if code == Code::MPV_ERROR_VO_INIT_FAILED.0 => (
                "PLAYER_ERROR",
                "Video output could not be initialized. Check graphics drivers and hardware-decoding settings.",
            ),
            Some(code) if code == Code::MPV_ERROR_NOTHING_TO_PLAY.0 => (
                "NO_MEDIA_ERROR",
                "No playable audio or video stream was found. Try a different media source or server-side transcoding.",
            ),
            _ => (
                "PLAYER_ERROR",
                "Playback failed. Retry the item, or copy diagnostics from About for troubleshooting.",
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaybackSnapshot {
    pub presence: PlayerPresence,
    pub phase: PlaybackPhase,
    pub seeking: bool,
    pub buffering: bool,
    pub media_type: MediaType,
    pub position_us: i64,
    pub variant_switch_pending: bool,
    pub rate: f64,
    pub duration_us: i64,
    pub fullscreen: bool,
    pub maximized_before_fullscreen: bool,
    pub display_hz: f64,
    pub buffered: Vec<PlaybackBufferedRange>,
    pub diagnostics: PlaybackDiagnostics,
    pub last_error: Option<PlaybackFailure>,
}

impl PlaybackSnapshot {
    pub(crate) fn fresh() -> Self {
        Self {
            rate: 1.0,
            ..Default::default()
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackEventKind {
    Started = 0,
    Paused = 1,
    Finished = 2,
    Canceled = 3,
    Error = 4,
    SeekingChanged = 5,
    BufferingChanged = 6,
    MediaTypeChanged = 7,
    TrackLoaded = 8,
    PositionChanged = 9,
    DurationChanged = 10,
    RateChanged = 11,
    FullscreenChanged = 12,
    BufferedRangesChanged = 14,
    DisplayHzChanged = 15,
    MetadataChanged = 16,
    ArtworkChanged = 17,
    QueueCapsChanged = 18,
    Seeked = 19,
    DiagnosticsChanged = 20,
}

#[derive(Clone, Debug)]
pub struct PlaybackEvent {
    pub kind: PlaybackEventKind,
    pub flag: bool,
    pub error_message: String,
    pub error_code: Option<i32>,
    pub snapshot: PlaybackSnapshot,
    pub metadata: MediaMetadata,
    pub artwork_uri: String,
    pub can_go_next: bool,
    pub can_go_prev: bool,
}

impl PlaybackEvent {
    pub(crate) fn new(kind: PlaybackEventKind) -> Self {
        Self {
            kind,
            flag: false,
            error_message: String::new(),
            error_code: None,
            snapshot: PlaybackSnapshot::default(),
            metadata: MediaMetadata::default(),
            artwork_uri: String::new(),
            can_go_next: false,
            can_go_prev: false,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackActionKind {
    ApplyPendingTrackSelectionAndPlay = 0,
}

#[derive(Clone, Copy, Debug)]
pub struct PlaybackAction {
    pub kind: PlaybackActionKind,
}
