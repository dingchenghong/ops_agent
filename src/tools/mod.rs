//! 工具集：给 Agent 提供真正能动手的能力
//!
//! - `run_shell`     执行任意 shell 命令（危险命令被硬拦截，其余需用户确认）
//! - `read_file` / `write_file` / `list_dir` 文件读写（写入需确认）
//! - `kubectl`       Kubernetes 集群操作（只读命令自动执行，写操作需确认）
//! - `service_manage` systemd 服务管理（查询自动执行，启停需确认）
//! - `system_info`   系统巡检（CPU/内存/磁盘/网络/进程/日志，自动执行）

mod fs_ops;
mod k8s;
mod shell;
mod system;

use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;

pub use fs_ops::{list_dir, read_file, write_file};
pub use k8s::kubectl;
pub use shell::run_shell;
pub use system::system_info;

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
}

impl ToolResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self { ok: true, output: output.into() }
    }

    pub fn err(output: impl Into<String>) -> Self {
        Self { ok: false, output: output.into() }
    }
}

/// 所有工具的函数定义（OpenAI function calling 格式）
pub fn definitions() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "run_shell",
                "description": "在本地 Linux 主机执行 shell 命令（bash -lc）。用于查状态、排故障、执行 systemctl/journalctl/ps/ss/df/docker 等命令。命令输出（含 stderr 与退出码）会返回给你。破坏性命令需要用户确认。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "完整 shell 命令，例如 journalctl -u nginx -n 100 --no-pager" },
                        "cwd": { "type": "string", "description": "工作目录，可省略" },
                        "timeout_secs": { "type": "integer", "description": "超时秒数，默认 60，最大 300" }
                    },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "system_info",
                "description": "采集本机系统信息。kind=overview 总览（内核/负载/内存/磁盘）、disk 磁盘、memory 内存、cpu CPU 占用、processes 进程 TOP、network 网卡与监听端口、services 运行中的服务、failed 启动失败的服务、logs 最近告警日志。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["overview", "disk", "memory", "cpu", "processes", "network", "services", "failed", "logs"]
                        }
                    },
                    "required": ["kind"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "kubectl",
                "description": "执行 kubectl 命令管理 Kubernetes 集群。会自动带上用户配置的 --context 与 --namespace。args 为 kubectl 之后的参数，例如 \"get pods -A -o wide\"、\"describe pod web-0 -n prod\"、\"logs deploy/api --tail=200\"、\"top nodes\"。apply 前请先用 write_file 写好 YAML 再 apply -f。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "args": { "type": "string", "description": "kubectl 子命令与参数" },
                        "namespace": { "type": "string", "description": "覆盖默认 namespace，可选" },
                        "context": { "type": "string", "description": "覆盖默认 context，可选" }
                    },
                    "required": ["args"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "service_manage",
                "description": "管理 systemd 服务。action=status 查看状态（自动执行），list 列出服务，其余（start/stop/restart/reload/enable/disable）需要用户确认。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["status", "list", "start", "stop", "restart", "reload", "enable", "disable"] },
                        "unit": { "type": "string", "description": "服务名，如 nginx.service" }
                    },
                    "required": ["action"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "读取文本文件内容（带行号）。用于查看配置、日志片段。大文件请用 offset/limit 分段读取。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件绝对路径" },
                        "offset": { "type": "integer", "description": "起始行号，从 1 开始" },
                        "limit": { "type": "integer", "description": "最多读取行数，默认 400，最大 2000" }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "写入文本文件（覆盖或新建，自动创建父目录）。写入前请先 read_file 确认原内容；修改系统配置前建议先备份。需要用户确认。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "文件绝对路径" },
                        "content": { "type": "string", "description": "完整文件内容" }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "列出目录内容（名称、类型、大小）。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "目录路径，默认当前目录" },
                        "all": { "type": "boolean", "description": "是否包含隐藏文件，默认 true" }
                    },
                    "required": []
                }
            }
        }),
    ]
}

/// 该工具调用是否需要用户确认
pub fn requires_approval(cfg: &Config, name: &str, args: &Value) -> bool {
    if cfg.auto_approve_all {
        return false;
    }
    match name {
        "run_shell" | "write_file" => true,
        "kubectl" => {
            let a = args.get("args").and_then(|v| v.as_str()).unwrap_or("");
            !k8s::is_read_only(a)
        }
        "service_manage" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
            !matches!(action, "status" | "list")
        }
        _ => false,
    }
}

/// 审批弹窗里展示的操作摘要
pub fn preview(name: &str, args: &Value) -> String {
    let s = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match name {
        "run_shell" => {
            let mut out = format!("$ {}", s("command"));
            let cwd = s("cwd");
            if !cwd.is_empty() {
                out.push_str(&format!("\n工作目录：{cwd}"));
            }
            out
        }
        "write_file" => {
            let content = s("content");
            let lines: Vec<&str> = content.lines().take(12).collect();
            format!(
                "写入文件：{}\n（共 {} 行）\n{}",
                s("path"),
                content.lines().count(),
                lines.join("\n")
            )
        }
        "kubectl" => {
            let mut cmd = String::from("kubectl");
            let ctx = s("context");
            if !ctx.is_empty() {
                cmd.push_str(&format!(" --context {ctx}"));
            }
            let ns = s("namespace");
            if !ns.is_empty() {
                cmd.push_str(&format!(" -n {ns}"));
            }
            cmd.push(' ');
            cmd.push_str(&s("args"));
            cmd
        }
        "service_manage" => format!("systemctl {} {}", s("action"), s("unit")),
        _ => serde_json::to_string_pretty(args).unwrap_or_default(),
    }
}

pub async fn execute(cfg: &Config, name: &str, args: &Value) -> ToolResult {
    let timeout = Duration::from_secs(cfg.shell_timeout_secs.clamp(5, 300));
    match name {
        "run_shell" => run_shell(args, timeout).await,
        "system_info" => system_info(args, timeout).await,
        "kubectl" => kubectl(cfg, args, timeout).await,
        "service_manage" => system::service_manage(args, timeout).await,
        "read_file" => read_file(args),
        "write_file" => write_file(args),
        "list_dir" => list_dir(args),
        other => ToolResult::err(format!("未知工具：{other}")),
    }
}

/// 执行命令并收集 stdout/stderr/退出码
pub(crate) async fn capture(mut cmd: tokio::process::Command, timeout: Duration) -> ToolResult {
    use std::process::Stdio;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PAGER", "cat")
        .env("SYSTEMD_PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("TERM", "dumb")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("OPS_AGENT", "1")
        .kill_on_drop(true);

    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let code = out
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            let mut text = format!("exit_code: {code}");
            if !stdout.trim().is_empty() {
                text.push_str("\n--- stdout ---\n");
                text.push_str(stdout.trim_end());
            }
            if !stderr.trim().is_empty() {
                text.push_str("\n--- stderr ---\n");
                text.push_str(stderr.trim_end());
            }
            if stdout.trim().is_empty() && stderr.trim().is_empty() {
                text.push_str("\n（命令无输出）");
            }
            ToolResult { ok: out.status.success(), output: truncate_middle(&text, 14000) }
        }
        Ok(Err(e)) => ToolResult::err(format!("无法启动命令：{e}")),
        Err(_) => ToolResult::err(format!(
            "命令执行超时（{} 秒），已被终止。可以尝试缩小查询范围或加 --tail/--since 参数。",
            timeout.as_secs()
        )),
    }
}

/// 保留头尾、截断中间的输出，避免撑爆上下文
pub(crate) fn truncate_middle(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let head_len = max * 2 / 3;
    let tail_len = max - head_len;
    let head: String = s.chars().take(head_len).collect();
    let tail: String = s.chars().skip(total - tail_len).collect();
    format!(
        "{head}\n\n...（省略 {} 字符）...\n\n{tail}",
        total - head_len - tail_len
    )
}
