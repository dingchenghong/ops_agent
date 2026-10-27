//! 界面布局计算：同一份布局既用于渲染，也用于鼠标命中判定

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, BorderType};

use super::util::display_width;
use crate::app::App;

pub struct FrameLayout {
    /// 顶部菜单栏
    pub menu: Rect,
    /// 菜单项命中区
    pub toggle_sidebar: Rect,
    pub brand: Rect,
    pub settings: Rect,
    pub about: Rect,
    /// 左侧会话栏（含边框）
    pub sidebar: Rect,
    /// 会话栏内部可用区
    pub sidebar_inner: Rect,
    /// 侧边栏右边界（可拖动改宽度）
    pub sidebar_divider: Rect,
    /// 「新对话」按钮命中区
    pub new_chat: Rect,
    /// 每个会话条目的命中区（session_id, rect）
    pub session_items: Vec<(i64, Rect)>,
    /// 右侧主区域
    pub main: Rect,
    pub chat: Rect,
    pub input: Rect,
    /// 输入框上边框（可拖动改高度）
    pub input_divider: Rect,
    /// 状态行：与输入框底边框同一行（整屏最后一行）
    pub footer: Rect,
}

/// 侧边栏宽度允许范围
pub const SIDEBAR_MIN: u16 = 12;
pub const SIDEBAR_DEFAULT: u16 = 26;
/// 输入框高度允许范围（含边框）
pub const INPUT_MIN: u16 = 3;
pub const INPUT_MAX: u16 = 18;
pub const INPUT_DEFAULT: u16 = 4;

pub fn compute(area: Rect, app: &mut App) -> FrameLayout {
    let [menu, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);

    // 侧边栏：宽度可拖动，也可整体收起
    let max_sidebar = area.width.saturating_sub(26).clamp(SIDEBAR_MIN, 60);
    let want = app.sidebar_width.clamp(SIDEBAR_MIN, max_sidebar.max(SIDEBAR_MIN));
    let (sidebar, main) = if app.sidebar_open && area.width >= 50 {
        let [s, m] =
            Layout::horizontal([Constraint::Length(want), Constraint::Min(20)]).areas(body);
        (s, m)
    } else {
        (
            Rect { x: body.x, y: body.y, width: 0, height: body.height },
            body,
        )
    };

    // 输入框高度可拖动。
    // 注意：输入框的最后一行同时就是状态行（整屏最后一行），左边栏的底边框也落在这一行，
    // 这样窗口底部只有一条连续的横线，不会再出现「线比文字高一行」的错位感。
    let max_input = main.height.saturating_sub(6).clamp(INPUT_MIN, INPUT_MAX);
    let input_h = app.input_height.clamp(INPUT_MIN, max_input);
    let [chat, input] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(input_h)]).areas(main);
    // 状态行 = 输入框底边框所在行
    let footer = Rect {
        x: input.x,
        y: input.y + input.height.saturating_sub(1),
        width: input.width,
        height: input.height.min(1),
    };

    // 菜单项位置
    let name_w = display_width("◆ OpsAgent") as u16;
    let gap = 2u16;
    let toggle_sidebar = Rect { x: area.x, y: menu.y, width: 3, height: 1 };
    let brand = Rect { x: area.x + 4, y: menu.y, width: name_w, height: 1 };
    let settings = Rect {
        x: brand.x + name_w + gap,
        y: menu.y,
        width: display_width("设置") as u16,
        height: 1,
    };
    let about = Rect {
        x: settings.x + settings.width + gap,
        y: menu.y,
        width: display_width("关于") as u16,
        height: 1,
    };

    let block = Block::bordered().border_type(BorderType::Rounded);
    let sidebar_inner = block.inner(sidebar);

    // 让选中的会话保持可见
    let view = sidebar_inner.height.saturating_sub(2) as usize;
    let total = app.sessions.len();
    let sel = app.sidebar_idx.saturating_sub(1);
    let max_scroll = total.saturating_sub(view);
    if view > 0 {
        if sel >= app.sidebar_scroll + view {
            app.sidebar_scroll = sel + 1 - view;
        }
        if sel < app.sidebar_scroll {
            app.sidebar_scroll = sel;
        }
    }
    app.sidebar_scroll = app.sidebar_scroll.min(max_scroll);

    let new_chat = Rect {
        x: sidebar_inner.x,
        y: sidebar_inner.y,
        width: sidebar_inner.width,
        height: 1.min(sidebar_inner.height),
    };

    let mut session_items = Vec::new();
    if sidebar_inner.height > 2 {
        for (i, s) in app
            .sessions
            .iter()
            .enumerate()
            .skip(app.sidebar_scroll)
            .take(view)
        {
            let y = sidebar_inner.y + 2 + (i - app.sidebar_scroll) as u16;
            session_items.push((
                s.id,
                Rect { x: sidebar_inner.x, y, width: sidebar_inner.width, height: 1 },
            ));
        }
    }

    let sidebar_divider = Rect {
        x: sidebar.x + sidebar.width.saturating_sub(1),
        y: body.y,
        width: if sidebar.width > 0 { 1 } else { 0 },
        height: body.height,
    };
    let input_divider = Rect { x: input.x, y: input.y, width: input.width, height: 1 };

    FrameLayout {
        menu,
        toggle_sidebar,
        brand,
        settings,
        about,
        sidebar,
        sidebar_divider,
        sidebar_inner,
        new_chat,
        session_items,
        main,
        chat,
        input,
        input_divider,
        footer,
    }
}
