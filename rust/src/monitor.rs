//! 后台监控状态机：轮询网络状态、自动登录、退避重试。
//!
//! 采用「共享状态 + 心跳线程」模型：界面与监控线程通过 [`Shared`] 交换数据，
//! 界面只读快照、写完标志位即返回，不阻塞渲染。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::net;
use crate::portal;

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
    /// 运行日志（最新在末尾）
    pub logs: VecDeque<String>,
}

/// 「网络与目标不符」的一次提示
#[derive(Debug, Clone)]
pub struct Mismatch {
    /// 当前连接的网络
    pub current: String,
    /// 配置里设定的目标网络
    pub target: String,
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
        let line = format!("[{}] {text}", now_text());
        self.logs.push_back(line);
        while self.logs.len() > MAX_LOGS {
            self.logs.pop_front();
        }
    }
}

/// 登录后快速重试的间隔（校园网未登录 / 正在切换）
const POLL_FAST: Duration = Duration::from_secs(2);
/// 稳定在线时的间隔（省电）
const POLL_SLOW: Duration = Duration::from_secs(10);
/// 登录冷却基数，随连续失败次数指数退避
const LOGIN_COOLDOWN: Duration = Duration::from_secs(3);
/// 退避上限
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// SSID 去抖：读到空值时，若这段时间内读到过就认为仍在该网络
const SSID_DEBOUNCE: Duration = Duration::from_secs(6);
/// 联网检测结果缓存时长
const NET_CACHE_TTL: Duration = Duration::from_secs(15);
/// 切换网络的最小间隔，避免抖动
const SWITCH_COOLDOWN: Duration = Duration::from_secs(20);
/// WiFi 扫描结果缓存时长
const SCAN_TTL: Duration = Duration::from_secs(60);
/// 日志保留条数
const MAX_LOGS: usize = 400;
/// 心跳间隔：保证暂停、退出、手动连接能被及时响应
const HEARTBEAT: Duration = Duration::from_millis(200);

/// 监控线程的私有状态
struct Monitor {
    /// 下次轮询前的间隔
    interval: Duration,
    /// 下次轮询的时间点
    next_tick: Instant,
    ssid_last: String,
    ssid_last_at: Option<Instant>,
    last_attempt: Option<Instant>,
    fail_count: u32,
    net_cache: Option<(bool, Instant)>,
    last_switch: Option<Instant>,
    visible_cache: Option<(Vec<String>, Instant)>,
    /// 「网络与目标不符」提示是否已弹过（同一网络只弹一次）
    mismatch_prompted: bool,
    /// 上次弹提示时的网络名
    mismatch_last_ssid: String,
    /// 启动时是否已有可用账号密码（用于「首次登录自动开启自启」）
    had_cred: bool,
}

impl Monitor {
    fn new() -> Self {
        Self {
            interval: POLL_FAST,
            next_tick: Instant::now(),
            ssid_last: String::new(),
            ssid_last_at: None,
            last_attempt: None,
            fail_count: 0,
            net_cache: None,
            last_switch: None,
            visible_cache: None,
            mismatch_prompted: false,
            mismatch_last_ssid: String::new(),
            had_cred: true, // 由 spawn 按启动时的实际情况覆盖
        }
    }

    /// 读取带去抖的当前 SSID
    fn stable_ssid(&mut self) -> String {
        let raw = net::current_ssid();
        if !raw.is_empty() {
            self.ssid_last = raw.clone();
            self.ssid_last_at = Some(Instant::now());
            return raw;
        }
        if let Some(at) = self.ssid_last_at
            && !self.ssid_last.is_empty()
            && at.elapsed() < SSID_DEBOUNCE
        {
            return self.ssid_last.clone();
        }
        self.ssid_last.clear();
        self.ssid_last_at = None;
        String::new()
    }

    /// 带缓存的联网检测
    fn internet_cached(&mut self, ttl: Duration) -> bool {
        if let Some((value, at)) = self.net_cache
            && at.elapsed() < ttl
        {
            return value;
        }
        let value = net::internet_ok();
        self.net_cache = Some((value, Instant::now()));
        value
    }

    /// 带缓存的 WiFi 扫描
    fn visible_cached(&mut self, force: bool) -> Vec<String> {
        if let Some((list, at)) = &self.visible_cache
            && !force
            && at.elapsed() < SCAN_TTL
        {
            return list.clone();
        }
        let list = net::visible_ssids(force);
        self.visible_cache = Some((list.clone(), Instant::now()));
        list
    }

    /// 连续失败后的退避时长
    fn backoff(&self) -> Duration {
        let factor = 2u32.saturating_pow(self.fail_count.min(4));
        (LOGIN_COOLDOWN * factor).min(MAX_BACKOFF)
    }

    /// 一次完整轮询
    fn tick(&mut self, shared: &Arc<Mutex<Shared>>) {
        let cfg = update(shared, |s| s.cfg.clone());
        let ssid = self.stable_ssid();
        update(shared, |s| s.ssid = display_ssid(&ssid));
        // 网络变了 → 允许重新弹一次「目标不符」提示
        if ssid != self.mismatch_last_ssid {
            self.mismatch_prompted = false;
        }

        // 设了目标网络但当前不在其上
        if !cfg.target_ssid.is_empty() && ssid != cfg.target_ssid {
            self.handle_off_target(shared, &cfg, &ssid);
            return;
        }

        // 不是校园网 → 无需登录
        if !cfg.is_campus_ssid(&ssid) {
            let online = !ssid.is_empty() && self.internet_cached(NET_CACHE_TTL);
            let current = display_ssid(&ssid);
            update(shared, |s| {
                if online {
                    s.set_status(Level::Ok, format!("已联网 · {current}（非校园网，无需登录）"));
                } else {
                    s.set_status(Level::Info, format!("未连接校园网（当前：{current}）"));
                }
            });
            self.interval = POLL_SLOW;
            return;
        }

        if !cfg.auto {
            self.interval = POLL_SLOW;
            return;
        }

        // ① 认证服务器可达吗？（约 800ms）不可达说明网络尚未就绪，快速重试以抢时机
        if !net::portal_reachable(&cfg.portal_host, cfg.portal_port) {
            update(shared, |s| {
                s.set_status(Level::Warn, format!("网络准备中（{ssid}）…"));
            });
            self.interval = POLL_FAST;
            return;
        }

        // ② 已经联网？（结果缓存 15 秒）
        if self.internet_cached(NET_CACHE_TTL) {
            update(shared, |s| {
                s.set_status(Level::Ok, format!("已联网 · {ssid} · {}", now_text()));
            });
            self.interval = POLL_SLOW;
            return;
        }

        // ③ 未联网 → 冷却（含指数退避）后登录
        self.interval = POLL_FAST;
        if let Some(at) = self.last_attempt
            && at.elapsed() < self.backoff()
        {
            return;
        }
        self.do_login(shared, &cfg, &ssid);
    }

    /// 当前不在目标网络上的处理
    fn handle_off_target(&mut self, shared: &Arc<Mutex<Shared>>, cfg: &Config, ssid: &str) {
        let current = display_ssid(ssid);
        let target = cfg.target_ssid.clone();
        if !cfg.switch_network {
            let online = self.internet_cached(NET_CACHE_TTL);
            update(shared, |s| {
                if online {
                    s.set_status(Level::Ok, format!("已联网 · {current}（自动切换已关闭）"));
                } else {
                    s.set_status(Level::Info, format!("当前：{current}（自动切换已关闭）"));
                }
            });
            self.interval = POLL_SLOW;
            return;
        }
        if self.visible_cached(false).iter().any(|name| name == &target) {
            // 目标在附近 → 切过去（带冷却，防抖动）
            if self
                .last_switch
                .is_none_or(|at| at.elapsed() >= SWITCH_COOLDOWN)
            {
                self.last_switch = Some(Instant::now());
                update(shared, |s| {
                    s.log(format!("切换目标网络：{target}（当前 {current}）"));
                });
                net::connect_ssid(&target);
            }
            update(shared, |s| {
                s.set_status(Level::Warn, format!("正在切换到「{target}」…"));
            });
            self.interval = POLL_FAST;
            return;
        }
        // 目标不在附近（例如连了手机热点）→ 不折腾、不抢网
        let online = self.internet_cached(NET_CACHE_TTL);
        update(shared, |s| {
            if online {
                s.set_status(Level::Ok, format!("已联网 · {current}（目标网「{target}」不在附近）"));
            } else {
                s.set_status(Level::Info, format!("当前：{current}（目标网「{target}」不在附近）"));
            }
        });
        self.interval = POLL_SLOW;
        // 当前有网络、目标是别的且不在附近 → 提示一次（暂停 / 忽略 / 彻底关闭）
        if !ssid.is_empty() && !self.mismatch_prompted {
            self.mismatch_prompted = true;
            self.mismatch_last_ssid = ssid.to_string();
            update(shared, |s| {
                s.log(format!("提示：当前「{current}」≠ 目标「{target}」且目标不在附近"));
                s.mismatch = Some(Mismatch {
                    current: current.clone(),
                    target: target.clone(),
                });
            });
        }
    }

    /// 执行一次登录
    fn do_login(&mut self, shared: &Arc<Mutex<Shared>>, cfg: &Config, ssid: &str) {
        let (user, password) = update(shared, |s| (s.cfg.school_id.clone(), s.password.clone()));
        if user.is_empty() || password.is_empty() {
            update(shared, |s| {
                s.set_status(Level::Error, "检测到断网，但没有可用账号密码，请填写");
            });
            return;
        }
        self.last_attempt = Some(Instant::now());
        let ip = net::local_ip(&cfg.portal_host, cfg.portal_port);
        update(shared, |s| {
            s.set_status(Level::Info, format!("正在登录（{ssid}）…"));
        });
        let result = portal::login(cfg, &user, &password, &ip);
        update(shared, |s| {
            s.log(format!(
                "登录 id={user} carrier={} ip={ip} => ok={} msg={}",
                cfg.carrier.label(),
                result.ok,
                result.msg
            ));
        });
        if result.ok {
            self.fail_count = 0;
            self.net_cache = Some((true, Instant::now()));
            let at = now_text();
            update(shared, |s| {
                let msg = result.msg.clone();
                s.last_success = Some(at.clone());
                s.set_status(Level::Ok, format!("✔ {at} 连接成功 · {msg}"));
            });
            self.apply_first_login_autostart(shared);
        } else {
            self.fail_count = self.fail_count.saturating_add(1);
            update(shared, |s| {
                if result.authfail {
                    s.set_status(
                        Level::Error,
                        format!("✘ 账号或密码错误，请检查后重试：{}", result.msg),
                    );
                } else {
                    s.set_status(Level::Error, format!("✘ 连接失败：{}", result.msg));
                }
            });
        }
    }

    /// 界面点「连接」：忽略冷却，立即尝试一次
    fn manual_connect(&mut self, shared: &Arc<Mutex<Shared>>) {
        self.last_attempt = None;
        self.fail_count = 0;
        self.net_cache = None;
        let cfg = update(shared, |s| s.cfg.clone());
        let ssid = self.stable_ssid();
        update(shared, |s| s.ssid = display_ssid(&ssid));

        // 目标网络与当前不符 → 先切过去
        if !cfg.target_ssid.is_empty() && ssid != cfg.target_ssid && cfg.switch_network {
            if self.visible_cached(false).iter().any(|n| n == &cfg.target_ssid) {
                let target = cfg.target_ssid.clone();
                update(shared, |s| {
                    s.set_status(Level::Warn, format!("正在切换到「{target}」…"));
                });
                net::connect_ssid(&target);
                self.interval = POLL_FAST;
                return;
            }
            let current = display_ssid(&ssid);
            update(shared, |s| {
                s.set_status(
                    Level::Error,
                    format!("目标网络「{}」不在附近，无法切换（当前：{current}）", cfg.target_ssid),
                );
            });
            return;
        }

        // 非校园网无需登录
        if !cfg.is_campus_ssid(&ssid) {
            let name = if ssid.is_empty() {
                "你的网络".to_string()
            } else {
                ssid.clone()
            };
            update(shared, |s| {
                s.set_status(Level::Ok, format!("已连接「{name}」（非校园网，无需登录）"));
            });
            return;
        }

        if !net::portal_reachable(&cfg.portal_host, cfg.portal_port) {
            update(shared, |s| {
                s.set_status(Level::Warn, "网络准备中，请稍候…");
            });
            self.interval = POLL_FAST;
            return;
        }
        self.do_login(shared, &cfg, &ssid);
    }

    /// 界面点「重新扫描」
    fn scan(&mut self, shared: &Arc<Mutex<Shared>>) {
        let saved = net::saved_ssids();
        let list = self.visible_cached(true);
        update(shared, |s| {
            s.log(format!(
                "扫描到 {} 个 WiFi（已保存 {} 个）",
                list.len(),
                saved.len()
            ));
            s.visible = list;
            s.saved = saved;
        });
    }

    /// 首次登录成功时自动开启开机自启（免除用户手动勾选）
    fn apply_first_login_autostart(&mut self, shared: &Arc<Mutex<Shared>>) {
        if self.had_cred {
            return;
        }
        self.had_cred = true;
        if update(shared, |s| s.cfg.boot) {
            return;
        }
        match crate::autostart::set_enabled(true) {
            Ok(()) => update(shared, |s| {
                s.cfg.boot = true;
                s.save_requested = true;
                s.log("首次登录成功 → 已自动开启「开机自动启动」");
            }),
            Err(err) => update(shared, |s| {
                s.log(format!("自动开启开机自启失败：{err}"));
            }),
        }
    }
}

/// 启动后台监控线程
pub fn spawn(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut monitor = Monitor::new();
        // 记录启动时是否已有可用账号密码（供「首次登录自动开启自启」判断）
        monitor.had_cred = update(&shared, |s| {
            !s.cfg.school_id.is_empty() && !s.password.is_empty()
        });
        // 启动自愈：把校园网配置批量设为「自动连接」，避免开机连不上校园网
        let keyword = update(&shared, |s| s.cfg.ssid_keyword.clone());
        let fixed = net::repair_profiles_auto(&keyword);
        if fixed > 0 {
            update(&shared, |s| {
                s.log(format!("启动自愈：已将 {fixed} 个校园网配置设为自动连接"));
            });
        }
        // 先填一次已保存列表（不强制扫描，避免拖慢启动）
        let saved = net::saved_ssids();
        update(&shared, |s| s.saved = saved);
        loop {
            let (quit, manual, scan) = update(&shared, |s| {
                let flags = (s.quit, s.manual_connect, s.scan_requested);
                s.manual_connect = false;
                s.scan_requested = false;
                flags
            });
            if quit {
                break;
            }
            if scan {
                monitor.scan(&shared);
            }
            if manual {
                monitor.manual_connect(&shared);
            }
            if Instant::now() >= monitor.next_tick {
                if update(&shared, |s| s.paused) {
                    update(&shared, |s| {
                        s.set_status(Level::Info, "已暂停：暂不检测网络、不自动登录、不切换网络");
                    });
                    monitor.interval = POLL_SLOW;
                } else {
                    monitor.tick(&shared);
                }
                monitor.next_tick = Instant::now() + monitor.interval;
            }
            ctx.request_repaint();
            std::thread::sleep(HEARTBEAT);
        }
    });
}

/// 加锁读取共享状态；锁中毒时照常取用
pub fn lock(shared: &Arc<Mutex<Shared>>) -> std::sync::MutexGuard<'_, Shared> {
    match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 在锁内修改共享状态；锁中毒时也能继续工作
fn update<T>(shared: &Arc<Mutex<Shared>>, f: impl FnOnce(&mut Shared) -> T) -> T {
    let mut guard = match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

/// SSID 为空时统一的显示文本
fn display_ssid(ssid: &str) -> String {
    if ssid.is_empty() {
        "无".to_string()
    } else {
        ssid.to_string()
    }
}

/// 当前本地时间（HH:MM:SS）
fn now_text() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 退避按失败次数递增且有上限() {
        let mut monitor = Monitor::new();
        assert_eq!(monitor.backoff(), Duration::from_secs(3));
        monitor.fail_count = 2;
        assert_eq!(monitor.backoff(), Duration::from_secs(12));
        monitor.fail_count = 9;
        // 2^min(9,4) = 16 → 3s * 16 = 48s；60s 上限留给今后调整基数时用
        assert_eq!(monitor.backoff(), Duration::from_secs(48));
    }

    #[test]
    fn 空ssid显示为无() {
        assert_eq!(display_ssid(""), "无");
        assert_eq!(display_ssid("CMCC-GNNUN"), "CMCC-GNNUN");
    }
}
