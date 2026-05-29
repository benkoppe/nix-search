use std::time::Duration;

use maud::{Markup, html};
use time::OffsetDateTime;

use crate::AppState;
use crate::maintenance;

const PROJECT_GITHUB_URL: &str = "https://github.com/benkoppe/nixsearch";

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
                div.footer-status {
                    span.footer-updated { (updated_text) }
                    @if let Some(next) = &next_text {
                        span.footer-separator { "\u{a0}\u{b7}\u{a0}" }
                        span.footer-next { (next) }
                    }
                }
                a.footer-link href=(PROJECT_GITHUB_URL) target="_blank" rel="noopener noreferrer"
                    aria-label="nixsearch GitHub repository" {
                    svg.footer-github-mark xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" aria-hidden="true" focusable="false" {
                        path fill="currentColor" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82A7.6 7.6 0 0 1 8 3.87c.68 0 1.36.09 2 .26 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z" {}
                    }
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
