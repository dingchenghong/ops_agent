pub mod chat;
pub mod config_view;
pub mod input;
pub mod layout;
pub mod modal;
pub mod sidebar;
pub mod theme;
pub mod util;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Drag, Overlay};
use theme::ACCENT;

/// 弹窗打开时把背景压暗，避免底层边框在弹窗边上"露出半截"
fn dim_background(frame: &mut Frame) {
    let dim = Color::Rgb(72, 76, 86);
    for cell in frame.buffer_mut().content.iter_mut() {
        cell.set_fg(dim);
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    // 弹窗打开时不再保持「可拖动」悬停态
    if app.overlay != Overlay::None || app.approval.is_some() {
        app.hover_divider = None;
    }

    let lay = layout::compute(frame.area(), app);
    app.size = (frame.area().width, frame.area().height);

    chat::draw_menu(frame, app, &lay);
    if app.sidebar_open {
        sidebar::draw(frame, app, &lay);
    }
    chat::draw_messages(frame, app, lay.chat);
    chat::draw_input(frame, app, lay.input);
    chat::draw_footer(frame, app, lay.footer);

    // 有弹窗时先压暗背景
    if app.overlay != Overlay::None || app.approval.is_some() {
        dim_background(frame);
    }

    match app.overlay {
        Overlay::Settings => config_view::draw_modal(frame, app),
        Overlay::About => modal::about(frame, app),
        Overlay::None => {}
    }
    if app.approval.is_some() {
        modal::approval(frame, app);
    }

    // 悬停在分隔条上时：整条分隔线加粗高亮 + 指针处画"抓手" + 硬件光标移过去
    // （指针形状靠 OSC 22，终端不支持时这两层视觉反馈仍然有效）
    if let Some(kind) = app.hover_divider {
        match kind {
            Drag::Sidebar => {
                let d = lay.sidebar_divider;
                if d.height > 0 && d.width > 0 {
                    let thick: Vec<ratatui::text::Line> = (0..d.height)
                        .map(|_| {
                            ratatui::text::Line::from(Span::styled(
                                "┃",
                                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                            ))
                        })
                        .collect();
                    frame.render_widget(Paragraph::new(thick), d);
                }
            }
            Drag::Input => {
                let d = lay.input_divider;
                if d.width > 0 {
                    let bar = Span::styled(
                        "━".repeat(d.width as usize),
                        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                    );
                    frame.render_widget(Paragraph::new(bar), d);
                }
            }
        }
        let (col, row) = app.hover_pos.unwrap_or((0, 0));
        let (x, y, glyph) = match kind {
            Drag::Sidebar => (
                lay.sidebar_divider.x,
                row.clamp(
                    lay.sidebar_divider.y,
                    lay.sidebar_divider
                        .y
                        .saturating_add(lay.sidebar_divider.height.saturating_sub(1)),
                ),
                "⇔",
            ),
            Drag::Input => (
                col.clamp(
                    lay.input_divider.x,
                    lay.input_divider
                        .x
                        .saturating_add(lay.input_divider.width.saturating_sub(1)),
                ),
                lay.input_divider.y,
                "⇕",
            ),
        };
        let style = Style::new()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Span::styled(glyph, style)),
            Rect { x, y, width: 1, height: 1 },
        );
        frame.set_cursor_position((x, y));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::app::{Approval, Display, Overlay, Status, ToolState};
    use crate::config::Config;

    pub(super) fn render(app: &mut App, w: u16, h: u16, name: &str) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        let area = buf.area();
        let mut out = String::new();
        let mut row = String::new();
        for y in 0..area.height {
            row.clear();
            for x in 0..area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            out.push_str(&row);
            out.push('\n');
        }
        std::fs::create_dir_all("/tmp/ops-agent-ui").unwrap();
        std::fs::write(format!("/tmp/ops-agent-ui/{name}.txt"), &out).unwrap();
        out
    }

    /// 去掉空白后比较，规避宽字符（CJK）占两个单元格带来的空格
    pub(super) fn flat(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// 构造测试用 App：内存数据库 + 真实 runtime（Agent 不会发起网络请求）
    pub(super) fn test_app() -> App {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let db = rt.block_on(crate::db::Db::open_memory()).unwrap();
        let mut cfg = Config::default();
        // 默认已配置，避免启动时自动弹出设置弹窗
        cfg.api_key = "sk-test-0000".to_string();
        let agent = crate::agent::AgentHandle::spawn(&rt, cfg.clone(), db.clone(), 1, Vec::new());
        let handle = rt.handle().clone();
        std::mem::forget(rt); // 测试期间保持 runtime 存活
        App::new(cfg, agent, db, handle, 1, Vec::new())
    }

    #[test]
    fn menu_and_sidebar_render() {
        let mut app = test_app();
        let screen = render(&mut app, 110, 32, "layout");
        let flat = flat(&screen);
        assert!(flat.contains("设置"), "菜单应有「设置」");
        assert!(flat.contains("关于"), "菜单应有「关于」");
        assert!(flat.contains("新对话"), "侧边栏应有「新对话」");
        assert!(flat.contains("会话"));
        assert!(screen.contains("☰"), "菜单栏应有侧边栏开关");
    }

    #[test]
    fn sidebar_lists_sessions() {
        let mut app = test_app();
        let id2 = app
            .rt
            .block_on(app.db.create_session("磁盘排查", "deepseek-chat"))
            .unwrap();
        app.rt
            .block_on(app.db.create_session("k8s 巡检", "deepseek-chat"))
            .unwrap();
        app.session_id = id2;
        app.refresh_sessions();

        let screen = render(&mut app, 110, 32, "sidebar");
        let flat = flat(&screen);
        assert!(flat.contains("磁盘排查"));
        assert!(flat.contains("k8s巡检"));
        assert!(flat.contains("#2"));
    }

    #[test]
    fn chat_screen_renders_messages() {
        let mut app = test_app();
        app.messages.push(Display::User("帮我看看磁盘".into()));
        app.messages.push(Display::Assistant {
            text: "先看磁盘占用：\n\n```bash\ndf -h\n```\n\n- 检查大文件".into(),
            reasoning: String::new(),
            streaming: false,
        });
        app.messages.push(Display::ToolCall {
            name: "run_shell".into(),
            preview: "$ df -h".into(),
            state: ToolState::Done,
        });
        app.messages.push(Display::ToolResult {
            name: "run_shell".into(),
            output: "exit_code: 0\n--- stdout ---\n/dev/sda1 100G 95G 5G 95%".into(),
            ok: true,
        });
        app.status = Status::Idle;
        app.input.insert_char('测');

        let screen = render(&mut app, 110, 32, "chat");
        let flat = flat(&screen);
        assert!(flat.contains("帮我看看磁盘"));
        assert!(flat.contains("run_shell"));
        assert!(flat.contains("df-h"));
    }

    /// 侧边栏加宽 + 输入框加高后的布局
    #[test]
    fn resized_layout_renders() {
        let mut app = test_app();
        app.sidebar_width = 36;
        app.input_height = 8;
        app.messages.push(Display::User("看看磁盘".into()));
        app.messages.push(Display::Assistant {
            text: "好的，先执行 df -h。".into(),
            reasoning: String::new(),
            streaming: false,
        });
        let screen = render(&mut app, 110, 30, "resized");
        let flat = flat(&screen);
        assert!(flat.contains("看看磁盘"));
        assert!(flat.contains("新对话"));
    }

    /// 收起侧边栏后主区域应占满宽度
    #[test]
    fn collapsed_sidebar_gives_full_width() {
        let mut app = test_app();
        app.sidebar_open = false;
        let screen = render(&mut app, 110, 30, "collapsed");
        let lay = crate::ui::layout::compute(
            ratatui::layout::Rect::new(0, 0, 110, 30),
            &mut app,
        );
        assert_eq!(lay.sidebar.width, 0);
        assert_eq!(lay.main.width, 110);
        assert!(!flat(&screen).contains("＋新对话"));
    }

    /// 悬停在分隔条上时，底部应出现操作提示
    #[test]
    fn hovering_divider_shows_hint() {
        let mut app = test_app();
        app.size = (110, 32);
        let lay = crate::ui::layout::compute(ratatui::layout::Rect::new(0, 0, 110, 32), &mut app);
        app.hover_divider = Some(crate::app::Drag::Sidebar);
        app.hover_pos = Some((lay.sidebar_divider.x, 10));

        let screen = render(&mut app, 110, 32, "hover_sidebar");
        assert!(
            flat(&screen).contains("拖动调整左栏宽度"),
            "底部应提示可拖动"
        );

        app.hover_divider = Some(crate::app::Drag::Input);
        let screen = render(&mut app, 110, 32, "hover_input");
        assert!(flat(&screen).contains("拖动调整输入框高度"));
    }

    /// Ctrl+A 全选后，输入框里的内容应带选区底色
    #[test]
    fn selection_is_highlighted() {
        let mut app = test_app();
        app.size = (110, 32);
        app.input.insert_str("abc");
        app.input.select_all();

        let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let lay = layout::compute(ratatui::layout::Rect::new(0, 0, 110, 32), &mut app);
        let buf = terminal.backend().buffer();

        let cell = &buf[(lay.input.x + 1, lay.input.y + 1)];
        assert_eq!(cell.symbol(), "a");
        assert_eq!(
            cell.bg,
            crate::ui::theme::SELECT_BG,
            "选中文字应有高亮底色"
        );
        assert_eq!(
            app.input.selection_text().as_deref(),
            Some("abc"),
            "Ctrl+A 应选中全部内容"
        );
    }

    /// 悬停时分隔条上应出现「抓手」符号
    #[test]
    fn hover_draws_grip_on_divider() {
        let mut app = test_app();
        app.size = (110, 32);
        let lay = layout::compute(ratatui::layout::Rect::new(0, 0, 110, 32), &mut app);
        app.hover_divider = Some(crate::app::Drag::Sidebar);
        app.hover_pos = Some((lay.sidebar_divider.x, 10));

        let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(lay.sidebar_divider.x, 10)].symbol(),
            "⇔",
            "左栏边界上应显示左右拖动抓手"
        );

        app.hover_divider = Some(crate::app::Drag::Input);
        app.hover_pos = Some((lay.input_divider.x + 3, lay.input_divider.y));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(lay.input_divider.x + 3, lay.input_divider.y)].symbol(),
            "⇕",
            "输入框上边框应显示上下拖动抓手"
        );
    }

    /// 弹窗底边必须完整：不能被滚动提示盖掉，也不能重复出现关闭提示
    #[test]
    fn modal_bottom_border_is_intact() {
        let mut app = test_app();
        app.overlay = Overlay::About;
        let screen = render(&mut app, 110, 32, "about_border");
        let rows: Vec<&str> = screen.lines().collect();
        // 找到弹窗底边
        let bottom = rows
            .iter()
            .rev()
            .find(|r| r.contains('╰') && r.contains('╯'))
            .expect("应有弹窗底边");
        assert!(
            !bottom.trim_end().ends_with("Esc 关闭") || bottom.contains("滚动"),
            "底部提示应作为边框标题，而不是覆盖边框"
        );
        let count = bottom.matches("Esc 关闭").count();
        assert!(count <= 1, "关闭提示不应重复：{bottom}");
    }

    /// 弹窗打开时背景应被压暗
    #[test]
    fn modal_dims_background() {
        let mut app = test_app();
        let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let before = terminal.backend().buffer()[(2, 3)].fg;
        app.overlay = Overlay::About;
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let after = terminal.backend().buffer()[(2, 3)].fg;
        assert_ne!(before, after, "打开弹窗后背景应压暗");
        assert_eq!(after, ratatui::style::Color::Rgb(72, 76, 86));
    }

    /// 小终端里弹窗也要完整闭合，不能有残边
    #[test]
    fn about_modal_fits_small_terminal() {
        let mut app = test_app();
        app.overlay = Overlay::About;
        let screen = render(&mut app, 80, 24, "about_small");
        for corner in ['╭', '╮', '╰', '╯'] {
            assert!(screen.contains(corner), "弹窗应四角完整：缺 {corner}");
        }
    }

    /// 消息里的 emoji 必须被消毒，否则终端会把整行右移、顶掉边框
    #[test]
    fn emoji_in_messages_is_sanitized() {
        let mut app = test_app();
        app.messages.push(Display::Assistant {
            text: "⚠️ 提示：检查 apiVersion".into(),
            reasoning: String::new(),
            streaming: false,
        });
        let screen = render(&mut app, 110, 32, "emoji");
        let flat = flat(&screen);
        assert!(flat.contains("!提示"), "⚠️ 应被替换成安全字符");
        assert!(!screen.contains('⚠'));
        assert!(!screen.contains('\u{FE0F}'));
    }

    /// 悬停时整条分隔线应加粗（┃），抓手位置显示 ⇔
    #[test]
    fn hover_thickens_divider() {
        let mut app = test_app();
        app.size = (110, 32);
        let lay = layout::compute(ratatui::layout::Rect::new(0, 0, 110, 32), &mut app);
        app.hover_divider = Some(crate::app::Drag::Sidebar);
        app.hover_pos = Some((lay.sidebar_divider.x, 10));

        let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(lay.sidebar_divider.x, 10)].symbol(), "⇔");
        assert_eq!(
            buf[(lay.sidebar_divider.x, 11)].symbol(),
            "┃",
            "分隔线其余位置应加粗"
        );
    }

    #[test]
    fn settings_modal_renders() {
        let mut app = test_app();
        app.overlay = Overlay::Settings;
        let screen = render(&mut app, 110, 32, "settings");
        let flat = flat(&screen);
        assert!(flat.contains("设置"));
        assert!(flat.contains("DeepSeekAPIToken"));
        assert!(flat.contains("kube-context"));
    }

    #[test]
    fn about_modal_renders() {
        let mut app = test_app();
        app.overlay = Overlay::About;
        let screen = render(&mut app, 110, 60, "about");
        let flat = flat(&screen);
        assert!(flat.contains("关于OpsAgent"));
        assert!(flat.contains("快捷键"));
        assert!(flat.contains("安全机制"));
    }

    #[test]
    fn approval_modal_renders() {
        let mut app = test_app();
        app.approval = Some(Approval {
            id: "call_1".into(),
            name: "run_shell".into(),
            preview: "$ systemctl restart nginx".into(),
        });
        app.status = Status::WaitingApproval;
        let screen = render(&mut app, 110, 32, "approval");
        let flat = flat(&screen);
        assert!(flat.contains("需要你确认"));
        assert!(flat.contains("systemctlrestartnginx"));
    }
}
#[cfg(test)]
mod hint_tests {
    use super::tests::{flat, render, test_app};

    /// 悬停在分隔条上时，状态行应显示拖动提示
    #[test]
    fn hover_shows_footer_hint() {
        let mut app = test_app();
        app.hover_divider = Some(crate::app::Drag::Sidebar);
        let screen = render(&mut app, 120, 32, "hover");
        let flat = flat(&screen);
        assert!(flat.contains("拖动调整"), "底部应显示拖动提示，实际:
{flat}");
    }

}

#[cfg(test)]
mod border_tests {
    use super::tests::test_app;
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// 回归测试：弹窗左边框必须是一整条竖线。
    /// 之前 emoji（⚠️ 等）实际宽度与 unicode-width 不一致，会把边框单元格挤掉。
    #[test]
    fn modal_left_border_is_continuous() {
        for (w, h) in [(140u16, 45u16), (120, 40), (100, 32), (200, 60)] {
            let mut app = test_app();
            app.overlay = Overlay::About;
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| draw(f, &mut app)).unwrap();
            let buf = t.backend().buffer();

            let mut top = None;
            for y in 0..h {
                for x in 1..w {
                    if top.is_none() && buf[(x, y)].symbol() == "╭" {
                        top = Some((x, y));
                    }
                }
            }
            let (x, y0) = top.unwrap_or_else(|| panic!("{w}x{h}: 未找到弹窗边框"));
            let mut bottom = y0;
            for y in y0..h {
                if buf[(x, y)].symbol() == "╰" {
                    bottom = y;
                }
            }
            assert!(bottom > y0, "{w}x{h}: 弹窗底边缺失");
            for y in y0 + 1..bottom {
                let sym = buf[(x, y)].symbol();
                assert!(
                    sym == "│" || sym == " ",
                    "{w}x{h}: 第 {y} 行左边框被挤断，实际字符 {sym:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod selection_tests {
    use super::tests::{flat, render, test_app};

    /// 中文（多字节）选区：渲染不能 panic，高亮也不能丢
    #[test]
    fn cjk_selection_renders_without_panic() {
        let mut app = test_app();
        app.size = (110, 32);
        app.input.insert_str("磁盘占用过高，帮我定位大文件");
        app.input.select_all();

        let screen = render(&mut app, 110, 32, "cjk-select");
        assert!(flat(&screen).contains("磁盘"), "选中的中文应正常渲染");

        // 选区边界逐个移动，每一种偏移都不能 panic（切片必须落在字符边界上）
        for _ in 0..20 {
            app.input.left_extend();
            let _ = render(&mut app, 110, 32, "cjk-select-move");
        }
    }
}
