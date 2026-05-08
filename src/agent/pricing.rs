//! Per-model USD pricing for cost reporting.
//!
//! Values in $ per 1M tokens. Returns `None` for unknown models so that the
//! caller can set `total_cost_usd = None`. Public DeepSeek list pricing as of
//! late 2025 — adjust as needed.

pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

pub fn model_pricing(model: &str) -> Option<ModelPricing> {
    let m = model.to_lowercase();
    let p = match m.as_str() {
        "deepseek-v4-pro" | "deepseek-v4" => (0.55, 2.19),
        "deepseek-v4-flash" => (0.14, 0.55),
        "deepseek-reasoner" | "deepseek-r1" => (0.55, 2.19),
        "deepseek-chat" | "deepseek-v3" => (0.27, 1.10),
        _ => return None,
    };
    Some(ModelPricing {
        input_per_mtok: p.0,
        output_per_mtok: p.1,
    })
}

/// Convert a turn's `UsageInfo` into a USD cost given the model. Returns
/// `None` if pricing is unknown.
pub fn turn_cost_usd(model: &str, prompt_tokens: u32, completion_tokens: u32) -> Option<f64> {
    let p = model_pricing(model)?;
    let cost = (prompt_tokens as f64 / 1_000_000.0) * p.input_per_mtok
        + (completion_tokens as f64 / 1_000_000.0) * p.output_per_mtok;
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
