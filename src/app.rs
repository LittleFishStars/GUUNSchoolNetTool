//! egui 界面：状态栏 + 配置表单 + 目标网络 + 运行日志。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{self, Carrier};
use crate::dialogs::{self, ExitAnswer, MismatchAnswer};
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
    /// 正在等待「彻底关闭」确认
    confirming_exit: bool,
    /// 退出确认框里「不再提示」的勾选状态
    no_more_prompt: bool,
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
        // 配置里的开机自启与实际状态可能不一致（用户在系统里改过），以实际为准
        {
            let actual = crate::autostart::is_enabled();
            let mut guard = lock(&shared);
            if guard.cfg.boot != actual {
                guard.cfg.boot = actual;
                guard.log(format!(
                    "开机自启实际状态为{}，已同步配置",
                    if actual { "已启用" } else { "未启用" }
                ));
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
            confirming_exit: false,
            no_more_prompt: false,
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

        let (mut cfg, mut password, status, level, mut paused, logs, saved, visible, ssid, mismatch) = {
            let guard = lock(&self.shared);
            (
                guard.cfg.clone(),
                guard.password.clone(),
                guard.status.clone(),
                guard.level,
                guard.paused,
                guard.logs.iter().cloned().collect::<Vec<_>>(),
                guard.saved.clone(),
                guard.visible.clone(),
                guard.ssid.clone(),
                guard.mismatch.clone(),
            )
        };
        let mut show_logs = self.show_logs;
        let mut password_visible = self.password_visible;
        let mut connect = false;
        let mut save = false;
        let mut scan = false;
        let mut quit = false;
        let mut confirming_exit = self.confirming_exit;

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
            ui.checkbox(&mut cfg.boot, "开机自动启动");

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("目标网络");
                let selected = if cfg.target_ssid.is_empty() {
                    "自动（任意校园网）".to_string()
                } else {
                    cfg.target_ssid.clone()
                };
                egui::ComboBox::from_id_salt("target_ssid")
                    .selected_text(selected)
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut cfg.target_ssid,
                            String::new(),
                            "自动（任意校园网）",
                        );
                        // 只有系统已保存的网络才能作目标
                        for name in &saved {
                            ui.selectable_value(&mut cfg.target_ssid, name.clone(), name);
                        }
                    });
                if ui.button("重新扫描").clicked() {
                    scan = true;
                }
            });
            // 附近可见但未保存的网络仅供参照：连接它们需要密码，系统也不会记住
            let unsaved: Vec<&str> = visible
                .iter()
                .filter(|name| !saved.contains(name))
                .map(String::as_str)
                .collect();
            if !unsaved.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "附近可见（未保存，仅供参照）：{}",
                        unsaved.join("、")
                    ))
                    .size(11.0)
                    .weak(),
                );
            }
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
                    // 勾过「不再提示」就直接退，否则先弹确认框
                    if cfg.no_exit_confirm {
                        quit = true;
                        save = true;
                    } else {
                        confirming_exit = true;
                    }
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
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(crate::quotes::today())
                    .italics()
                    .size(12.0)
                    .color(egui::Color32::from_rgb(120, 90, 30)),
            );
        });

        let boot_changed = {
            let mut guard = lock(&self.shared);
            let changed = guard.cfg.boot != cfg.boot;
            guard.cfg = cfg.clone();
            guard.password = password.clone();
            guard.paused = paused;
            if scan {
                guard.scan_requested = true;
            }
            if connect {
                guard.manual_connect = true;
            }
            changed
        };
        // 用户刚改了「开机自动启动」→ 立即写入系统
        if boot_changed {
            match crate::autostart::set_enabled(cfg.boot) {
                Ok(()) => {
                    let text = if cfg.boot {
                        "已开启开机自动启动"
                    } else {
                        "已关闭开机自动启动"
                    };
                    lock(&self.shared).log(text);
                }
                Err(err) => lock(&self.shared).log(format!("设置开机自启失败：{err}")),
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
        // 后台请求保存（例如首次登录自动开启了开机自启）
        let save_requested = {
            let mut guard = lock(&self.shared);
            std::mem::take(&mut guard.save_requested)
        };
        if save_requested {
            self.save_config(&cfg, &password);
        }

        // 「网络与目标不符」提示框
        if let Some(prompt) = mismatch {
            match dialogs::mismatch(&ctx, &prompt.current, &prompt.target) {
                Some(MismatchAnswer::Pause) => {
                    let mut guard = lock(&self.shared);
                    guard.paused = true;
                    guard.mismatch = None;
                }
                Some(MismatchAnswer::Ignore) => lock(&self.shared).mismatch = None,
                Some(MismatchAnswer::Exit) => {
                    lock(&self.shared).mismatch = None;
                    quit = true;
                }
                None => {}
            }
        }

        // 「彻底关闭」确认框
        if confirming_exit {
            match dialogs::exit_confirm(&ctx, &mut self.no_more_prompt) {
                Some(ExitAnswer::Confirm { no_more_prompt }) => {
                    confirming_exit = false;
                    if no_more_prompt && !cfg.no_exit_confirm {
                        cfg.no_exit_confirm = true;
                        lock(&self.shared).cfg.no_exit_confirm = true;
                        self.save_config(&cfg, &password);
                    }
                    quit = true;
                }
                Some(ExitAnswer::Cancel) => confirming_exit = false,
                None => {}
            }
        }
        self.confirming_exit = confirming_exit;

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
