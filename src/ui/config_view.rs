//! 配置界面（DeepSeek Token / 模型 / 集群参数）

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use ratatui::Frame;

use super::theme::*;
use super::util::{clip, display_width};
use crate::app::{App, FieldKind, F_AUTO_APPROVE};
use crate::config::mask_secret;

const LABEL_W: usize = 22;

fn pad_label(label: &str) -> String {
    let w = display_width(label);
    if w >= LABEL_W {
        return label.to_string();
    }
    format!("{label}{}", " ".repeat(LABEL_W - w))
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [head, body, info, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_head(frame, app, head);
    draw_fields(frame, app, body);
    draw_info(frame, app, info);
    draw_foot(frame, app, foot);
}

fn draw_head(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let left = Line::from(vec![
        Span::styled("⚙ 设置", strong(ACCENT)),
        Span::styled("  ·  ", dim()),
        Span::styled(app.db.file().display().to_string(), dim()),
    ]);
    let right = Line::from(vec![
        Span::styled(
            if app.cfg.has_key() { "Token 已配置" } else { "Token 未配置" },
            Style::new().fg(if app.cfg.has_key() { OK } else { ERR }),
        ),
        Span::styled("  ", base()),
    ]);
    let rw = right.width() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Length(rw)]).areas(area);
    frame.render_widget(Paragraph::new(left), l);
    frame.render_widget(Paragraph::new(right), r);
}

fn draw_fields(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(block_border(true))
        .title(Line::from(Span::styled(" 连接与运行参数 ", strong(ACCENT))))
        .padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let rows = app.form.fields.len();
    let view_h = inner.height as usize;
    let offset = if app.form.idx >= view_h {
        app.form.idx + 1 - view_h
    } else {
        0
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor: Option<(u16, u16)> = None;

    for i in offset..rows.min(offset + view_h) {
        let field = &app.form.fields[i];
        let selected = i == app.form.idx;
        let editing = selected && app.form.editing;

        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(if selected {
            Span::styled("▸ ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
        } else {
            Span::raw("  ")
        });
        spans.push(Span::styled(
            pad_label(field.label),
            if selected {
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
            } else {
                dim()
            },
        ));
        spans.push(Span::styled("  ", base()));

        let used = 2 + LABEL_W + 2;
        let avail = (inner.width as usize).saturating_sub(used);

        if editing {
            let buf = app.form.buffer.clone();
            spans.push(Span::styled(
                clip(&buf, avail),
                Style::new().fg(ACCENT).bg(CODE_BG),
            ));
            let x = inner.x + (used + display_width(&buf)).min(inner.width.saturating_sub(1) as usize) as u16;
            cursor = Some((x, inner.y + (i - offset) as u16));
        } else {
            let (text, style) = match field.kind {
                FieldKind::Secret => {
                    if field.value.trim().is_empty() {
                        if !app.cfg.effective_key().is_empty() {
                            ("（使用环境变量 DEEPSEEK_API_KEY）".to_string(), Style::new().fg(DIM))
                        } else {
                            ("未配置 —— 回车填写".to_string(), Style::new().fg(ERR))
                        }
                    } else {
                        (mask_secret(field.value.trim()), Style::new().fg(TOOL))
                    }
                }
                FieldKind::Bool => {
                    let on = field.value.eq_ignore_ascii_case("true");
                    if on {
                        ("[x] 已开启（所有操作不再确认）".to_string(), Style::new().fg(ERR))
                    } else {
                        ("[ ] 关闭（默认）".to_string(), Style::new().fg(OK))
                    }
                }
                _ => {
                    if field.value.trim().is_empty() {
                        ("（空）".to_string(), dim())
                    } else {
                        (field.value.clone(), Style::new().fg(TEXT))
                    }
                }
            };
            spans.push(Span::styled(clip(&text, avail), style));
        }

        let mut line = Line::from(spans);
        if selected {
            line = line.style(Style::new().bg(ratatui::style::Color::Rgb(34, 40, 50)));
        }
        lines.push(line);
    }

    frame.render_widget(Paragraph::new(lines), inner);
    if let Some((x, y)) = cursor {
        frame.set_cursor_position((x, y));
    }
}

fn draw_info(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let hint = app.form.current().hint;
    let secret_state = if app.cfg.effective_key().is_empty() {
        Span::styled("未配置", Style::new().fg(ERR))
    } else {
        Span::styled("已就绪", Style::new().fg(OK))
    };

    let text = vec![
        Line::from(vec![
            Span::styled(" 说明 ", strong(ACCENT)),
            Span::styled(hint, dim()),
        ]),
        Line::from(vec![
            Span::styled(" Token ", dim()),
            secret_state,
            Span::styled(
                "   ·   留空则读取环境变量 DEEPSEEK_API_KEY　·　保存后写入 SQLite 的 settings 表（回车自动保存）",
                dim(),
            ),
        ]),
        Line::from(vec![
            Span::styled(" 提示 ", dim()),
            Span::styled(
                if app.form.fields[F_AUTO_APPROVE].value.eq_ignore_ascii_case("true") {
                    "「跳过操作确认」已开启，Agent 会直接执行命令，请务必只在测试环境使用"
                } else {
                    "涉及修改系统的操作（执行命令、写文件、改集群）都会弹窗让你确认"
                },
                if app.form.fields[F_AUTO_APPROVE].value.eq_ignore_ascii_case("true") {
                    Style::new().fg(WARN)
                } else {
                    Style::new().fg(TEXT)
                },
            ),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(text).wrap(ratatui::widgets::Wrap { trim: true }),
        area,
    );
}

fn draw_foot(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let w = area.width as usize;
    if let Some(t) = app.active_toast() {
        let style = if t.error { Style::new().fg(ERR) } else { Style::new().fg(OK) };
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&t.text, w), style)),
            area,
        );
        return;
    }
    let hint = if app.form.editing {
        " 输入中：Enter 确认 · Esc 取消 · Tab 跳到下一项 · Ctrl+U 清空"
    } else {
        " ↑↓ 选择 · Enter/空格 编辑或切换 · Ctrl+S 保存 · Ctrl+T 测试连接 · Ctrl+R 还原 · Esc 返回"
    };
    frame.render_widget(Paragraph::new(Span::styled(clip(hint, w), dim())), area);
}
