//! OpsAgent —— 面向 Linux 运维人员的 AI Agent（TUI）
//!
//! 用法：
//! ```text
//! cargo run --release
//! ```
//! 首次启动会进入配置界面，填写 DeepSeek API Token 即可开始使用。
//! 配置与会话存放在 SQLite（`~/.local/share/ops-agent/ops-agent.db`）。

mod agent;
mod app;
mod config;
mod db;
mod llm;
mod prompt;
mod tools;
mod ui;

use anyhow::Result;

use crate::agent::AgentHandle;
use crate::config::Config;
use crate::db::Db;

fn main() -> Result<()> {
    // 后台运行时：Agent 的网络访问、命令执行、数据库读写都在这里跑
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .thread_name("ops-agent")
        .build()?;

    // 数据库：打不开就降级为内存库，功能保留但不落盘
    let db = match runtime.block_on(Db::open()) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("⚠ 无法打开数据库（{e:#}），本次运行的配置与会话不会保存");
            runtime.block_on(Db::open_memory())?
        }
    };

    // 配置：优先读数据库；首次运行时尝试迁移旧版 config.toml
    let settings = runtime.block_on(db.load_settings()).unwrap_or_default();
    let (cfg, imported_legacy) = if settings.is_empty() {
        match Config::legacy_toml() {
            Some(cfg) => (cfg, true),
            None => (Config::default(), false),
        }
    } else {
        (Config::from_settings(&settings), false)
    };
    if imported_legacy {
        let _ = runtime.block_on(db.save_settings(&cfg.to_settings()));
    }

    // 会话：恢复最近一次，没有则新建
    let session_id = match runtime.block_on(db.latest_session())? {
        Some(id) => id,
        None => runtime.block_on(db.create_session("新会话", &cfg.model))?,
    };
    let stored = runtime.block_on(db.load_messages(session_id)).unwrap_or_default();
    let history = agent::history_from_stored(&stored);

    let agent = AgentHandle::spawn(&runtime, cfg.clone(), db.clone(), session_id, history);
    let mut app = app::App::new(cfg, agent, db, runtime.handle().clone(), session_id, stored);

    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();

    drop(runtime);
    result
}
