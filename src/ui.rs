//! 主界面渲染。
//!
//! 只负责画界面，并把「用户这一帧做了什么」通过 [`Actions`] 返回给
//! [`crate::app`]；这里不直接改动后台状态，界面与逻辑因此可以各自演进。

use std::path::Path;

use crate::config::{Carrier, Config};
use crate::state::{Level, UpdateState};

/// 正常/成功色
const OK_COLOR: egui::Color32 = egui::Color32::from_rgb(0x2e, 0xa0, 0x43);
/// 失败色
const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);

/// 界面自身的临时状态（不写入配置文件，重启即复位）
#[derive(Debug, Clone, Copy)]
pub struct UiState {
    /// 是否展开运行日志
    pub show_logs: bool,
    /// 密码是否明文显示
    pub password_visible: bool,
    /// 是否正在等待「彻底关闭」确认
    pub confirming_exit: bool,
    /// 退出确认框里「不再提示」的勾选状态
    pub no_more_prompt: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            show_logs: true,
            password_visible: false,
            confirming_exit: false,
            no_more_prompt: false,
        }
    }
}

/// 渲染所需的只读数据
pub struct Snapshot<'a> {
    /// 状态栏文本
    pub status: &'a str,
    /// 状态栏级别
    pub level: Level,
    /// 当前网络名
    pub ssid: &'a str,
    /// 每日名言（空串表示尚未取到）
    pub quote: &'a str,
    /// 系统已保存的 WiFi（可作目标）
    pub saved: &'a [String],
    /// 附近可见的 WiFi（仅参照）
    pub visible: &'a [String],
    /// 运行日志
    pub logs: &'a [String],
    /// 配置文件路径
    pub config_path: &'a Path,
    /// 自动更新的当前状态
    pub update: Option<&'a UpdateState>,
}

/// 用户在这一帧做出的操作
#[derive(Debug, Default, Clone, Copy)]
pub struct Actions {
    /// 点了「连接」（连接后窗口会自动收起，见 [`crate::app`]）
    pub connect: bool,
    /// 点了「彻底退出」
    pub quit: bool,
    /// 点了「重新扫描」
    pub scan: bool,
    /// 点了「检查更新」
    pub check_update: bool,
    /// 点了「立即更新」
    pub apply_update: bool,
}

/// 画主界面，返回本帧的用户操作
pub fn draw(
    ui: &mut egui::Ui,
    snapshot: &Snapshot<'_>,
    cfg: &mut Config,
    password: &mut String,
    ui_state: &mut UiState,
    paused: &mut bool,
) -> Actions {
    let mut actions = Actions::default();

    header(ui, snapshot);
    ui.label(
        egui::RichText::new(snapshot.status)
            .color(level_color(snapshot.level))
            .size(15.0),
    );
    ui.add_space(8.0);

    credential_form(ui, cfg, password, ui_state);
    toggles(ui, cfg);
    target_network(ui, snapshot, cfg, &mut actions);
    control_buttons(ui, cfg, paused, ui_state, &mut actions);
    update_banner(ui, snapshot, &mut actions);
    log_panel(ui, snapshot, ui_state);
    footer(ui, snapshot);

    actions
}

/// 标题行：程序版本 + 当前网络
fn header(ui: &mut egui::Ui, snapshot: &Snapshot<'_>) {
    ui.horizontal(|ui| {
        ui.heading(format!("校园网自连 v{}", env!("CARGO_PKG_VERSION")));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("当前网络：{}", snapshot.ssid));
        });
    });
    ui.separator();
}

/// 运营商 / 学号 / 密码
fn credential_form(ui: &mut egui::Ui, cfg: &mut Config, password: &mut String, state: &mut UiState) {
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
                    egui::TextEdit::singleline(password)
                        .password(!state.password_visible)
                        .desired_width(170.0),
                );
                ui.toggle_value(&mut state.password_visible, "显示");
            });
            ui.end_row();
        });
}

/// 行为开关
fn toggles(ui: &mut egui::Ui, cfg: &mut Config) {
    ui.add_space(4.0);
    ui.checkbox(&mut cfg.remember, "记住密码");
    ui.checkbox(&mut cfg.auto, "自动登录（断线自动重连）");
    ui.checkbox(&mut cfg.boot, "开机自动启动");
}

/// 目标网络选择与附近网络提示
fn target_network(
    ui: &mut egui::Ui,
    snapshot: &Snapshot<'_>,
    cfg: &mut Config,
    actions: &mut Actions,
) {
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
                ui.selectable_value(&mut cfg.target_ssid, String::new(), "自动（任意校园网）");
                // 只有系统已保存的网络才能作目标
                for name in snapshot.saved {
                    ui.selectable_value(&mut cfg.target_ssid, name.clone(), name);
                }
            });
        if ui.button("重新扫描").clicked() {
            actions.scan = true;
        }
    });

    // 附近可见但未保存的网络仅供参照：连接它们需要密码，系统也不会记住
    let unsaved: Vec<&str> = snapshot
        .visible
        .iter()
        .filter(|name| !snapshot.saved.contains(name))
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
}

/// 连接 / 暂停 / 退出
fn control_buttons(
    ui: &mut egui::Ui,
    cfg: &Config,
    paused: &mut bool,
    state: &mut UiState,
    actions: &mut Actions,
) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui
            .add_sized([96.0, 30.0], egui::Button::new("连接"))
            .on_hover_text("点击后窗口自动收起，登录与断线重连在后台继续；再双击程序可唤出窗口")
            .clicked()
        {
            actions.connect = true;
        }
        let pause_label = if *paused { "继续" } else { "暂停" };
        if ui
            .add_sized([96.0, 30.0], egui::Button::new(pause_label))
            .clicked()
        {
            *paused = !*paused;
        }
        if ui
            .add_sized([96.0, 30.0], egui::Button::new("彻底退出"))
            .clicked()
        {
            // 勾过「不再提示」就直接退出，否则交由外层弹确认框
            if cfg.no_exit_confirm {
                actions.quit = true;
            } else {
                state.confirming_exit = true;
            }
        }
        if ui
            .add_sized([96.0, 30.0], egui::Button::new("检查更新"))
            .clicked()
        {
            actions.check_update = true;
        }
    });
}

/// 运行日志
fn log_panel(ui: &mut egui::Ui, snapshot: &Snapshot<'_>, state: &mut UiState) {
    ui.add_space(6.0);
    ui.separator();
    ui.checkbox(&mut state.show_logs, "显示运行日志");
    if state.show_logs {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .max_height(150.0)
            .show(ui, |ui| {
                for line in snapshot.logs {
                    ui.label(egui::RichText::new(line).monospace().size(12.0));
                }
            });
    }
}

/// 自动更新提示：检查中 / 有新版本 / 下载进度 / 已就绪 / 失败
fn update_banner(ui: &mut egui::Ui, snapshot: &Snapshot<'_>, actions: &mut Actions) {
    let Some(state) = snapshot.update else {
        return;
    };
    ui.add_space(6.0);
    match state {
        UpdateState::Checking => {
            ui.label(egui::RichText::new("正在检查更新…").size(12.0).weak());
        }
        UpdateState::Available(info) => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("发现新版本 v{}", info.version))
                        .size(13.0)
                        .color(OK_COLOR),
                );
                if ui.button("立即更新").clicked() {
                    actions.apply_update = true;
                }
            });
            if !info.notes.is_empty() {
                ui.label(
                    egui::RichText::new(first_lines(&info.notes, 3))
                        .size(11.0)
                        .weak(),
                );
            }
        }
        UpdateState::Downloading(done, total) => {
            let text = if *total > 0 {
                format!(
                    "正在下载更新… {} / {} KB",
                    done / 1024,
                    total / 1024
                )
            } else {
                format!("正在下载更新… {} KB", done / 1024)
            };
            ui.label(egui::RichText::new(text).size(12.0));
            if *total > 0 {
                let ratio = (*done as f32 / *total as f32).clamp(0.0, 1.0);
                ui.add(egui::ProgressBar::new(ratio).desired_width(240.0));
            }
        }
        UpdateState::Ready(version) => {
            ui.label(
                egui::RichText::new(format!("✔ 已更新到 v{version}，请重启程序生效"))
                    .size(13.0)
                    .color(OK_COLOR),
            );
        }
        UpdateState::Failed(err) => {
            ui.label(
                egui::RichText::new(format!("✘ 更新失败：{err}"))
                    .size(12.0)
                    .color(ERROR_COLOR),
            );
        }
    }
}

/// 取多行文本的前若干行（更新说明可能很长）
fn first_lines(text: &str, count: usize) -> String {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .take(count)
        .collect::<Vec<_>>()
        .join("  ")
}

/// 底部：配置文件路径 + 每日名言
fn footer(ui: &mut egui::Ui, snapshot: &Snapshot<'_>) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(format!("配置文件：{}", snapshot.config_path.display()))
            .size(11.0)
            .weak(),
    );
    if !snapshot.quote.is_empty() {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(snapshot.quote)
                .italics()
                .size(12.0)
                .color(egui::Color32::from_rgb(120, 90, 30)),
        );
    }
}

/// 状态级别对应的颜色
fn level_color(level: Level) -> egui::Color32 {
    match level {
        Level::Info => egui::Color32::from_rgb(0x90, 0x90, 0x90),
        Level::Ok => OK_COLOR,
        Level::Warn => egui::Color32::from_rgb(0xd9, 0x7a, 0x0a),
        Level::Error => ERROR_COLOR,
    }
}
