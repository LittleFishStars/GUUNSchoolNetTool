//! 界面与后台线程之间共享的状态，以及配套的访问辅助。
//!
//! 采用「共享状态 + 心跳线程」模型：界面只读快照、写完标志位即返回，
//! 监控线程与托盘也通过 [`lock`] / [`update`] 读写同一份 [`Shared`]。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::config::Config;

/// 状态文本的级别，决定界面颜色
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// 普通信息
    Info,
    /// 正常/成功
    Ok,
    /// 需要注意（等待、切换中）
    Warn,
    /// 失败
    Error,
}

/// 「网络与目标不符」的一次提示
#[derive(Debug, Clone)]
pub struct Mismatch {
    /// 当前连接的网络
    pub current: String,
    /// 配置里设定的目标网络
    pub target: String,
}

/// 日志保留条数
const MAX_LOGS: usize = 400;

/// 界面与监控线程共享的状态
pub struct Shared {
    /// 当前配置（界面可改）
    pub cfg: Config,
    /// 明文密码（界面可改；保存时经 [`crate::secret`] 处理）
    pub password: String,
    /// 状态栏文本
    pub status: String,
    /// 状态栏级别
    pub level: Level,
    /// 是否暂停（暂停后不检测、不登录、不切换）
    pub paused: bool,
    /// 是否请求退出
    pub quit: bool,
    /// 界面请求立即登录一次
    pub manual_connect: bool,
    /// 托盘请求显示窗口
    pub show_window: bool,
    /// 界面请求重新扫描 WiFi
    pub scan_requested: bool,
    /// 最近一次成功连接的时间文本
    pub last_success: Option<String>,
    /// 当前网络名
    pub ssid: String,
    /// 最近一次扫描到的 WiFi 列表
    pub visible: Vec<String>,
    /// 系统已保存的 WiFi 列表（只有已保存的才能作自动连接目标）
    pub saved: Vec<String>,
    /// 待用户处理的「网络与目标不符」提示
    pub mismatch: Option<Mismatch>,
    /// 请求界面把配置写回磁盘（例如首次登录自动开启了开机自启）
    pub save_requested: bool,
    /// 当前显示的每日名言（取自古文岛，失败时用内置文案）
    pub quote: String,
    /// 运行日志（最新在末尾）
    pub logs: VecDeque<String>,
}

impl Shared {
    /// 构造初始共享状态
    pub fn new(cfg: Config, password: String) -> Self {
        Self {
            cfg,
            password,
            status: "就绪".into(),
            level: Level::Info,
            paused: false,
            quit: false,
            manual_connect: false,
            show_window: false,
            scan_requested: false,
            last_success: None,
            ssid: String::new(),
            visible: Vec::new(),
            saved: Vec::new(),
            mismatch: None,
            save_requested: false,
            quote: String::new(),
            logs: VecDeque::new(),
        }
    }

    /// 更新状态栏文本
    pub fn set_status(&mut self, level: Level, text: impl Into<String>) {
        self.level = level;
        self.status = text.into();
    }

    /// 追加一条运行日志
    pub fn log(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.logs.push_back(format!("[{}] {text}", now_text()));
        while self.logs.len() > MAX_LOGS {
            self.logs.pop_front();
        }
    }
}

/// 加锁读取共享状态；锁中毒时照常取用
pub fn lock(shared: &Arc<Mutex<Shared>>) -> MutexGuard<'_, Shared> {
    match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 在锁内修改共享状态；锁中毒时也能继续工作
pub fn update<T>(shared: &Arc<Mutex<Shared>>, f: impl FnOnce(&mut Shared) -> T) -> T {
    f(&mut lock(shared))
}

/// SSID 为空时统一的显示文本
pub fn display_ssid(ssid: &str) -> String {
    if ssid.is_empty() {
        "无".to_string()
    } else {
        ssid.to_string()
    }
}

/// 当前本地时间（HH:MM:SS）
pub fn now_text() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空ssid显示为无() {
        assert_eq!(display_ssid(""), "无");
        assert_eq!(display_ssid("CMCC-GNNUN"), "CMCC-GNNUN");
    }

    #[test]
    fn 日志超出上限会淘汰最旧的() {
        let mut shared = Shared::new(Config::default(), String::new());
        for index in 0..MAX_LOGS + 10 {
            shared.log(format!("第 {index} 条"));
        }
        assert_eq!(shared.logs.len(), MAX_LOGS);
        assert!(shared.logs.front().unwrap().contains("第 10 条"));
    }
}
