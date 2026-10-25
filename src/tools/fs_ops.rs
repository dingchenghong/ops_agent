//! 文件系统工具

use std::fs;
use std::path::Path;

use serde_json::Value;

use super::ToolResult;

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub fn read_file(args: &Value) -> ToolResult {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
    if path.is_empty() {
        return ToolResult::err("path 不能为空");
    }
    let p = Path::new(path);
    let meta = match fs::metadata(p) {
        Ok(m) => m,
        Err(e) => return ToolResult::err(format!("无法访问 {path}：{e}")),
    };
    if meta.is_dir() {
        return ToolResult::err(format!("{path} 是目录，请使用 list_dir"));
    }
    if meta.len() > MAX_FILE_BYTES {
        return ToolResult::err(format!(
            "{path} 大小为 {:.1} MB，超出单次读取上限，请用 run_shell 配合 head/tail/sed 分段查看",
            meta.len() as f64 / 1048576.0
        ));
    }

    let bytes = match fs::read(p) {
        Ok(b) => b,
        Err(e) => return ToolResult::err(format!("读取失败：{e}")),
    };
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return ToolResult::err(format!("{path} 看起来是二进制文件，已拒绝读取"));
    }
    let text = String::from_utf8_lossy(&bytes);

    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(400)
        .clamp(1, 2000) as usize;

    let total_lines = text.lines().count();
    let mut out = format!("文件：{path}（{total_lines} 行，{} 字节）", bytes.len());
    for (i, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
        out.push_str(&format!("\n{:>6}\t{}", i + 1, line));
    }
    let end = (offset - 1 + limit).min(total_lines);
    if end < total_lines {
        out.push_str(&format!(
            "\n...（已显示第 {offset}-{end} 行，共 {total_lines} 行，可用 offset={} 继续读取）",
            end + 1
        ));
    }
    ToolResult::ok(out)
}

pub fn write_file(args: &Value) -> ToolResult {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    if path.is_empty() {
        return ToolResult::err("path 不能为空");
    }
    let p = Path::new(path);
    let existed = p.exists();
    let old_len = fs::metadata(p).map(|m| m.len()).unwrap_or(0);

    if let Some(dir) = p.parent() {
        if !dir.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(dir) {
                return ToolResult::err(format!("创建目录失败：{e}"));
            }
        }
    }

    // 先写临时文件再 rename，避免写一半导致配置文件损坏
    let tmp = p.with_extension(format!(
        "{}.tmp",
        p.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    if let Err(e) = fs::write(&tmp, content) {
        return ToolResult::err(format!("写入失败：{e}"));
    }
    if let Err(e) = fs::rename(&tmp, p) {
        let _ = fs::remove_file(&tmp);
        return ToolResult::err(format!("落盘失败：{e}"));
    }

    let action = if existed { "已覆盖" } else { "已创建" };
    let mut msg = format!(
        "{action} {path}（{} 行，{} 字节）",
        content.lines().count(),
        content.len()
    );
    if existed {
        msg.push_str(&format!("，原文件 {old_len} 字节"));
    }
    ToolResult::ok(msg)
}

pub fn list_dir(args: &Value) -> ToolResult {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(".");
    let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(true);

    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => return ToolResult::err(format!("无法读取目录 {path}：{e}")),
    };

    let mut rows: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !all && name.starts_with('.') {
            continue;
        }
        let meta = entry.metadata().ok();
        let (kind, size) = match meta {
            Some(m) if m.is_dir() => ("d", "-".to_string()),
            Some(m) => (
                "-",
                if m.len() >= 1024 {
                    format!("{:.1}K", m.len() as f64 / 1024.0)
                } else {
                    format!("{}B", m.len())
                },
            ),
            None => ("?", "?".to_string()),
        };
        rows.push((format!("{kind} {:>8}  {}", size, name), name));
    }
    rows.sort_by(|a, b| a.1.cmp(&b.1));

    let total = rows.len();
    let mut out = format!("目录：{path}（{total} 项）");
    for (line, _) in rows.iter().take(400) {
        out.push_str("\n");
        out.push_str(line);
    }
    if total > 400 {
        out.push_str(&format!("\n...（还有 {} 项未显示）", total - 400));
    }
    ToolResult::ok(out)
}
