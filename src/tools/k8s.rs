//! Kubernetes 工具：kubectl 封装
//!
//! 只读命令（get/describe/logs/top/...）自动执行；写操作（apply/delete/scale/...）
//! 由上层要求用户确认后再执行。

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::{capture, ToolResult};
use crate::config::Config;

/// 只读（安全）动词
const READ_VERBS: &[&str] = &[
    "get", "describe", "logs", "log", "top", "version", "api-resources", "api-versions",
    "explain", "cluster-info", "events", "diff", "wait", "completion", "config", "auth",
];

/// 会改变集群状态的动词
const WRITE_VERBS: &[&str] = &[
    "apply", "create", "delete", "patch", "edit", "scale", "rollout", "set", "label",
    "annotate", "cordon", "uncordon", "drain", "taint", "exec", "cp", "replace", "expose",
    "autoscale", "run", "certificate", "debug", "attach", "port-forward", "proxy", "cp",
];

pub fn available() -> bool {
    std::env::var("PATH")
        .map(|p| {
            p.split(':')
                .any(|d| !d.is_empty() && Path::new(d).join("kubectl").exists())
        })
        .unwrap_or(false)
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// 找到命令里真正的 kubectl 动词及其后的参数
fn split_verb(args: &str) -> Option<(String, Vec<String>)> {
    let tokens: Vec<String> = args.split_whitespace().map(|t| t.to_string()).collect();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].to_lowercase();
        if !t.starts_with('-') && (READ_VERBS.contains(&t.as_str()) || WRITE_VERBS.contains(&t.as_str())) {
            return Some((t, tokens[i + 1..].to_vec()));
        }
        i += 1;
    }
    None
}

pub fn is_read_only(args: &str) -> bool {
    let Some((verb, rest)) = split_verb(args) else {
        return false;
    };
    match verb.as_str() {
        "config" => matches!(
            rest.first().map(|s| s.to_lowercase()).as_deref(),
            Some("view") | Some("get-contexts") | Some("current-context") | Some("get-clusters")
        ),
        "auth" => matches!(
            rest.first().map(|s| s.to_lowercase()).as_deref(),
            Some("can-i") | Some("whoami")
        ),
        v => READ_VERBS.contains(&v),
    }
}

/// 集群级资源或已显式指定 namespace 时，不再自动加 -n
fn skip_namespace(args: &str) -> bool {
    let tokens: Vec<String> = args.split_whitespace().map(|t| t.to_lowercase()).collect();

    if tokens.iter().any(|t| {
        t == "-n"
            || t == "--namespace"
            || t.starts_with("--namespace=")
            || t == "-a"
            || t == "--all-namespaces"
    }) {
        return true;
    }

    // 集群级资源
    const CLUSTER_SCOPED: &[&str] = &[
        "node", "nodes", "no", "namespace", "namespaces", "ns", "pv", "pvs",
        "persistentvolume", "persistentvolumes", "storageclass", "storageclasses", "sc",
        "clusterrole", "clusterroles", "clusterrolebinding", "clusterrolebindings", "csr",
        "componentstatus", "apigroup", "apigroups", "crd", "customresourcedefinition",
        "customresourcedefinitions", "psp", "ingressclass", "priorityclass", "runtimeclass",
    ];
    if tokens.iter().any(|t| CLUSTER_SCOPED.contains(&t.as_str())) {
        return true;
    }

    let Some((verb, _)) = split_verb(args) else {
        return true;
    };
    matches!(
        verb.as_str(),
        "version" | "api-resources" | "api-versions" | "cluster-info" | "completion"
    )
}

pub async fn kubectl(cfg: &Config, args: &Value, timeout: Duration) -> ToolResult {
    let raw = args
        .get("args")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if raw.is_empty() {
        return ToolResult::err("args 不能为空，例如 \"get pods -A -o wide\"");
    }
    if !available() {
        return ToolResult::err(
            "当前主机未找到 kubectl。可以先用 run_shell 检查（which kubectl / ls ~/.kube/config），\
或告知用户需要安装并配置 kubeconfig。",
        );
    }

    let context = args
        .get("context")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| cfg.kubectl_context.trim().to_string());
    let namespace = args
        .get("namespace")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| cfg.kubectl_namespace.trim().to_string());

    let mut cmdline = String::from("kubectl");
    if !context.is_empty() {
        cmdline.push_str(&format!(" --context {}", shell_quote(&context)));
    }
    if !namespace.is_empty() && !skip_namespace(&raw) {
        cmdline.push_str(&format!(" -n {}", shell_quote(&namespace)));
    }
    cmdline.push(' ');
    cmdline.push_str(&raw);

    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-lc").arg(&cmdline);
    let mut result = capture(cmd, timeout).await;
    result.output = format!("命令：{cmdline}\n{}", result.output);

    if !result.ok {
        let lower = result.output.to_lowercase();
        if lower.contains("unknown flag") || lower.contains("unknown shorthand flag") {
            result.output.push_str(
                "\n提示：参数可能不被当前 kubectl 版本支持，可用 `kubectl <verb> --help` 确认。",
            );
        } else if lower.contains("connection refused") || lower.contains("no such host")
            || lower.contains("unable to connect")
        {
            result.output.push_str("\n提示：无法连上 API Server，请检查 kubeconfig 与网络连通性。");
        } else if lower.contains("forbidden") || lower.contains("unauthorized") {
            result.output.push_str("\n提示：当前 kubeconfig 身份权限不足。");
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_detection() {
        assert!(is_read_only("get pods -A"));
        assert!(is_read_only("--context prod get nodes"));
        assert!(is_read_only("logs deploy/api --tail=100"));
        assert!(is_read_only("top nodes"));
        assert!(!is_read_only("delete pod web-0"));
        assert!(!is_read_only("apply -f /tmp/x.yaml"));
        assert!(!is_read_only("rollout restart deploy/api"));
        assert!(!is_read_only("scale deploy/api --replicas=3"));
    }

    #[test]
    fn namespace_injection() {
        assert!(skip_namespace("get nodes -o wide"));
        assert!(skip_namespace("get pods -A"));
        assert!(skip_namespace("get pods -n prod"));
        assert!(!skip_namespace("get pods"));
        assert!(!skip_namespace("describe pod web-0"));
    }
}
