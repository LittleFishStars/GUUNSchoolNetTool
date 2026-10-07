//! 后台监控状态机：轮询网络状态、自动登录、退避重试。
//!
//! 采用「共享状态 + 心跳线程」模型：界面与监控线程通过 [`crate::state::Shared`]
//! 交换数据，界面只读快照、写完标志位即返回，不阻塞渲染。
//!
//! 轮询决策见 [`decide`]，登录/扫描等主动动作见 [`actions`]。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::net;
use crate::state::{Level, Shared, update};

mod actions;
mod decide;

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
        // 读到空：若刚刚还读到过，就当作「仍在该网络」，容忍开机/重连时的瞬时闪断
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
}
