//! 5-field cron grammar wrapper.
//!
//! Wraps the [`cron`] crate with stricter parsing: only standard 5-field
//! `minute hour day-of-month month day-of-week` expressions are accepted.
//! Extended syntax (`L`, `W`, `?`, name aliases like `MON`/`JAN`) is rejected
//! to match the Claude Code spec.

use std::str::FromStr;

use chrono::{DateTime, Utc};
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
                let ok = ch.is_ascii_digit()
                    || matches!(ch, '*' | '/' | ',' | '-');
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
        let parsed = cron::Schedule::from_str(&six)
            .map_err(|e| format!("cron: parse error: {e}"))?;

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
    pub fn next_after(&mut self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.ensure_parsed();
        self.parsed
            .as_ref()
            .and_then(|s| s.after(&after).next())
    }

    /// Approximate interval between consecutive fires, used for jitter sizing.
    pub fn approx_interval_seconds(&mut self) -> i64 {
        let now = Utc::now();
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
}
