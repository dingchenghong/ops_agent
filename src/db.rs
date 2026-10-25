//! SQLite 持久化层（sqlx 0.9）
//!
//! 存两类东西：
//! - `settings`：键值对形式的配置（API Token、模型、kube 上下文…）
//! - `sessions` + `messages`：对话会话与逐条消息，重启后可继续
//!
//! 数据库默认位于 `~/.local/share/ops-agent/ops-agent.db`（遵循 XDG data 目录）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE TABLE IF NOT EXISTS sessions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    title      TEXT NOT NULL DEFAULT '',
    model      TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE TABLE IF NOT EXISTS messages (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role         TEXT NOT NULL,
    content      TEXT NOT NULL DEFAULT '',
    reasoning    TEXT NOT NULL DEFAULT '',
    tool_calls   TEXT,
    tool_call_id TEXT,
    tool_name    TEXT,
    ok           INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, id);
CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);
"#;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
    path: PathBuf,
}

/// 一条持久化后的消息（与 LLM 消息结构对齐，额外记录用于 UI 还原的字段）
#[derive(Debug, Clone, Default)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    pub reasoning: String,
    /// assistant 的 tool_calls 原始 JSON
    pub tool_calls: Option<String>,
    /// tool 消息对应的 tool_call_id
    pub tool_call_id: Option<String>,
    /// tool 消息对应的工具名
    pub tool_name: Option<String>,
    /// tool 执行是否成功
    pub ok: bool,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: i64,
    pub title: String,
    pub model: String,
    pub updated_at: String,
    pub messages: i64,
}

impl Db {
    pub fn path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ops-agent")
            .join("ops-agent.db")
    }

    pub async fn open() -> Result<Self> {
        Self::open_at(Self::path()).await
    }

    pub async fn open_at(path: PathBuf) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建数据目录失败：{}", dir.display()))?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(opts)
            .await
            .with_context(|| format!("打开数据库失败：{}", path.display()))?;

        let db = Self { pool, path };
        db.migrate().await?;
        Ok(db)
    }

    /// 内存库：数据库不可用时的降级方案（本次运行不落盘，但功能可用）
    pub async fn open_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .context("创建内存数据库失败")?;
        let db = Self { pool, path: PathBuf::from(":memory:") };
        db.migrate().await?;
        Ok(db)
    }

    pub fn file(&self) -> &Path {
        &self.path
    }

    pub fn is_memory(&self) -> bool {
        self.path.to_string_lossy() == ":memory:"
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::raw_sql(SCHEMA)
            .execute(&self.pool)
            .await
            .context("初始化数据库表结构失败")?;
        Ok(())
    }

    // ---------------- settings ----------------

    pub async fn load_settings(&self) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(&self.pool)
            .await
            .context("读取配置失败")?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<String, _>("key"), r.get::<String, _>("value")))
            .collect())
    }

    /// 全量写入配置（事务，幂等）
    pub async fn save_settings(&self, pairs: &[(String, String)]) -> Result<()> {
        let mut tx = self.pool.begin().await.context("开启事务失败")?;
        for (k, v) in pairs {
            sqlx::query(
                "INSERT INTO settings(key, value, updated_at) VALUES (?1, ?2, datetime('now','localtime'))
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            )
            .bind(k)
            .bind(v)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("写入配置项 {k} 失败"))?;
        }
        tx.commit().await.context("提交配置失败")?;
        Ok(())
    }

    // ---------------- sessions ----------------

    pub async fn create_session(&self, title: &str, model: &str) -> Result<i64> {
        let res = sqlx::query("INSERT INTO sessions(title, model) VALUES (?1, ?2)")
            .bind(title)
            .bind(model)
            .execute(&self.pool)
            .await
            .context("创建会话失败")?;
        Ok(res.last_insert_rowid())
    }

    pub async fn latest_session(&self) -> Result<Option<i64>> {
        let row = sqlx::query(
            "SELECT id FROM sessions ORDER BY datetime(updated_at) DESC, id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .context("查询会话失败")?;
        Ok(row.map(|r| r.get::<i64, _>("id")))
    }

    pub async fn touch_session(&self, id: i64, model: &str) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET updated_at = datetime('now','localtime'), model = ?2 WHERE id = ?1",
        )
        .bind(id)
        .bind(model)
        .execute(&self.pool)
        .await
        .ok();
        Ok(())
    }

    pub async fn set_title(&self, id: i64, title: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET title = ?2 WHERE id = ?1")
            .bind(id)
            .bind(title)
            .execute(&self.pool)
            .await
            .ok();
        Ok(())
    }

    pub async fn list_sessions(&self, limit: i64) -> Result<Vec<SessionInfo>> {
        let rows = sqlx::query(
            "SELECT s.id, s.title, s.model, s.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) AS cnt
             FROM sessions s
             ORDER BY datetime(s.updated_at) DESC, s.id DESC
             LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("读取会话列表失败")?;
        Ok(rows
            .into_iter()
            .map(|r| SessionInfo {
                id: r.get::<i64, _>("id"),
                title: r.get::<String, _>("title"),
                model: r.get::<String, _>("model"),
                updated_at: r.get::<String, _>("updated_at"),
                messages: r.get::<i64, _>("cnt"),
            })
            .collect())
    }

    // ---------------- messages ----------------

    pub async fn append_message(&self, session_id: i64, m: &StoredMessage) -> Result<()> {
        sqlx::query(
            "INSERT INTO messages(session_id, role, content, reasoning, tool_calls, tool_call_id, tool_name, ok)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(session_id)
        .bind(&m.role)
        .bind(&m.content)
        .bind(&m.reasoning)
        .bind(&m.tool_calls)
        .bind(&m.tool_call_id)
        .bind(&m.tool_name)
        .bind(if m.ok { 1_i64 } else { 0_i64 })
        .execute(&self.pool)
        .await
        .context("写入消息失败")?;
        Ok(())
    }

    pub async fn load_messages(&self, session_id: i64) -> Result<Vec<StoredMessage>> {
        let rows = sqlx::query(
            "SELECT role, content, reasoning, tool_calls, tool_call_id, tool_name, ok
             FROM messages WHERE session_id = ?1 ORDER BY id ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .context("读取历史消息失败")?;

        Ok(rows
            .into_iter()
            .map(|r| StoredMessage {
                role: r.get::<String, _>("role"),
                content: r.get::<String, _>("content"),
                reasoning: r.get::<String, _>("reasoning"),
                tool_calls: r.get::<Option<String>, _>("tool_calls"),
                tool_call_id: r.get::<Option<String>, _>("tool_call_id"),
                tool_name: r.get::<Option<String>, _>("tool_name"),
                ok: r.get::<i64, _>("ok") != 0,
            })
            .collect())
    }

    pub async fn clear_messages(&self, session_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM messages WHERE session_id = ?1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .context("清空会话失败")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settings_round_trip() {
        let db = Db::open_memory().await.unwrap();
        assert!(db.load_settings().await.unwrap().is_empty());

        let pairs = vec![
            ("api_key".to_string(), "sk-test".to_string()),
            ("model".to_string(), "deepseek-chat".to_string()),
        ];
        db.save_settings(&pairs).await.unwrap();
        // 二次写入应更新而不是重复插入
        db.save_settings(&[("api_key".to_string(), "sk-new".to_string())])
            .await
            .unwrap();

        let got = db.load_settings().await.unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.contains(&("api_key".to_string(), "sk-new".to_string())));
    }

    #[tokio::test]
    async fn messages_round_trip() {
        let db = Db::open_memory().await.unwrap();
        let id = db.create_session("测试", "deepseek-chat").await.unwrap();
        assert_eq!(db.latest_session().await.unwrap(), Some(id));

        db.append_message(
            id,
            &StoredMessage { role: "user".into(), content: "磁盘满了".into(), ok: true, ..Default::default() },
        )
        .await
        .unwrap();
        db.append_message(
            id,
            &StoredMessage {
                role: "tool".into(),
                content: "status: success\nok".into(),
                tool_name: Some("run_shell".into()),
                tool_call_id: Some("call_1".into()),
                ok: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let msgs = db.load_messages(id).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].tool_name.as_deref(), Some("run_shell"));

        let list = db.list_sessions(10).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].messages, 2);

        db.clear_messages(id).await.unwrap();
        assert!(db.load_messages(id).await.unwrap().is_empty());
    }
}
