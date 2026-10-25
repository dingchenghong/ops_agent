pub mod chat;
pub mod config_view;
pub mod input;
pub mod modal;
pub mod theme;
pub mod util;

use ratatui::Frame;

use crate::app::{App, Screen};

pub fn draw(frame: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::Chat => chat::draw(frame, app),
        Screen::Config => config_view::draw(frame, app),
    }
    if app.help {
        modal::help(frame, app);
    }
    if app.approval.is_some() {
        modal::approval(frame, app);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::app::{Approval, Display, Status, ToolState};
    use crate::config::Config;

    fn render(app: &mut App, w: u16, h: u16, name: &str) -> String {
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
    fn flat(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// 构造测试用 App：内存数据库 + 真实 runtime（Agent 不会发起网络请求）
    fn test_app() -> App {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let db = rt.block_on(crate::db::Db::open_memory()).unwrap();
        let cfg = Config::default();
        let agent = crate::agent::AgentHandle::spawn(&rt, cfg.clone(), db.clone(), 1, Vec::new());
        App::new(cfg, agent, db, rt.handle().clone(), 1, Vec::new())
    }

    #[test]
    fn config_screen_renders() {
        let mut app = test_app();
        app.screen = Screen::Config;
        let screen = render(&mut app, 100, 30, "config");
        let flat = flat(&screen);
        assert!(flat.contains("设置"));
        assert!(flat.contains("DeepSeekAPIToken"));
        assert!(flat.contains("kube-context"));
    }

    #[test]
    fn chat_screen_renders_messages() {
        let mut app = test_app();
        app.screen = Screen::Chat;
        app.messages.push(Display::User("帮我看看磁盘".into()));
        app.messages.push(Display::Assistant {
            text: "先看磁盘占用：\n\n```bash\ndf -h\n```\n\n- 检查大文件\n- 确认日志目录".into(),
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
            output: "exit_code: 0\n--- stdout ---\nFilesystem  Size Used Avail Use%\n/dev/sda1   100G  95G   5G  95%".into(),
            ok: true,
        });
        app.messages.push(Display::Error("示例错误".into()));
        app.status = Status::Idle;
        app.input.insert_char('测');

        let screen = render(&mut app, 100, 30, "chat");
        let flat = flat(&screen);
        assert!(flat.contains("帮我看看磁盘"));
        assert!(flat.contains("run_shell"));
        assert!(flat.contains("df-h"));
    }

    #[test]
    fn approval_modal_renders() {
        let mut app = test_app();
        app.screen = Screen::Chat;
        app.approval = Some(Approval {
            id: "call_1".into(),
            name: "run_shell".into(),
            preview: "$ systemctl restart nginx".into(),
        });
        app.status = Status::WaitingApproval;
        let screen = render(&mut app, 100, 30, "approval");
        let flat = flat(&screen);
        assert!(flat.contains("需要你确认"));
        assert!(flat.contains("systemctlrestartnginx"));
    }

    #[test]
    fn help_modal_renders() {
        let mut app = test_app();
        app.help = true;
        let screen = render(&mut app, 100, 34, "help");
        let flat = flat(&screen);
        assert!(flat.contains("快捷键"));
        assert!(flat.contains("安全机制"));
    }
}
