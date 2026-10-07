//! 校园网自连 —— Rust + egui 重写版（跨平台）。
//!
//! 对应 PowerShell 版 v1.0.6 的核心功能：ePortal 自动登录、断线自动重连、
//! 自适应轮询、目标网络选择、配置读写。
//!
//! - Windows 专属部分（DPAPI 密码保护、netsh WiFi 操作等）通过 `cfg` 条件编译启用；
//! - 其他平台使用 base64 混淆与 NetworkManager(`nmcli`) 实现同等能力。

mod app;
mod autostart;
mod config;
mod dialogs;
mod fonts;
mod monitor;
mod net;
mod portal;
mod quotes;
mod secret;
mod tray;

use std::sync::{Arc, Mutex};

fn main() -> eframe::Result {
    let path = config::config_path();
    let cfg = config::load(&path);
    let password = secret::unprotect(&cfg.pass_enc);
    let shared = Arc::new(Mutex::new(monitor::Shared::new(cfg, password)));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([500.0, 620.0])
            .with_min_inner_size([440.0, 520.0])
            .with_title("校园网自连"),
        ..Default::default()
    };
    eframe::run_native(
        "校园网自连",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(shared, path, cc)))),
    )
}
