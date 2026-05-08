//! Deterministic jitter for scheduled task fire times.
//!
//! Matches the Claude Code spec:
//! - Recurring tasks fire up to 30 minutes after the scheduled time, or up to
//!   half the interval (whichever is smaller) for sub-hourly intervals.
//! - One-shot tasks scheduled at `:00` or `:30` of the hour fire up to 90
//!   seconds *early*.
//!
//! The offset is derived from the task ID, so the same task always gets the
//! same offset.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Duration, Timelike, Utc};

/// Deterministic positive integer keyed off `task_id`.
fn task_hash(task_id: &str) -> u64 {
    let mut h = DefaultHasher::new();
    task_id.hash(&mut h);
    h.finish()
}

/// Compute the actual fire time after applying jitter.
///
/// `interval_seconds` is the approximate cadence between consecutive fires for
/// a recurring task. Pass `None` for one-shot tasks.
pub fn apply(
    task_id: &str,
    nominal: DateTime<Utc>,
    interval_seconds: Option<i64>,
    recurring: bool,
) -> DateTime<Utc> {
    let h = task_hash(task_id);

    if recurring {
        let cap_secs = match interval_seconds {
            // ≥ 1 hour → 30-minute cap
            Some(s) if s >= 3600 => 30 * 60,
            // sub-hourly → half-interval cap (min 1s)
            Some(s) => (s / 2).max(1),
            None => 30 * 60,
        };
        let offset = (h % (cap_secs as u64)) as i64;
        return nominal + Duration::seconds(offset);
    }

    // One-shot at top/bottom of the hour fires up to 90s early.
    if nominal.minute() == 0 || nominal.minute() == 30 {
        let early = ((h % 90) as i64) + 1; // 1..=90
        return nominal - Duration::seconds(early);
    }

    nominal
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn deterministic_for_same_id() {
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 18, 0, 0).unwrap();
        let a = apply("task-abcd", now, Some(3600), true);
        let b = apply("task-abcd", now, Some(3600), true);
        assert_eq!(a, b);
    }

    #[test]
    fn hourly_offset_within_30min() {
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 18, 0, 0).unwrap();
        let fired = apply("task-aaaa", now, Some(3600), true);
        let delta = (fired - now).num_seconds();
        assert!((0..1800).contains(&delta), "delta={delta}");
    }

    #[test]
    fn five_min_offset_within_half_interval() {
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 18, 0, 0).unwrap();
        let fired = apply("task-bbbb", now, Some(300), true);
        let delta = (fired - now).num_seconds();
        assert!((0..150).contains(&delta), "delta={delta}");
    }

    #[test]
    fn one_shot_top_of_hour_fires_early() {
        let nominal = Utc.with_ymd_and_hms(2026, 5, 8, 18, 0, 0).unwrap();
        let fired = apply("task-cccc", nominal, None, false);
        let delta = (nominal - fired).num_seconds();
        assert!((1..=90).contains(&delta), "delta={delta}");
    }

    #[test]
    fn one_shot_off_minute_unchanged() {
        let nominal = Utc.with_ymd_and_hms(2026, 5, 8, 18, 3, 0).unwrap();
        let fired = apply("task-dddd", nominal, None, false);
        assert_eq!(fired, nominal);
    }

    #[test]
    fn one_minute_interval_uses_30s_cap() {
        // A 60s interval halves to a 30s jitter cap.
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 18, 0, 0).unwrap();
        let fired = apply("task-eeee", now, Some(60), true);
        let delta = (fired - now).num_seconds();
        assert!((0..30).contains(&delta), "delta={delta}");
    }

    #[test]
    fn no_interval_recurring_uses_30min_cap() {
        // When `interval_seconds=None` for a recurring task, the cap defaults
        // to 30 minutes.
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 18, 0, 0).unwrap();
        let fired = apply("task-ffff", now, None, true);
        let delta = (fired - now).num_seconds();
        assert!((0..1800).contains(&delta), "delta={delta}");
    }

    #[test]
    fn one_shot_at_30_past_fires_early() {
        // The :30 mark is also treated as "top/bottom of the hour" — fires
        // up to 90s early.
        let nominal = Utc.with_ymd_and_hms(2026, 5, 8, 18, 30, 0).unwrap();
        let fired = apply("task-gggg", nominal, None, false);
        let delta = (nominal - fired).num_seconds();
        assert!((1..=90).contains(&delta), "delta={delta}");
    }

    #[test]
    fn deterministic_across_different_nominals() {
        // Same task_id + same interval → same offset, regardless of nominal
        // time. (The offset is only a function of the task_id and the cap.)
        let n1 = Utc.with_ymd_and_hms(2026, 5, 8, 0, 0, 0).unwrap();
        let n2 = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let off1 = (apply("task-hhhh", n1, Some(3600), true) - n1).num_seconds();
        let off2 = (apply("task-hhhh", n2, Some(3600), true) - n2).num_seconds();
        assert_eq!(off1, off2);
    }

    #[test]
    fn distinct_ids_likely_get_distinct_offsets() {
        // Probabilistic but reliable: across 16 ids, at least two distinct
        // offsets should appear under a 30-minute cap (1800 buckets, 16 draws,
        // collision is virtually impossible).
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 18, 0, 0).unwrap();
        let mut offsets = std::collections::HashSet::new();
        for i in 0..16u8 {
            let id = format!("task-{i:02x}");
            let off = (apply(&id, now, Some(3600), true) - now).num_seconds();
            offsets.insert(off);
        }
        assert!(offsets.len() >= 2, "all 16 ids hashed to the same bucket");
    }

    #[test]
    fn zero_offset_is_possible() {
        // Sanity: the offset range is [0, cap), so picking many ids should
        // include at least one fire-time identical to nominal eventually.
        // We don't assert a specific id; we just verify the range bound.
        let now = Utc.with_ymd_and_hms(2026, 5, 8, 18, 0, 0).unwrap();
        for i in 0..1000u32 {
            let id = format!("t-{i}");
            let delta = (apply(&id, now, Some(3600), true) - now).num_seconds();
            assert!((0..1800).contains(&delta), "id={id} delta={delta}");
        }
    }
}
