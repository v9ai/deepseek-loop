//! Per-model USD pricing for cost reporting.
//!
//! Values in $ per 1M tokens. Returns `None` for unknown models so that the
//! caller can set `total_cost_usd = None`. DeepSeek bills prompt tokens at two
//! rates: a discounted **cache-hit** rate for prefix tokens served from their
//! context cache, and the full **cache-miss** rate for everything else. The
//! API reports the split in `usage.prompt_cache_hit_tokens` /
//! `prompt_cache_miss_tokens`; when present we use the split rates, otherwise
//! we fall back to charging the entire prompt at the miss rate.

use crate::types::UsageInfo;

pub struct ModelPricing {
    /// Rate for prompt tokens that missed the context cache. Also the
    /// fallback rate when cache stats are not reported.
    pub input_per_mtok: f64,
    /// Rate for prompt tokens served from the context cache. Typically ~25%
    /// of `input_per_mtok`.
    pub cached_input_per_mtok: f64,
    pub output_per_mtok: f64,
}

pub fn model_pricing(model: &str) -> Option<ModelPricing> {
    let m = model.to_lowercase();
    // (cache_miss_input, cache_hit_input, output) per 1M tokens.
    let (miss, hit, out) = match m.as_str() {
        "deepseek-v4-pro" | "deepseek-v4" => (0.55, 0.14, 2.19),
        "deepseek-v4-flash" => (0.14, 0.04, 0.55),
        "deepseek-reasoner" | "deepseek-r1" => (0.55, 0.14, 2.19),
        "deepseek-chat" | "deepseek-v3" => (0.27, 0.07, 1.10),
        _ => return None,
    };
    Some(ModelPricing {
        input_per_mtok: miss,
        cached_input_per_mtok: hit,
        output_per_mtok: out,
    })
}

/// Convert a turn's `UsageInfo` into a USD cost given the model. Uses the
/// cache-hit/miss split when the API reported it; otherwise charges the full
/// prompt at the miss rate. Returns `None` if pricing is unknown.
pub fn turn_cost_usd(model: &str, usage: &UsageInfo) -> Option<f64> {
    let p = model_pricing(model)?;
    let (hit, miss) = match (
        usage.prompt_cache_hit_tokens,
        usage.prompt_cache_miss_tokens,
    ) {
        (Some(h), Some(m)) => (h, m),
        (Some(h), None) => (h, usage.prompt_tokens.saturating_sub(h)),
        (None, Some(m)) => (usage.prompt_tokens.saturating_sub(m), m),
        (None, None) => (0, usage.prompt_tokens),
    };
    let cost = (hit as f64 / 1_000_000.0) * p.cached_input_per_mtok
        + (miss as f64 / 1_000_000.0) * p.input_per_mtok
        + (usage.completion_tokens as f64 / 1_000_000.0) * p.output_per_mtok;
    Some(cost)
}

/// Map OpenAI `finish_reason` to a Claude-style `stop_reason`.
pub fn map_stop_reason(finish_reason: &str) -> Option<String> {
    let r = match finish_reason {
        "stop" => "end_turn",
        "tool_calls" => "tool_use",
        "length" => "max_tokens",
        "content_filter" => "refusal",
        _ => return Some(finish_reason.to_string()),
    };
    Some(r.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_hit_costs_less_than_miss() {
        // 1M prompt tokens all served from cache vs all missed.
        let hit = UsageInfo {
            prompt_tokens: 1_000_000,
            completion_tokens: 0,
            total_tokens: 1_000_000,
            prompt_cache_hit_tokens: Some(1_000_000),
            prompt_cache_miss_tokens: Some(0),
        };
        let miss = UsageInfo {
            prompt_tokens: 1_000_000,
            completion_tokens: 0,
            total_tokens: 1_000_000,
            prompt_cache_hit_tokens: Some(0),
            prompt_cache_miss_tokens: Some(1_000_000),
        };
        let hit_cost = turn_cost_usd("deepseek-chat", &hit).unwrap();
        let miss_cost = turn_cost_usd("deepseek-chat", &miss).unwrap();
        assert!(hit_cost < miss_cost);
        assert!((hit_cost - 0.07).abs() < 1e-9);
        assert!((miss_cost - 0.27).abs() < 1e-9);
    }

    #[test]
    fn missing_cache_fields_charge_full_rate() {
        let usage = UsageInfo {
            prompt_tokens: 1_000_000,
            completion_tokens: 0,
            total_tokens: 1_000_000,
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
        };
        let cost = turn_cost_usd("deepseek-chat", &usage).unwrap();
        assert!((cost - 0.27).abs() < 1e-9);
    }

    #[test]
    fn split_cache_fields_apply_blended_rate() {
        let usage = UsageInfo {
            prompt_tokens: 1_000_000,
            completion_tokens: 0,
            total_tokens: 1_000_000,
            prompt_cache_hit_tokens: Some(800_000),
            prompt_cache_miss_tokens: Some(200_000),
        };
        let cost = turn_cost_usd("deepseek-chat", &usage).unwrap();
        // 0.8 * 0.07 + 0.2 * 0.27 = 0.056 + 0.054 = 0.110
        assert!((cost - 0.110).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_returns_none() {
        let usage = UsageInfo::default();
        assert!(turn_cost_usd("gpt-9", &usage).is_none());
    }
}
