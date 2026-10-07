//! 配置读写。
//!
//! 字段名沿用 PowerShell 版（`config.json`）的 camelCase 命名，便于复用已有配置；
//! Rust 版新增的字段带默认值，旧配置文件仍可正常加载。
//!
//! 配置文件的查找顺序：
//! 1. `--config <路径>` 命令行参数
//! 2. 可执行文件同目录的 `config.json`（便携模式，与 PowerShell 版一致）
//! 3. 用户配置目录 `guunnet/config.json`（Linux `~/.config/guunnet`）

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 运营商。JSON 中使用中文字面量，与 PowerShell 版配置兼容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Carrier {
    #[serde(rename = "移动")]
    Mobile,
    #[default]
    #[serde(rename = "电信")]
    Telecom,
    #[serde(rename = "联通")]
    Unicom,
}

impl Carrier {
    /// 全部运营商，供界面下拉框遍历
    pub const ALL: [Carrier; 3] = [Carrier::Mobile, Carrier::Telecom, Carrier::Unicom];

    /// ePortal 账号后缀：电信 `@dx`、联通 `@lt`、移动无后缀
    pub fn suffix(self) -> &'static str {
        match self {
            Carrier::Mobile => "",
            Carrier::Telecom => "@dx",
            Carrier::Unicom => "@lt",
        }
    }

    /// 界面显示名
    pub fn label(self) -> &'static str {
        match self {
            Carrier::Mobile => "移动",
            Carrier::Telecom => "电信",
            Carrier::Unicom => "联通",
        }
    }
}

/// 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// 学号（统一身份认证账号）
    pub school_id: String,
    /// 受保护的密码密文，见 [`crate::secret`]
    pub pass_enc: String,
    /// 运营商
    pub carrier: Carrier,
    /// 是否记住密码
    pub remember: bool,
    /// 是否自动登录（断线重连）
    pub auto: bool,
    /// 是否开机自启（仅 Windows 生效）
    pub boot: bool,
    /// 目标网络 SSID，空表示不限定
    pub target_ssid: String,
    /// 当前不在目标网络时是否自动切换
    pub switch_network: bool,
    /// 彻底退出时是否跳过确认框
    pub no_exit_confirm: bool,
    /// 启动时自动检查新版本
    pub auto_update: bool,
    /// 认证服务器地址
    pub portal_host: String,
    /// 认证服务器端口
    pub portal_port: u16,
    /// 校园网 SSID 关键词，用于判定"当前是否在校园网"
    pub ssid_keyword: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            school_id: String::new(),
            pass_enc: String::new(),
            carrier: Carrier::default(),
            remember: true,
            auto: true,
            boot: false,
            target_ssid: String::new(),
            switch_network: true,
            no_exit_confirm: false,
            auto_update: true,
            portal_host: "10.10.90.2".into(),
            portal_port: 801,
            ssid_keyword: "CMCC-GNNUN".into(),
        }
    }
}

impl Config {
    /// 认证服务器基址，如 `http://10.10.90.2:801`
    pub fn portal_base(&self) -> String {
        format!("http://{}:{}", self.portal_host, self.portal_port)
    }

    /// 当前 SSID 是否属于校园网（按关键词匹配）
    pub fn is_campus_ssid(&self, ssid: &str) -> bool {
        !ssid.is_empty() && ssid.contains(&self.ssid_keyword)
    }
}

/// 解析配置文件路径
pub fn config_path() -> PathBuf {
    if let Some(explicit) = explicit_config_arg() {
        return explicit;
    }
    if let Some(dir) = portable_dir() {
        return dir.join("config.json");
    }
    let base = directories::ProjectDirs::from("", "", "guunnet")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("config.json")
}

/// 从命令行读取 `--config <路径>`
fn explicit_config_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--config" || arg == "-c" {
            return args.next().map(PathBuf::from);
        }
        if let Some(rest) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(rest));
        }
    }
    None
}

/// 便携模式判定：程序同目录已有 `config.json` 或 `portable` 标记文件
fn portable_dir() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let has_cfg = dir.join("config.json").exists();
    let has_marker = dir.join("portable").exists();
    (has_cfg || has_marker).then_some(dir)
}

/// 读取配置；文件不存在或损坏时返回默认配置
pub fn load(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 写入配置（自动创建父目录）
pub fn save(path: &Path, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(cfg).context("序列化配置失败")?;
    std::fs::write(path, text).with_context(|| format!("写入配置失败: {}", path.display()))
}
