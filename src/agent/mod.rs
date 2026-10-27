//! Agent 运行时：持有会话历史，驱动「模型 → 工具 → 模型」的循环
//!
//! UI 线程与 Agent 任务之间通过两个 channel 通信：
//! - `UiCommand`：UI → Agent（提交问题、审批、取消、更新配置）
//! - `AgentEvent`：Agent → UI（文本增量、工具调用、错误）

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::config::Config;
use crate::db::{Db, StoredMessage};
use crate::llm::{self, Message};
use crate::prompt;
use crate::tools::{self, ToolResult};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// 正文增量
    Delta(String),
    /// 思维链增量（deepseek-reasoner）
    Reasoning(String),
    /// 本轮正文结束
    AssistantDone,
    /// 模型请求调用工具
    ToolCall { id: String, name: String, preview: String },
    /// 需要用户确认
    ApprovalNeeded { id: String, name: String, preview: String },
    /// 用户已确认/拒绝
    ApprovalDone { id: String, allow: bool },
    ToolResult { id: String, name: String, output: String, ok: bool },
    Info(String),
    Error(String),
    /// 当前会话切换：reset=true 表示界面需要清空（新建 / 清空会话）
    SessionChanged { id: i64, reset: bool },
    /// 整轮结束（可以接受下一次输入）
    TurnDone,
}

#[derive(Debug)]
pub enum UiCommand {
    Submit(String),
    Approve { id: String, allow: bool },
    Cancel,
    Clear,
    NewSession,
    SwitchSession(i64),
    ListSessions,
    UpdateConfig(Box<Config>),
    TestConnection { api_key: String, base_url: String },
    Shutdown,
}

enum Approval {
    Allow,
    Deny,
    Abort,
}

/// UI 侧句柄
pub struct AgentHandle {
    cmd: UnboundedSender<UiCommand>,
    evt: UnboundedReceiver<AgentEvent>,
    cancel: Arc<AtomicBool>,
}

impl AgentHandle {
    pub fn spawn(
        rt: &tokio::runtime::Runtime,
        cfg: Config,
        db: Db,
        session_id: i64,
        history: Vec<Message>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, evt_rx) = unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));

        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();

        let session = Session {
            cfg,
            db,
            session_id,
            history,
            http,
            system_prompt: prompt::build(),
            queued: None,
            cmd: cmd_rx,
            evt: evt_tx,
            cancel: cancel.clone(),
        };
        rt.spawn(async move { session.run().await });

        Self { cmd: cmd_tx, evt: evt_rx, cancel }
    }

    pub fn submit(&self, text: String) {
        let _ = self.cmd.send(UiCommand::Submit(text));
    }

    pub fn approve(&self, id: &str, allow: bool) {
        let _ = self.cmd.send(UiCommand::Approve { id: id.to_string(), allow });
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd.send(UiCommand::Cancel);
    }

    pub fn clear(&self) {
        let _ = self.cmd.send(UiCommand::Clear);
    }

    pub fn new_session(&self) {
        let _ = self.cmd.send(UiCommand::NewSession);
    }

    pub fn list_sessions(&self) {
        let _ = self.cmd.send(UiCommand::ListSessions);
    }

    pub fn switch_session(&self, id: i64) {
        let _ = self.cmd.send(UiCommand::SwitchSession(id));
    }

    pub fn update_config(&self, cfg: &Config) {
        let _ = self.cmd.send(UiCommand::UpdateConfig(Box::new(cfg.clone())));
    }

    pub fn test_connection(&self, api_key: String, base_url: String) {
        let _ = self.cmd.send(UiCommand::TestConnection { api_key, base_url });
    }

    pub fn shutdown(&self) {
        let _ = self.cmd.send(UiCommand::Shutdown);
    }

    pub fn try_recv(&mut self) -> Option<AgentEvent> {
        self.evt.try_recv().ok()
    }
}

/// 把数据库里的历史消息还原成 LLM 消息序列（用于重启后继续对话）
pub fn history_from_stored(stored: &[StoredMessage]) -> Vec<Message> {
    stored
        .iter()
        .filter_map(|m| match m.role.as_str() {
            "user" => Some(Message::user(m.content.clone())),
            "assistant" => {
                let calls = m
                    .tool_calls
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<Vec<llm::ToolCall>>(s).ok());
                let has_calls = calls.as_ref().map(|c| !c.is_empty()).unwrap_or(false);
                let content = if m.content.trim().is_empty() {
                    None
                } else {
                    Some(m.content.clone())
                };
                if content.is_none() && !has_calls {
                    return None;
                }
                Some(Message::assistant(
                    content,
                    if has_calls { calls } else { None },
                    if m.reasoning.trim().is_empty() {
                        None
                    } else {
                        Some(m.reasoning.clone())
                    },
                ))
            }
            "tool" => Some(Message::tool(
                m.tool_call_id.clone().unwrap_or_default(),
                m.content.clone(),
            )),
            _ => None,
        })
        .collect()
}

struct Session {
    cfg: Config,
    db: Db,
    session_id: i64,
    history: Vec<Message>,
    http: reqwest::Client,
    system_prompt: String,
    queued: Option<String>,
    cmd: UnboundedReceiver<UiCommand>,
    evt: UnboundedSender<AgentEvent>,
    cancel: Arc<AtomicBool>,
}

impl Session {
    async fn run(mut self) {
        let _ = self.evt.send(AgentEvent::Info(format!(
            "已就绪 · 模型 {} · 输入问题开始对话（F1 查看帮助，F2 配置 Token）",
            self.cfg.model
        )));
        while let Some(cmd) = self.cmd.recv().await {
            if !self.handle_command(cmd).await {
                break;
            }
            if let Some(text) = self.queued.take() {
                self.turn(text).await;
            }
        }
    }

    async fn handle_command(&mut self, cmd: UiCommand) -> bool {
        match cmd {
            UiCommand::Submit(text) => {
                self.turn(text).await;
                true
            }
            UiCommand::Approve { .. } => true,
            UiCommand::Cancel => {
                self.cancel.store(true, Ordering::SeqCst);
                true
            }
            UiCommand::Clear => {
                self.history.clear();
                self.queued = None;
                if let Err(e) = self.db.clear_messages(self.session_id).await {
                    let _ = self.evt.send(AgentEvent::Error(format!("清空历史失败：{e:#}")));
                }
                let _ = self.evt.send(AgentEvent::SessionChanged { id: self.session_id, reset: true });
                true
            }
            UiCommand::NewSession => {
                self.history.clear();
                self.queued = None;
                match self.db.create_session("新会话", &self.cfg.model).await {
                    Ok(id) => {
                        self.session_id = id;
                        let _ = self.evt.send(AgentEvent::SessionChanged { id, reset: true });
                    }
                    Err(e) => {
                        let _ = self
                            .evt
                            .send(AgentEvent::Error(format!("新建会话失败：{e:#}")));
                    }
                }
                true
            }
            UiCommand::SwitchSession(id) => {
                match self.db.load_messages(id).await {
                    Ok(msgs) => {
                        self.history = history_from_stored(&msgs);
                        self.session_id = id;
                        self.queued = None;
                        let _ = self
                            .evt
                            .send(AgentEvent::SessionChanged { id, reset: false });
                    }
                    Err(e) => {
                        let _ = self
                            .evt
                            .send(AgentEvent::Error(format!("切换会话失败：{e:#}")));
                    }
                }
                true
            }
            UiCommand::ListSessions => {
                let msg = match self.db.list_sessions(10).await {
                    Ok(list) if list.is_empty() => "还没有历史会话".to_string(),
                    Ok(list) => {
                        let mut m = String::from("最近的会话：\n");
                        for s in list {
                            let title = if s.title.trim().is_empty() {
                                "（无标题）".to_string()
                            } else {
                                s.title.clone()
                            };
                            m.push_str(&format!(
                                "  #{} {} · {} 条 · {} · {}\n",
                                s.id, title, s.messages, s.model, s.updated_at
                            ));
                        }
                        m.push_str("\n/new 新建会话，/clear 清空当前会话");
                        m
                    }
                    Err(e) => format!("读取会话列表失败：{e:#}"),
                };
                let _ = self.evt.send(AgentEvent::Info(msg));
                true
            }
            UiCommand::UpdateConfig(cfg) => {
                self.cfg = *cfg;
                true
            }
            UiCommand::TestConnection { api_key, base_url } => {
                let base = if base_url.trim().is_empty() {
                    crate::config::DEFAULT_BASE_URL.to_string()
                } else {
                    base_url
                };
                match llm::test_connection(&self.http, &api_key, &base).await {
                    Ok(msg) => {
                        let _ = self.evt.send(AgentEvent::Info(format!("连接测试：{msg}")));
                    }
                    Err(e) => {
                        let _ = self
                            .evt
                            .send(AgentEvent::Error(format!("连接测试失败：{e:#}")));
                    }
                }
                true
            }
            UiCommand::Shutdown => false,
        }
    }

    fn base_history(&self) -> Vec<Message> {
        let mut v = Vec::with_capacity(self.history.len() + 1);
        v.push(Message::system(self.system_prompt.clone()));
        v.extend(self.history.iter().cloned());
        v
    }

    async fn turn(&mut self, text: String) {
        self.cancel.store(false, Ordering::SeqCst);
        let is_first = self.history.is_empty();
        self.history.push(Message::user(text.clone()));
        self.persist(StoredMessage {
            role: "user".into(),
            content: text.clone(),
            ok: true,
            ..Default::default()
        })
        .await;
        if is_first {
            let _ = self.db.set_title(self.session_id, &session_title(&text)).await;
        }

        let defs = tools::definitions();
        let max_rounds = self.cfg.max_tool_rounds.clamp(1, 64);

        for round in 1..=max_rounds {
            let messages = self.base_history();
            let outcome = llm::stream_chat(
                &self.http,
                &self.cfg,
                &messages,
                &defs,
                &self.evt,
                &self.cancel,
            )
            .await;

            let outcome = match outcome {
                Ok(o) => o,
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("已取消") {
                        let _ = self.evt.send(AgentEvent::Info("已取消本次请求".into()));
                    } else {
                        let _ = self.evt.send(AgentEvent::Error(format!("{e:#}")));
                    }
                    let _ = self.evt.send(AgentEvent::TurnDone);
                    return;
                }
            };

            let has_calls = !outcome.tool_calls.is_empty();
            self.history.push(Message::assistant(
                if outcome.content.is_empty() && has_calls {
                    None
                } else {
                    Some(outcome.content.clone())
                },
                if has_calls { Some(outcome.tool_calls.clone()) } else { None },
                if outcome.reasoning.is_empty() { None } else { Some(outcome.reasoning.clone()) },
            ));
            self.persist(StoredMessage {
                role: "assistant".into(),
                content: outcome.content.clone(),
                reasoning: outcome.reasoning.clone(),
                tool_calls: if has_calls {
                    serde_json::to_string(&outcome.tool_calls).ok()
                } else {
                    None
                },
                ok: true,
                ..Default::default()
            })
            .await;
            let _ = self.evt.send(AgentEvent::AssistantDone);

            if !has_calls {
                break;
            }

            for call in outcome.tool_calls.iter() {
                if self.cancel.load(Ordering::SeqCst) {
                    let _ = self.evt.send(AgentEvent::Info("已取消后续操作".into()));
                    let _ = self.evt.send(AgentEvent::TurnDone);
                    return;
                }

                let name = call.function.name.clone();
                let args = parse_args(&call.function.arguments);
                let prev = tools::preview(&name, &args);
                let _ = self.evt.send(AgentEvent::ToolCall {
                    id: call.id.clone(),
                    name: name.clone(),
                    preview: prev.clone(),
                });

                let need_approval = tools::requires_approval(&self.cfg, &name, &args);
                if need_approval {
                    match self.wait_approval(&call.id, &name, &prev).await {
                        Approval::Allow => {}
                        Approval::Deny => {
                            let msg = "用户拒绝执行该操作。请不要重复请求同一个操作，\
改为向用户解释建议的命令并说明风险，等待用户自行执行。";
                            let _ = self.evt.send(AgentEvent::ToolResult {
                                id: call.id.clone(),
                                name: name.clone(),
                                output: msg.to_string(),
                                ok: false,
                            });
                            self.history.push(Message::tool(call.id.clone(), msg));
                            self.persist(StoredMessage {
                                role: "tool".into(),
                                content: msg.to_string(),
                                tool_call_id: Some(call.id.clone()),
                                tool_name: Some(name.clone()),
                                ok: false,
                                ..Default::default()
                            })
                            .await;
                            continue;
                        }
                        Approval::Abort => {
                            let _ = self.evt.send(AgentEvent::Info("已取消后续操作".into()));
                            let _ = self.evt.send(AgentEvent::TurnDone);
                            return;
                        }
                    }
                }

                let result = tools::execute(&self.cfg, &name, &args).await;
                let status = if result.ok { "success" } else { "error" };
                let _ = self.evt.send(AgentEvent::ToolResult {
                    id: call.id.clone(),
                    name: name.clone(),
                    output: result.output.clone(),
                    ok: result.ok,
                });
                let tool_content = format!("status: {status}\n{}", result.output);
                self.history.push(Message::tool(call.id.clone(), tool_content.clone()));
                self.persist(StoredMessage {
                    role: "tool".into(),
                    content: tool_content,
                    tool_call_id: Some(call.id.clone()),
                    tool_name: Some(name.clone()),
                    ok: result.ok,
                    ..Default::default()
                })
                .await;
            }

            if round == max_rounds {
                let _ = self.evt.send(AgentEvent::Error(format!(
                    "工具调用达到上限（{max_rounds} 轮），已停止。可以让它总结当前进展后再继续。"
                )));
            }
        }

        let _ = self.db.touch_session(self.session_id, &self.cfg.model).await;
        let _ = self.evt.send(AgentEvent::TurnDone);
    }

    async fn persist(&self, m: StoredMessage) {
        if let Err(e) = self.db.append_message(self.session_id, &m).await {
            let _ = self
                .evt
                .send(AgentEvent::Error(format!("保存消息到数据库失败：{e:#}")));
        }
    }

    async fn wait_approval(&mut self, id: &str, name: &str, preview: &str) -> Approval {
        let _ = self.evt.send(AgentEvent::ApprovalNeeded {
            id: id.to_string(),
            name: name.to_string(),
            preview: preview.to_string(),
        });

        loop {
            match self.cmd.recv().await {
                Some(UiCommand::Approve { id: rid, allow }) if rid == id => {
                    let _ = self.evt.send(AgentEvent::ApprovalDone { id: id.to_string(), allow });
                    return if allow { Approval::Allow } else { Approval::Deny };
                }
                Some(UiCommand::Cancel) => {
                    self.cancel.store(true, Ordering::SeqCst);
                    let _ = self.evt.send(AgentEvent::ApprovalDone { id: id.to_string(), allow: false });
                    return Approval::Deny;
                }
                Some(UiCommand::UpdateConfig(cfg)) => {
                    self.cfg = *cfg;
                    let _ = self.evt.send(AgentEvent::Info("配置已更新".into()));
                }
                Some(UiCommand::Submit(text)) => {
                    self.queued = Some(text);
                    let _ = self
                        .evt
                        .send(AgentEvent::Info("已排队，当前操作处理完后会自动执行".into()));
                }
                Some(UiCommand::Shutdown) | None => {
                    return Approval::Abort;
                }
                Some(_) => {}
            }
        }
    }
}

/// 用首条用户消息生成会话标题
fn session_title(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = one_line.chars().take(40).collect();
    let mut t: String = chars.into_iter().collect();
    if one_line.chars().count() > 40 {
        t.push('…');
    }
    t
}

fn parse_args(raw: &str) -> Value {
    let raw = raw.trim();
    if raw.is_empty() {
        return json!({});
    }
    serde_json::from_str(raw).unwrap_or_else(|_| json!({ "_raw_arguments": raw }))
}

/// 便捷构造函数：给测试或外部调用
#[allow(dead_code)]
pub fn tool_output(name: &str, r: &ToolResult) -> String {
    format!("{name}: {}", r.output)
}
