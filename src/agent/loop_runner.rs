//! Claude-Code-shaped streaming agent loop.
//!
//! `run(...)` returns an async stream of [`SdkMessage`]. The loop:
//!
//! 1. Yields `System{Init}` carrying the session id.
//! 2. POSTs to the configured chat-completions endpoint.
//! 3. On `finish_reason == "tool_calls"`: yields one `Assistant` message with
//!    text + tool_use blocks, runs each tool through the permission gate
//!    (read-only tools in parallel, mutating tools sequentially), yields one
//!    `User` message containing all tool_result blocks, and continues.
//! 4. On any other finish reason: yields the final `Assistant` text and a
//!    `Result{Success}` carrying usage, cost, and turn count.
//! 5. Enforces `max_turns` and `max_budget_usd`; transport errors yield
//!    `Result{ErrorDuringExecution}`.

use std::sync::Arc;

use async_stream::stream;
use futures::future::join_all;
use futures::stream::Stream;
use serde_json::{json, Value};

use crate::client::HttpClient;
use crate::types::{
    tool_result_msg, ChatContent, ChatMessage, ChatRequest, FunctionSchema, ToolSchema, UsageInfo,
};

use super::messages::{ContentBlock, ResultSubtype, SdkMessage, SystemSubtype};
use super::options::RunOptions;
use super::permissions::{PermissionDecision, PermissionMode};
use super::pricing::{map_stop_reason, turn_cost_usd};
use super::tool::Tool;

/// Run the agent loop and stream `SdkMessage`s in turn order.
///
/// `tools` is wrapped in an `Arc` so callers can reuse the same registry
/// across multiple runs.
pub fn run<H>(
    http: H,
    api_key: String,
    tools: Arc<Vec<Box<dyn Tool>>>,
    user_prompt: String,
    opts: RunOptions,
) -> impl Stream<Item = SdkMessage>
where
    H: HttpClient + Send + Sync + 'static,
{
    stream! {
        // Resolve session and emit init.
        let session_id = opts
            .session_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        yield SdkMessage::System {
            subtype: SystemSubtype::Init,
            session_id: session_id.clone(),
            data: json!({
                "model": opts.model,
                "permission_mode": opts.permission_mode,
                "max_turns": opts.max_turns,
                "max_budget_usd": opts.max_budget_usd,
            }),
        };

        // Hide disallowed/non-allowed tools from the model entirely.
        let visible_tools: Vec<&Box<dyn Tool>> = tools
            .iter()
            .filter(|t| {
                let n = t.name();
                if opts.disallowed_tools.iter().any(|d| d == n) {
                    return false;
                }
                if let Some(allow) = &opts.allowed_tools {
                    return allow.iter().any(|a| a == n);
                }
                true
            })
            .collect();

        let tool_schemas: Vec<ToolSchema> = visible_tools
            .iter()
            .map(|t| {
                let def = t.definition();
                ToolSchema {
                    r#type: "function".into(),
                    function: FunctionSchema {
                        name: def.name,
                        description: def.description,
                        parameters: def.parameters,
                    },
                }
            })
            .collect();

        // Conversation history.
        let mut messages: Vec<ChatMessage> = Vec::new();
        if !opts.system_prompt.is_empty() {
            messages.push(ChatMessage {
                role: "system".into(),
                content: ChatContent::Text(opts.system_prompt.clone()),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }
        messages.push(ChatMessage {
            role: "user".into(),
            content: ChatContent::Text(user_prompt),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        let url = format!("{}/chat/completions", opts.base_url);
        let mut num_turns: u32 = 0;
        let mut total_prompt_tokens: u32 = 0;
        let mut total_completion_tokens: u32 = 0;
        let mut total_cost: Option<f64> = turn_cost_usd(&opts.model, 0, 0).map(|_| 0.0);
        let mut last_stop_reason: Option<String> = None;

        loop {
            let request = ChatRequest {
                model: opts.model.clone(),
                messages: messages.clone(),
                tools: if tool_schemas.is_empty() { None } else { Some(tool_schemas.clone()) },
                tool_choice: if tool_schemas.is_empty() {
                    None
                } else {
                    Some(json!("auto"))
                },
                temperature: Some(opts.effort.temperature()),
                max_tokens: Some(opts.effort.max_tokens()),
                stream: Some(false),
                reasoning_effort: Some(match opts.effort {
                    crate::types::EffortLevel::Max => "max".into(),
                    crate::types::EffortLevel::High => "high".into(),
                    crate::types::EffortLevel::Medium => "medium".into(),
                    crate::types::EffortLevel::Low => "low".into(),
                }),
                thinking: Some(json!({"type": "enabled"})),
            };

            let resp = match http.post_json(&url, &api_key, &request).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "agent loop transport error");
                    yield SdkMessage::Result {
                        subtype: ResultSubtype::ErrorDuringExecution,
                        result: None,
                        total_cost_usd: total_cost,
                        usage: usage_info(total_prompt_tokens, total_completion_tokens),
                        num_turns,
                        session_id,
                        stop_reason: last_stop_reason,
                    };
                    return;
                }
            };

            // Accumulate usage / cost from this turn.
            if let Some(u) = &resp.usage {
                total_prompt_tokens = total_prompt_tokens.saturating_add(u.prompt_tokens);
                total_completion_tokens = total_completion_tokens.saturating_add(u.completion_tokens);
                if let (Some(running), Some(turn)) = (
                    total_cost.as_mut(),
                    turn_cost_usd(&opts.model, u.prompt_tokens, u.completion_tokens),
                ) {
                    *running += turn;
                }
            }

            let Some(choice) = resp.choices.into_iter().next() else {
                yield SdkMessage::Result {
                    subtype: ResultSubtype::ErrorDuringExecution,
                    result: None,
                    total_cost_usd: total_cost,
                    usage: usage_info(total_prompt_tokens, total_completion_tokens),
                    num_turns,
                    session_id,
                    stop_reason: last_stop_reason,
                };
                return;
            };

            let finish_reason = choice.finish_reason.as_deref().unwrap_or("stop");
            last_stop_reason = map_stop_reason(finish_reason);
            let assistant_msg = choice.message;

            if finish_reason == "tool_calls" {
                let tool_calls = assistant_msg.tool_calls.clone().unwrap_or_default();

                // Build the Assistant SdkMessage (text + tool_use blocks).
                let mut content_blocks: Vec<ContentBlock> = Vec::new();
                let text = assistant_msg.content.as_str();
                if !text.is_empty() {
                    content_blocks.push(ContentBlock::Text { text: text.to_string() });
                }
                let parsed_calls: Vec<(String, String, Value)> = tool_calls
                    .iter()
                    .map(|c| {
                        let args: Value =
                            serde_json::from_str(&c.function.arguments).unwrap_or(json!({}));
                        (c.id.clone(), c.function.name.clone(), args)
                    })
                    .collect();
                for (id, name, input) in &parsed_calls {
                    content_blocks.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                }
                yield SdkMessage::Assistant {
                    content: content_blocks,
                    stop_reason: last_stop_reason.clone(),
                };

                // Persist assistant turn in history (carries tool_calls).
                messages.push(assistant_msg);

                // Permission gate.
                let mut decisions: Vec<(String, String, Value, PermissionDecision, bool)> =
                    Vec::with_capacity(parsed_calls.len());
                for (id, name, args) in parsed_calls {
                    let tool_ref = visible_tools.iter().find(|t| t.name() == name);
                    let read_only = tool_ref.map(|t| t.read_only_hint()).unwrap_or(false);

                    let mode_decision = opts.permission_mode.evaluate(&name, read_only);
                    let final_decision = match (mode_decision, &opts.pre_tool_hook) {
                        (PermissionDecision::Allow, _) => PermissionDecision::Allow,
                        (PermissionDecision::Deny(r), _) => PermissionDecision::Deny(r),
                        (PermissionDecision::Ask, Some(hook)) => {
                            match hook.check(&name, &args).await {
                                PermissionDecision::Ask => PermissionDecision::Deny(format!(
                                    "Tool `{name}` requires approval and the hook returned Ask"
                                )),
                                d => d,
                            }
                        }
                        (PermissionDecision::Ask, None) => {
                            if matches!(opts.permission_mode, PermissionMode::BypassPermissions) {
                                PermissionDecision::Allow
                            } else {
                                PermissionDecision::Deny(format!(
                                    "Tool `{name}` not pre-approved and no permission hook configured"
                                ))
                            }
                        }
                    };

                    decisions.push((id, name, args, final_decision, read_only));
                }

                // Partition allowed calls into read-only (parallel) and mutating (sequential).
                let mut tool_results: Vec<(String, Result<String, String>)> = Vec::new();
                let mut parallel_idxs: Vec<usize> = Vec::new();
                let mut sequential_idxs: Vec<usize> = Vec::new();
                for (i, (_, _, _, d, ro)) in decisions.iter().enumerate() {
                    if matches!(d, PermissionDecision::Allow) {
                        if *ro {
                            parallel_idxs.push(i);
                        } else {
                            sequential_idxs.push(i);
                        }
                    }
                }

                // Run parallel set.
                if !parallel_idxs.is_empty() {
                    let futs = parallel_idxs.iter().map(|&i| {
                        let (id, name, args, _, _) = &decisions[i];
                        let id = id.clone();
                        let name = name.clone();
                        let args = args.clone();
                        let tools = Arc::clone(&tools);
                        async move {
                            let res = match tools.iter().find(|t| t.name() == name) {
                                Some(t) => t.call_json(args).await,
                                None => Err(format!("Unknown tool: {name}")),
                            };
                            (id, res)
                        }
                    });
                    let outs = join_all(futs).await;
                    for (id, res) in outs {
                        tool_results.push((id, res));
                    }
                }

                // Run sequential set.
                for i in sequential_idxs {
                    let (id, name, args, _, _) = &decisions[i];
                    let res = match tools.iter().find(|t| t.name() == *name) {
                        Some(t) => t.call_json(args.clone()).await,
                        None => Err(format!("Unknown tool: {name}")),
                    };
                    tool_results.push((id.clone(), res));
                }

                // Append denials as synthetic error tool_results.
                for (id, _name, _args, d, _) in &decisions {
                    if let PermissionDecision::Deny(reason) = d {
                        tool_results.push((id.clone(), Err(reason.clone())));
                    }
                }

                // Re-order results to match the original tool-call order so the
                // model sees them in the same sequence it requested.
                let id_order: Vec<String> = decisions.iter().map(|d| d.0.clone()).collect();
                tool_results.sort_by_key(|(id, _)| {
                    id_order.iter().position(|x| x == id).unwrap_or(usize::MAX)
                });

                // Append tool_result messages to history; build user SdkMessage.
                let mut user_blocks: Vec<ContentBlock> = Vec::with_capacity(tool_results.len());
                for (call_id, res) in &tool_results {
                    let (content_str, is_error) = match res {
                        Ok(s) => (s.clone(), false),
                        Err(e) => (e.clone(), true),
                    };
                    messages.push(tool_result_msg(call_id, &content_str));
                    user_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: call_id.clone(),
                        content: content_str,
                        is_error,
                    });
                }
                yield SdkMessage::User { content: user_blocks };

                num_turns = num_turns.saturating_add(1);

                if let Some(limit) = opts.max_turns {
                    if num_turns >= limit {
                        yield SdkMessage::Result {
                            subtype: ResultSubtype::ErrorMaxTurns,
                            result: None,
                            total_cost_usd: total_cost,
                            usage: usage_info(total_prompt_tokens, total_completion_tokens),
                            num_turns,
                            session_id,
                            stop_reason: last_stop_reason,
                        };
                        return;
                    }
                }
                if let (Some(budget), Some(cost)) = (opts.max_budget_usd, total_cost) {
                    if cost >= budget {
                        yield SdkMessage::Result {
                            subtype: ResultSubtype::ErrorMaxBudgetUsd,
                            result: None,
                            total_cost_usd: total_cost,
                            usage: usage_info(total_prompt_tokens, total_completion_tokens),
                            num_turns,
                            session_id,
                            stop_reason: last_stop_reason,
                        };
                        return;
                    }
                }
            } else {
                // Final assistant turn — text only.
                let text = assistant_msg.content.as_str().to_string();
                yield SdkMessage::Assistant {
                    content: vec![ContentBlock::Text { text: text.clone() }],
                    stop_reason: last_stop_reason.clone(),
                };
                yield SdkMessage::Result {
                    subtype: ResultSubtype::Success,
                    result: Some(text),
                    total_cost_usd: total_cost,
                    usage: usage_info(total_prompt_tokens, total_completion_tokens),
                    num_turns,
                    session_id,
                    stop_reason: last_stop_reason,
                };
                return;
            }
        }
    }
}

fn usage_info(prompt: u32, completion: u32) -> Option<UsageInfo> {
    if prompt == 0 && completion == 0 {
        None
    } else {
        Some(UsageInfo {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt.saturating_add(completion),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures::StreamExt;
    use serde_json::json;

    use crate::agent::permissions::PermissionMode;
    use crate::agent::tool::ToolDefinition;
    use crate::client::HttpClient;
    use crate::error::Result as DResult;
    use crate::types::{
        ChatContent, ChatMessage, ChatRequest, ChatResponse, Choice, FunctionCall, ToolCall,
        UsageInfo,
    };

    /// Returns a queued sequence of [`ChatResponse`] values, panicking if the
    /// loop calls the API more times than expected. `seen_requests` is shared
    /// across clones so tests can inspect it after the loop has consumed the
    /// mock.
    #[derive(Clone)]
    struct MockHttp {
        queue: Arc<Mutex<Vec<ChatResponse>>>,
        seen_requests: Arc<Mutex<Vec<ChatRequest>>>,
    }

    impl MockHttp {
        fn new(queue: Vec<ChatResponse>) -> Self {
            Self {
                queue: Arc::new(Mutex::new(queue)),
                seen_requests: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl HttpClient for MockHttp {
        async fn post_json(
            &self,
            _url: &str,
            _bearer: &str,
            body: &ChatRequest,
        ) -> DResult<ChatResponse> {
            self.seen_requests.lock().unwrap().push(body.clone());
            let mut q = self.queue.lock().unwrap();
            assert!(!q.is_empty(), "MockHttp: queue exhausted");
            Ok(q.remove(0))
        }
    }

    fn assistant_text(text: &str) -> ChatResponse {
        ChatResponse {
            id: "test".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".into(),
                    content: ChatContent::Text(text.into()),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        }
    }

    fn assistant_tool_call(id: &str, name: &str, args: serde_json::Value) -> ChatResponse {
        ChatResponse {
            id: "test".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".into(),
                    content: ChatContent::Null,
                    reasoning_content: None,
                    tool_calls: Some(vec![ToolCall {
                        id: id.into(),
                        r#type: "function".into(),
                        function: FunctionCall {
                            name: name.into(),
                            arguments: args.to_string(),
                        },
                    }]),
                    tool_call_id: None,
                    name: None,
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: Some(UsageInfo {
                prompt_tokens: 8,
                completion_tokens: 4,
                total_tokens: 12,
            }),
        }
    }

    /// Minimal tool used by the loop tests — just echoes its args back.
    struct EchoTool {
        name: &'static str,
        read_only: bool,
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            self.name
        }
        fn read_only_hint(&self) -> bool {
            self.read_only
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.to_string(),
                description: "echo".into(),
                parameters: json!({"type":"object"}),
            }
        }
        async fn call_json(&self, args: serde_json::Value) -> std::result::Result<String, String> {
            Ok(format!("echoed {}", args))
        }
    }

    fn tools(items: Vec<(&'static str, bool)>) -> Arc<Vec<Box<dyn Tool>>> {
        Arc::new(
            items
                .into_iter()
                .map(|(n, ro)| {
                    Box::new(EchoTool {
                        name: n,
                        read_only: ro,
                    }) as Box<dyn Tool>
                })
                .collect(),
        )
    }

    async fn collect(
        http: MockHttp,
        toolset: Arc<Vec<Box<dyn Tool>>>,
        prompt: &str,
        opts: RunOptions,
    ) -> Vec<SdkMessage> {
        run(http, "test-key".into(), toolset, prompt.into(), opts)
            .collect()
            .await
    }

    #[tokio::test]
    async fn text_only_emits_assistant_then_success() {
        let http = MockHttp::new(vec![assistant_text("hello world")]);
        let msgs = collect(http, tools(vec![]), "hi", RunOptions::default()).await;

        assert!(matches!(msgs[0], SdkMessage::System { .. }));
        assert!(matches!(&msgs[1], SdkMessage::Assistant { .. }));
        match &msgs[2] {
            SdkMessage::Result {
                subtype,
                result: Some(t),
                num_turns,
                ..
            } => {
                assert_eq!(*subtype, ResultSubtype::Success);
                assert_eq!(t, "hello world");
                assert_eq!(*num_turns, 0);
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_call_then_text_completes_successfully() {
        let http = MockHttp::new(vec![
            assistant_tool_call("c1", "echo_ro", json!({"x": 1})),
            assistant_text("done"),
        ]);
        let msgs = collect(
            http,
            tools(vec![("echo_ro", true)]),
            "hi",
            RunOptions::default().permission_mode(PermissionMode::BypassPermissions),
        )
        .await;

        // System, Assistant(tool_use), User(tool_result), Assistant(text), Result.
        assert_eq!(msgs.len(), 5, "msgs={msgs:?}");
        match &msgs[1] {
            SdkMessage::Assistant { content, .. } => {
                assert!(matches!(content[0], ContentBlock::ToolUse { .. }));
            }
            _ => panic!(),
        }
        match &msgs[2] {
            SdkMessage::User { content } => match &content[0] {
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => {
                    assert_eq!(tool_use_id, "c1");
                    assert!(!is_error);
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
        match &msgs[4] {
            SdkMessage::Result {
                subtype, num_turns, ..
            } => {
                assert_eq!(*subtype, ResultSubtype::Success);
                assert_eq!(*num_turns, 1);
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn max_turns_stops_with_error_subtype() {
        let http = MockHttp::new(vec![
            assistant_tool_call("c1", "echo_ro", json!({})),
            assistant_tool_call("c2", "echo_ro", json!({})),
        ]);
        let msgs = collect(
            http,
            tools(vec![("echo_ro", true)]),
            "loop",
            RunOptions::default()
                .max_turns(1)
                .permission_mode(PermissionMode::BypassPermissions),
        )
        .await;
        let last = msgs.last().unwrap();
        match last {
            SdkMessage::Result {
                subtype, num_turns, ..
            } => {
                assert_eq!(*subtype, ResultSubtype::ErrorMaxTurns);
                assert_eq!(*num_turns, 1);
            }
            _ => panic!("expected Result"),
        }
    }

    #[tokio::test]
    async fn plan_mode_denies_mutating_tool() {
        // Loop sees a single tool call, plan-mode denies it, then the final
        // assistant turn says "ok".
        let http = MockHttp::new(vec![
            assistant_tool_call("c1", "echo_mut", json!({})),
            assistant_text("ok"),
        ]);
        let msgs = collect(
            http,
            tools(vec![("echo_mut", false)]),
            "do",
            RunOptions::default().permission_mode(PermissionMode::Plan),
        )
        .await;
        // Find the User(tool_result) message and assert is_error=true.
        let denied = msgs
            .iter()
            .find_map(|m| match m {
                SdkMessage::User { content } => Some(content.clone()),
                _ => None,
            })
            .expect("expected a User tool_result message");
        match &denied[0] {
            ContentBlock::ToolResult {
                is_error, content, ..
            } => {
                assert!(*is_error);
                assert!(content.contains("Plan mode"), "msg={content}");
            }
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn legacy_builder_prompt_round_trips_text() {
        // Validates the back-compat `AgentBuilder` → `DeepSeekAgent::prompt`
        // surface that `crates/research` depends on.
        use crate::agent::AgentBuilder;
        let http = MockHttp::new(vec![assistant_text("hello back")]);
        let agent = AgentBuilder::new(http, "test-key", "deepseek-chat")
            .preamble("you are a test")
            .build();
        let out = agent.prompt("hi".into()).await.expect("prompt ok");
        assert_eq!(out, "hello back");
    }

    #[tokio::test]
    async fn disallowed_tool_is_hidden_from_request() {
        let http = MockHttp::new(vec![assistant_text("nothing to do")]);
        let mock = http.clone();
        let _ = collect(
            http,
            tools(vec![("echo_ro", true), ("echo_mut", false)]),
            "hi",
            RunOptions::default().disallowed_tools(["echo_mut"]),
        )
        .await;
        let req = &mock.seen_requests.lock().unwrap()[0];
        let names: Vec<String> = req
            .tools
            .as_ref()
            .map(|s| s.iter().map(|t| t.function.name.clone()).collect())
            .unwrap_or_default();
        assert_eq!(names, vec!["echo_ro".to_string()]);
    }
}
