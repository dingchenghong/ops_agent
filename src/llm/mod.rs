//! DeepSeek（OpenAI 兼容）流式客户端
//!
//! 只实现 Agent 需要的最小能力：`/chat/completions` 的 SSE 流式解析（支持 tool_calls 增量拼接）
//! 以及 `/models` 连通性检测。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::agent::AgentEvent;
use crate::config::Config;

fn default_fn_type() -> String {
    "function".to_string()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FunctionCall {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default = "default_fn_type")]
    pub kind: String,
    #[serde(default)]
    pub function: FunctionCall,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// deepseek-reasoner 的思维链；按官方要求不回传给接口
    #[allow(dead_code)]
    #[serde(skip_serializing, skip_deserializing)]
    pub reasoning_content: Option<String>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self { role: "system".into(), content: Some(text.into()), ..Default::default() }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self { role: "user".into(), content: Some(text.into()), ..Default::default() }
    }

    pub fn assistant(content: Option<String>, tool_calls: Option<Vec<ToolCall>>, reasoning: Option<String>) -> Self {
        Self { role: "assistant".into(), content, tool_calls, tool_call_id: None, reasoning_content: reasoning }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_call_id: Some(tool_call_id.into()),
            ..Default::default()
        }
    }
}

#[derive(Default, Debug)]
pub struct StreamOutcome {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…（已截断）")
}

fn endpoint(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

/// 流式对话；文本增量通过 `sink` 推给 UI，返回本轮完整结果
pub async fn stream_chat(
    http: &reqwest::Client,
    cfg: &Config,
    messages: &[Message],
    tools: &[Value],
    sink: &UnboundedSender<AgentEvent>,
    cancel: &Arc<AtomicBool>,
) -> Result<StreamOutcome> {
    let key = cfg.effective_key();
    if key.is_empty() {
        bail!("尚未配置 DeepSeek API Token，请按 F2 打开配置界面填写");
    }

    let mut body = json!({
        "model": cfg.model,
        "messages": messages,
        "stream": true,
        "temperature": cfg.temperature,
        "max_tokens": cfg.max_tokens,
    });
    if !tools.is_empty() {
        // 用 as_object_mut 而不是 body["tools"]：后者在 Value 非对象时会 panic
        if let Some(obj) = body.as_object_mut() {
            obj.insert("tools".to_string(), json!(tools));
            obj.insert("tool_choice".to_string(), json!("auto"));
        }
    }

    let resp = http
        .post(endpoint(&cfg.base_url, "/chat/completions"))
        .bearer_auth(&key)
        .header("Accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .context("请求 DeepSeek 接口失败（请检查网络或 base_url）")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let hint = match status.as_u16() {
            401 => "（API Token 无效或已过期）",
            402 => "（账户余额不足）",
            429 => "（请求过于频繁，请稍后重试）",
            _ => "",
        };
        bail!("DeepSeek 返回 HTTP {status}{hint}：{}", truncate(text.trim(), 600));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut out = StreamOutcome::default();
    let mut calls: BTreeMap<u64, ToolCall> = BTreeMap::new();
    let mut finished = false;

    'outer: loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("已取消");
        }
        // 单个数据块最长等待 120 秒，避免网络半死挂住整轮对话
        let next = tokio::time::timeout(Duration::from_secs(120), stream.next())
            .await
            .map_err(|_| anyhow!("响应超时（120 秒未收到数据）"))?;
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.context("读取响应流失败")?;
        buf.extend_from_slice(&chunk);

        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim_end_matches(['\r', '\n']);
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                finished = true;
                break 'outer;
            }
            let v: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(err) = v.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("未知错误");
                bail!("接口返回错误：{msg}");
            }
            consume_chunk(&v, &mut out, &mut calls, sink);
        }
    }

    if !finished {
        // 流被服务端提前关闭但没有 [DONE]，如果什么都没拿到就当作失败
        if out.content.is_empty() && calls.is_empty() {
            bail!("响应流意外结束，未收到任何内容");
        }
    }

    let mut idx = 0usize;
    out.tool_calls = calls
        .into_values()
        .map(|mut c| {
            if c.id.is_empty() {
                c.id = format!("call_{}_{idx}", chrono::Local::now().timestamp_millis());
            }
            idx += 1;
            c
        })
        .collect();

    Ok(out)
}

fn consume_chunk(
    v: &Value,
    out: &mut StreamOutcome,
    calls: &mut BTreeMap<u64, ToolCall>,
    sink: &UnboundedSender<AgentEvent>,
) {
    let Some(choice) = v.pointer("/choices/0") else {
        return;
    };
    if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
        out.finish_reason = Some(fr.to_string());
    }
    let Some(delta) = choice.get("delta") else {
        return;
    };

    if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
        if !text.is_empty() {
            out.content.push_str(text);
            let _ = sink.send(AgentEvent::Delta(text.to_string()));
        }
    }
    if let Some(text) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
        if !text.is_empty() {
            out.reasoning.push_str(text);
            let _ = sink.send(AgentEvent::Reasoning(text.to_string()));
        }
    }
    if let Some(list) = delta.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in list {
            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
            let entry = calls.entry(idx).or_insert_with(|| ToolCall {
                id: String::new(),
                kind: default_fn_type(),
                function: FunctionCall::default(),
            });
            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                if !id.is_empty() {
                    entry.id = id.to_string();
                }
            }
            if let Some(kind) = tc.get("type").and_then(|t| t.as_str()) {
                entry.kind = kind.to_string();
            }
            if let Some(f) = tc.get("function") {
                if let Some(name) = f.get("name").and_then(|n| n.as_str()) {
                    if !name.is_empty() && entry.function.name.is_empty() {
                        entry.function.name = name.to_string();
                    }
                }
                if let Some(args) = f.get("arguments").and_then(|a| a.as_str()) {
                    entry.function.arguments.push_str(args);
                }
            }
        }
    }
}

/// 测试连通性：GET /models
pub async fn test_connection(http: &reqwest::Client, api_key: &str, base_url: &str) -> Result<String> {
    if api_key.trim().is_empty() {
        bail!("API Token 为空");
    }
    let resp = http
        .get(endpoint(base_url, "/models"))
        .bearer_auth(api_key.trim())
        .send()
        .await
        .context("无法连接接口地址")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("HTTP {status}：{}", truncate(text.trim(), 300)));
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let models = v
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if models.is_empty() {
        Ok("连接成功".to_string())
    } else {
        Ok(format!("连接成功，可用模型：{models}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn parses_streamed_tool_call_deltas() {
        let (tx, mut rx) = unbounded_channel();
        let mut out = StreamOutcome::default();
        let mut calls: BTreeMap<u64, ToolCall> = BTreeMap::new();

        let chunks = vec![
            json!({"choices":[{"index":0,"delta":{"reasoning_content":"想一下"}}]}),
            json!({"choices":[{"index":0,"delta":{"content":"我先查磁盘"}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"run_shell","arguments":"{\"comm"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"and\":\"df -h\"}"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        ];
        for c in &chunks {
            consume_chunk(c, &mut out, &mut calls, &tx);
        }

        assert_eq!(out.content, "我先查磁盘");
        assert_eq!(out.reasoning, "想一下");
        assert_eq!(out.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(calls.len(), 1);
        let call = calls.get(&0).unwrap();
        assert_eq!(call.id, "call_a");
        assert_eq!(call.function.name, "run_shell");
        assert_eq!(call.function.arguments, "{\"command\":\"df -h\"}");

        // 文本与思维链增量各推了一次
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 2);
    }

    #[test]
    fn assistant_message_does_not_echo_reasoning() {
        let m = Message::assistant(None, None, Some("内部思考".into()));
        let s = serde_json::to_string(&m).unwrap();
        assert!(!s.contains("reasoning"));
        assert!(!s.contains("内部思考"));
        assert!(s.contains("\"role\":\"assistant\""));
    }

    /// 端到端：用本地 HTTP 服务返回一段 SSE，验证 reqwest + 解析器协同工作
    #[tokio::test]
    async fn streams_over_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let body = concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"你好\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"，磁盘 95%\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        });

        let cfg = Config {
            api_key: "sk-test".to_string(),
            base_url: format!("http://{addr}"),
            model: "deepseek-chat".to_string(),
            ..Default::default()
        };
        let http = reqwest::Client::new();
        let (tx, mut rx) = unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));

        let out = stream_chat(&http, &cfg, &[Message::user("hi")], &[], &tx, &cancel)
            .await
            .unwrap();

        assert_eq!(out.content, "你好，磁盘 95%");
        assert_eq!(out.finish_reason.as_deref(), Some("stop"));
        assert!(out.tool_calls.is_empty());

        // 两次增量都推给了 UI
        let mut deltas = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AgentEvent::Delta(t) = ev {
                deltas.push(t);
            }
        }
        assert_eq!(deltas, vec!["你好".to_string(), "，磁盘 95%".to_string()]);
    }

    #[test]
    fn tool_message_carries_call_id() {
        let m = Message::tool("call_1", "ok");
        let v: Value = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(v["role"], "tool");
        assert_eq!(v["tool_call_id"], "call_1");
        assert_eq!(v["content"], "ok");
    }
}
