//! Per-account Gmail quota gate.
//!
//! Header sync and body download share one user's quota. JMAP never consults
//! this. A quota response pauses that Gmail account until Retry-After, or for
//! one published per-minute window when the server omits the header. The gate
//! does not invent a faster send rate.

use std::collections::{BTreeMap, VecDeque};

/// Gmail's per-user budget cited by the metadata backfill interval.
pub(crate) const USER_UNITS_PER_MINUTE: u32 = 6_000;
pub(crate) const WINDOW_MS: i64 = 60_000;
/// `messages.get` cost used to size the metadata page. Bodies use the same method.
pub(crate) const MESSAGES_GET_UNITS: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryAfter {
    /// Delta seconds from `Retry-After: 120`.
    Seconds(i64),
    /// Absolute instant in milliseconds from an HTTP-date.
    UntilMs(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QuotaNotice {
    pub retry_after: Option<RetryAfter>,
    /// Daily quotas do not refill when the minute turns.
    pub daily: bool,
}

#[derive(Debug, Default)]
struct AccountPace {
    pause_until_ms: i64,
    spent: VecDeque<(i64, u32)>,
}

/// Sliding one-minute budget plus a hard pause, keyed by connected account.
#[derive(Debug, Default)]
pub(crate) struct GmailBudget {
    accounts: BTreeMap<i64, AccountPace>,
}

impl GmailBudget {
    /// Milliseconds to wait before spending `units` at `now_ms`.
    /// A wait of 0 records the spend. A positive wait records nothing.
    pub(crate) fn delay_ms(&mut self, account_id: i64, units: u32, now_ms: i64) -> i64 {
        let pace = self.accounts.entry(account_id).or_default();
        if pace.pause_until_ms > now_ms {
            return pace.pause_until_ms - now_ms;
        }
        pace.spent
            .retain(|(at, _)| now_ms.saturating_sub(*at) < WINDOW_MS);
        let used = pace.spent.iter().map(|(_, units)| *units).sum::<u32>();
        if units == 0 || used.saturating_add(units) <= USER_UNITS_PER_MINUTE {
            pace.spent.push_back((now_ms, units));
            return 0;
        }
        let mut freed = 0u32;
        let need = used.saturating_add(units) - USER_UNITS_PER_MINUTE;
        for (at, spent) in &pace.spent {
            freed = freed.saturating_add(*spent);
            if freed >= need {
                return (at + WINDOW_MS).saturating_sub(now_ms).max(1);
            }
        }
        WINDOW_MS
    }

    pub(crate) fn note_quota(&mut self, account_id: i64, now_ms: i64, notice: QuotaNotice) {
        let until = pause_until_ms(now_ms, notice);
        let pace = self.accounts.entry(account_id).or_default();
        if until > pace.pause_until_ms {
            pace.pause_until_ms = until;
        }
    }

    pub(crate) fn paused(&self, account_id: i64, now_ms: i64) -> bool {
        self.accounts
            .get(&account_id)
            .is_some_and(|pace| pace.pause_until_ms > now_ms)
    }
}

pub(crate) fn pause_until_ms(now_ms: i64, notice: QuotaNotice) -> i64 {
    match notice.retry_after {
        Some(RetryAfter::Seconds(seconds)) => {
            now_ms.saturating_add(seconds.saturating_mul(1000).max(0))
        }
        Some(RetryAfter::UntilMs(until)) => until.max(now_ms),
        None if notice.daily => now_ms.saturating_add(15 * WINDOW_MS),
        None => now_ms.saturating_add(WINDOW_MS),
    }
}

/// `Retry-After` as delta-seconds or an HTTP-date. `now_ms` is the fake clock
/// for the date form. Unreadable values are ignored so the caller uses the
/// per-minute window instead of guessing a short retry.
pub(crate) fn parse_retry_after(value: &str, now_ms: i64) -> Option<RetryAfter> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(RetryAfter::Seconds(seconds.max(0)));
    }
    httpdate_ms(value).map(|absolute| RetryAfter::UntilMs(absolute.max(now_ms)))
}

pub(crate) fn quota_notice(
    message: &str,
    retry_after: Option<&str>,
    now_ms: i64,
) -> Option<QuotaNotice> {
    if !is_quota_message(message) {
        return None;
    }
    let embedded = message
        .split("retry-after=")
        .nth(1)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let daily = message.to_ascii_lowercase().contains("daily");
    Some(QuotaNotice {
        retry_after: retry_after
            .or(embedded)
            .and_then(|value| parse_retry_after(value, now_ms)),
        daily,
    })
}

pub(crate) fn is_quota_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("rate limited")
        || message.contains("ratelimitexceeded")
        || message.contains("userratelimitexceeded")
        || message.contains("quotaexceeded")
        || message.contains("dailylimitexceeded")
        || message.contains("units per minute")
        || message.contains("quota exceeded")
}

fn httpdate_ms(value: &str) -> Option<i64> {
    // IMF-fixdate: "Sun, 06 Nov 1994 08:49:37 GMT"
    let mut parts = value.split_whitespace();
    let _weekday = parts.next()?;
    let day: u32 = parts.next()?.trim_end_matches(',').parse().ok()?;
    let month = month_index(parts.next()?)?;
    let year: i32 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    let mut clock = time.split(':');
    let hour: u32 = clock.next()?.parse().ok()?;
    let minute: u32 = clock.next()?.parse().ok()?;
    let second: u32 = clock.next()?.parse().ok()?;
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let naive = date.and_hms_opt(hour, minute, second)?;
    Some(naive.and_utc().timestamp_millis())
}

fn month_index(name: &str) -> Option<u32> {
    Some(match &name.to_ascii_lowercase()[..] {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

pub(crate) fn shared() -> &'static std::sync::Mutex<GmailBudget> {
    static BUDGET: std::sync::OnceLock<std::sync::Mutex<GmailBudget>> = std::sync::OnceLock::new();
    BUDGET.get_or_init(|| std::sync::Mutex::new(GmailBudget::default()))
}

pub(crate) fn reserve_delay_ms(account_id: i64, units: u32, now_ms: i64) -> i64 {
    let mut budget = shared().lock().unwrap_or_else(|poison| poison.into_inner());
    budget.delay_ms(account_id, units, now_ms)
}

pub(crate) fn note_account_quota(account_id: i64, now_ms: i64, notice: QuotaNotice) {
    let mut budget = shared().lock().unwrap_or_else(|poison| poison.into_inner());
    budget.note_quota(account_id, now_ms, notice);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_minute_quota_without_retry_after_waits_one_window() {
        let now = 1_700_000_000_000;
        let notice = quota_notice(
            "Gmail rate limited: Quota exceeded for quota metric 'Queries' and limit 'Queries per minute per user'.",
            None,
            now,
        )
        .unwrap();
        assert!(!notice.daily);
        assert_eq!(pause_until_ms(now, notice), now + WINDOW_MS);
        let mut budget = GmailBudget::default();
        budget.note_quota(2, now, notice);
        assert!(budget.paused(2, now + 1_000));
        assert!(!budget.paused(2, now + WINDOW_MS));
        assert!(
            !budget.paused(7, now + 1_000),
            "another account, including JMAP, is not paused"
        );
    }

    #[test]
    fn retry_after_seconds_and_http_date_set_the_pause() {
        let now = 1_700_000_000_000;
        let seconds = quota_notice(
            "Gmail rate limited: Units per minute per user",
            Some("120"),
            now,
        )
        .unwrap();
        assert_eq!(seconds.retry_after, Some(RetryAfter::Seconds(120)));
        assert_eq!(pause_until_ms(now, seconds), now + 120_000);

        let expected = chrono::NaiveDate::from_ymd_opt(2024, 11, 6)
            .unwrap()
            .and_hms_opt(8, 49, 37)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert!(expected > now);
        assert_eq!(
            parse_retry_after("Wed, 06 Nov 2024 08:49:37 GMT", now).unwrap(),
            RetryAfter::UntilMs(expected)
        );
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", now).unwrap(),
            RetryAfter::UntilMs(now),
            "a Retry-After date already in the past does not extend the pause"
        );
        assert!(parse_retry_after("not-a-date", now).is_none());
    }

    #[test]
    fn the_shared_minute_fills_and_the_next_gmail_call_waits() {
        let mut budget = GmailBudget::default();
        let now = 5_000_000;
        assert_eq!(budget.delay_ms(2, USER_UNITS_PER_MINUTE, now), 0);
        let wait = budget.delay_ms(2, MESSAGES_GET_UNITS, now + 2_000);
        assert_eq!(wait, WINDOW_MS - 2_000);
        assert_eq!(budget.delay_ms(7, MESSAGES_GET_UNITS, now + 2_000), 0);
        assert_eq!(
            budget.delay_ms(2, MESSAGES_GET_UNITS, now + WINDOW_MS),
            0,
            "the clock, not a fixed sleep, reopens the window"
        );
    }

    #[test]
    fn a_quota_pause_blocks_even_when_the_minute_still_has_room() {
        let mut budget = GmailBudget::default();
        let now = 9_000;
        budget.note_quota(
            2,
            now,
            QuotaNotice {
                retry_after: Some(RetryAfter::Seconds(90)),
                daily: false,
            },
        );
        assert_eq!(budget.delay_ms(2, 5, now + 1_000), 89_000);
        assert_eq!(budget.delay_ms(3, 5, now + 1_000), 0);
    }
}
