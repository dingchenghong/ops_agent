//! 系统巡检与 systemd 服务管理

use std::time::Duration;

use serde_json::Value;

use super::{capture, ToolResult};

async fn sh(cmdline: &str, timeout: Duration) -> ToolResult {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-lc").arg(cmdline);
    capture(cmd, timeout).await
}

pub async fn system_info(args: &Value, timeout: Duration) -> ToolResult {
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("overview")
        .to_lowercase();

    let cmds: Vec<(&str, &str)> = match kind.as_str() {
        "overview" => vec![
            ("系统", "uname -a"),
            ("运行时间/负载", "uptime"),
            ("内存", "free -h"),
            ("磁盘", "df -hT -x tmpfs -x devtmpfs 2>/dev/null | head -20"),
            ("CPU 核心数", "nproc"),
        ],
        "disk" => vec![
            ("磁盘使用", "df -hT -x tmpfs -x devtmpfs 2>/dev/null"),
            ("块设备", "lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT 2>/dev/null"),
            ("inode", "df -i -x tmpfs -x devtmpfs 2>/dev/null | head -20"),
        ],
        "memory" => vec![
            ("内存概览", "free -h"),
            ("内存明细", "head -20 /proc/meminfo"),
            ("换页情况", "vmstat 1 3"),
        ],
        "cpu" => vec![
            ("负载", "uptime"),
            ("CPU TOP", "ps -eo pid,comm,%cpu,%mem --sort=-%cpu 2>/dev/null | head -15"),
        ],
        "processes" => vec![
            ("内存 TOP", "ps -eo pid,ppid,user,%cpu,%mem,etime,comm --sort=-%mem 2>/dev/null | head -20"),
            ("进程数", "ps -e --no-headers 2>/dev/null | wc -l"),
        ],
        "network" => vec![
            ("网卡", "ip -br a 2>/dev/null || ifconfig -a"),
            ("路由", "ip route 2>/dev/null"),
            ("监听端口", "ss -tunlp 2>/dev/null | head -40 || netstat -tunlp 2>/dev/null | head -40"),
        ],
        "services" => vec![(
            "运行中的服务",
            "systemctl list-units --type=service --state=running --no-pager --no-legend 2>/dev/null | head -40",
        )],
        "failed" => vec![(
            "启动失败的服务",
            "systemctl --failed --no-pager 2>/dev/null; echo; journalctl -p err -n 40 --no-pager 2>/dev/null",
        )],
        "logs" => vec![
            ("最近告警日志（warning 及以上）", "journalctl -p warning -n 60 --no-pager 2>/dev/null"),
            ("OOM / 内核错误", "dmesg -T 2>/dev/null | grep -Ei 'oom|error|fail' | tail -20"),
        ],
        other => {
            return ToolResult::err(format!(
                "不支持的 kind：{other}，可选值：overview/disk/memory/cpu/processes/network/services/failed/logs"
            ))
        }
    };

    let mut out = String::new();
    for (label, cmdline) in cmds {
        let r = sh(cmdline, timeout).await;
        out.push_str(&format!("## {label}\n{}\n\n", r.output.trim_end()));
    }
    ToolResult::ok(out.trim_end().to_string())
}

fn valid_unit(unit: &str) -> bool {
    !unit.is_empty()
        && unit.len() <= 200
        && unit
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':'))
}

pub async fn service_manage(args: &Value, timeout: Duration) -> ToolResult {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status")
        .to_lowercase();

    if action == "list" {
        return sh(
            "systemctl list-units --type=service --all --no-pager 2>/dev/null | head -60",
            timeout,
        )
        .await;
    }

    let unit = args
        .get("unit")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if !valid_unit(&unit) {
        return ToolResult::err(format!(
            "服务名不合法：`{unit}`。只允许字母、数字与 - _ . @ : 字符。"
        ));
    }

    let cmdline = match action.as_str() {
        "status" => format!("systemctl status {unit} --no-pager -l 2>&1 | head -40"),
        "start" | "stop" | "restart" | "reload" | "enable" | "disable" => {
            // 执行后顺带验证状态，方便 Agent 判断结果
            format!("systemctl {action} {unit} 2>&1; echo '--- 当前状态 ---'; systemctl is-active {unit} 2>&1")
        }
        other => {
            return ToolResult::err(format!(
                "不支持的 action：{other}，可选值：status/list/start/stop/restart/reload/enable/disable"
            ))
        }
    };

    let mut result = sh(&cmdline, timeout).await;
    result.output = format!("命令：{cmdline}\n{}", result.output);
    if action == "status" {
        // systemctl status 对未运行的服务返回非 0，但输出是有价值的诊断信息
        result.ok = true;
    }
    result
}
