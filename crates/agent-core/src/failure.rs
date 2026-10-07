//! Why a model call failed, in kinds the UI can act on: whether waiting and
//! trying again is worth it, and until when.
//!
//! A failure is classified where the most is known about it — the HTTP status
//! and error body for a built-in provider, the JSON-RPC error and its `data`
//! for an external agent — and from the error text otherwise. The text is the
//! last resort: it is what an older session log or an agent that reports
//! nothing structured leaves.

use std::time::Duration;

use chrono::{DateTime, Datelike, Local, LocalResult, NaiveDate, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compaction::is_context_overflow_error;

/// What kind of failure ended a model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// The network, a timeout, an overloaded or failing server: passes on its
    /// own, usually soon.
    Transient,
    /// A short-term rate limit (too many requests): passes within minutes.
    RateLimited,
    /// The account's usage limit, quota or credit: passes at its reset, if
    /// it has one.
    QuotaExhausted,
    /// Credentials missing, wrong or expired: needs a sign-in or a key.
    Auth,
    /// The request itself was refused (malformed, an unknown model):
    /// repeating it as it is changes nothing.
    BadRequest,
    /// The conversation no longer fits the model's context.
    ContextExhausted,
    /// The external agent's process ended or stopped answering.
    AgentDied,
    #[default]
    Unknown,
}

impl FailureKind {
    /// Whether the failure passes on its own, so that waiting and trying
    /// again is worth it.
    #[must_use]
    pub fn passes(self) -> bool {
        matches!(
            self,
            Self::Transient | Self::RateLimited | Self::QuotaExhausted
        )
    }
}

/// A classified failure of a model call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    /// When the limit resets, in Unix seconds, when the error said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<u64>,
}

/// How long a limit without a stated reset is waited out before the next
/// try, at first; doubled each try up to [`MAX_UNKNOWN_WAIT`].
const FIRST_LIMIT_WAIT: Duration = Duration::from_secs(60);
/// The first wait after a transient failure the provider's own retries did
/// not get past; doubled each try up to [`MAX_TRANSIENT_WAIT`].
const FIRST_TRANSIENT_WAIT: Duration = Duration::from_secs(15);
const MAX_TRANSIENT_WAIT: Duration = Duration::from_secs(5 * 60);
const MAX_UNKNOWN_WAIT: Duration = Duration::from_secs(30 * 60);
/// Added to a stated reset: a clock that runs a little ahead of the
/// provider's, or a reset rounded down to the minute, would otherwise try a
/// moment too early and fail again.
const RESET_GRACE: Duration = Duration::from_secs(30);

impl Failure {
    #[must_use]
    pub fn new(kind: FailureKind) -> Self {
        Self {
            kind,
            resets_at: None,
        }
    }

    #[must_use]
    pub fn with_reset(mut self, resets_at: Option<u64>) -> Self {
        if resets_at.is_some() {
            self.resets_at = resets_at;
        }
        self
    }

    /// The failure with the reset time `text` states, when it has none yet
    /// and waiting cures it.
    #[must_use]
    pub fn with_reset_from(mut self, text: &str) -> Self {
        if self.resets_at.is_none() && self.kind.passes() {
            self.resets_at = parse_reset(text, Local::now());
        }
        self
    }

    /// Classify a failure from its text alone, reading a reset time out of
    /// it when it states one.
    #[must_use]
    pub fn from_text(message: &str) -> Self {
        Self::from_text_at(message, Local::now())
    }

    /// [`Self::from_text`] at a given moment, for tests.
    #[must_use]
    pub fn from_text_at(message: &str, now: DateTime<Local>) -> Self {
        let kind = kind_of_text(message);
        let resets_at = if kind.passes() {
            parse_reset(message, now)
        } else {
            None
        };
        Self { kind, resets_at }
    }

    /// Classify an HTTP error response: its status, its body (the error
    /// objects of OpenAI, Anthropic and Gemini are read) and its
    /// `Retry-After` header.
    #[must_use]
    pub fn from_http(status: u16, body: &str, retry_after: Option<&str>) -> Self {
        Self::from_http_at(status, body, retry_after, Local::now())
    }

    /// [`Self::from_http`] at a given moment, for tests.
    #[must_use]
    pub fn from_http_at(
        status: u16,
        body: &str,
        retry_after: Option<&str>,
        now: DateTime<Local>,
    ) -> Self {
        let codes = error_codes(body);
        let has = |names: &[&str]| codes.iter().any(|code| names.contains(&code.as_str()));
        let text_kind = kind_of_text(body);
        let kind = if is_context_overflow_error(body) || has(&["request_too_large"]) {
            FailureKind::ContextExhausted
        } else if has(&["insufficient_quota", "billing_error", "billing_not_active"])
            || status == 402
            || text_kind == FailureKind::QuotaExhausted
        {
            FailureKind::QuotaExhausted
        } else if has(&[
            "rate_limit_error",
            "rate_limit_exceeded",
            "resource_exhausted",
        ]) || status == 429
        {
            FailureKind::RateLimited
        } else if has(&[
            "overloaded_error",
            "unavailable",
            "api_error",
            "server_error",
        ]) {
            FailureKind::Transient
        } else if has(&[
            "authentication_error",
            "invalid_api_key",
            "permission_error",
            "unauthenticated",
            "permission_denied",
        ]) || matches!(status, 401 | 403)
        {
            FailureKind::Auth
        } else if matches!(status, 408 | 409 | 425 | 500..=599) {
            FailureKind::Transient
        } else if status == 413 {
            FailureKind::ContextExhausted
        } else if (400..500).contains(&status) {
            FailureKind::BadRequest
        } else {
            text_kind
        };
        let resets_at = if kind.passes() {
            retry_after
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|secs| secs.is_finite() && *secs >= 0.0)
                .map(|secs| unix(now) + secs.ceil() as u64)
                .or_else(|| parse_reset(body, now))
        } else {
            None
        };
        Self { kind, resets_at }
    }

    /// How long to wait before trying again for the `attempt`-th time (from
    /// zero), `now` in Unix seconds: until the stated reset, or a backoff
    /// that grows with each attempt when none is stated. `None` for a
    /// failure waiting does not cure.
    #[must_use]
    pub fn retry_delay(&self, attempt: u32, now: u64) -> Option<Duration> {
        if !self.kind.passes() {
            return None;
        }
        if let Some(at) = self.resets_at {
            return Some(Duration::from_secs(at.saturating_sub(now)) + RESET_GRACE);
        }
        let (first, cap) = match self.kind {
            FailureKind::Transient => (FIRST_TRANSIENT_WAIT, MAX_TRANSIENT_WAIT),
            _ => (FIRST_LIMIT_WAIT, MAX_UNKNOWN_WAIT),
        };
        Some((first * 2u32.saturating_pow(attempt.min(16))).min(cap))
    }
}

/// What to do when a run stops on a failure that passes on its own — a
/// usage or rate limit, a network outage — once the provider's own quick
/// retries are spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitPolicy {
    /// Show the choices and let the user pick.
    #[default]
    Ask,
    /// Wait for the reset (or back off) and try again on its own.
    Wait,
    /// Stop, as any other failure does.
    Stop,
}

impl LimitPolicy {
    pub const ALL: [LimitPolicy; 3] = [Self::Ask, Self::Wait, Self::Stop];

    /// The value as the config file writes it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Wait => "wait",
            Self::Stop => "stop",
        }
    }
}

/// The kind a failure's text speaks of. Phrases are checked in order of how
/// much they tell: a usage limit also reads as a rate limit, a dead agent's
/// "connection" as a network error.
fn kind_of_text(message: &str) -> FailureKind {
    let text = message.to_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    if any(&[
        "closed the connection",
        "the agent exited",
        "agent was shut down",
        "cannot write to the agent",
        "process exited",
        "transport_lost",
        "worker_shutdown",
    ]) {
        FailureKind::AgentDied
    } else if is_context_overflow_error(message) {
        FailureKind::ContextExhausted
    } else if any(&[
        "usage limit",
        "session limit",
        "weekly limit",
        "hit your limit",
        "out of extra usage",
        "insufficient_quota",
        "exceeded your current quota",
        "quota exceeded",
        "credit balance",
        "out of credits",
        "billing",
    ]) {
        FailureKind::QuotaExhausted
    } else if any(&[
        "rate limit",
        "rate_limit",
        "rate-limit",
        "too many requests",
        "http 429",
        "resource_exhausted",
    ]) {
        FailureKind::RateLimited
    } else if any(&[
        "authenticat",
        "unauthorized",
        "/login",
        "please log in",
        "please sign in",
        "invalid api key",
        "invalid x-api-key",
        "api key not valid",
        "http 401",
        "http 403",
    ]) {
        FailureKind::Auth
    } else if any(&[
        "overloaded",
        "temporarily unavailable",
        "service unavailable",
        "bad gateway",
        "gateway timeout",
        "internal server error",
        "server error",
        "http 5",
        "timed out",
        "timeout",
        "no reply within",
        "transport error",
        "stream error",
        "stream interrupted",
        "connection",
        "network",
        "dns",
        "unreachable",
    ]) {
        FailureKind::Transient
    } else if any(&[
        "invalid_request",
        "invalid request",
        "model_not_found",
        "model not found",
        "http 400",
        "http 404",
        "http 422",
    ]) {
        FailureKind::BadRequest
    } else {
        FailureKind::Unknown
    }
}

/// The machine-readable codes of an error body, lower-cased: OpenAI's
/// `error.code` and `error.type`, Anthropic's `error.type`, Gemini's
/// `error.status`, whatever of them the body has.
fn error_codes(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    // Gemini answers a list with one error object in it.
    let value = match value {
        Value::Array(mut items) if !items.is_empty() => items.swap_remove(0),
        other => other,
    };
    let error = value.get("error").unwrap_or(&value);
    ["code", "type", "status"]
        .iter()
        .filter_map(|key| error.get(*key).and_then(Value::as_str))
        .map(str::to_lowercase)
        .collect()
}

fn unix(time: DateTime<Local>) -> u64 {
    u64::try_from(time.timestamp()).unwrap_or(0)
}

/// When a limit resets, as the error text states it: after a span ("try
/// again in 4 days 3 hours", "retry in 41.5s") or at a time of day, with or
/// without a date ("resets 9:50pm", "try again at Oct 9th, 2026 5:00 PM").
/// A time names the local clock — a zone named beside it is not read — and
/// one already past today is tomorrow's.
#[must_use]
pub fn parse_reset(text: &str, now: DateTime<Local>) -> Option<u64> {
    let lower = text.to_lowercase();
    for marker in [
        "resets at ",
        "resets ",
        "reset at ",
        "try again at ",
        "available again at ",
        "retry after ",
        "try again after ",
        "try again in ",
        "retry in ",
        "resets in ",
        "available again in ",
    ] {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let rest = &lower[start + marker.len()..];
        let found = if marker.ends_with("in ") {
            parse_span(rest).map(|span| unix(now) + span.as_secs())
        } else {
            parse_span(rest)
                .map(|span| unix(now) + span.as_secs())
                .or_else(|| parse_moment(rest, now))
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// A span such as `4 days 3 hours 2 minutes`, `2h 5m`, `41.5s`: number and
/// unit pairs, summed; stops at the first word that is neither.
fn parse_span(text: &str) -> Option<Duration> {
    let mut total = 0.0_f64;
    let mut seen = false;
    let mut rest = text.trim_start();
    loop {
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if digits == 0 {
            break;
        }
        let Ok(number) = rest[..digits].parse::<f64>() else {
            break;
        };
        let after = rest[digits..].trim_start();
        let unit_len = after
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(after.len());
        let seconds = match &after[..unit_len] {
            "d" | "day" | "days" => 86_400.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "ms" => 0.001,
            _ => break,
        };
        total += number * seconds;
        seen = true;
        rest = after[unit_len..]
            .trim_start_matches(|c: char| c == ',' || c.is_whitespace())
            .trim_start_matches("and ")
            .trim_start();
    }
    (seen && total.is_finite()).then(|| Duration::from_secs(total.ceil() as u64))
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// A moment: an optional `Mon D[th][, YYYY]`, an optional `at`, then a time
/// of day — `9:50pm`, `5 pm`, `21:50`. A bare time already past is
/// tomorrow's; a date without a year already past is next year's.
fn parse_moment(text: &str, now: DateTime<Local>) -> Option<u64> {
    let mut rest = text.trim_start();
    let mut date = None;
    if let Some(month) = MONTHS.iter().position(|m| rest.starts_with(m)) {
        let after_name = rest.trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '.');
        let after_name = after_name.trim_start();
        let day_len = after_name
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_name.len());
        let day: u32 = after_name[..day_len].parse().ok()?;
        let mut tail = after_name[day_len..]
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .trim_start_matches(',')
            .trim_start();
        let year_len = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        let year = if year_len == 4 {
            let year: i32 = tail[..year_len].parse().ok()?;
            tail = tail[year_len..].trim_start_matches(',').trim_start();
            Some(year)
        } else {
            None
        };
        date = Some((month as u32 + 1, day, year));
        rest = tail;
    }
    rest = rest.strip_prefix("at ").unwrap_or(rest).trim_start();
    let time = parse_time_of_day(rest)?;
    let today = now.date_naive();
    let local = |day: NaiveDate| match Local.from_local_datetime(&day.and_time(time)) {
        LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => Some(at),
        LocalResult::None => None,
    };
    let at = match date {
        Some((month, day, Some(year))) => local(NaiveDate::from_ymd_opt(year, month, day)?)?,
        Some((month, day, None)) => {
            let this_year = local(NaiveDate::from_ymd_opt(today.year(), month, day)?)?;
            if this_year + chrono::Duration::days(1) < now {
                local(NaiveDate::from_ymd_opt(today.year() + 1, month, day)?)?
            } else {
                this_year
            }
        }
        None => {
            let at = local(today)?;
            if at <= now {
                local(today.succ_opt()?)?
            } else {
                at
            }
        }
    };
    Some(unix(at))
}

/// `9:50pm`, `9:50 PM`, `5pm`, `21:50`, `9.50pm`; `None` for anything else.
fn parse_time_of_day(text: &str) -> Option<NaiveTime> {
    let hour_len = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    if hour_len == 0 || hour_len > 2 {
        return None;
    }
    let mut hour: u32 = text[..hour_len].parse().ok()?;
    let mut rest = &text[hour_len..];
    let mut minute = 0;
    let mut has_minutes = false;
    if let Some(after) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('.')) {
        let minute_len = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if minute_len != 2 {
            return None;
        }
        minute = after[..2].parse().ok()?;
        rest = &after[2..];
        has_minutes = true;
    }
    let rest = rest.trim_start();
    let meridiem = if rest.starts_with("am") || rest.starts_with("a.m") {
        Some(false)
    } else if rest.starts_with("pm") || rest.starts_with("p.m") {
        Some(true)
    } else {
        None
    };
    match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            hour %= 12;
            if pm {
                hour += 12;
            }
        }
        // A bare number is no time of day ("in 5"); `21:50` is.
        None if !has_minutes => return None,
        None => {}
    }
    NaiveTime::from_hms_opt(hour, minute, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-07 18:00 local time.
    fn evening() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 7, 18, 0, 0).unwrap()
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> u64 {
        unix(Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap())
    }

    #[test]
    fn a_claude_session_limit_is_a_quota_that_resets_at_its_time() {
        let failure = Failure::from_text_at(
            "session/prompt: Internal error: You've hit your session limit · resets 9:50pm (Europe/Samara) (code -32603)",
            evening(),
        );
        assert_eq!(failure.kind, FailureKind::QuotaExhausted);
        assert_eq!(failure.resets_at, Some(at(2026, 10, 7, 21, 50)));
    }

    #[test]
    fn a_time_already_past_today_is_tomorrows() {
        let failure = Failure::from_text_at("You've hit your limit · resets 5am", evening());
        assert_eq!(failure.resets_at, Some(at(2026, 10, 8, 5, 0)));
    }

    #[test]
    fn a_weekly_limit_names_its_date() {
        let failure = Failure::from_text_at(
            "You've hit your weekly limit · resets Oct 9, 5pm",
            evening(),
        );
        assert_eq!(failure.kind, FailureKind::QuotaExhausted);
        assert_eq!(failure.resets_at, Some(at(2026, 10, 9, 17, 0)));
        let failure = Failure::from_text_at(
            "You've hit your usage limit. Try again at Oct 9th, 2026 5:00 PM.",
            evening(),
        );
        assert_eq!(failure.resets_at, Some(at(2026, 10, 9, 17, 0)));
    }

    #[test]
    fn a_span_counts_from_now() {
        let now = evening();
        let failure = Failure::from_text_at(
            "You've hit your usage limit. Upgrade to Pro or try again in 4 days 3 hours 2 minutes.",
            now,
        );
        assert_eq!(
            failure.resets_at,
            Some(unix(now) + 4 * 86_400 + 3 * 3_600 + 2 * 60)
        );
        let failure = Failure::from_text_at("Quota exceeded. Please retry in 41.5s.", now);
        assert_eq!(failure.resets_at, Some(unix(now) + 42));
    }

    #[test]
    fn a_rate_limit_without_a_time_has_no_reset() {
        let failure = Failure::from_text_at("Rate limit reached for requests", evening());
        assert_eq!(failure.kind, FailureKind::RateLimited);
        assert_eq!(failure.resets_at, None);
    }

    #[test]
    fn texts_name_their_kinds() {
        let kind = |text: &str| Failure::from_text_at(text, evening()).kind;
        assert_eq!(
            kind("session/prompt: the agent closed the connection"),
            FailureKind::AgentDied
        );
        assert_eq!(
            kind("transport error: Connection refused"),
            FailureKind::Transient
        );
        assert_eq!(kind("HTTP 529: Overloaded"), FailureKind::Transient);
        assert_eq!(
            kind("Invalid API key · Please run /login"),
            FailureKind::Auth
        );
        assert_eq!(
            kind("prompt is too long: 210000 tokens > 200000 maximum"),
            FailureKind::ContextExhausted
        );
        assert_eq!(kind("HTTP 404: model_not_found"), FailureKind::BadRequest);
        assert_eq!(kind("something odd"), FailureKind::Unknown);
        // A clock time in a message that is no limit is not read as a reset.
        assert_eq!(
            Failure::from_text_at("HTTP 400: bad field, resets 9:50pm", evening()).resets_at,
            None
        );
    }

    #[test]
    fn http_errors_are_read_from_status_body_and_header() {
        let now = evening();
        let failure = Failure::from_http_at(
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
            Some("30"),
            now,
        );
        assert_eq!(failure.kind, FailureKind::RateLimited);
        assert_eq!(failure.resets_at, Some(unix(now) + 30));

        let quota = Failure::from_http_at(
            429,
            r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
            None,
            now,
        );
        assert_eq!(quota.kind, FailureKind::QuotaExhausted);

        let gemini = Failure::from_http_at(
            429,
            r#"[{"error":{"code":429,"message":"Resource has been exhausted","status":"RESOURCE_EXHAUSTED"}}]"#,
            None,
            now,
        );
        assert_eq!(gemini.kind, FailureKind::RateLimited);

        let kind = |status, body: &str| Failure::from_http_at(status, body, None, now).kind;
        assert_eq!(
            kind(529, r#"{"error":{"type":"overloaded_error"}}"#),
            FailureKind::Transient
        );
        assert_eq!(kind(401, "{}"), FailureKind::Auth);
        assert_eq!(kind(402, "{}"), FailureKind::QuotaExhausted);
        assert_eq!(kind(503, ""), FailureKind::Transient);
        assert_eq!(
            kind(
                400,
                r#"{"error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#
            ),
            FailureKind::ContextExhausted
        );
        assert_eq!(
            kind(400, r#"{"error":{"type":"invalid_request_error"}}"#),
            FailureKind::BadRequest
        );
        // A header on a failure waiting does not cure is not a reset.
        assert_eq!(
            Failure::from_http_at(401, "{}", Some("30"), now).resets_at,
            None
        );
    }

    #[test]
    fn the_wait_runs_to_the_reset_or_backs_off() {
        let reset = Failure {
            kind: FailureKind::QuotaExhausted,
            resets_at: Some(1_000),
        };
        assert_eq!(
            reset.retry_delay(0, 400),
            Some(Duration::from_secs(600) + RESET_GRACE)
        );
        // A reset already past waits only the grace.
        assert_eq!(reset.retry_delay(3, 2_000), Some(RESET_GRACE));

        let network = Failure::new(FailureKind::Transient);
        assert_eq!(network.retry_delay(0, 0), Some(FIRST_TRANSIENT_WAIT));
        assert_eq!(network.retry_delay(1, 0), Some(FIRST_TRANSIENT_WAIT * 2));
        assert_eq!(network.retry_delay(30, 0), Some(MAX_TRANSIENT_WAIT));

        let limit = Failure::new(FailureKind::RateLimited);
        assert_eq!(limit.retry_delay(0, 0), Some(FIRST_LIMIT_WAIT));
        assert_eq!(limit.retry_delay(30, 0), Some(MAX_UNKNOWN_WAIT));

        assert_eq!(Failure::new(FailureKind::Auth).retry_delay(0, 0), None);
    }

    #[test]
    fn times_of_day_read_in_their_common_spellings() {
        let time = parse_time_of_day;
        assert_eq!(time("9:50pm"), NaiveTime::from_hms_opt(21, 50, 0));
        assert_eq!(time("9:50 pm"), NaiveTime::from_hms_opt(21, 50, 0));
        assert_eq!(time("12am"), NaiveTime::from_hms_opt(0, 0, 0));
        assert_eq!(time("12 pm"), NaiveTime::from_hms_opt(12, 0, 0));
        assert_eq!(time("21:50"), NaiveTime::from_hms_opt(21, 50, 0));
        assert_eq!(time("5"), None);
        assert_eq!(time("13pm"), None);
    }
}
