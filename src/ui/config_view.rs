//! 设置弹窗（顶部菜单「设置」/ F2 打开）

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::Frame;

use super::theme::*;
use super::util::{clip, display_width};
use crate::app::{App, FieldKind, F_AUTO_APPROVE};
use crate::config::mask_secret;

const LABEL_W: usize = 22;
const SEL_BG: Color = Color::Rgb(36, 44, 56);

fn pad_label(label: &str) -> String {
    let w = display_width(label);
    if w >= LABEL_W {
        return label.to_string();
    }
    format!("{label}{}", " ".repeat(LABEL_W - w))
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

pub fn draw_modal(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let w = area.width.saturating_sub(6).min(92).max(46);
    let h = area.height.saturating_sub(4).min(26).max(12);
    let rect = centered(area, w, h);

    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::from(vec![
            Span::styled(" 设置 ", strong(ACCENT)),
            // Span::styled(" 配置写入 SQLite 的 settings 表", dim()),
        ]))
        .title_bottom(Line::from(Span::styled(" Enter 确认并保存 · Esc 关闭 ", dim())))
        .padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height < 6 || inner.width < 24 {
        return;
    }

    let [fields_area, info_area] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(5)]).areas(inner);
    draw_fields(frame, app, fields_area);
    draw_info(frame, app, info_area);
}

fn draw_fields(frame: &mut Frame, app: &mut App, area: Rect) {
    let rows = app.form.fields.len();
    let view_h = area.height as usize;
    let offset = if app.form.idx >= view_h {
        app.form.idx + 1 - view_h
    } else {
        0
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cursor: Option<(u16, u16)> = None;

    for i in offset..rows.min(offset + view_h) {
        let field = app.form.at(i);
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
        let avail = (area.width as usize).saturating_sub(used);

        if editing {
            let buf = app.form.buffer.clone();
            let style = if app.form.sel_all {
                // Ctrl+A 全选：整段高亮，按 Backspace 可一键清空
                Style::new().fg(Color::White).bg(SELECT_BG)
            } else {
                Style::new().fg(ACCENT).bg(CODE_BG)
            };
            spans.push(Span::styled(clip(&buf, avail), style));
            let x = area.x
                + (used + display_width(&buf)).min(area.width.saturating_sub(1) as usize) as u16;
            cursor = Some((x, area.y + (i - offset) as u16));
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
            line = line.style(Style::new().bg(SEL_BG));
        }
        lines.push(line);
    }

    frame.render_widget(Paragraph::new(lines), area);
    if let Some((x, y)) = cursor {
        frame.set_cursor_position((x, y));
    }
}

fn draw_info(frame: &mut Frame, app: &App, area: Rect) {
    let hint = app.form.current().hint;
    let (key_text, key_style) = if app.cfg.effective_key().is_empty() {
        ("未配置", Style::new().fg(ERR))
    } else {
        ("已就绪", Style::new().fg(OK))
    };
    let auto = app.form.at(F_AUTO_APPROVE).value.eq_ignore_ascii_case("true");

    let text = vec![
        Line::from(vec![
            Span::styled(" 说明 ", strong(ACCENT)),
            Span::styled(clip(hint, (area.width as usize).saturating_sub(8)), dim()),
        ]),
        Line::from(vec![
            Span::styled(" Token ", dim()),
            Span::styled(key_text, key_style),
            Span::styled("　·　留空读环境变量 DEEPSEEK_API_KEY", dim()),
        ]),
        Line::from(vec![
            Span::styled(" 存储 ", dim()),
            Span::styled(
                clip(&app.db.file().display().to_string(), (area.width as usize).saturating_sub(8)),
                Style::new().fg(TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled(" 提示 ", dim()),
            Span::styled(
                if auto {
                    "「跳过操作确认」已开启，Agent 会直接执行命令（仅建议测试环境）"
                } else {
                    "涉及修改系统的操作都会弹窗让你确认"
                },
                if auto { Style::new().fg(WARN) } else { dim() },
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(text), area);
}
