use std::time::Duration;

use maud::{Markup, html};
use time::OffsetDateTime;

use crate::AppState;
use crate::maintenance;

pub fn render_footer(state: &AppState) -> Markup {
    let generated_at = *state
        .generated_at
        .read()
        .expect("generated_at lock poisoned");
    let now = OffsetDateTime::now_utc();

    let elapsed = duration_between(generated_at, now);
    let updated_text = format!("Updated {}", format_elapsed(elapsed));

    let next_text = if state.config.server.schedule.enabled {
        let interval = state
            .config
            .server
            .schedule
            .parse_interval()
            .expect("schedule interval already validated");

        maintenance::next_due(generated_at, interval).map(|next_due| {
            if now >= next_due {
                "updating soon".to_owned()
            } else {
                let remaining = duration_between(now, next_due);
                format!("next in {}", format_remaining(remaining))
            }
        })
    } else {
        None
    };

    html! {
        footer.footer {
            div.footer-inner {
                span.footer-updated { (updated_text) }
                @if let Some(next) = &next_text {
                    span.footer-separator { "\u{a0}\u{b7}\u{a0}" }
                    span.footer-next { (next) }
                }
            }
        }
    }
}

fn duration_between(earlier: OffsetDateTime, later: OffsetDateTime) -> Duration {
    if later <= earlier {
        return Duration::ZERO;
    }

    (later - earlier).try_into().unwrap_or(Duration::ZERO)
}

fn format_elapsed(duration: Duration) -> String {
    let secs = duration.as_secs();

    if secs < 60 {
        return "just now".to_owned();
    }

    let minutes = secs / 60;

    if minutes < 60 {
        return format!("{minutes}m ago");
    }

    let hours = minutes / 60;

    if hours < 48 {
        return format!("{hours}h ago");
    }

    let days = hours / 24;
    format!("{days}d ago")
}

fn format_remaining(duration: Duration) -> String {
    let secs = duration.as_secs();

    if secs < 60 {
        return "<1m".to_owned();
    }

    let minutes = secs / 60;

    if minutes < 60 {
        return format!("{minutes}m");
    }

    let hours = minutes / 60;

    if hours < 48 {
        return format!("{hours}h");
    }

    let days = hours / 24;
    format!("{days}d")
}

#[cfg(test)]
mod tests {
    use super::{format_elapsed, format_remaining};
    use std::time::Duration;

    #[test]
    fn elapsed_just_now() {
        assert_eq!(format_elapsed(Duration::from_secs(0)), "just now");
        assert_eq!(format_elapsed(Duration::from_secs(59)), "just now");
    }

    #[test]
    fn elapsed_minutes() {
        assert_eq!(format_elapsed(Duration::from_secs(60)), "1m ago");
        assert_eq!(format_elapsed(Duration::from_secs(45 * 60)), "45m ago");
    }

    #[test]
    fn elapsed_hours() {
        assert_eq!(format_elapsed(Duration::from_secs(3600)), "1h ago");
        assert_eq!(format_elapsed(Duration::from_secs(47 * 3600)), "47h ago");
    }

    #[test]
    fn elapsed_days() {
        assert_eq!(format_elapsed(Duration::from_secs(48 * 3600)), "2d ago");
        assert_eq!(format_elapsed(Duration::from_secs(7 * 24 * 3600)), "7d ago");
    }

    #[test]
    fn remaining_under_minute() {
        assert_eq!(format_remaining(Duration::from_secs(0)), "<1m");
        assert_eq!(format_remaining(Duration::from_secs(59)), "<1m");
    }

    #[test]
    fn remaining_minutes() {
        assert_eq!(format_remaining(Duration::from_secs(60)), "1m");
        assert_eq!(format_remaining(Duration::from_secs(30 * 60)), "30m");
    }

    #[test]
    fn remaining_hours() {
        assert_eq!(format_remaining(Duration::from_secs(3600)), "1h");
        assert_eq!(format_remaining(Duration::from_secs(23 * 3600)), "23h");
    }

    #[test]
    fn remaining_days() {
        assert_eq!(format_remaining(Duration::from_secs(48 * 3600)), "2d");
        assert_eq!(format_remaining(Duration::from_secs(5 * 24 * 3600)), "5d");
    }
}
