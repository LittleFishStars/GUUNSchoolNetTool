//! 轮询决策：判断此刻该等待、切换网络、登录，还是放慢节奏。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{Monitor, NET_CACHE_TTL, POLL_FAST, POLL_SLOW, SWITCH_COOLDOWN};
use crate::config::Config;
use crate::net;
use crate::state::{Level, Mismatch, Shared, display_ssid, now_text, update};

impl Monitor {
    /// 一次完整轮询
    pub(super) fn tick(&mut self, shared: &Arc<Mutex<Shared>>) {
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

        // ① 认证服务器可达吗？不可达说明网络尚未就绪，快速重试以抢时机
        if !net::portal_reachable(&cfg.portal_host, cfg.portal_port) {
            update(shared, |s| {
                s.set_status(Level::Warn, format!("网络准备中（{ssid}）…"));
            });
            self.interval = POLL_FAST;
            return;
        }

        // ② 已经联网？（结果有 15 秒缓存）
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

        // 用户关掉了「自动切换」→ 如实报告当前状态，不抢网
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

        // 目标在附近 → 切过去（带冷却，防抖动）
        if self.visible_cached(false).iter().any(|name| name == &target) {
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
                s.set_status(
                    Level::Ok,
                    format!("已联网 · {current}（目标网「{target}」不在附近）"),
                );
            } else {
                s.set_status(
                    Level::Info,
                    format!("当前：{current}（目标网「{target}」不在附近）"),
                );
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
}
