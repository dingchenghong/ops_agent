//! 右侧主区域：顶部菜单栏、对话内容、输入框、状态行

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;

use super::theme::*;
use super::util::{
    clip, cursor_in, display_width, floor_boundary, sanitize, wrap_layout, wrap_plain,
};
use crate::app::{App, Display, Overlay, Status, ToolState};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn tick(app: &App) -> usize {
    (app.started.elapsed().as_millis() / 120) as usize % SPINNER.len()
}

/// 顶部菜单栏：☰ 侧边栏开关 + 品牌 + 设置 / 关于 + 右侧状态
pub fn draw_menu(frame: &mut Frame, app: &App, lay: &super::layout::FrameLayout) {
    let icon_style = if app.sidebar_open {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(DIM)
    };
    // 注意：☰ 在 unicode-width 中是双宽字符，必须给 2 列
    frame.render_widget(
        Paragraph::new(Span::styled("☰", icon_style)),
        Rect { x: lay.toggle_sidebar.x + 1, y: lay.menu.y, width: 2, height: 1 },
    );
    frame.render_widget(
        Paragraph::new(Span::styled("◆ OpsAgent", strong(ACCENT))),
        lay.brand,
    );

    let item = |active: bool| {
        if active {
            Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(TEXT)
        }
    };
    frame.render_widget(
        Paragraph::new(Span::styled("设置", item(app.overlay == Overlay::Settings))),
        lay.settings,
    );
    frame.render_widget(
        Paragraph::new(Span::styled("关于", item(app.overlay == Overlay::About))),
        lay.about,
    );

    // 右侧状态
    let (icon, label, color) = match &app.status {
        Status::Idle => ("●", "就绪".to_string(), OK),
        Status::Thinking => (SPINNER[tick(app)], "思考中".to_string(), ACCENT),
        Status::Responding => (SPINNER[tick(app)], "生成中".to_string(), ACCENT),
        Status::Tool(name) => (SPINNER[tick(app)], format!("执行 {name}"), TOOL),
        Status::WaitingApproval => ("||", "等待确认".to_string(), WARN),
    };
    let right = Line::from(vec![
        Span::styled(app.cfg.model.clone(), Style::new().fg(ASSISTANT)),
        Span::styled("  ·  ", dim()),
        Span::styled(
            app.cfg.namespace().to_string(),
            Style::new().fg(TOOL),
        ),
        Span::styled("  ", dim()),
        Span::styled(icon, Style::new().fg(color).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(label, Style::new().fg(color)),
        Span::styled(" ", base()),
    ]);
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), lay.menu);
}

pub fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::new().padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let width = inner.width.max(12) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if app.messages.is_empty() {
        welcome(&mut lines, width);
    }
    for msg in &app.messages {
        render_message(&mut lines, msg, width);
    }

    let total = lines.len();
    let view_h = inner.height as usize;
    let max_scroll = total.saturating_sub(view_h);
    if app.follow {
        app.scroll = max_scroll;
    } else {
        app.scroll = app.scroll.min(max_scroll);
        if app.scroll >= max_scroll {
            app.follow = true;
        }
    }

    frame.render_widget(
        Paragraph::new(lines).scroll((app.scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );

    if total > view_h {
        let mut state = ScrollbarState::new(max_scroll).position(app.scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(ACCENT_SOFT))
                .thumb_style(Style::new().fg(ACCENT)),
            area,
            &mut state,
        );
    }
}

fn welcome(out: &mut Vec<Line<'static>>, width: usize) {
    out.push(Line::from(""));
    out.push(Line::from(vec![
        Span::styled("  OpsAgent", strong(ACCENT)),
        Span::styled("  Linux / Kubernetes 运维智能体", dim()),
    ]));
    out.push(Line::from(""));
    let tips = [
        ("巡检", "帮我做一次系统巡检，看看有没有风险"),
        ("排障", "/var 只剩 3%，定位大文件并给出清理方案"),
        ("服务", "nginx 起不来，帮我查原因"),
        ("集群", "列出所有 CrashLoopBackOff 的 Pod 并分析原因"),
        ("发布", "把 prod 命名空间的 api 镜像更新到 v1.2.3"),
    ];
    for (tag, text) in tips {
        let tag = format!("{tag:<4}");
        for (i, l) in wrap_plain(text, width.saturating_sub(10)).iter().enumerate() {
            if i == 0 {
                out.push(Line::from(vec![
                    Span::styled("  ", base()),
                    Span::styled(tag.clone(), Style::new().fg(TOOL)),
                    Span::styled("  ", base()),
                    Span::styled(l.clone(), dim()),
                ]));
            } else {
                out.push(Line::from(vec![
                    Span::styled("        ", base()),
                    Span::styled(l.clone(), dim()),
                ]));
            }
        }
    }
    out.push(Line::from(""));
    out.push(Line::from(vec![
        Span::styled("  ", base()),
        Span::styled("Tab 切换侧边栏与输入框 · 左上「设置 / 关于」可点击", dim()),
    ]));
    out.push(Line::from(""));
}

fn tool_color(state: ToolState) -> Color {
    match state {
        ToolState::Pending | ToolState::Running => TOOL,
        ToolState::Waiting => WARN,
        ToolState::Done => OK,
        ToolState::Denied => ERR,
    }
}

fn render_message(out: &mut Vec<Line<'static>>, msg: &Display, width: usize) {
    match msg {
        Display::User(text) => {
            let text = sanitize(text);
            out.push(Line::from(vec![Span::styled("❯ 你", strong(USER))]));
            for l in wrap_plain(&text, width.saturating_sub(2)) {
                out.push(Line::from(vec![
                    Span::styled("  ", base()),
                    Span::styled(l, Style::new().fg(USER)),
                ]));
            }
            out.push(Line::from(""));
        }
        Display::Assistant { text, reasoning, streaming } => {
            let text = sanitize(text);
            let reasoning = sanitize(reasoning);
            let mut head = vec![Span::styled("◆ DeepSeek", strong(ASSISTANT))];
            if *streaming {
                head.push(Span::styled(" …", dim()));
            }
            out.push(Line::from(head));

            if !reasoning.trim().is_empty() {
                let lines = wrap_plain(&reasoning, width.saturating_sub(4));
                let start = lines.len().saturating_sub(4);
                for l in &lines[start..] {
                    out.push(Line::from(vec![
                        Span::styled("  ⋯ ", dim().add_modifier(Modifier::ITALIC)),
                        Span::styled(l.clone(), dim().add_modifier(Modifier::ITALIC)),
                    ]));
                }
            }

            if text.trim().is_empty() && *streaming {
                out.push(Line::from(vec![Span::styled("  ", base()), Span::styled("…", dim())]));
            } else {
                render_markdown(out, &text, width.saturating_sub(2));
            }
            out.push(Line::from(""));
        }
        Display::ToolCall { name, preview, state } => {
            let color = tool_color(*state);
            out.push(Line::from(vec![
                Span::styled("  » ", Style::new().fg(color)),
                Span::styled(name.clone(), strong(color)),
                Span::styled(format!("   {}", state.label()), dim()),
            ]));
            let first = sanitize(preview.lines().next().unwrap_or(""));
            for l in wrap_plain(&first, width.saturating_sub(6)).into_iter().take(2) {
                out.push(Line::from(vec![
                    Span::styled("    ", base()),
                    Span::styled(l, dim()),
                ]));
            }
        }
        Display::ToolResult { output, ok, .. } => {
            let output = sanitize(output);
            let head_color = if *ok { DIM } else { ERR };
            out.push(Line::from(vec![
                Span::styled("    ╰ ", Style::new().fg(head_color)),
                Span::styled(if *ok { "输出" } else { "输出（失败）" }, Style::new().fg(head_color)),
            ]));
            let total = output.lines().count();
            let shown = 10usize;
            for l in output.lines().take(shown) {
                for w in wrap_plain(l, width.saturating_sub(8)) {
                    out.push(Line::from(vec![
                        Span::styled("    │ ", Style::new().fg(ACCENT_SOFT)),
                        Span::styled(w, Style::new().fg(if *ok { TEXT } else { ERR })),
                    ]));
                }
            }
            if total > shown {
                out.push(Line::from(vec![
                    Span::styled("    │ ", Style::new().fg(ACCENT_SOFT)),
                    Span::styled(format!("… 其余 {} 行已省略", total - shown), dim()),
                ]));
            }
            out.push(Line::from(""));
        }
        Display::Info(text) => {
            let text = sanitize(text);
            for (i, l) in wrap_plain(&text, width.saturating_sub(4)).into_iter().enumerate() {
                if i == 0 {
                    out.push(Line::from(vec![
                        Span::styled("  · ", dim()),
                        Span::styled(l, dim().add_modifier(Modifier::ITALIC)),
                    ]));
                } else {
                    out.push(Line::from(vec![
                        Span::styled("    ", base()),
                        Span::styled(l, dim().add_modifier(Modifier::ITALIC)),
                    ]));
                }
            }
        }
        Display::Error(text) => {
            let text = sanitize(text);
            for (i, l) in wrap_plain(&text, width.saturating_sub(4)).into_iter().enumerate() {
                if i == 0 {
                    out.push(Line::from(vec![
                        Span::styled("  ✗ ", strong(ERR)),
                        Span::styled(l, Style::new().fg(ERR)),
                    ]));
                } else {
                    out.push(Line::from(vec![
                        Span::styled("    ", base()),
                        Span::styled(l, Style::new().fg(ERR)),
                    ]));
                }
            }
            out.push(Line::from(""));
        }
    }
}

/// 极简 Markdown：代码块、标题、列表、行内代码
fn render_markdown(out: &mut Vec<Line<'static>>, text: &str, width: usize) {
    let width = width.max(12);
    let mut in_code = false;

    for raw in text.split('\n') {
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") {
            if in_code {
                out.push(Line::from(Span::styled("  └──────", Style::new().fg(ACCENT_SOFT))));
                in_code = false;
            } else {
                let lang = trimmed.trim_start_matches('`').trim();
                let label = if lang.is_empty() { "code".to_string() } else { lang.to_string() };
                out.push(Line::from(vec![
                    Span::styled("  ┌─ ", Style::new().fg(ACCENT_SOFT)),
                    Span::styled(label, dim()),
                ]));
                in_code = true;
            }
            continue;
        }

        if in_code {
            for l in wrap_plain(line, width.saturating_sub(4)) {
                out.push(Line::from(vec![
                    Span::styled("  │ ", Style::new().fg(ACCENT_SOFT)),
                    Span::styled(l, Style::new().fg(Color::Rgb(186, 226, 200)).bg(CODE_BG)),
                ]));
            }
            continue;
        }

        if trimmed.is_empty() {
            out.push(Line::from(""));
            continue;
        }

        if trimmed.starts_with('#') {
            let content = trimmed.trim_start_matches('#').trim();
            for l in wrap_plain(content, width.saturating_sub(2)) {
                out.push(Line::from(vec![
                    Span::styled("  ", base()),
                    Span::styled(l, strong(ACCENT)),
                ]));
            }
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("> ") {
            for l in wrap_plain(rest, width.saturating_sub(4)) {
                out.push(Line::from(vec![
                    Span::styled("  ▏ ", Style::new().fg(ACCENT_SOFT)),
                    Span::styled(l, dim()),
                ]));
            }
            continue;
        }

        let (bullet, body) = if let Some(rest) = trimmed.strip_prefix("- ") {
            ("• ", rest.to_string())
        } else if let Some(rest) = trimmed.strip_prefix("* ") {
            ("• ", rest.to_string())
        } else {
            ("", trimmed.to_string())
        };
        let prefix = if bullet.is_empty() { "  ".to_string() } else { format!("  {bullet}") };
        let pw = display_width(&prefix);
        let lines = wrap_plain(&body, width.saturating_sub(pw));
        for (i, l) in lines.iter().enumerate() {
            let mut spans: Vec<Span<'static>> = Vec::new();
            if i == 0 {
                spans.push(Span::styled(prefix.clone(), Style::new().fg(ACCENT)));
            } else {
                spans.push(Span::raw(" ".repeat(pw)));
            }
            spans.extend(inline_spans(l, base()));
            out.push(Line::from(spans));
        }
    }
}

/// 处理 `行内代码` 与 **粗体**
fn inline_spans(line: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut in_code = false;
    for part in line.split('`') {
        if in_code {
            if !part.is_empty() {
                spans.push(Span::styled(
                    part.to_string(),
                    Style::new().fg(ACCENT).bg(CODE_BG),
                ));
            }
        } else {
            let mut bold = false;
            for seg in part.split("**") {
                if !seg.is_empty() {
                    let style = if bold {
                        base_style.add_modifier(Modifier::BOLD)
                    } else {
                        base_style
                    };
                    spans.push(Span::styled(seg.to_string(), style));
                }
                bold = !bold;
            }
        }
        in_code = !in_code;
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base_style));
    }
    spans
}

pub fn draw_input(frame: &mut Frame, app: &mut App, area: Rect) {
    let active = !app.busy() && !app.focus_sidebar;
    let hovered = app.hover_divider == Some(crate::app::Drag::Input);
    let border_style = if hovered {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        block_border(active)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Line::from(vec![Span::styled(
            " ✎ 输入 ",
            Style::new().fg(if active { ACCENT } else { ACCENT_SOFT }),
        )]))
        .title_bottom(Line::from(Span::styled(
            " Enter 发送 · Ctrl+J 换行 ",
            dim(),
        )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let width = inner.width.max(4) as usize;

    if app.input.is_empty() {
        let placeholder = "描述你的运维问题，例如：磁盘占用过高，帮我定位并清理";
        frame.render_widget(
            Paragraph::new(Span::styled(clip(placeholder, width), dim())),
            inner,
        );
        if active {
            frame.set_cursor_position((inner.x, inner.y));
        }
        return;
    }

    let layout = wrap_layout(&app.input.value, width);
    let cursor_byte = app.input.byte_cursor();
    let (row, col) = cursor_in(&layout, cursor_byte);
    let view_h = inner.height as usize;
    let offset = row.saturating_sub(view_h.saturating_sub(1));

    // 选区（字节范围），用于把每行切成「前 / 选中 / 后」三段
    let sel = app
        .input
        .selection()
        .map(|(s, e)| (app.input.byte_at(s), app.input.byte_at(e)));

    let lines: Vec<Line> = layout
        .iter()
        .skip(offset)
        .take(view_h)
        .map(|l| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let hit = sel
                .map(|(ss, se)| (ss.clamp(l.start, l.end), se.clamp(l.start, l.end)))
                .filter(|(a, b)| a < b);
            match hit {
                Some((a, b)) => {
                    // 用 floor_boundary 对齐到字符边界：
                    // 即使外部传来的偏移漂移，也只会切到字符中间之前的边界，不会 panic
                    let ra = floor_boundary(&l.text, a.saturating_sub(l.start));
                    let rb = floor_boundary(&l.text, b.saturating_sub(l.start)).max(ra);
                    if ra > 0 {
                        spans.push(Span::styled(l.text[..ra].to_string(), base()));
                    }
                    spans.push(Span::styled(
                        l.text[ra..rb].to_string(),
                        Style::new().fg(Color::White).bg(SELECT_BG),
                    ));
                    if rb < l.text.len() {
                        spans.push(Span::styled(l.text[rb..].to_string(), base()));
                    }
                }
                None => spans.push(Span::styled(l.text.clone(), base())),
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);

    if active {
        let cy = inner.y + (row - offset) as u16;
        let cx = inner.x + (col as u16).min(inner.width.saturating_sub(1));
        frame.set_cursor_position((cx, cy));
    }
}

pub fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let w = area.width as usize;
    if let Some(t) = app.active_toast() {
        let style = if t.error { Style::new().fg(ERR) } else { Style::new().fg(OK) };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ⓘ ", style.add_modifier(Modifier::BOLD)),
                Span::styled(clip(&t.text, w.saturating_sub(4)), style),
            ])),
            area,
        );
        return;
    }
    if let Some(hover) = app.hover_hint() {
        // 做成通栏高亮徽标，比一行小字更容易被看到
        let text = format!(" {hover} ");
        frame.render_widget(
            Paragraph::new(Span::styled(
                clip(&text, w),
                Style::new()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            area,
        );
        return;
    }
    let hint = if app.busy() {
        " Esc 取消当前任务 · Ctrl+C 退出".to_string()
    } else if app.cfg.auto_approve_all {
        " ! 已开启「跳过确认」 · 设置 里可关闭 · Ctrl+C 退出".to_string()
    } else {
        " Tab 侧边栏 · Ctrl+A 全选 · Ctrl+C 复制 · Ctrl+U 清空 · F1 关于 · F2 设置".to_string()
    };
    let style = if app.cfg.auto_approve_all && !app.busy() {
        Style::new().fg(WARN)
    } else {
        dim()
    };
    frame.render_widget(Paragraph::new(Span::styled(clip(&hint, w), style)), area);
}
