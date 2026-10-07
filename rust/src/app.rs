//! egui 界面：状态栏 + 配置表单 + 目标网络 + 运行日志。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{self, Carrier};
use crate::monitor::{self, Level, Shared};
use crate::secret;

/// 应用界面状态
pub struct App {
    shared: Arc<Mutex<Shared>>,
    config_path: PathBuf,
    show_logs: bool,
    password_visible: bool,
    /// 系统托盘（环境不支持时为 None）
    tray: Option<crate::tray::Tray>,
}

impl App {
    /// 构造界面并启动后台监控线程
    pub fn new(
        shared: Arc<Mutex<Shared>>,
        config_path: PathBuf,
        cc: &eframe::CreationContext<'_>,
    ) -> Self {
        monitor::spawn(shared.clone(), cc.egui_ctx.clone());
        let font = crate::fonts::install(&cc.egui_ctx);
        {
            let mut guard = lock(&shared);
            match font {
                Some(path) => guard.log(format!("已加载中文字体：{path}")),
                None => guard.log("警告：未找到系统中文字体，界面中文可能显示为方框"),
            }
        }
        let tray = crate::tray::spawn(shared.clone());
        if tray.is_some() {
            let mut guard = lock(&shared);
            guard.log("系统托盘已启动");
        }
        Self {
            shared,
            config_path,
            show_logs: true,
            password_visible: false,
            tray,
        }
    }

    /// 把界面上的配置写回磁盘（密码按「记住密码」决定是否加密保存）
    fn save_config(&self, cfg: &config::Config, password: &str) {
        let mut to_save = cfg.clone();
        to_save.pass_enc = if cfg.remember {
            secret::protect(password)
        } else {
            String::new()
        };
        let mut guard = lock(&self.shared);
        match config::save(&self.config_path, &to_save) {
            Ok(()) => {
                guard.cfg = to_save;
                guard.log(format!("配置已保存到 {}", self.config_path.display()));
            }
            Err(err) => guard.log(format!("保存配置失败：{err}")),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 定时刷新，让后台状态变化能反映到界面
        ctx.request_repaint_after(Duration::from_millis(400));

        let (mut cfg, mut password, status, level, mut paused, logs, visible, ssid) = {
            let guard = lock(&self.shared);
            (
                guard.cfg.clone(),
                guard.password.clone(),
                guard.status.clone(),
                guard.level,
                guard.paused,
                guard.logs.iter().cloned().collect::<Vec<_>>(),
                guard.visible.clone(),
                guard.ssid.clone(),
            )
        };
        let mut show_logs = self.show_logs;
        let mut password_visible = self.password_visible;
        let mut connect = false;
        let mut save = false;
        let mut scan = false;
        let mut quit = false;

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("校园网自连 v{}", env!("CARGO_PKG_VERSION")));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("当前网络：{ssid}"));
                });
            });
            ui.separator();

            ui.label(
                egui::RichText::new(&status)
                    .color(level_color(level))
                    .size(15.0),
            );
            ui.add_space(8.0);

            egui::Grid::new("config_grid")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label("运营商");
                    egui::ComboBox::from_id_salt("carrier")
                        .selected_text(cfg.carrier.label())
                        .show_ui(ui, |ui| {
                            for carrier in Carrier::ALL {
                                ui.selectable_value(&mut cfg.carrier, carrier, carrier.label());
                            }
                        });
                    ui.end_row();

                    ui.label("学号");
                    ui.add(
                        egui::TextEdit::singleline(&mut cfg.school_id)
                            .desired_width(200.0)
                            .hint_text("统一身份认证账号"),
                    );
                    ui.end_row();

                    ui.label("密码");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut password)
                                .password(!password_visible)
                                .desired_width(170.0),
                        );
                        ui.toggle_value(&mut password_visible, "显示");
                    });
                    ui.end_row();
                });

            ui.add_space(4.0);
            ui.checkbox(&mut cfg.remember, "记住密码");
            ui.checkbox(&mut cfg.auto, "自动登录（断线自动重连）");
            #[cfg(windows)]
            ui.checkbox(&mut cfg.boot, "开机自动启动");

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("目标网络");
                let selected = if cfg.target_ssid.is_empty() {
                    "不限定".to_string()
                } else {
                    cfg.target_ssid.clone()
                };
                egui::ComboBox::from_id_salt("target_ssid")
                    .selected_text(selected)
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut cfg.target_ssid, String::new(), "不限定");
                        for name in &visible {
                            ui.selectable_value(&mut cfg.target_ssid, name.clone(), name);
                        }
                    });
                if ui.button("重新扫描").clicked() {
                    scan = true;
                }
            });
            ui.checkbox(&mut cfg.switch_network, "不在目标网络时自动切换");

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add_sized([96.0, 30.0], egui::Button::new("连接"))
                    .clicked()
                {
                    connect = true;
                    save = true;
                }
                if ui
                    .add_sized([96.0, 30.0], egui::Button::new(if paused { "继续" } else { "暂停" }))
                    .clicked()
                {
                    paused = !paused;
                }
                if ui
                    .add_sized([96.0, 30.0], egui::Button::new("彻底退出"))
                    .clicked()
                {
                    quit = true;
                    save = true;
                }
            });

            ui.add_space(6.0);
            ui.separator();
            ui.checkbox(&mut show_logs, "显示运行日志");
            if show_logs {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for line in &logs {
                            ui.label(egui::RichText::new(line).monospace().size(12.0));
                        }
                    });
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("配置文件：{}", self.config_path.display()))
                    .size(11.0)
                    .weak(),
            );
        });

        {
            let mut guard = lock(&self.shared);
            guard.cfg = cfg.clone();
            guard.password = password.clone();
            guard.paused = paused;
            if scan {
                guard.scan_requested = true;
            }
            if connect {
                guard.manual_connect = true;
            }
        }
        // 处理托盘交互（Windows 下需在界面循环里轮询菜单事件）
        crate::tray::poll(&mut self.tray, &self.shared);
        // 托盘请求显示窗口 → 把窗口提到前台
        let show_window = {
            let mut guard = lock(&self.shared);
            std::mem::take(&mut guard.show_window)
        };
        if show_window {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.show_logs = show_logs;
        self.password_visible = password_visible;
        if save {
            self.save_config(&cfg, &password);
        }
        if quit {
            lock(&self.shared).quit = true;
            self.tray = None; // 关闭托盘图标
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// 状态级别对应的颜色
fn level_color(level: Level) -> egui::Color32 {
    match level {
        Level::Info => egui::Color32::from_rgb(0x90, 0x90, 0x90),
        Level::Ok => egui::Color32::from_rgb(0x2e, 0xa0, 0x43),
        Level::Warn => egui::Color32::from_rgb(0xd9, 0x7a, 0x0a),
        Level::Error => egui::Color32::from_rgb(0xd0, 0x3a, 0x3a),
    }
}

/// 加锁读取共享状态；锁中毒时照常取用
fn lock(shared: &Arc<Mutex<Shared>>) -> std::sync::MutexGuard<'_, Shared> {
    monitor::lock(shared)
}
