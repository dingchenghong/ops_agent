//! 弹窗：操作确认 / 帮助

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::Frame;

use super::theme::*;
use super::util::{clip, display_width, wrap_plain};
use crate::app::App;

fn centered_exact(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

fn section(lines: &mut Vec<Line<'static>>, title: &str) {
    lines.push(Line::from(vec![Span::styled(
        format!("▌ {title}"),
        strong(ACCENT),
    )]));
}

/// 「键 - 值」两列布局，值自动换行并与值列对齐
fn kv(lines: &mut Vec<Line<'static>>, key: &str, value: &str, key_w: usize, width: usize) {
    let key_text = clip(key, key_w.saturating_sub(2));
    let pad = key_w.saturating_sub(display_width(&key_text));
    let head = format!("  {key_text}{}", " ".repeat(pad));
    let indent = " ".repeat(display_width(&head));
    let avail = width.saturating_sub(display_width(&head)).max(12);
    for (i, l) in wrap_plain(value, avail).iter().enumerate() {
        if i == 0 {
            lines.push(Line::from(vec![
                Span::styled(head.clone(), Style::new().fg(TOOL)),
                Span::styled(l.clone(), Style::new().fg(TEXT)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::styled(l.clone(), Style::new().fg(TEXT)),
            ]));
        }
    }
}

fn wrapped_count(text: &str, width: usize) -> usize {
    text.lines()
        .map(|l| wrap_plain(l, width.max(8)).len().max(1))
        .sum::<usize>()
        .max(1)
}

pub fn approval(frame: &mut Frame, app: &App) {
    let Some(appr) = app.approval.as_ref() else {
        return;
    };
    let area = frame.area();
    let w = area.width.saturating_sub(6).min(96).max(44);
    let inner_w = w.saturating_sub(4) as usize;
    let body_lines = wrapped_count(&appr.preview, inner_w).min(14) as u16;
    let h = (body_lines + 6).clamp(10, area.height.saturating_sub(2).max(10));
    let rect = centered_exact(area, w, h);

    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(WARN))
        .title(Line::from(Span::styled(
            " ⚠ 需要你确认 ",
            Style::new().fg(WARN).add_modifier(Modifier::BOLD),
        )))
        .padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let [title_area, body_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("⚙ ", Style::new().fg(TOOL)),
            Span::styled(appr.name.clone(), strong(TOOL)),
        ])),
        title_area,
    );

    // 预览区：按宽度自行换行，超出高度则截断
    let max_rows = body_area.height as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for raw in appr.preview.lines() {
        for l in wrap_plain(raw, inner_w.saturating_sub(2)) {
            lines.push(Line::from(Span::styled(l, Style::new().fg(TEXT))));
        }
    }
    if lines.len() > max_rows && max_rows > 1 {
        lines.truncate(max_rows - 1);
        lines.push(Line::from(Span::styled("…（内容过长已省略）", dim())));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::new()
                .style(Style::new().bg(CODE_BG))
                .padding(Padding::new(1, 1, 0, 0)),
        ),
        body_area,
    );

    let hints = vec![
        Line::from(vec![
            Span::styled("Y", strong(OK)),
            Span::styled(" / ", dim()),
            Span::styled("Enter", strong(OK)),
            Span::styled(" 允许执行        ", dim()),
            Span::styled("N", strong(ERR)),
            Span::styled(" / ", dim()),
            Span::styled("Esc", strong(ERR)),
            Span::styled(" 拒绝", dim()),
        ]),
        Line::from(Span::styled(
            "拒绝不会中断对话，Agent 会改为给你可直接执行的命令",
            dim(),
        )),
    ];
    frame.render_widget(Paragraph::new(hints).alignment(Alignment::Center), hint_area);
}

pub fn help(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let w = area.width.saturating_sub(8).min(104).max(44);
    // 边框 2 + 左右 padding 4
    let inner_w = w.saturating_sub(6) as usize;
    let key_w = 30usize.min(inner_w.saturating_sub(20));

    let mut lines: Vec<Line<'static>> = Vec::new();

    section(&mut lines, "快捷键");
    for (k, v) in [
        ("Enter", "发送消息"),
        ("Ctrl+J / Shift+Enter", "输入换行（多行命令）"),
        ("Esc", "取消正在进行的任务 / 清空输入框"),
        ("↑ / ↓", "浏览历史输入"),
        ("PgUp / PgDn", "滚动对话（Ctrl+Home 回到顶部，Ctrl+End 回到底部）"),
        ("Ctrl+L", "清空当前会话与 Agent 上下文"),
        ("F1 / Ctrl+H", "打开本帮助"),
        ("F2 / Ctrl+K", "打开配置（API Token、模型、k8s 上下文）"),
        ("Ctrl+C", "退出程序"),
    ] {
        kv(&mut lines, k, v, key_w, inner_w);
    }

    lines.push(Line::from(""));
    section(&mut lines, "斜杠命令");
    for (k, v) in [
        ("/new", "新建会话（旧会话仍保存在数据库里）"),
        ("/sessions", "查看最近的会话列表"),
        ("/clear", "清空当前会话的消息与上下文"),
        ("/config", "打开配置界面"),
        ("/quit", "退出程序"),
    ] {
        kv(&mut lines, k, v, key_w, inner_w);
    }

    lines.push(Line::from(""));
    section(&mut lines, "能做什么");
    for (k, v) in [
        ("run_shell", "执行任意 shell 命令：巡检、排障、systemctl、journalctl、docker…"),
        ("system_info", "系统总览 / 磁盘 / 内存 / CPU / 进程 / 网络 / 服务 / 日志"),
        ("service_manage", "systemd 服务查询与启停（启停需确认）"),
        ("kubectl", "集群查询、describe、logs、apply、scale、rollout（写操作需确认）"),
        ("read_file / write_file", "查看配置、落盘 YAML（写入需确认）"),
        ("list_dir", "列出目录内容"),
    ] {
        kv(&mut lines, k, v, key_w, inner_w);
    }

    lines.push(Line::from(""));
    section(&mut lines, "安全机制");
    for v in [
        "所有会改变系统的工具调用都会弹窗确认，按 y 才会执行",
        "mkfs、dd of=/dev/*、rm -rf / 等不可逆命令被硬性拦截",
        "配置与会话保存在 ~/.local/share/ops-agent/ops-agent.db（SQLite，权限 600）",
        "对话内容会发送到 DeepSeek 接口，请勿在其中粘贴敏感凭据",
    ] {
        for (i, l) in wrap_plain(v, inner_w.saturating_sub(6))
            .iter()
            .enumerate()
        {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled("  • ", Style::new().fg(ACCENT)),
                    Span::styled(l.clone(), dim()),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("    ", base()),
                    Span::styled(l.clone(), dim()),
                ]));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  当前模型：", dim()),
        Span::styled(app.cfg.model.clone(), Style::new().fg(ASSISTANT)),
        Span::styled("　跳过确认：", dim()),
        Span::styled(
            if app.cfg.auto_approve_all { "是（危险）" } else { "否" },
            Style::new().fg(if app.cfg.auto_approve_all { ERR } else { OK }),
        ),
    ]));

    // 高度自适应内容（边框 2 行）
    let need = lines.len() as u16 + 3;
    let h = need.clamp(10, area.height.saturating_sub(2).max(10));
    let rect = centered_exact(area, w, h);

    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(Span::styled(" 帮助 / 快捷键 ", strong(ACCENT))))
        .title_bottom(Line::from(Span::styled(" Esc 关闭 ", dim())))
        .padding(Padding::new(2, 2, 0, 0));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let view = inner.height as usize;
    let total = lines.len();
    let max_scroll = total.saturating_sub(view);
    let scroll = app.help_scroll.min(max_scroll);
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(view).collect();
    frame.render_widget(Paragraph::new(shown), inner);

    if total > view {
        let hint = format!(" ↑↓ / PgUp PgDn 滚动（{}/{}）· Esc 关闭 ", scroll + 1, max_scroll + 1);
        let area2 = Rect {
            y: rect.y + rect.height.saturating_sub(1),
            height: 1,
            ..rect
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hint, dim())).alignment(Alignment::Right),
            area2,
        );
    }
}
