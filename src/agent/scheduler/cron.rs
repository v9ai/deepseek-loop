//! 5-field cron grammar wrapper.
//!
//! Wraps the [`cron`] crate with stricter parsing: only standard 5-field
//! `minute hour day-of-month month day-of-week` expressions are accepted.
//! Extended syntax (`L`, `W`, `?`, name aliases like `MON`/`JAN`) is rejected
//! to match the Claude Code spec.

use std::str::FromStr;

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronExpr {
    /// The original 5-field expression, exactly as the user provided it.
    expr: String,
    /// 6-field form (`0 <expr>`) cached so we don't re-parse on every fire.
    #[serde(skip)]
    parsed: Option<cron::Schedule>,
}

impl CronExpr {
    /// Parse a 5-field cron expression. Rejects extended syntax.
    pub fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input.trim();
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "cron: expected 5 fields (min hour dom month dow), got {}",
                fields.len()
            ));
        }

        // Reject extended syntax. The `cron` crate accepts `L`/`W`/`?` and
        // name aliases (`MON`, `JAN`); the Claude Code spec doesn't, and it's
        // simpler to reject than to whitelist.
        for f in &fields {
            for ch in f.chars() {
                let ok = ch.is_ascii_digit() || matches!(ch, '*' | '/' | ',' | '-');
                if !ok {
                    return Err(format!(
                        "cron: unsupported character '{ch}' in field '{f}' \
                         (extended syntax like L/W/?/MON/JAN is not supported)"
                    ));
                }
            }
        }

        // Prepend `0 ` for seconds — the `cron` crate uses 6-field internally.
        let six = format!("0 {trimmed}");
        let parsed =
            cron::Schedule::from_str(&six).map_err(|e| format!("cron: parse error: {e}"))?;

        Ok(Self {
            expr: trimmed.to_string(),
            parsed: Some(parsed),
        })
    }

    /// The original expression, e.g. `"*/5 * * * *"`.
    pub fn as_str(&self) -> &str {
        &self.expr
    }

    /// Compute the next fire time strictly after `after`.
    ///
    /// Cron expressions are interpreted in the **host's local timezone**
    /// (matching the Claude Code `/loop` spec). The returned fire time is
    /// stored as `DateTime<Utc>` so persisted task records stay timezone-
    /// independent.
    pub fn next_after(&mut self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.ensure_parsed();
        let local_after: DateTime<Local> = after.with_timezone(&Local);
        self.parsed
            .as_ref()
            .and_then(|s| s.after(&local_after).next())
            .map(|t| t.with_timezone(&Utc))
    }

    /// Approximate interval between consecutive fires, used for jitter sizing.
    pub fn approx_interval_seconds(&mut self) -> i64 {
        let now: DateTime<Local> = Local::now();
        self.ensure_parsed();
        let Some(sched) = self.parsed.as_ref() else {
            return 3600;
        };
        let mut iter = sched.after(&now);
        let Some(a) = iter.next() else { return 3600 };
        let Some(b) = iter.next() else { return 3600 };
        (b - a).num_seconds().max(60)
    }

    fn ensure_parsed(&mut self) {
        if self.parsed.is_none() {
            let six = format!("0 {}", self.expr);
            self.parsed = cron::Schedule::from_str(&six).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_5_min() {
        let e = CronExpr::parse("*/5 * * * *").unwrap();
        assert_eq!(e.as_str(), "*/5 * * * *");
    }

    #[test]
    fn parses_weekday_morning() {
        CronExpr::parse("0 9 * * 1-5").unwrap();
    }

    #[test]
    fn rejects_wrong_field_count() {
        let err = CronExpr::parse("0 9 * *").unwrap_err();
        assert!(err.contains("5 fields"), "got: {err}");
    }

    #[test]
    fn rejects_l_syntax() {
        let err = CronExpr::parse("0 9 L * *").unwrap_err();
        assert!(err.contains("unsupported character"), "got: {err}");
    }

    #[test]
    fn rejects_name_alias() {
        let err = CronExpr::parse("0 9 * * MON").unwrap_err();
        assert!(err.contains("unsupported character"), "got: {err}");
    }

    #[test]
    fn next_after_advances() {
        let mut e = CronExpr::parse("*/5 * * * *").unwrap();
        let now = Utc::now();
        let nxt = e.next_after(now).unwrap();
        assert!(nxt > now);
        assert!((nxt - now).num_minutes() <= 5);
    }

    #[test]
    fn next_after_uses_local_timezone() {
        // `0 9 * * *` should mean 9 AM **local**, not 9 AM UTC. We verify by
        // converting the returned UTC fire time to local time and confirming
        // the local hour is 9.
        use chrono::{Local, Timelike};
        let mut e = CronExpr::parse("0 9 * * *").unwrap();
        let now = Utc::now();
        let nxt_utc = e.next_after(now).expect("should have a next fire");
        let nxt_local: DateTime<Local> = nxt_utc.with_timezone(&Local);
        assert_eq!(
            nxt_local.hour(),
            9,
            "next fire should be at 9 AM local; got {nxt_local} (UTC {nxt_utc})"
        );
        assert_eq!(nxt_local.minute(), 0);
    }

    #[test]
    fn parses_step_with_range() {
        // `*/15` is the most common step shape; verify it parses end-to-end.
        let e = CronExpr::parse("*/15 * * * *").unwrap();
        assert_eq!(e.as_str(), "*/15 * * * *");
    }

    #[test]
    fn parses_explicit_list() {
        let e = CronExpr::parse("0,15,30,45 * * * *").unwrap();
        assert_eq!(e.as_str(), "0,15,30,45 * * * *");
    }

    #[test]
    fn parses_range_in_dow() {
        // weekday range — already covered for parse, here we also fire it.
        let mut e = CronExpr::parse("0 9 * * 1-5").unwrap();
        let now = Utc::now();
        let nxt = e.next_after(now).expect("should have a next fire");
        assert!(nxt > now);
    }

    #[test]
    fn parses_with_surrounding_whitespace() {
        // Trim should make leading/trailing whitespace harmless.
        let e = CronExpr::parse("   */5 * * * *   ").unwrap();
        assert_eq!(e.as_str(), "*/5 * * * *");
    }

    #[test]
    fn rejects_empty_string() {
        let err = CronExpr::parse("").unwrap_err();
        assert!(err.contains("5 fields"), "got: {err}");
    }

    #[test]
    fn rejects_too_many_fields() {
        let err = CronExpr::parse("0 0 9 * * *").unwrap_err();
        assert!(err.contains("5 fields"), "got: {err}");
    }

    #[test]
    fn rejects_w_syntax() {
        let err = CronExpr::parse("0 9 1W * *").unwrap_err();
        assert!(err.contains("unsupported character"), "got: {err}");
    }

    #[test]
    fn rejects_question_mark() {
        let err = CronExpr::parse("0 9 ? * *").unwrap_err();
        assert!(err.contains("unsupported character"), "got: {err}");
    }

    #[test]
    fn rejects_month_alias() {
        let err = CronExpr::parse("0 9 1 JAN *").unwrap_err();
        assert!(err.contains("unsupported character"), "got: {err}");
    }

    #[test]
    fn rejects_garbage_numbers() {
        // 5-field shape passes the field-count check but fails the underlying
        // cron parse. The character whitelist would actually allow this through
        // (digits + `-`), so it must surface a "parse error".
        let err = CronExpr::parse("99 99 99 99 99").unwrap_err();
        assert!(err.contains("parse error"), "got: {err}");
    }

    #[test]
    fn next_after_is_strictly_monotonic() {
        // Three consecutive next_after calls must return strictly increasing
        // times. This guards against an off-by-one where `after` is treated
        // inclusively.
        let mut e = CronExpr::parse("*/5 * * * *").unwrap();
        let t0 = Utc::now();
        let t1 = e.next_after(t0).unwrap();
        let t2 = e.next_after(t1).unwrap();
        let t3 = e.next_after(t2).unwrap();
        assert!(t1 > t0);
        assert!(t2 > t1);
        assert!(t3 > t2);
        // And the gap between consecutive fires for `*/5` is exactly 5 min.
        assert_eq!((t2 - t1).num_minutes(), 5);
        assert_eq!((t3 - t2).num_minutes(), 5);
    }

    #[test]
    fn approx_interval_for_every_5_min() {
        let mut e = CronExpr::parse("*/5 * * * *").unwrap();
        assert_eq!(e.approx_interval_seconds(), 300);
    }

    #[test]
    fn approx_interval_for_hourly() {
        let mut e = CronExpr::parse("0 * * * *").unwrap();
        assert_eq!(e.approx_interval_seconds(), 3600);
    }

    #[test]
    fn approx_interval_for_daily() {
        let mut e = CronExpr::parse("0 0 * * *").unwrap();
        assert_eq!(e.approx_interval_seconds(), 86_400);
    }

    #[test]
    fn approx_interval_floored_at_60s() {
        // `*/1 * * * *` — every minute → 60s. The `.max(60)` floor keeps
        // sub-minute schedulers (which we don't actually parse) safe.
        let mut e = CronExpr::parse("*/1 * * * *").unwrap();
        assert_eq!(e.approx_interval_seconds(), 60);
    }

    #[test]
    fn serde_round_trip_preserves_expr() {
        // CronExpr derives Serialize/Deserialize and skips `parsed`. A round
        // trip through JSON must preserve the original expression and keep
        // `next_after` working after deserialization.
        let original = CronExpr::parse("*/15 * * * *").unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let mut decoded: CronExpr = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.as_str(), "*/15 * * * *");
        // ensure_parsed should re-hydrate the schedule lazily.
        let now = Utc::now();
        let nxt = decoded
            .next_after(now)
            .expect("should fire after rehydrate");
        assert!(nxt > now);
    }

    #[test]
    fn clone_does_not_share_parsed_cache() {
        // CronExpr is Clone; the clone should still work even though `parsed`
        // is `#[serde(skip)]` and might be `None` after a round-trip.
        let original = CronExpr::parse("*/5 * * * *").unwrap();
        let mut cloned = original.clone();
        let now = Utc::now();
        assert!(cloned.next_after(now).is_some());
    }
}
