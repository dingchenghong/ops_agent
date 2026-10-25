//! 应用状态与事件循环

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::agent::{AgentEvent, AgentHandle};
use crate::config::{Config, DEFAULT_BASE_URL, DEFAULT_MODEL};
use crate::db::{Db, StoredMessage};
use crate::ui;
use crate::ui::input::TextInput;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Chat,
    Config,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolState {
    Pending,
    Waiting,
    Running,
    Done,
    Denied,
}

impl ToolState {
    pub fn label(self) -> &'static str {
        match self {
            ToolState::Pending => "准备执行",
            ToolState::Waiting => "等待确认",
            ToolState::Running => "执行中",
            ToolState::Done => "已完成",
            ToolState::Denied => "已拒绝",
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum Display {
    User(String),
    Assistant {
        text: String,
        reasoning: String,
        streaming: bool,
    },
    ToolCall {
        name: String,
        preview: String,
        state: ToolState,
    },
    ToolResult {
        name: String,
        output: String,
        ok: bool,
    },
    Info(String),
    Error(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Status {
    Idle,
    Thinking,
    Responding,
    Tool(String),
    WaitingApproval,
}

impl Status {
    pub fn is_busy(&self) -> bool {
        !matches!(self, Status::Idle)
    }
}

#[derive(Clone, Debug)]
pub struct Approval {
    pub id: String,
    pub name: String,
    pub preview: String,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldKind {
    Secret,
    Text,
    Number,
    Float,
    Bool,
}

pub struct FormField {
    pub label: &'static str,
    pub hint: &'static str,
    pub kind: FieldKind,
    pub value: String,
}

/// 配置项下标（与 `ConfigForm::from` 中的顺序一致）
pub const F_API_KEY: usize = 0;
pub const F_BASE_URL: usize = 1;
pub const F_MODEL: usize = 2;
pub const F_TEMP: usize = 3;
pub const F_MAX_TOKENS: usize = 4;
pub const F_ROUNDS: usize = 5;
pub const F_TIMEOUT: usize = 6;
pub const F_AUTO_APPROVE: usize = 7;
pub const F_CONTEXT: usize = 8;
pub const F_NAMESPACE: usize = 9;

pub struct ConfigForm {
    pub fields: Vec<FormField>,
    pub idx: usize,
    pub editing: bool,
    pub buffer: String,
}

impl ConfigForm {
    pub fn from(cfg: &Config) -> Self {
        Self {
            fields: vec![
                FormField {
                    label: "DeepSeek API Token",
                    hint: "在 platform.deepseek.com 创建。保存后写入配置文件（权限 600）；留空则读取环境变量 DEEPSEEK_API_KEY",
                    kind: FieldKind::Secret,
                    value: cfg.api_key.clone(),
                },
                FormField {
                    label: "接口地址 Base URL",
                    hint: "默认 https://api.deepseek.com；任何 OpenAI 兼容服务都可以填在这里",
                    kind: FieldKind::Text,
                    value: cfg.base_url.clone(),
                },
                FormField {
                    label: "模型",
                    hint: "deepseek-chat（支持工具调用，推荐）/ deepseek-reasoner（深度推理）",
                    kind: FieldKind::Text,
                    value: cfg.model.clone(),
                },
                FormField {
                    label: "temperature",
                    hint: "0.0 - 2.0，运维场景建议 0.1 - 0.4，值越低越稳定",
                    kind: FieldKind::Float,
                    value: format!("{:.2}", cfg.temperature),
                },
                FormField {
                    label: "max_tokens",
                    hint: "单次回复上限，256 - 65536",
                    kind: FieldKind::Number,
                    value: cfg.max_tokens.to_string(),
                },
                FormField {
                    label: "max_tool_rounds",
                    hint: "单轮对话最多工具调用轮次（1-64），防止模型陷入循环",
                    kind: FieldKind::Number,
                    value: cfg.max_tool_rounds.to_string(),
                },
                FormField {
                    label: "命令超时(秒)",
                    hint: "单条 shell / kubectl 命令的超时时间（5-300）",
                    kind: FieldKind::Number,
                    value: cfg.shell_timeout_secs.to_string(),
                },
                FormField {
                    label: "跳过操作确认",
                    hint: "开启后所有工具直接执行、不再弹窗（危险，仅建议测试环境使用）",
                    kind: FieldKind::Bool,
                    value: cfg.auto_approve_all.to_string(),
                },
                FormField {
                    label: "kube-context",
                    hint: "等价于 kubectl --context，留空使用当前上下文",
                    kind: FieldKind::Text,
                    value: cfg.kubectl_context.clone(),
                },
                FormField {
                    label: "kube-namespace",
                    hint: "默认 namespace，留空为 default",
                    kind: FieldKind::Text,
                    value: cfg.kubectl_namespace.clone(),
                },
            ],
            idx: 0,
            editing: false,
            buffer: String::new(),
        }
    }

    pub fn current(&self) -> &FormField {
        &self.fields[self.idx]
    }

    pub fn next(&mut self) {
        self.idx = (self.idx + 1) % self.fields.len();
    }

    pub fn prev(&mut self) {
        self.idx = (self.idx + self.fields.len() - 1) % self.fields.len();
    }

    pub fn begin_edit(&mut self) {
        if self.current().kind == FieldKind::Bool {
            self.toggle();
            return;
        }
        self.buffer = self.current().value.clone();
        self.editing = true;
    }

    pub fn toggle(&mut self) {
        let v = self.fields[self.idx].value.eq_ignore_ascii_case("true");
        self.fields[self.idx].value = (!v).to_string();
    }

    pub fn commit(&mut self) -> Result<()> {
        let idx = self.idx;
        let raw = self.buffer.trim().to_string();
        match self.fields[idx].kind {
            FieldKind::Float => {
                let v: f64 = raw.parse().with_context(|| "请输入数字，例如 0.3")?;
                if !(0.0..=2.0).contains(&v) {
                    bail!("temperature 必须在 0.0 到 2.0 之间");
                }
            }
            FieldKind::Number => {
                let v: u32 = raw.parse().with_context(|| "请输入整数")?;
                match idx {
                    F_MAX_TOKENS if !(256..=65536).contains(&v) => {
                        bail!("max_tokens 需在 256 - 65536 之间")
                    }
                    F_ROUNDS if !(1..=64).contains(&v) => {
                        bail!("max_tool_rounds 需在 1 - 64 之间")
                    }
                    F_TIMEOUT if !(5..=300).contains(&v) => {
                        bail!("命令超时需在 5 - 300 秒之间")
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        self.fields[idx].value = raw;
        self.editing = false;
        self.buffer.clear();
        Ok(())
    }

    pub fn commit_quiet(&mut self) {
        if self.editing {
            if self.commit().is_err() {
                self.editing = false;
                self.buffer.clear();
            }
        }
    }

    fn val(&self, i: usize) -> String {
        self.fields[i].value.trim().to_string()
    }

    pub fn to_config(&self, base: &Config) -> Result<Config> {
        let mut cfg = base.clone();
        cfg.api_key = self.val(F_API_KEY);
        cfg.base_url = {
            let v = self.val(F_BASE_URL);
            if v.is_empty() {
                DEFAULT_BASE_URL.to_string()
            } else {
                v
            }
        };
        if !(cfg.base_url.starts_with("http://") || cfg.base_url.starts_with("https://")) {
            bail!("接口地址需以 http:// 或 https:// 开头");
        }
        cfg.model = {
            let v = self.val(F_MODEL);
            if v.is_empty() {
                DEFAULT_MODEL.to_string()
            } else {
                v
            }
        };
        cfg.temperature = self.val(F_TEMP).parse().context("temperature 非法")?;
        cfg.max_tokens = self.val(F_MAX_TOKENS).parse().context("max_tokens 非法")?;
        cfg.max_tool_rounds = self.val(F_ROUNDS).parse().context("max_tool_rounds 非法")?;
        cfg.shell_timeout_secs = self.val(F_TIMEOUT).parse().context("命令超时非法")?;
        cfg.auto_approve_all = self.fields[F_AUTO_APPROVE].value.eq_ignore_ascii_case("true");
        cfg.kubectl_context = self.val(F_CONTEXT);
        cfg.kubectl_namespace = self.val(F_NAMESPACE);
        Ok(cfg)
    }
}

pub struct App {
    pub cfg: Config,
    pub agent: AgentHandle,
    pub db: Db,
    pub rt: tokio::runtime::Handle,
    pub session_id: i64,
    /// 配置表单是否有未落盘的修改（避免被旧配置覆盖）
    pub form_dirty: bool,
    pub screen: Screen,
    pub messages: Vec<Display>,
    pub input: TextInput,
    pub status: Status,
    pub scroll: usize,
    pub follow: bool,
    pub help: bool,
    pub help_scroll: usize,
    pub approval: Option<Approval>,
    pub form: ConfigForm,
    pub toast: Option<Toast>,
    pub quit: bool,
    pub started: Instant,
    pub turns: usize,
}

impl App {
    pub fn new(
        cfg: Config,
        agent: AgentHandle,
        db: Db,
        rt: tokio::runtime::Handle,
        session_id: i64,
        restored: Vec<StoredMessage>,
    ) -> Self {
        let need_key = !cfg.has_key();
        let form = ConfigForm::from(&cfg);
        let messages = display_from_stored(&restored);
        let mut app = Self {
            cfg,
            agent,
            db,
            rt,
            session_id,
            form_dirty: false,
            screen: Screen::Chat,
            messages,
            input: TextInput::new(),
            status: Status::Idle,
            scroll: 0,
            follow: true,
            help: false,
            help_scroll: 0,
            approval: None,
            form,
            toast: None,
            quit: false,
            started: Instant::now(),
            turns: 0,
        };
        if app.db.is_memory() {
            app.messages
                .insert(0, Display::Error("数据库不可用：配置与会话本次运行不会保存".into()));
        }
        if need_key {
            app.screen = Screen::Config;
            app.toast_err("首次使用：填写 DeepSeek API Token，回车即保存");
        }
        app
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            self.pump();
            terminal.draw(|frame| ui::draw(frame, self))?;
            if event::poll(Duration::from_millis(30))? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
                    _ => {}
                }
            }
        }
        self.agent.shutdown();
        Ok(())
    }

    pub fn busy(&self) -> bool {
        self.status.is_busy() || self.approval.is_some()
    }

    // ---------- 事件处理 ----------

    fn pump(&mut self) {
        while let Some(ev) = self.agent.try_recv() {
            self.on_agent_event(ev);
        }
    }

    fn on_agent_event(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Delta(text) => {
                match self.messages.last_mut() {
                    Some(Display::Assistant { text: body, streaming, .. }) if *streaming => {
                        body.push_str(&text);
                    }
                    _ => self.messages.push(Display::Assistant {
                        text,
                        reasoning: String::new(),
                        streaming: true,
                    }),
                }
                self.status = Status::Responding;
                self.follow = true;
            }
            AgentEvent::Reasoning(text) => {
                match self.messages.last_mut() {
                    Some(Display::Assistant { reasoning, streaming, .. }) if *streaming => {
                        reasoning.push_str(&text);
                    }
                    _ => self.messages.push(Display::Assistant {
                        text: String::new(),
                        reasoning: text,
                        streaming: true,
                    }),
                }
                self.status = Status::Thinking;
                self.follow = true;
            }
            AgentEvent::AssistantDone => {
                self.close_assistant();
            }
            AgentEvent::ToolCall { name, preview, .. } => {
                self.close_assistant();
                if let Some(Display::ToolCall { state, .. }) = self
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|m| matches!(m, Display::ToolCall { .. }))
                {
                    if *state != ToolState::Done && *state != ToolState::Denied {
                        *state = ToolState::Done;
                    }
                }
                self.messages.push(Display::ToolCall {
                    name: name.clone(),
                    preview,
                    state: ToolState::Pending,
                });
                self.status = Status::Tool(name);
                self.follow = true;
            }
            AgentEvent::ApprovalNeeded { id, name, preview } => {
                self.mark_last_tool(ToolState::Waiting);
                self.approval = Some(Approval { id, name, preview });
                self.status = Status::WaitingApproval;
                self.follow = true;
            }
            AgentEvent::ApprovalDone { allow, .. } => {
                self.mark_last_tool(if allow { ToolState::Running } else { ToolState::Denied });
                self.approval = None;
                self.status = if allow { Status::Thinking } else { Status::Responding };
            }
            AgentEvent::ToolResult { output, name, ok, .. } => {
                self.mark_last_tool(ToolState::Done);
                self.messages.push(Display::ToolResult { name, output, ok });
                self.status = Status::Thinking;
                self.follow = true;
            }
            AgentEvent::Info(text) => {
                self.messages.push(Display::Info(text));
                self.follow = true;
            }
            AgentEvent::Error(text) => {
                self.messages.push(Display::Error(text));
                self.status = Status::Idle;
                self.follow = true;
            }
            AgentEvent::SessionChanged { id } => {
                let switched = id != self.session_id;
                self.session_id = id;
                self.messages.clear();
                self.close_assistant();
                self.status = Status::Idle;
                self.scroll = 0;
                self.follow = true;
                if switched {
                    self.toast_ok(format!("已新建会话 #{id}（历史记录已保存）"));
                }
            }
            AgentEvent::TurnDone => {
                self.close_assistant();
                self.status = Status::Idle;
                self.approval = None;
                self.turns += 1;
                self.follow = true;
            }
        }
    }

    fn close_assistant(&mut self) {
        if let Some(Display::Assistant { streaming, .. }) = self.messages.last_mut() {
            *streaming = false;
        }
    }

    fn mark_last_tool(&mut self, state: ToolState) {
        if let Some(Display::ToolCall { state: s, .. }) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m, Display::ToolCall { .. }))
        {
            *s = state;
        }
    }

    // ---------- 键盘 ----------

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // 全局：Ctrl+C 退出
        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            self.quit = true;
            return;
        }

        if self.approval.is_some() {
            self.on_approval_key(key);
            return;
        }
        if self.help {
            match key.code {
                KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('q') => self.help = false,
                KeyCode::Up | KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(3)
                }
                KeyCode::Down | KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(3),
                KeyCode::Home => self.help_scroll = 0,
                _ => {}
            }
            return;
        }

        match self.screen {
            Screen::Chat => self.on_chat_key(key),
            Screen::Config => self.on_config_key(key),
        }
    }

    fn on_approval_key(&mut self, key: KeyEvent) {
        let Some(appr) = self.approval.clone() else {
            return;
        };
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.agent.approve(&appr.id, true);
                self.approval = None;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.agent.approve(&appr.id, false);
                self.approval = None;
                self.toast_ok("已拒绝该操作");
            }
            _ => {}
        }
    }

    fn on_chat_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        match key.code {
            KeyCode::Char('q') if ctrl => self.quit = true,
            KeyCode::Esc => {
                if self.busy() {
                    self.agent.cancel();
                    self.toast_ok("正在取消…");
                } else if !self.input.is_empty() {
                    self.input.clear();
                }
            }
            KeyCode::Enter if !shift && !alt => self.submit(),
            KeyCode::Enter => self.input.insert_newline(),
            KeyCode::Char('j') if ctrl => self.input.insert_newline(),
            KeyCode::F(1) => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::Char('h') if ctrl => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::F(2) => self.open_config(),
            KeyCode::Char('k') if ctrl => self.open_config(),
            KeyCode::Char('l') if ctrl => self.clear_chat(),
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char('w') if ctrl => self.input.delete_word(),
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            KeyCode::Up if ctrl => self.scroll_up(3),
            KeyCode::Down if ctrl => self.scroll_down(3),
            KeyCode::Home if ctrl => {
                self.scroll = 0;
                self.follow = false;
            }
            KeyCode::End if ctrl => self.follow = true,
            KeyCode::Up => self.input.history_prev(),
            KeyCode::Down => self.input.history_next(),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Char(c) if !ctrl => self.input.insert_char(c),
            _ => {}
        }
    }

    fn on_config_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if self.form.editing {
            match key.code {
                // Esc 也视为“确认当前输入”：避免用户填了 Token 按 Esc 后丢失
                KeyCode::Esc => {
                    self.form.commit_quiet();
                    self.form_dirty = true;
                    self.form.editing = false;
                    self.auto_save();
                }
                KeyCode::Enter => {
                    if let Err(e) = self.form.commit() {
                        self.toast_err(format!("{e}"));
                    } else {
                        self.form_dirty = true;
                        self.auto_save();
                    }
                }
                KeyCode::Tab => {
                    self.form.commit_quiet();
                    self.form.next();
                    self.form.begin_edit();
                }
                KeyCode::Backspace => {
                    self.form.buffer.pop();
                    self.form_dirty = true;
                }
                KeyCode::Char('u') if ctrl => self.form.buffer.clear(),
                KeyCode::Char(c) if !ctrl => {
                    self.form.buffer.push(c);
                    self.form_dirty = true;
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::F(2) => {
                if self.form_dirty {
                    self.save_config();
                }
                self.screen = Screen::Chat;
            }
            KeyCode::Up | KeyCode::BackTab => self.form.prev(),
            KeyCode::Down | KeyCode::Tab => self.form.next(),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let is_bool = self.form.current().kind == FieldKind::Bool;
                self.form.begin_edit();
                if is_bool {
                    self.form_dirty = true;
                    self.auto_save();
                }
            }
            KeyCode::Char('s') if ctrl => self.save_config(),
            KeyCode::Char('t') if ctrl => self.test_connection(),
            KeyCode::Char('r') if ctrl => {
                self.form = ConfigForm::from(&self.cfg);
                self.form_dirty = false;
                self.toast_ok("已恢复为当前生效的配置");
            }
            _ => {}
        }
    }

    // ---------- 动作 ----------

    fn submit(&mut self) {
        if self.input.is_empty() {
            return;
        }
        let trimmed = self.input.value.trim().to_string();
        if trimmed.is_empty() {
            return;
        }

        match trimmed.as_str() {
            "/quit" | "/exit" | "/q" => {
                self.quit = true;
                return;
            }
            "/help" | "/?" => {
                self.help = true;
                self.help_scroll = 0;
                return;
            }
            "/config" => {
                self.open_config();
                return;
            }
            "/clear" => {
                self.clear_chat();
                return;
            }
            "/sessions" => {
                self.agent.list_sessions();
                return;
            }
            "/new" => {
                if self.busy() {
                    self.toast_err("正在处理任务，稍后再新建会话");
                    return;
                }
                self.messages.clear();
                self.agent.new_session();
                return;
            }
            _ => {}
        }

        if !self.cfg.has_key() {
            self.toast_err("还没有配置 API Token，请先填写（F2）");
            self.open_config();
            return;
        }
        if self.busy() {
            self.toast_err("正在处理上一个任务，请稍候（Esc 可取消）");
            return;
        }

        // 真正发送时才清空输入（并写入历史）
        self.input.take();
        self.messages.push(Display::User(trimmed.clone()));
        self.status = Status::Thinking;
        self.follow = true;
        self.agent.submit(trimmed);
    }

    fn clear_chat(&mut self) {
        self.messages.clear();
        self.agent.clear();
        self.scroll = 0;
        self.follow = true;
        self.toast_ok("已清空会话");
    }

    fn open_config(&mut self) {
        // 表单里有未保存的修改时不要覆盖，否则用户刚输入的 Token 会被冲掉
        if !self.form_dirty {
            self.form = ConfigForm::from(&self.cfg);
        }
        self.screen = Screen::Config;
    }

    fn save_config(&mut self) {
        self.form.commit_quiet();
        match self.form.to_config(&self.cfg) {
            Ok(cfg) => self.persist_config(cfg, true),
            Err(e) => self.toast_err(format!("{e}")),
        }
    }

    /// 编辑过程中的即时保存：不弹错误、不打断输入
    fn auto_save(&mut self) {
        match self.form.to_config(&self.cfg) {
            Ok(cfg) => self.persist_config(cfg, true),
            Err(e) => self.toast_err(format!("{e}")),
        }
    }

    /// 写入 SQLite 的 settings 表，并让 Agent 立即使用新配置
    fn persist_config(&mut self, cfg: Config, notify: bool) {
        let pairs = cfg.to_settings();
        match self.rt.block_on(self.db.save_settings(&pairs)) {
            Ok(()) => {
                self.cfg = cfg;
                self.agent.update_config(&self.cfg);
                self.form_dirty = false;
                if notify {
                    if self.db.is_memory() {
                        self.toast_err("数据库不可用：配置仅本次运行有效");
                    } else {
                        self.toast_ok(format!("已保存 → {}", self.db.file().display()));
                    }
                }
            }
            Err(e) => self.toast_err(format!("保存到数据库失败：{e:#}")),
        }
    }

    fn test_connection(&mut self) {
        let key = self.form.fields[F_API_KEY].value.trim().to_string();
        let base = self.form.fields[F_BASE_URL].value.trim().to_string();
        let key = if key.is_empty() { self.cfg.effective_key() } else { key };
        self.agent.test_connection(key, base);
        self.toast_ok("正在测试连接…");
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.follow = false;
        self.scroll = self.scroll.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_add(n);
    }

    pub fn toast_ok(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), at: Instant::now(), error: false });
    }

    pub fn toast_err(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), at: Instant::now(), error: true });
    }

    pub fn active_toast(&self) -> Option<&Toast> {
        self.toast
            .as_ref()
            .filter(|t| t.at.elapsed() < Duration::from_secs(4))
    }
}

/// 把数据库里的历史消息还原成聊天界面条目（重启后可继续看历史）
pub fn display_from_stored(stored: &[StoredMessage]) -> Vec<Display> {
    let mut out: Vec<Display> = Vec::new();
    for m in stored {
        match m.role.as_str() {
            "user" => out.push(Display::User(m.content.clone())),
            "assistant" => {
                if !m.content.trim().is_empty() || !m.reasoning.trim().is_empty() {
                    out.push(Display::Assistant {
                        text: m.content.clone(),
                        reasoning: m.reasoning.clone(),
                        streaming: false,
                    });
                }
                if let Some(json) = m.tool_calls.as_deref() {
                    if let Ok(calls) = serde_json::from_str::<Vec<crate::llm::ToolCall>>(json) {
                        for c in calls {
                            let args: serde_json::Value =
                                serde_json::from_str(&c.function.arguments).unwrap_or_default();
                            out.push(Display::ToolCall {
                                name: c.function.name.clone(),
                                preview: crate::tools::preview(&c.function.name, &args),
                                state: ToolState::Done,
                            });
                        }
                    }
                }
            }
            "tool" => {
                // 存的是 "status: ok\n<output>"，展示时去掉状态行
                let output = m
                    .content
                    .strip_prefix("status: ")
                    .and_then(|rest| rest.split_once('\n').map(|(_, body)| body.to_string()))
                    .unwrap_or_else(|| m.content.clone());
                out.push(Display::ToolResult {
                    name: m.tool_name.clone().unwrap_or_else(|| "tool".into()),
                    output,
                    ok: m.ok,
                });
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    fn app_with_db(rt: &tokio::runtime::Runtime) -> (App, Db) {
        let db = rt.block_on(Db::open_memory()).unwrap();
        let cfg = Config::default();
        let agent = AgentHandle::spawn(rt, cfg.clone(), db.clone(), 1, Vec::new());
        (App::new(cfg, agent, db.clone(), rt.handle().clone(), 1, Vec::new()), db)
    }

    /// 回归测试：在配置页填完 Token 回车后，必须立刻写入数据库并生效
    #[test]
    fn editing_token_saves_immediately() {
        let rt = runtime();
        let (mut app, db) = app_with_db(&rt);

        app.form.idx = F_API_KEY;
        app.form.begin_edit();
        app.form.buffer = "sk-regression-123456".to_string();
        app.form.commit().unwrap();
        app.form_dirty = true;
        app.save_config();

        assert!(app.cfg.has_key(), "内存中的配置应已更新");
        let pairs = rt.block_on(db.load_settings()).unwrap();
        let saved = pairs
            .iter()
            .find(|(k, _)| k == "api_key")
            .map(|(_, v)| v.clone());
        assert_eq!(saved.as_deref(), Some("sk-regression-123456"));
    }

    /// 回归测试：打开配置页不能冲掉未保存的输入
    #[test]
    fn open_config_keeps_dirty_form() {
        let rt = runtime();
        let (mut app, _db) = app_with_db(&rt);

        app.form.idx = F_API_KEY;
        app.form.fields[F_API_KEY].value = "sk-typed-but-not-saved".to_string();
        app.form_dirty = true;
        app.open_config();

        assert_eq!(app.form.fields[F_API_KEY].value, "sk-typed-but-not-saved");
    }

    /// 会话内容应能写入并从数据库还原
    #[test]
    fn messages_are_restored_into_display() {
        let rt = runtime();
        let db = rt.block_on(Db::open_memory()).unwrap();
        let id = rt.block_on(db.create_session("测试会话", "deepseek-chat")).unwrap();
        rt.block_on(db.append_message(
            id,
            &StoredMessage {
                role: "user".into(),
                content: "磁盘满了".into(),
                ok: true,
                ..Default::default()
            },
        ))
        .unwrap();

        let stored = rt.block_on(db.load_messages(id)).unwrap();
        let display = display_from_stored(&stored);
        assert_eq!(display.len(), 1);
        match &display[0] {
            Display::User(t) => assert_eq!(t, "磁盘满了"),
            _ => panic!("应还原为用户消息"),
        }
        let history = crate::agent::history_from_stored(&stored);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, "user");
    }
}
