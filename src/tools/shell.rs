//! 本地 shell 命令执行（带高危命令硬拦截）

use std::time::Duration;

use serde_json::Value;

use super::{capture, ToolResult};

pub async fn run_shell(args: &Value, default_timeout: Duration) -> ToolResult {
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if command.is_empty() {
        return ToolResult::err("command 不能为空");
    }
    if let Some(reason) = catastrophic(&command) {
        return ToolResult::err(format!(
            "已被安全策略拦截：{reason}属于不可逆的破坏性操作。请你不要尝试绕过，改为向用户说明风险，\
并给出手动执行的步骤，由用户自己在终端中决定是否执行。"
        ));
    }

    let timeout = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .map(|s| Duration::from_secs(s.clamp(1, 300)))
        .unwrap_or(default_timeout);

    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-lc").arg(&command);
    if let Some(cwd) = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cmd.current_dir(cwd);
    }

    capture(cmd, timeout).await
}

/// 识别不可逆的灾难性命令；返回命中的风险描述
fn catastrophic(command: &str) -> Option<&'static str> {
    let lower = command.to_lowercase();
    // 归一化空白，避免 `rm  -rf   /` 之类的绕过
    let compact = lower.split_whitespace().collect::<Vec<_>>().join(" ");

    if compact.contains("mkfs") {
        return Some("格式化文件系统（mkfs）");
    }
    if compact.contains("dd ") && compact.contains("of=/dev/") {
        return Some("直接覆写块设备（dd of=/dev/*）");
    }
    if compact.contains("of=/dev/sd") || compact.contains("of=/dev/nvme") {
        return Some("覆写磁盘设备");
    }
    if compact.contains(":(){") || compact.contains(": () {") {
        return Some("fork 炸弹");
    }

    // 对根目录做删除/改权限/移动：只允许精确命中 `/`、`/*`、`/ --no-preserve-root`
    let args: Vec<&str> = compact.split_whitespace().collect();
    let touches_root = args.iter().any(|t| {
        matches!(*t, "/" | "/*" | "/.") || t.starts_with("--no-preserve-root")
    });
    let destructive = args
        .iter()
        .any(|t| matches!(*t, "rm" | "chmod" | "chown" | "mv" | "rmdir" | "shred"));
    if touches_root && destructive {
        return Some("对根目录执行删除 / 权限修改");
    }

    None
}

#[cfg(test)]
mod tests {
    use super::catastrophic;

    #[test]
    fn blocks_root_removal() {
        assert!(catastrophic("rm -rf /").is_some());
        assert!(catastrophic("sudo rm  -rf  /").is_some());
        assert!(catastrophic("rm -rf /*").is_some());
        assert!(catastrophic("mkfs.ext4 /dev/sdb1").is_some());
        assert!(catastrophic("dd if=/dev/zero of=/dev/sda").is_some());
    }

    #[test]
    fn allows_normal_commands() {
        assert!(catastrophic("rm -rf /tmp/build").is_none());
        assert!(catastrophic("df -h").is_none());
        assert!(catastrophic("journalctl -u nginx -n 100").is_none());
        assert!(catastrophic("chmod -R 777 /var/www").is_none());
        assert!(catastrophic("chown -R deploy:deploy /srv/app").is_none());
        assert!(catastrophic("systemctl restart nginx").is_none());
    }

    #[test]
    fn blocks_root_permission_change() {
        assert!(catastrophic("chmod -R 777 /").is_some());
        assert!(catastrophic("chown -r root:root /").is_some());
    }
}
