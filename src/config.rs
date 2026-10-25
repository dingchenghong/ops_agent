//! 配置模型，以及它与 SQLite `settings` 表之间的映射（见 `db.rs`）
//!
//! API Token 支持两种来源（优先级从高到低）：
//! 1. 数据库中的 `api_key`
//! 2. 环境变量 `DEEPSEEK_API_KEY`

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-chat";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// DeepSeek API Token
    pub api_key: String,
    /// 接口地址（OpenAI 兼容）
    pub base_url: String,
    /// 模型名称：deepseek-chat / deepseek-reasoner
    pub model: String,
    pub temperature: f64,
    pub max_tokens: u32,
    /// 单轮对话中最多允许的工具调用轮次
    pub max_tool_rounds: u32,
    /// 跳过所有操作确认（危险，仅建议在测试环境使用）
    pub auto_approve_all: bool,
    /// kubectl 使用的 context，留空表示使用当前 context
    pub kubectl_context: String,
    /// kubectl 默认 namespace，留空表示 default
    pub kubectl_namespace: String,
    /// 单条 shell 命令默认超时（秒）
    pub shell_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            temperature: 0.3,
            max_tokens: 4096,
            max_tool_rounds: 16,
            auto_approve_all: false,
            kubectl_context: String::new(),
            kubectl_namespace: String::new(),
            shell_timeout_secs: 60,
        }
    }
}

impl Config {
    /// 修正空值 / 越界的配置
    pub fn normalized(mut self) -> Self {
        if self.base_url.trim().is_empty() {
            self.base_url = DEFAULT_BASE_URL.to_string();
        }
        if self.model.trim().is_empty() {
            self.model = DEFAULT_MODEL.to_string();
        }
        if self.max_tool_rounds == 0 {
            self.max_tool_rounds = 16;
        }
        if self.shell_timeout_secs == 0 {
            self.shell_timeout_secs = 60;
        }
        if !(0.0..=2.0).contains(&self.temperature) {
            self.temperature = 0.3;
        }
        self
    }

    /// 转成 `settings` 表的键值对
    pub fn to_settings(&self) -> Vec<(String, String)> {
        let kv = |k: &str, v: String| (k.to_string(), v);
        vec![
            kv("api_key", self.api_key.trim().to_string()),
            kv("base_url", self.base_url.clone()),
            kv("model", self.model.clone()),
            kv("temperature", format!("{:.2}", self.temperature)),
            kv("max_tokens", self.max_tokens.to_string()),
            kv("max_tool_rounds", self.max_tool_rounds.to_string()),
            kv("auto_approve_all", self.auto_approve_all.to_string()),
            kv("kubectl_context", self.kubectl_context.clone()),
            kv("kubectl_namespace", self.kubectl_namespace.clone()),
            kv("shell_timeout_secs", self.shell_timeout_secs.to_string()),
        ]
    }

    /// 从 `settings` 表还原；非法值会被跳过并打印告警
    pub fn from_settings(pairs: &[(String, String)]) -> Self {
        let mut cfg = Self::default();
        for (k, v) in pairs {
            if let Err(e) = cfg.apply(k, v) {
                eprintln!("忽略非法配置项 {k}：{e}");
            }
        }
        cfg.normalized()
    }

    fn apply(&mut self, key: &str, value: &str) -> Result<()> {
        let v = value.trim();
        match key {
            "api_key" => self.api_key = v.to_string(),
            "base_url" => self.base_url = v.to_string(),
            "model" => self.model = v.to_string(),
            "temperature" => self.temperature = v.parse()?,
            "max_tokens" => self.max_tokens = v.parse()?,
            "max_tool_rounds" => self.max_tool_rounds = v.parse()?,
            "auto_approve_all" => {
                self.auto_approve_all = matches!(v, "true" | "1" | "yes" | "on")
            }
            "kubectl_context" => self.kubectl_context = v.to_string(),
            "kubectl_namespace" => self.kubectl_namespace = v.to_string(),
            "shell_timeout_secs" => self.shell_timeout_secs = v.parse()?,
            _ => {}
        }
        Ok(())
    }

    /// 0.1 版本用 TOML 保存配置，这里只用于一次性迁移
    pub fn legacy_toml_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ops-agent")
            .join("config.toml")
    }

    pub fn legacy_toml() -> Option<Self> {
        let path = Self::legacy_toml_path();
        let text = std::fs::read_to_string(&path).ok()?;
        match toml::from_str::<Config>(&text) {
            Ok(cfg) => Some(cfg.normalized()),
            Err(e) => {
                eprintln!("旧配置文件解析失败（{}）：{e}", path.display());
                None
            }
        }
    }

    /// 实际生效的 Token（配置为空时回落到环境变量）
    pub fn effective_key(&self) -> String {
        let k = self.api_key.trim();
        if !k.is_empty() {
            return k.to_string();
        }
        std::env::var("DEEPSEEK_API_KEY")
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    pub fn has_key(&self) -> bool {
        !self.effective_key().is_empty()
    }

    /// kubectl 使用的 namespace
    pub fn namespace(&self) -> &str {
        let ns = self.kubectl_namespace.trim();
        if ns.is_empty() {
            "default"
        } else {
            ns
        }
    }
}

pub fn mask_secret(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len().max(1));
    }
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars.iter().skip(chars.len().saturating_sub(4)).collect();
    format!("{head}****{tail}")
}
