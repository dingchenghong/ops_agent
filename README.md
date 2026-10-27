# OpsAgent

面向 Linux 运维人员的 AI Agent，用 Rust + [ratatui](https://ratatui.rs) 写的终端界面，接入 DeepSeek，
可以真的**动手**做运维：跑命令、查服务、看日志、读写配置、管理 Kubernetes 集群。
配置与会话持久化在本地 SQLite（sqlx 0.9），重启后继续上次的对话。

```
 ◆ OpsAgent  设置  关于                            deepseek-chat · default  ● 就绪
╭ 会话 3 ────────────────╮  ❯ 你
│ ＋ 新对话               │    /var 只剩 3%，帮我定位大文件并给出清理方案
│────────────────────────│
│   #3 k8s 巡检           │  ◆ DeepSeek
│ ▌ #2 磁盘排查  ●        │    先看一下磁盘和 inode 的实际占用：
│   #1 新会话             │    ┌─ bash
│                        │    │ df -hT -x tmpfs -x devtmpfs
│                        │    └──────
│                        │    ⚙ run_shell  已完成
│                        │      ╰ 输出
│                        │        /dev/sda1 ext4 100G 95G 5G 95% /
│                        │╭ ✎ 输入 ────────────────────────────────╮
│                        ││ 描述你的运维问题…                        │
╰────────────────────────╯╰ Enter 发送 · Ctrl+J 换行 ───────────────╯
 Tab 侧边栏 · F1 关于 · F2 设置 · Ctrl+L 清空 · Ctrl+C 退出
```

## 特性

- **流式对话**：SSE 逐字输出，支持 `deepseek-reasoner` 的思维链（灰色折叠显示）
- **工具调用可视化**：每次工具调用都显示命令、状态、输出，可滚动回看
- **操作审批**：会改变系统的动作必须按键确认；拒绝后 Agent 改为给出可复制的命令
- **高危命令硬拦截**：`mkfs`、`dd of=/dev/*`、`rm -rf /`、对 `/` 改权限等直接阻断
- **Kubernetes 管理**：`kubectl` 工具自动携带配置的 `--context` / `-n`；只读命令免确认，写操作需确认
- **系统巡检**：CPU / 内存 / 磁盘 / 网络 / 进程 / 服务 / 失败单元 / 告警日志一键采集
- **配置即改即存**：在配置页改完按 `回车` 立刻写入数据库，聊天页马上可用
- **会话持久化**：每条消息（含工具调用）写入 SQLite，重启自动恢复上次会话
- **三栏布局**：顶部菜单（设置 / 关于）、左侧会话列表、右侧对话 + 输入框，支持**鼠标点击**
- **可调布局**：拖动左栏右边界改宽度（拖到最左自动收起）、拖动输入框上边框改高度；`☰` / `Ctrl+B` 收起侧边栏；偏好写入数据库，下次启动保持
- **会话管理**：`＋ 新对话` 新建，点历史会话即可切换并加载其全部消息
- **中文友好**：按显示宽度（CJK 双宽）换行与光标定位，不会错位；
  对 Emoji / 变体选择符等「终端里宽度不可靠」的字符做消毒替换，避免整行右移把边框顶掉

## 快速开始

```bash
cargo run --release
```

首次启动会进入配置界面：

1. `Enter` 编辑 `DeepSeek API Token`，粘贴 `sk-...`
2. 再按 `Enter`（或 `Esc`）——**立即保存**，底部会提示 `已保存 → ~/.local/share/ops-agent/ops-agent.db`
3. `Ctrl+T` 可以测试连通性
4. `Esc` 返回聊天，输入问题即可

也可以不写数据库，直接用环境变量：

```bash
export DEEPSEEK_API_KEY=sk-xxxx
./target/release/ops-agent
```

## 数据存储

所有状态都在一个 SQLite 文件里，默认 `~/.local/share/ops-agent/ops-agent.db`（遵循 `XDG_DATA_HOME`）：

| 表 | 用途 |
| --- | --- |
| `settings` | 配置键值对（API Token、模型、温度、kube 上下文…） |
| `sessions` | 会话（标题、模型、更新时间） |
| `messages` | 逐条消息（role、内容、思维链、tool_calls、工具输出、是否成功） |

```bash
# 看一眼当前配置
sqlite3 ~/.local/share/ops-agent/ops-agent.db "select key, value from settings;"

# 看最近的会话
sqlite3 ~/.local/share/ops-agent/ops-agent.db \
  "select id, title, model, updated_at from sessions order by updated_at desc limit 5;"
```

数据库打不开时会自动降级为内存库（界面顶部会有提示），功能可用但本次运行不落盘。

## 界面与交互

- **顶部菜单**：`设置`（配置弹窗）、`关于`（软件说明 / 快捷键 / 安全说明）
- **左侧会话栏**：最上面 `＋ 新对话`，下面是按更新时间排序的历史会话（`●` 表示当前会话）；
  点击即可切换，输入框会自动加载该会话的历史消息
- **右侧**：上方是对话内容（可滚动），下方是输入框
- **鼠标**：菜单、新对话、会话条目均可点击；对话区滚轮滚动；
  **拖动**左栏右边界改变宽度（拖到最左侧即收起），**拖动**输入框上边框改变高度
- **拖动提示**：鼠标移到分隔条上时，整条分隔线会加粗高亮（`┃` / `━`）、指针位置显示 `⇔` / `⇕` 抓手、
  底部状态行变成蓝底白字的提示条；同时把终端指针切换为调整光标并把文本光标改成实心方块
  （OSC 22，kitty / WezTerm / foot / Ghostty 等支持；不支持 OSC 22 的终端仍有三层视觉反馈）
- **布局记忆**：侧边栏宽度 / 是否展开 / 输入框高度保存在 `settings` 表（`ui_*` 键）

## 快捷键与命令

| 按键 | 作用 |
| --- | --- |
| `Enter` | 发送 / 确认编辑 |
| `Ctrl+J` / `Shift+Enter` | 输入换行（写多行命令） |
| `Tab` | 在「会话栏」与「输入框」之间切换焦点 |
| `Ctrl+B` / 点 `☰` | 收起 / 展开左侧会话栏 |
| `Alt+↑` / `Alt+↓` | 加高 / 压低输入框 |
| `↑` `↓` | 会话栏内选择会话 / 输入框内翻历史 |
| `Esc` | 取消任务、关闭弹窗、清空输入框；编辑配置时表示「确认并保存」 |
| `PgUp` `PgDn` | 滚动对话 |
| `y` / `n` | 审批弹窗中 允许 / 拒绝 |
| `F1` / `Ctrl+H` | 关于（含快捷键说明） |
| `F2` / `Ctrl+K` | 设置 |
| `Ctrl+L` | 清空当前会话 |
| `Ctrl+C` | 退出 |

### 输入框编辑

| 按键 | 作用 |
| --- | --- |
| `Ctrl+A` | 全选输入框内容（设置项里同样可用），再按 `Backspace` 就清空了 |
| `Ctrl+C` | 复制选中内容到系统剪贴板（OSC 52）；**没有选区时等同于退出程序** |
| `Ctrl+U` | 一键清空输入框 |
| `Ctrl+W` | 删除光标前的一个单词 |
| `Shift+←/→/Home/End` | 扩展选区，选中部分有高亮底色 |
| `Esc` | 先取消选区，再清空输入框 |
| `Backspace` / `Delete` | 有选区时直接删除选区 |

**粘贴**：程序启用了括号粘贴（bracketed paste），终端里 `Ctrl+Shift+V` / `Shift+Insert` / 右键粘贴的多行内容会整体插入，
不会把换行误当成回车逐行发送；粘贴到设置项时会自动去掉换行，刚进入编辑就粘贴会直接替换原值。

> 为什么 `Ctrl+V` 不行：终端把 `Ctrl+V` 用作「按字面插入下一个字符」，事件根本不会交给程序，
> 所以按 `Ctrl+V` 时程序会提示改用 `Ctrl+Shift+V` / `Shift+Insert`。
> 复制用的 OSC 52 需要终端支持（xterm / kitty / WezTerm / foot / iTerm2 / Windows Terminal 等均支持；tmux 需 `set -g set-clipboard on`）。

斜杠命令：`/new`（新建会话）、`/sessions`（会话列表）、`/clear`、`/settings`、`/help`、`/quit`。

## 内置工具

| 工具 | 说明 | 是否需要确认 |
| --- | --- | --- |
| `run_shell` | `bash -lc` 执行任意命令，返回 stdout/stderr/退出码 | 是 |
| `system_info` | 总览 / 磁盘 / 内存 / CPU / 进程 / 网络 / 服务 / 失败单元 / 日志 | 否 |
| `service_manage` | systemd 服务 status / list / start / stop / restart / reload / enable / disable | 启停类需要 |
| `kubectl` | 任意 kubectl 子命令，自动带 context/namespace | 写操作需要 |
| `read_file` | 带行号读文件，支持分段 | 否 |
| `write_file` | 原子写入（临时文件 + rename），自动建目录 | 是 |
| `list_dir` | 列目录（含大小/类型） | 否 |

## 配置项

存在 `settings` 表，也可以在配置界面里改：

| 键 | 默认值 | 说明 |
| --- | --- | --- |
| `api_key` | 空 | DeepSeek Token；为空时读 `DEEPSEEK_API_KEY` |
| `base_url` | `https://api.deepseek.com` | 任意 OpenAI 兼容网关 |
| `model` | `deepseek-chat` | `deepseek-chat` 支持工具调用 |
| `temperature` | `0.30` | 0.0 - 2.0，运维场景建议 0.1 - 0.4 |
| `max_tokens` | `4096` | 单次回复上限 |
| `max_tool_rounds` | `16` | 单轮最多工具调用轮次 |
| `auto_approve_all` | `false` | 危险：跳过所有确认 |
| `kubectl_context` | 空 | 留空用当前 context |
| `kubectl_namespace` | 空 | 留空为 default |
| `shell_timeout_secs` | `60` | 单条命令超时 |

> 旧版本用 `~/.config/ops-agent/config.toml`；首次启动且数据库为空时会自动迁移一次。

## 代码结构

```
src/
├── main.rs            tokio 运行时、数据库/配置/会话初始化、终端生命周期
├── app.rs             应用状态机：输入、滚动、审批、配置表单、按键分发
├── db.rs              SQLite 持久化（settings / sessions / messages，sqlx 0.9）
├── config.rs          配置模型 ↔ settings 表映射、旧版 TOML 迁移
├── prompt.rs          系统提示词（启动时探测主机环境注入）
├── agent/mod.rs       Agent 循环：模型 → 工具 → 模型，含审批握手、取消、写库
├── llm/mod.rs         DeepSeek 流式 SSE 客户端、tool_calls 增量拼接、连通性测试
├── tools/
│   ├── mod.rs         工具定义（function calling schema）、审批策略、命令执行
│   ├── shell.rs       shell 执行 + 高危命令硬拦截
│   ├── fs_ops.rs      文件读写/列目录
│   ├── k8s.rs         kubectl 封装、只读判定、namespace 注入
│   └── system.rs      systemd 与系统巡检
└── ui/
    ├── layout.rs      布局与鼠标命中区计算（渲染与点击共用同一份）
    ├── sidebar.rs     左侧会话列表（新对话按钮 + 历史会话）
    ├── chat.rs        顶部菜单、对话区、Markdown 渲染、输入框
    ├── config_view.rs 设置弹窗
    ├── modal.rs       关于弹窗、审批弹窗
    ├── input.rs       多行输入（字符级光标 + 历史）
    ├── util.rs        按显示宽度换行与光标定位
    └── theme.rs       配色
```

**并发模型**：Agent 跑在 tokio 多线程 runtime 上，UI 线程以 30ms 周期轮询
`crossterm` 事件与 `AgentEvent` 通道；UI → Agent 用 `UiCommand` 通道（提交、审批、取消、改配置、新建会话），
Agent → UI 用 `AgentEvent` 通道（增量文本、工具调用、结果、错误、会话切换）。
配置写入与消息写入都走同一个 `SqlitePool`（WAL 模式）。

## 测试

```bash
cargo test
```

50 项：SSE tool_calls 增量解析、本地 HTTP 回环流式、消息序列化、高危命令拦截、
kubectl 只读与 namespace 判定、SQLite 读写往返、配置回车即时落库（回归）、
配置表单不被覆盖（回归）、历史还原、菜单/侧边栏点击命中（回归）、点击会话切换并加载消息、
侧边栏收起与拖动改宽、输入框拖动/快捷键改高、布局偏好持久化、
八个界面的渲染快照（`TestBackend`，产物在 `/tmp/ops-agent-ui/`）。

### 代码约定：生产代码不用 `unwrap()`

`src/main.rs` 顶部开启了 `deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)`
（仅在非测试构建生效），所以：

- 生产代码里写 `unwrap()` / `expect()` / `panic!` 会**直接编译失败**，错误请用
  `anyhow::Result` + `?` / `with_context()` 向上传播
- 确实需要“这里不可能失败”的地方，用 `if let` / `unwrap_or` / `unwrap_or_else` /
  `get()` 这类不 panic 的写法
- 单元测试（`#[cfg(test)]`）不受限制，`unwrap()` 在测试里是惯用写法

检查方式：

```bash
cargo clippy            # 生产代码，出现 unwrap 即报错
cargo clippy --all-targets -- -W clippy::indexing_slicing   # 顺带看越界风险
```

## 安全说明

- Agent 的所有写操作都要人确认；`auto_approve_all = true` 仅建议在测试环境使用
- API Token 明文存放在本地 SQLite（数据库文件权限 600 由用户目录保护），注意备份/分享风险
- 高危命令拦截只是兜底，不能替代人工判断；生产环境请先在预发验证
- 对话内容（包含命令输出）会发送到 DeepSeek 接口，**不要把密码、私钥、Token 粘进对话**

## 后续可扩展

- `ssh_exec` / 多主机批量执行、主机清单管理
- docker / podman 独立工具、helm、ansible
- K8s 走 `kube-rs` 直连 API（不依赖 kubectl，可做 watch 与自动诊断）
- 会话管理界面（切换/搜索历史会话）、操作审计日志
- 更多模型接入（任何 OpenAI 兼容网关，改 `base_url` + `model` 即可）
