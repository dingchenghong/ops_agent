//! 左侧会话列表：新对话按钮 + 历史会话

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::Frame;

use super::layout::FrameLayout;
use super::theme::*;
use super::util::clip;
use crate::app::App;

const SEL_BG: Color = Color::Rgb(36, 44, 56);

pub fn draw(frame: &mut Frame, app: &mut App, lay: &FrameLayout) {
    let focused = app.focus_sidebar;
    // 鼠标悬停在右边界时高亮，提示这里可以拖动
    let hovered = app.hover_divider == Some(crate::app::Drag::Sidebar);
    let border_style = if hovered {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        block_border(focused)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Line::from(vec![
            Span::styled(" 会话 ", strong(ACCENT)),
            Span::styled(format!("{} ", app.sessions.len()), dim()),
        ]));
    frame.render_widget(block, lay.sidebar);

    if lay.new_chat.height == 0 {
        return;
    }

    // 「新对话」按钮
    let active = app.sidebar_idx == 0;
    let mut style = Style::new().fg(ACCENT);
    if active && focused {
        style = style.bg(SEL_BG).add_modifier(Modifier::BOLD);
    } else if active {
        style = style.add_modifier(Modifier::BOLD);
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            clip(" ＋ 新对话", lay.new_chat.width as usize),
            style,
        )),
        lay.new_chat,
    );

    // 分隔线
    let sep_y = lay.new_chat.y.saturating_add(1);
    if lay.sidebar_inner.height > 1 && sep_y < lay.sidebar_inner.y + lay.sidebar_inner.height {
        let sep = ratatui::layout::Rect {
            x: lay.sidebar_inner.x,
            y: sep_y,
            width: lay.sidebar_inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                "─".repeat(sep.width as usize),
                Style::new().fg(ACCENT_SOFT),
            )),
            sep,
        );
    }

    for (i, (id, rect)) in lay.session_items.iter().enumerate() {
        let idx = app.sidebar_scroll + i + 1;
        let selected = focused && app.sidebar_idx == idx;
        let current = *id == app.session_id;

        let session = app.sessions.iter().find(|s| s.id == *id);
        let title = session
            .map(|s| s.title.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "新会话".to_string());

        let avail = (rect.width as usize).saturating_sub(8).max(4);
        let mut spans = vec![
            Span::styled(
                if current { " ▌ " } else { "   " },
                Style::new().fg(ACCENT),
            ),
            Span::styled(format!("#{} ", id), Style::new().fg(ACCENT_SOFT)),
            Span::styled(
                clip(&title, avail),
                if current { Style::new().fg(TEXT) } else { dim() },
            ),
        ];
        if current {
            spans.push(Span::styled(" ●", Style::new().fg(OK)));
        }

        let mut line = Line::from(spans);
        if selected {
            line = line.style(Style::new().bg(SEL_BG));
        }
        frame.render_widget(Paragraph::new(line), *rect);
    }

    // 空态
    if app.sessions.is_empty() && lay.sidebar_inner.height > 3 {
        let rect = ratatui::layout::Rect {
            x: lay.sidebar_inner.x,
            y: lay.sidebar_inner.y + 3,
            width: lay.sidebar_inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(clip(" 还没有历史会话", rect.width as usize), dim())),
            rect,
        );
    }
}
