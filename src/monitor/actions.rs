//! 主动动作：登录、手动连接、扫描 WiFi，以及首次登录自动开启自启。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{Monitor, POLL_FAST};
use crate::config::Config;
use crate::net;
use crate::portal;
use crate::state::{Level, Shared, display_ssid, now_text, update};

impl Monitor {
    /// 执行一次登录
    pub(super) fn do_login(&mut self, shared: &Arc<Mutex<Shared>>, cfg: &Config, ssid: &str) {
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
            return;
        }

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

    /// 界面点「连接」：忽略冷却，立即尝试一次
    pub(super) fn manual_connect(&mut self, shared: &Arc<Mutex<Shared>>) {
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
                    format!(
                        "目标网络「{}」不在附近，无法切换（当前：{current}）",
                        cfg.target_ssid
                    ),
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
    pub(super) fn scan(&mut self, shared: &Arc<Mutex<Shared>>) {
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
