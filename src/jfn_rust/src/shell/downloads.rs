use iced_core::{Alignment, Element, Length, Padding, widget::Id};
use iced_widget::{Column, Widget as _, button, column, progress_bar, row, scrollable, text};
use jfn_cef::{DownloadRow, DownloadState};
use time::{OffsetDateTime, SignedDuration, UtcOffset};

use super::{
    controls,
    theme::{self, Theme},
};

#[derive(Clone, Debug)]
pub enum Message {
    Resume(usize),
    Stop(usize),
    OpenFolder,
}

pub(super) fn update(message: Message) {
    match message {
        Message::Resume(id) => jfn_cef::resume_download(id),
        Message::Stop(id) => jfn_cef::stop_download(id),
        Message::OpenFolder => {
            if let (Some(directory), Some(platform)) =
                (jfn_cef::download_directory(), jfn_platform_abi::try_lease())
            {
                platform.open_path(directory);
            }
        }
    }
}

pub(super) fn view() -> Element<'static, Message, Theme, iced_wgpu::Renderer> {
    let Some(directory) = jfn_cef::download_directory() else {
        return text(
            "No download folder is available. Start Jellium with --download-dir to choose one.",
        )
        .boxed();
    };
    let folder = controls::action(
        Id::new("downloads-folder"),
        button(text("Open folder"))
            .on_press(Message::OpenFolder)
            .class(theme::ButtonClass::Tab),
        Message::OpenFolder,
    );
    let rows = jfn_cef::download_rows();
    let header = row![
        text(directory.display().to_string()).width(Length::Fill),
        folder
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let list = if rows.is_empty() {
        column![
            text("No downloads yet").size(20),
            text("Use Download on a Jellyfin item to save it here.")
        ]
        .spacing(8)
    } else {
        Column::with_children(rows.into_iter().map(download_row)).spacing(18)
    };
    column![
        header,
        scrollable(list.padding(Padding::from([6, 10]))).height(Length::Fill)
    ]
    .spacing(18)
    .width(Length::Fill)
    .height(Length::Fill)
    .boxed()
}

fn download_row(download: DownloadRow) -> Element<'static, Message, Theme, iced_wgpu::Renderer> {
    let DownloadRow {
        id,
        name,
        received,
        total,
        bits_per_second,
        state,
    } = download;
    let percent = (u128::from(received) * 100 / u128::from(total)) as u8;
    let (status, action) = match state {
        DownloadState::Downloading => ("Downloading".into(), Some(("Stop", Message::Stop(id)))),
        DownloadState::Stopping => ("Stopping…".into(), None),
        DownloadState::Paused => (
            "Paused".into(),
            Some((
                if received == 0 { "Start" } else { "Resume" },
                Message::Resume(id),
            )),
        ),
        DownloadState::Complete => ("Complete".into(), None),
        DownloadState::Failed(reason) => (reason, Some(("Retry", Message::Resume(id)))),
    };
    let mut detail = format!(
        "{status} · {percent}% · {} / {} MiB",
        received / (1024 * 1024),
        total / (1024 * 1024),
    );
    if let Some(rate) = bits_per_second {
        detail.push_str(&format!(" · {} avg.", format_bitrate(rate)));
    }
    let mut title = row![text(name).size(16).width(Length::Fill)]
        .spacing(12)
        .align_y(Alignment::Center);
    if let Some((label, message)) = action {
        title = title.push(controls::action(
            Id::from(format!("download-action-{id}")),
            button(text(label))
                .on_press(message.clone())
                .class(theme::ButtonClass::Tab),
            message,
        ));
    }
    let mut row = column![
        title,
        progress_bar(0.0..=100.0, f32::from(percent)).girth(6),
        text(detail).size(13)
    ];
    if let Some(remaining) =
        bits_per_second.and_then(|rate| estimate_remaining(total.saturating_sub(received), rate))
    {
        let now = OffsetDateTime::now_utc();
        let mut estimate = format!("{} left", format_remaining(remaining));
        if let Some(finish) = now.checked_add(remaining) {
            estimate.push_str(&format!(
                " · Estimated finish: {}",
                local_finish(now, finish)
            ));
        }
        row = row.push(text(estimate).size(13).boxed());
    }
    row.spacing(6).width(Length::Fill).boxed()
}

fn estimate_remaining(bytes_left: u64, rate: f64) -> Option<SignedDuration> {
    if bytes_left == 0 || !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    SignedDuration::checked_seconds_f64((bytes_left as f64 * 8.0 / rate).ceil().max(1.0))
}

fn format_remaining(remaining: SignedDuration) -> String {
    let seconds = remaining.whole_seconds();
    let minutes = seconds / 60;
    let hours = minutes / 60;
    match (hours / 24, hours, minutes) {
        (days, _, _) if days > 0 => format!("{days}d {}h", hours % 24),
        (_, hours, _) if hours > 0 => format!("{hours}h {}m", minutes % 60),
        (_, _, minutes) if minutes > 0 => format!("{minutes}m {}s", seconds % 60),
        _ => format!("{seconds}s"),
    }
}

fn local_finish(now: OffsetDateTime, finish: OffsetDateTime) -> String {
    let local = |instant| {
        let offset = UtcOffset::local_offset_at(instant).ok()?;
        instant.checked_to_offset(offset)
    };
    match (local(now), local(finish)) {
        (Some(now), Some(finish)) => format_finish(now, finish),
        _ => format!("{} UTC", format_finish(now, finish)),
    }
}

fn format_finish(now: OffsetDateTime, finish: OffsetDateTime) -> String {
    let clock = format!("{:02}:{:02}", finish.hour(), finish.minute());
    if now.date() == finish.date() {
        clock
    } else {
        format!("{} {clock}", finish.date())
    }
}

fn format_bitrate(bits_per_second: f64) -> String {
    if bits_per_second >= 1_000_000.0 {
        format!("{:.1} Mbps", bits_per_second / 1_000_000.0)
    } else if bits_per_second >= 1_000.0 {
        format!("{:.1} kbps", bits_per_second / 1_000.0)
    } else {
        format!("{bits_per_second:.0} bps")
    }
}

impl iced_widget::progress_bar::Catalog for Theme {
    type Class<'a> = ();

    fn default<'a>() -> Self::Class<'a> {}

    fn style(&self, _: &Self::Class<'_>) -> iced_widget::progress_bar::Style {
        iced_widget::progress_bar::Style {
            background: theme::FIELD.into(),
            bar: theme::ACCENT.into(),
            border: iced_core::Border::default().rounded(3),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_time_uses_bits_and_rounds_up() {
        let remaining = estimate_remaining(9_001, 1_000.0);
        assert_eq!(remaining, Some(SignedDuration::seconds(73)));
        assert_eq!(remaining.map(format_remaining).as_deref(), Some("1m 13s"));
        assert!(estimate_remaining(0, 1_000.0).is_none());
        assert!(estimate_remaining(1_000, 0.0).is_none());
        assert!(estimate_remaining(1_000, f64::NAN).is_none());
    }

    #[test]
    fn finish_time_includes_date_after_midnight() -> Result<(), time::error::ComponentRange> {
        let now = OffsetDateTime::from_unix_timestamp(86_340)?;
        let same_day = OffsetDateTime::from_unix_timestamp(86_399)?;
        let next_day = OffsetDateTime::from_unix_timestamp(86_460)?;
        assert_eq!(format_finish(now, same_day), "23:59");
        assert_eq!(format_finish(now, next_day), "1970-01-02 00:01");
        Ok(())
    }

    #[test]
    fn bitrate_uses_decimal_bits_per_second() {
        assert_eq!(super::format_bitrate(800.0), "800 bps");
        assert_eq!(super::format_bitrate(12_500.0), "12.5 kbps");
        assert_eq!(super::format_bitrate(24_800_000.0), "24.8 Mbps");
    }
}
