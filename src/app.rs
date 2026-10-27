//! 应用状态与事件循环

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::DefaultTerminal;

use crate::agent::{AgentEvent, AgentHandle};
use crate::config::{Config, DEFAULT_BASE_URL, DEFAULT_MODEL};
use crate::db::{Db, SessionInfo, StoredMessage};
use crate::ui;
use crate::ui::input::TextInput;

/// 覆盖在主界面之上的弹窗
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overlay {
    None,
    Settings,
    About,
}

/// 正在拖动的分隔条
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drag {
    /// 左右分栏（侧边栏宽度）
    Sidebar,
    /// 输入框上边框（输入框高度）
    Input,
}

/// 终端指针形状（悬停到可拖动区域时切换）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pointer {
    Default,
    EwResize,
    NsResize,
}

impl Pointer {
    fn sequence(self) -> &'static str {
        match self {
            // OSC 22 设置鼠标指针形状（kitty / WezTerm / foot / Ghostty 等支持）
            // CSI 2 SP q = 实心方块光标，比细线醒目
            Pointer::EwResize => "\x1b]22;ew-resize\x1b\\\x1b[2 q",
            Pointer::NsResize => "\x1b]22;ns-resize\x1b\\\x1b[2 q",
            Pointer::Default => "\x1b]22;default\x1b\\\x1b[0 q",
        }
    }
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
    /// 本次编辑中用户是否已手动输入过（用于「粘贴直接替换原值」）
    pub edited: bool,
    /// 设置项里 Ctrl+A 全选（简化版：整段选中）
    pub sel_all: bool,
}

/// 索引兜底用的空字段（`at()` 越界时返回它，避免 panic）
static EMPTY_FIELD: FormField = FormField {
    label: "",
    hint: "",
    kind: FieldKind::Text,
    value: String::new(),
};

impl ConfigForm {
    pub fn from(cfg: &Config) -> Self {
        Self {
            fields: vec![
                FormField {
                    label: "DeepSeek API Token",
                    hint: "在 platform.deepseek.com 创建。回车即写入 SQLite 的 settings 表；留空则读取环境变量 DEEPSEEK_API_KEY",
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
            edited: false,
            sel_all: false,
        }
    }

    /// 按索引取字段：越界时回退到一个空字段而不是 panic
    /// （索引由 `F_*` 常量与取模维护，正常不会越界，这里只是兜底）
    pub fn at(&self, i: usize) -> &FormField {
        self.fields.get(i).unwrap_or(&EMPTY_FIELD)
    }

    pub fn current(&self) -> &FormField {
        self.at(self.idx)
    }

    pub fn next(&mut self) {
        if self.fields.is_empty() {
            return;
        }
        self.idx = (self.idx + 1) % self.fields.len();
    }

    pub fn prev(&mut self) {
        if self.fields.is_empty() {
            return;
        }
        self.idx = (self.idx + self.fields.len() - 1) % self.fields.len();
    }

    pub fn begin_edit(&mut self) {
        if self.current().kind == FieldKind::Bool {
            self.toggle();
            return;
        }
        self.buffer = self.current().value.clone();
        self.editing = true;
        self.edited = false;
        self.sel_all = false;
    }

    pub fn toggle(&mut self) {
        if let Some(f) = self.fields.get_mut(self.idx) {
            let v = f.value.eq_ignore_ascii_case("true");
            f.value = (!v).to_string();
        }
    }

    pub fn commit(&mut self) -> Result<()> {
        let idx = self.idx;
        let raw = self.buffer.trim().to_string();
        match self.at(idx).kind {
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
        if let Some(f) = self.fields.get_mut(idx) {
            f.value = raw;
        }
        self.editing = false;
        self.buffer.clear();
        self.edited = false;
        self.sel_all = false;
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
        self.at(i).value.trim().to_string()
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
        cfg.auto_approve_all = self.at(F_AUTO_APPROVE).value.eq_ignore_ascii_case("true");
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
    pub overlay: Overlay,
    /// 左侧会话列表（来自 SQLite）
    pub sessions: Vec<SessionInfo>,
    /// 侧边栏选中项：0 = 新对话，1..=n 对应 sessions
    pub sidebar_idx: usize,
    pub sidebar_scroll: usize,
    /// 焦点是否在侧边栏
    pub focus_sidebar: bool,
    /// 最近一帧的终端尺寸（鼠标命中判定用）
    pub size: (u16, u16),
    /// 侧边栏宽度 / 是否展开 / 输入框高度（会持久化到 settings）
    pub sidebar_width: u16,
    pub sidebar_open: bool,
    pub input_height: u16,
    /// 正在拖动的分隔条
    pub drag: Option<Drag>,
    /// 鼠标当前悬停的可拖动分隔条
    pub hover_divider: Option<Drag>,
    /// 鼠标最近位置（用于把光标定位到分隔条上）
    pub hover_pos: Option<(u16, u16)>,
    /// 已经下发给终端的指针形状
    pointer: Pointer,
    pub messages: Vec<Display>,
    pub input: TextInput,
    pub status: Status,
    pub scroll: usize,
    pub follow: bool,
    pub about_scroll: usize,
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
        let sessions = rt.block_on(db.list_sessions(200)).unwrap_or_default();
        let sidebar_idx = sessions
            .iter()
            .position(|s| s.id == session_id)
            .map(|i| i + 1)
            .unwrap_or(0);

        // 界面偏好（侧边栏宽度、输入框高度）
        let mut sidebar_width = crate::ui::layout::SIDEBAR_DEFAULT;
        let mut sidebar_open = true;
        let mut input_height = crate::ui::layout::INPUT_DEFAULT;
        if let Ok(pairs) = rt.block_on(db.load_settings()) {
            for (k, v) in pairs {
                match k.as_str() {
                    "ui_sidebar_width" => {
                        if let Ok(w) = v.parse::<u16>() {
                            sidebar_width = w;
                        }
                    }
                    "ui_sidebar_open" => sidebar_open = v == "true",
                    "ui_input_height" => {
                        if let Ok(h) = v.parse::<u16>() {
                            input_height = h;
                        }
                    }
                    _ => {}
                }
            }
        }

        let mut app = Self {
            cfg,
            agent,
            db,
            rt,
            session_id,
            form_dirty: false,
            overlay: Overlay::None,
            sessions,
            sidebar_idx,
            sidebar_scroll: 0,
            focus_sidebar: false,
            size: (0, 0),
            sidebar_width,
            sidebar_open,
            input_height,
            drag: None,
            hover_divider: None,
            hover_pos: None,
            pointer: Pointer::Default,
            messages,
            input: TextInput::new(),
            status: Status::Idle,
            scroll: 0,
            follow: true,
            about_scroll: 0,
            approval: None,
            form,
            toast: None,
            quit: false,
            started: Instant::now(),
            turns: 0,
        };

        if app.db.is_memory() {
            app.messages.insert(
                0,
                Display::Error("数据库不可用：配置与会话本次运行不会保存".into()),
            );
        }
        if need_key {
            app.overlay = Overlay::Settings;
            app.toast_err("首次使用：填写 DeepSeek API Token，回车即保存");
        }
        app
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut modal_state = (self.overlay, self.approval.is_some());
        while !self.quit {
            self.pump();
            // 弹窗开关时整屏重绘，避免上一帧残留的错位字符
            let now = (self.overlay, self.approval.is_some());
            if now != modal_state {
                let _ = terminal.clear();
                modal_state = now;
            }
            terminal.draw(|frame| ui::draw(frame, self))?;
            self.sync_pointer();
            if event::poll(Duration::from_millis(30))? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
                    Event::Mouse(m) => self.on_mouse(m),
                    // 终端的括号粘贴（bracketed paste）会作为一个整体事件到达
                    Event::Paste(text) => self.on_paste(text),
                    _ => {}
                }
            }
        }
        // 复原指针与光标形状，避免退出后终端残留
        self.hover_divider = None;
        self.sync_pointer();
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
            AgentEvent::SessionChanged { id, reset } => {
                let switched = id != self.session_id;
                self.session_id = id;
                if reset {
                    self.messages.clear();
                    self.close_assistant();
                    self.status = Status::Idle;
                    self.scroll = 0;
                    self.follow = true;
                    if switched {
                        self.toast_ok(format!("已新建会话 #{id}（历史记录已保存）"));
                    }
                }
                self.refresh_sessions();
            }
            AgentEvent::TurnDone => {
                self.close_assistant();
                self.status = Status::Idle;
                self.approval = None;
                self.turns += 1;
                self.follow = true;
                // 会话标题/消息数可能已变化
                self.refresh_sessions();
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

        // 全局 Ctrl+C：有选中内容时复制，否则退出
        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            if self.copy_selection() {
                return;
            }
            self.quit = true;
            return;
        }
        if self.approval.is_some() {
            self.on_approval_key(key);
            return;
        }
        match self.overlay {
            Overlay::About => {
                self.on_about_key(key);
                return;
            }
            Overlay::Settings => {
                self.on_config_key(key);
                return;
            }
            Overlay::None => {}
        }
        if self.focus_sidebar {
            self.on_sidebar_key(key);
            return;
        }
        self.on_chat_key(key);
    }

    fn on_about_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('q') => self.overlay = Overlay::None,
            KeyCode::Up | KeyCode::PageUp => {
                self.about_scroll = self.about_scroll.saturating_sub(3)
            }
            KeyCode::Down | KeyCode::PageDown => {
                self.about_scroll = self.about_scroll.saturating_add(3)
            }
            KeyCode::Home => self.about_scroll = 0,
            _ => {}
        }
    }

    fn on_sidebar_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Tab | KeyCode::Esc => self.focus_sidebar = false,
            KeyCode::Up => self.sidebar_move(-1),
            KeyCode::Down => self.sidebar_move(1),
            KeyCode::Home => self.sidebar_idx = 0,
            KeyCode::End => self.sidebar_idx = self.sessions.len(),
            KeyCode::Enter => self.sidebar_activate(),
            KeyCode::Char('n') if ctrl => self.new_session(),
            _ => {}
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
                } else if self.input.selection().is_some() {
                    // 先取消选区，再清空
                    self.input.clear_selection();
                } else if !self.input.is_empty() {
                    self.input.clear();
                }
            }
            KeyCode::Enter if !shift && !alt => self.submit(),
            KeyCode::Enter => self.input.insert_newline(),
            KeyCode::Char('j') if ctrl => self.input.insert_newline(),
            KeyCode::Tab => self.focus_sidebar = true,
            KeyCode::F(1) => self.open_about(),
            KeyCode::Char('h') if ctrl => self.open_about(),
            KeyCode::F(2) => self.open_settings(),
            KeyCode::Char('k') if ctrl => self.open_settings(),
            KeyCode::Char('l') if ctrl => self.clear_chat(),
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char('a') if ctrl => self.input.select_all(),
            KeyCode::Char('w') if ctrl => self.input.delete_word(),
            KeyCode::Char('b') if ctrl => self.toggle_sidebar(),
            KeyCode::Char('v') if ctrl => {
                self.toast_err("请用 Ctrl+Shift+V 或 Shift+Insert 粘贴（终端不会把 Ctrl+V 交给程序）")
            }
            KeyCode::Up if alt => self.adjust_input_height(1),
            KeyCode::Down if alt => self.adjust_input_height(-1),
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            KeyCode::Up if ctrl => self.scroll_up(3),
            KeyCode::Down if ctrl => self.scroll_down(3),
            KeyCode::Left if shift => self.input.left_extend(),
            KeyCode::Right if shift => self.input.right_extend(),
            KeyCode::Home if shift => self.input.home_extend(),
            KeyCode::End if shift => self.input.end_extend(),
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
                    if self.form.sel_all {
                        self.form.buffer.clear();
                        self.form.sel_all = false;
                    } else {
                        self.form.buffer.pop();
                    }
                    self.form.edited = true;
                    self.form_dirty = true;
                }
                KeyCode::Char('u') if ctrl => {
                    self.form.buffer.clear();
                    self.form.sel_all = false;
                    self.form.edited = true;
                }
                KeyCode::Char('a') if ctrl => {
                    if !self.form.buffer.is_empty() {
                        self.form.sel_all = true;
                    }
                }
                KeyCode::Char('v') if ctrl => self.toast_err(
                    "请用 Ctrl+Shift+V 或 Shift+Insert 粘贴（终端不会把 Ctrl+V 交给程序）",
                ),
                KeyCode::Char(c) if !ctrl => {
                    let safe = crate::ui::util::sanitize_char(c);
                    if safe.is_empty() {
                        return;
                    }
                    if self.form.sel_all {
                        self.form.buffer.clear();
                        self.form.sel_all = false;
                    }
                    self.form.buffer.push_str(&safe);
                    self.form.edited = true;
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
                self.overlay = Overlay::None;
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

    /// 鼠标：菜单、新对话、会话条目、分隔条拖拽、滚动
    fn on_mouse(&mut self, m: MouseEvent) {
        if self.size.0 == 0 || self.size.1 == 0 {
            return;
        }
        let lay = crate::ui::layout::compute(Rect::new(0, 0, self.size.0, self.size.1), self);
        let pos = Position { x: m.column, y: m.row };
        let hit = |r: Rect| r.contains(pos);

        match m.kind {
            MouseEventKind::Moved => self.update_hover(m.column, m.row),
            MouseEventKind::Down(MouseButton::Left) => {
                self.update_hover(m.column, m.row);
                if hit(lay.toggle_sidebar) {
                    self.toggle_sidebar();
                } else if hit(lay.settings) {
                    self.open_settings();
                } else if hit(lay.about) {
                    self.open_about();
                } else if self.sidebar_open && hit(lay.sidebar_divider) {
                    self.drag = Some(Drag::Sidebar);
                } else if hit(lay.input_divider) {
                    self.drag = Some(Drag::Input);
                } else if hit(lay.new_chat) {
                    self.new_session();
                } else if let Some((id, _)) = lay.session_items.iter().find(|(_, r)| hit(*r)) {
                    let id = *id;
                    self.switch_session(id);
                } else if hit(lay.main) {
                    // 点击右侧任意位置都把焦点交还输入框
                    self.focus_sidebar = false;
                }
            }
            // 拖动分隔条：实时改变宽度 / 高度
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Some(Drag::Sidebar) => {
                    if m.column < crate::ui::layout::SIDEBAR_MIN {
                        // 拖到最左边直接收起
                        self.sidebar_open = false;
                        self.drag = None;
                        self.save_ui_prefs();
                    } else {
                        self.sidebar_width = m.column;
                    }
                }
                Some(Drag::Input) => {
                    // 输入框的下边界就是状态行（lay.footer）所在行，高度要把这一行算进去
                    let h = lay.footer.y.saturating_add(1).saturating_sub(m.row);
                    self.input_height =
                        h.clamp(crate::ui::layout::INPUT_MIN, crate::ui::layout::INPUT_MAX);
                }
                None => {}
            },
            MouseEventKind::Up(MouseButton::Left) => {
                if self.drag.take().is_some() {
                    self.save_ui_prefs();
                    // 松开后按当前鼠标位置重新判定悬停
                    self.update_hover(m.column, m.row);
                }
            }
            MouseEventKind::ScrollUp if hit(lay.chat) => self.scroll_up(3),
            MouseEventKind::ScrollDown if hit(lay.chat) => self.scroll_down(3),
            _ => {}
        }
    }

    /// 根据鼠标位置更新「可拖动」悬停状态
    fn update_hover(&mut self, column: u16, row: u16) {
        self.hover_pos = Some((column, row));
        // 拖动过程中保持指针形状
        if let Some(kind) = self.drag {
            self.hover_divider = Some(kind);
            return;
        }
        if self.size.0 == 0 || self.size.1 == 0 {
            return;
        }
        let lay = crate::ui::layout::compute(Rect::new(0, 0, self.size.0, self.size.1), self);
        let pos = Position { x: column, y: row };
        self.hover_divider = if self.sidebar_open
            && lay.sidebar_divider.width > 0
            && lay.sidebar_divider.contains(pos)
        {
            Some(Drag::Sidebar)
        } else if lay.input_divider.contains(pos) {
            Some(Drag::Input)
        } else {
            None
        };
    }

    /// 悬停在分隔条上时改变终端指针形状
    fn sync_pointer(&mut self) {
        let want = match self.hover_divider {
            Some(Drag::Sidebar) => Pointer::EwResize,
            Some(Drag::Input) => Pointer::NsResize,
            None => Pointer::Default,
        };
        if want == self.pointer {
            return;
        }
        self.pointer = want;
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = out.write_all(want.sequence().as_bytes());
        let _ = out.flush();
    }

    /// Ctrl+C：复制输入框选区（或设置项里的整个值）；返回是否复制了内容
    fn copy_selection(&mut self) -> bool {
        if self.overlay == Overlay::Settings && self.form.editing {
            let text = self.form.buffer.clone();
            if text.is_empty() {
                return false;
            }
            osc52_copy(&text);
            self.toast_ok(format!("已复制 {} 个字符到剪贴板", text.chars().count()));
            return true;
        }
        if self.overlay != Overlay::None || self.approval.is_some() {
            return false;
        }
        let Some(text) = self.input.selection_text() else {
            return false;
        };
        if text.is_empty() {
            return false;
        }
        osc52_copy(&text);
        self.toast_ok(format!("已复制 {} 个字符到剪贴板", text.chars().count()));
        true
    }

    /// 粘贴：按当前焦点插入，多行内容不会被误当成多次回车
    fn on_paste(&mut self, text: String) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if text.is_empty() {
            return;
        }
        // 弹窗/审批期间不接受粘贴
        if self.approval.is_some() || self.overlay == Overlay::About {
            return;
        }
        if self.overlay == Overlay::Settings {
            if self.form.editing {
                // 配置项都是单行值，去掉换行
                let cleaned: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
                let cleaned = crate::ui::util::sanitize(cleaned.trim());
                if self.form.sel_all || (!self.form.edited && !self.form.buffer.is_empty()) {
                    // 全选 or 刚进入编辑就粘贴 → 替换原值
                    self.form.buffer = cleaned;
                } else {
                    self.form.buffer.push_str(&cleaned);
                }
                self.form.sel_all = false;
                self.form.edited = true;
                self.form_dirty = true;
            }
            return;
        }
        self.focus_sidebar = false;
        self.input.insert_str(&text);
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
            "/help" | "/?" | "/about" => {
                self.open_about();
                return;
            }
            "/settings" | "/config" => {
                self.open_settings();
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
                self.new_session();
                return;
            }
            _ => {}
        }

        if !self.cfg.has_key() {
            self.toast_err("还没有配置 API Token，请先点「设置」填写");
            self.open_settings();
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

    pub fn open_settings(&mut self) {
        // 表单里有未保存的修改时不要覆盖，否则用户刚输入的 Token 会被冲掉
        if !self.form_dirty {
            self.form = ConfigForm::from(&self.cfg);
        }
        self.overlay = Overlay::Settings;
    }

    pub fn open_about(&mut self) {
        self.overlay = Overlay::About;
        self.about_scroll = 0;
    }

    /// 收起 / 展开左侧会话栏
    pub fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
        if !self.sidebar_open {
            self.focus_sidebar = false;
        }
        self.save_ui_prefs();
    }

    /// 调整输入框高度（含边框），+1 变高
    pub fn adjust_input_height(&mut self, delta: i16) {
        let h = self.input_height as i16 + delta;
        self.input_height = h.clamp(
            crate::ui::layout::INPUT_MIN as i16,
            crate::ui::layout::INPUT_MAX as i16,
        ) as u16;
        self.save_ui_prefs();
    }

    /// 记住界面偏好（侧边栏宽度 / 是否展开 / 输入框高度）
    fn save_ui_prefs(&mut self) {
        let pairs = vec![
            ("ui_sidebar_width".to_string(), self.sidebar_width.to_string()),
            ("ui_sidebar_open".to_string(), self.sidebar_open.to_string()),
            ("ui_input_height".to_string(), self.input_height.to_string()),
        ];
        let _ = self.rt.block_on(self.db.save_settings(&pairs));
    }

    /// 重新从数据库读取会话列表
    pub fn refresh_sessions(&mut self) {
        if let Ok(list) = self.rt.block_on(self.db.list_sessions(200)) {
            self.sessions = list;
        }
        let current = self
            .sessions
            .iter()
            .position(|s| s.id == self.session_id)
            .map(|i| i + 1);
        if let Some(idx) = current {
            self.sidebar_idx = idx;
        }
        self.sidebar_idx = self.sidebar_idx.min(self.sessions.len());
    }

    fn sidebar_move(&mut self, delta: isize) {
        let max = self.sessions.len() as isize;
        let cur = self.sidebar_idx.min(self.sessions.len()) as isize;
        self.sidebar_idx = (cur + delta).clamp(0, max) as usize;
    }

    fn sidebar_activate(&mut self) {
        if self.sidebar_idx == 0 {
            self.new_session();
        } else if let Some(s) = self.sessions.get(self.sidebar_idx - 1) {
            let id = s.id;
            self.switch_session(id);
        }
    }

    fn new_session(&mut self) {
        if self.busy() {
            self.toast_err("正在处理任务，稍后再新建会话");
            return;
        }
        self.focus_sidebar = false;
        self.agent.new_session();
    }

    fn switch_session(&mut self, id: i64) {
        if self.busy() {
            self.toast_err("当前任务进行中，稍后再切换会话");
            return;
        }
        match self.rt.block_on(self.db.load_messages(id)) {
            Ok(stored) => {
                self.messages = display_from_stored(&stored);
                self.session_id = id;
                self.scroll = 0;
                self.follow = true;
                self.status = Status::Idle;
                self.focus_sidebar = false;
                self.agent.switch_session(id);
                self.refresh_sessions();
                self.toast_ok(format!("已切换到会话 #{id}"));
            }
            Err(e) => self.toast_err(format!("加载会话失败：{e:#}")),
        }
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
        let key = self.form.at(F_API_KEY).value.trim().to_string();
        let base = self.form.at(F_BASE_URL).value.trim().to_string();
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

    /// 悬停在分隔条上的操作提示
    pub fn hover_hint(&self) -> Option<&'static str> {
        match self.hover_divider {
            Some(Drag::Sidebar) => Some("⇔ 按住拖动调整左栏宽度（拖到最左收起）"),
            Some(Drag::Input) => Some("⇕ 按住拖动调整输入框高度"),
            None => None,
        }
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

/// 通过 OSC 52 把文本写入系统剪贴板（终端支持时生效）
fn osc52_copy(text: &str) {
    use std::io::Write;
    let payload = base64_encode(text.as_bytes());
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{payload}\x07");
    let _ = out.flush();
}

/// 极简 base64 编码（避免额外依赖）
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
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

    pub(super) fn app() -> App {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let db = rt.block_on(Db::open_memory()).unwrap();
        let mut cfg = Config::default();
        cfg.api_key = "sk-test-0000".into();
        let agent = AgentHandle::spawn(&rt, cfg.clone(), db.clone(), 1, Vec::new());
        let handle = rt.handle().clone();
        std::mem::forget(rt);
        App::new(cfg, agent, db, handle, 1, Vec::new())
    }

    /// 在设置里输入 Token 并回车，必须立刻落到 SQLite 的 settings 表
    #[test]
    fn editing_token_saves_immediately() {
        let mut a = app();
        a.open_settings();
        assert_eq!(a.overlay, Overlay::Settings);

        a.form.idx = F_API_KEY;
        a.form.begin_edit();
        a.form.buffer = "sk-live-1234".into();
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));

        assert_eq!(a.cfg.api_key, "sk-live-1234");
        assert!(!a.form_dirty);
        let saved = a.rt.block_on(a.db.load_settings()).unwrap();
        assert!(saved.iter().any(|(k, v)| k == "api_key" && v == "sk-live-1234"));
    }

    /// 正在编辑的设置项不能被重新打开设置时冲掉
    #[test]
    fn open_settings_keeps_dirty_form() {
        let mut a = app();
        a.open_settings();
        a.form.idx = F_API_KEY;
        a.form.begin_edit();
        a.form.buffer = "sk-partial".into();
        a.form.edited = true;
        a.form_dirty = true;

        a.overlay = Overlay::None;
        a.open_settings();

        assert!(a.form.editing);
        assert_eq!(a.form.buffer, "sk-partial");
    }

    /// 重启后数据库里的历史消息要能还原成聊天界面
    #[test]
    fn messages_are_restored_into_display() {
        let stored = vec![
            StoredMessage {
                role: "user".into(),
                content: "根分区满了怎么办".into(),
                reasoning: String::new(),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
                ok: true,
            },
            StoredMessage {
                role: "assistant".into(),
                content: "先看下占用".into(),
                reasoning: "思考".into(),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
                ok: true,
            },
            StoredMessage {
                role: "tool".into(),
                content: "status: ok\n/dev/sda1 100%".into(),
                reasoning: String::new(),
                tool_calls: None,
                tool_call_id: Some("call_1".into()),
                tool_name: Some("shell".into()),
                ok: true,
            },
        ];
        let shown = display_from_stored(&stored);
        assert_eq!(shown.len(), 3);
        assert!(matches!(&shown[0], Display::User(t) if t.contains("根分区")));
        assert!(matches!(&shown[1], Display::Assistant { text, streaming, .. }
            if text == "先看下占用" && !streaming));
        assert!(matches!(&shown[2], Display::ToolResult { name, output, ok }
            if name == "shell" && *ok && output.contains("/dev/sda1")));
    }
}

#[cfg(test)]
mod layout_tests {
    use super::tests::app;
    use super::*;

    fn click(a: &mut App, x: u16, y: u16) {
        a.size = (120, 40);
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::empty(),
        });
    }

    /// 点菜单栏的「设置」「关于」应打开对应弹窗
    #[test]
    fn menu_clicks_open_overlays() {
        let mut a = app();
        a.size = (120, 40);
        let lay = crate::ui::layout::compute(Rect::new(0, 0, 120, 40), &mut a);

        click(&mut a, lay.settings.x, lay.settings.y);
        assert_eq!(a.overlay, Overlay::Settings);

        a.overlay = Overlay::None;
        click(&mut a, lay.about.x, lay.about.y);
        assert_eq!(a.overlay, Overlay::About);

        a.overlay = Overlay::None;
        click(&mut a, lay.toggle_sidebar.x, lay.toggle_sidebar.y);
        assert!(!a.sidebar_open);
    }

    /// 点左侧会话条目应切换到那个会话
    #[test]
    fn sidebar_click_switches_session() {
        let mut a = app();
        a.rt.block_on(a.db.create_session("磁盘告警排查", "deepseek-chat")).unwrap();
        let other = a.rt.block_on(a.db.create_session("nginx 502", "deepseek-chat")).unwrap();
        a.refresh_sessions();
        assert!(a.sessions.len() >= 2);

        a.size = (120, 40);
        let lay = crate::ui::layout::compute(Rect::new(0, 0, 120, 40), &mut a);
        let (id, rect) = lay
            .session_items
            .iter()
            .find(|(id, _)| *id == other)
            .copied()
            .expect("应能定位到会话条目");
        click(&mut a, rect.x + 1, rect.y);
        assert_eq!(a.session_id, id);
    }
}

#[cfg(test)]
mod resize_tests {
    use super::tests::app;
    use super::*;

    fn ev(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::empty() }
    }

    /// 侧边栏宽度可拖动，拖到最左边收起，且宽度会被记住
    #[test]
    fn sidebar_can_be_collapsed_and_dragged() {
        let mut a = app();
        a.size = (120, 40);
        let lay = crate::ui::layout::compute(Rect::new(0, 0, 120, 40), &mut a);
        let d = lay.sidebar_divider;

        a.on_mouse(ev(MouseEventKind::Down(MouseButton::Left), d.x, d.y));
        assert_eq!(a.drag, Some(Drag::Sidebar));
        a.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 40, d.y));
        assert_eq!(a.sidebar_width, 40);
        a.on_mouse(ev(MouseEventKind::Up(MouseButton::Left), 40, d.y));
        assert!(a.drag.is_none());

        // 拖到最左侧 → 收起
        let lay = crate::ui::layout::compute(Rect::new(0, 0, 120, 40), &mut a);
        let d = lay.sidebar_divider;
        a.on_mouse(ev(MouseEventKind::Down(MouseButton::Left), d.x, d.y));
        a.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 2, d.y));
        assert!(!a.sidebar_open);
    }

    /// 输入框高度可上下拖动，也能用 Alt+↑↓ 调整，并夹在允许范围内
    #[test]
    fn input_height_can_be_dragged_and_keyed() {
        let mut a = app();
        a.size = (120, 40);
        let lay = crate::ui::layout::compute(Rect::new(0, 0, 120, 40), &mut a);
        let d = lay.input_divider;
        let bottom = lay.footer.y;

        a.on_mouse(ev(MouseEventKind::Down(MouseButton::Left), d.x + 2, d.y));
        assert_eq!(a.drag, Some(Drag::Input));
        // 拖到「底边框算起第 8 行」高度
        let target_row = bottom.saturating_add(1).saturating_sub(8);
        a.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), d.x + 2, target_row));
        assert_eq!(a.input_height, 8);
        a.on_mouse(ev(MouseEventKind::Up(MouseButton::Left), d.x + 2, target_row));

        a.adjust_input_height(100);
        assert_eq!(a.input_height, crate::ui::layout::INPUT_MAX);
        a.adjust_input_height(-100);
        assert_eq!(a.input_height, crate::ui::layout::INPUT_MIN);
    }

    /// 界面偏好在松手后应写入 settings 表
    #[test]
    fn layout_prefs_persist() {
        let mut a = app();
        a.sidebar_width = 33;
        a.input_height = 7;
        a.save_ui_prefs();

        let saved = a.rt.block_on(a.db.load_settings()).unwrap();
        let get = |key: &str| {
            saved
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(get("ui_sidebar_width"), "33");
        assert_eq!(get("ui_input_height"), "7");
        assert_eq!(get("ui_sidebar_open"), "true");
    }

    /// 鼠标移到分隔条上要有悬停状态（用于换指针形状、显示提示）
    #[test]
    fn hovering_dividers_sets_resize_state() {
        let mut a = app();
        a.size = (120, 40);
        let lay = crate::ui::layout::compute(Rect::new(0, 0, 120, 40), &mut a);

        let d = lay.sidebar_divider;
        a.on_mouse(ev(MouseEventKind::Moved, d.x, d.y));
        assert_eq!(a.hover_divider, Some(Drag::Sidebar));
        assert!(a.hover_hint().is_some());

        let d = lay.input_divider;
        a.on_mouse(ev(MouseEventKind::Moved, d.x + 2, d.y));
        assert_eq!(a.hover_divider, Some(Drag::Input));

        // 移到聊天区 → 取消悬停
        a.on_mouse(ev(MouseEventKind::Moved, lay.chat.x + 2, lay.chat.y + 2));
        assert_eq!(a.hover_divider, None);
        assert!(a.hover_hint().is_none());
    }

    /// 粘贴多行文本到输入框：换行保留，不会被当成多次回车提交
    #[test]
    fn paste_inserts_into_chat_input() {
        let mut a = app();
        a.on_paste("kubectl get pods\r\nkubectl logs -f web\r\n".into());
        assert_eq!(a.input.value.trim_end(), "kubectl get pods\nkubectl logs -f web");
        assert!(
            !a.messages.iter().any(|m| matches!(m, Display::User(_))),
            "粘贴不应触发提交"
        );
    }

    /// 设置里刚进入编辑就粘贴 → 整体替换原值
    #[test]
    fn paste_into_settings_field() {
        let mut a = app();
        a.open_settings();
        a.form.idx = F_API_KEY;
        a.form.begin_edit();
        a.on_paste("sk-pasted-9999\n".into());
        assert_eq!(a.form.buffer, "sk-pasted-9999");
        assert!(a.form_dirty);
    }

    /// 关于弹窗打开时不应接收粘贴
    #[test]
    fn paste_ignored_when_about_open() {
        let mut a = app();
        a.open_about();
        a.on_paste("rm -rf /".into());
        assert!(a.input.is_empty());
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    #[test]
    fn base64_encoding() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"abc def"), "YWJjIGRlZg==");
        assert_eq!(base64_encode("磁盘".as_bytes()), "56OB55uY");
    }
}
