//! 应用装配：初始化各后台子系统，并驱动界面与共享状态之间的交互。
//!
//! 界面绘制在 [`crate::ui`]，监控逻辑在 [`crate::monitor`]；
//! 这里只做「取快照 → 画界面 → 写回结果」的编排。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{self, Config};
use crate::dialogs::{self, ExitAnswer, MismatchAnswer};
use crate::monitor;
use crate::quotes;
use crate::secret;
use crate::state::{Mismatch, Shared, UpdateState, lock};
use crate::ui::{self, Snapshot, UiState};

/// 界面刷新间隔
const REFRESH: Duration = Duration::from_millis(400);

/// eframe 应用的根对象
pub struct App {
    shared: Arc<Mutex<Shared>>,
    config_path: PathBuf,
    /// 界面自身的临时状态
    ui_state: UiState,
    /// 系统托盘（环境不支持时为 None）
    tray: Option<crate::tray::Tray>,
}

impl App {
    /// 构造界面并启动各后台子系统
    pub fn new(
        shared: Arc<Mutex<Shared>>,
        config_path: PathBuf,
        cc: &eframe::CreationContext<'_>,
    ) -> Self {
        monitor::spawn(shared.clone(), cc.egui_ctx.clone());
        install_font(&shared, cc);
        sync_autostart(&shared);
        spawn_quote_fetch(shared.clone(), &config_path, cc.egui_ctx.clone());

        if lock(&shared).cfg.auto_update {
            spawn_update_check(shared.clone(), cc.egui_ctx.clone());
        }
        let tray = crate::tray::spawn(shared.clone());
        if tray.is_some() {
            lock(&shared).log("系统托盘已启动");
        }
        Self {
            shared,
            config_path,
            ui_state: UiState::default(),
            tray,
        }
    }

    /// 把界面上的配置写回磁盘（密码按「记住密码」决定是否加密保存）
    fn save_config(&self, cfg: &Config, password: &str) {
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

    /// 把界面结果写回共享状态；返回「开机自启」开关是否被改动
    fn apply(&self, actions: ui::Actions, cfg: &Config, password: &str, paused: bool) -> bool {
        let mut guard = lock(&self.shared);
        let boot_changed = guard.cfg.boot != cfg.boot;
        guard.cfg = cfg.clone();
        guard.password = password.to_string();
        guard.paused = paused;
        if actions.scan {
            guard.scan_requested = true;
        }
        if actions.connect {
            guard.manual_connect = true;
        }
        boot_changed
    }

    /// 用户改了「开机自动启动」→ 立即写入系统
    fn apply_autostart(&self, enabled: bool) {
        let message = match crate::autostart::set_enabled(enabled) {
            Ok(()) if enabled => "已开启开机自动启动".to_string(),
            Ok(()) => "已关闭开机自动启动".to_string(),
            Err(err) => format!("设置开机自启失败：{err}"),
        };
        lock(&self.shared).log(message);
    }

    /// 托盘请求显示窗口 → 把窗口提到前台
    fn handle_show_window(&self, ctx: &egui::Context) {
        if !std::mem::take(&mut lock(&self.shared).show_window) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// 后台请求保存配置（例如首次登录自动开启了开机自启）
    fn handle_save_request(&self, cfg: &Config, password: &str) {
        if std::mem::take(&mut lock(&self.shared).save_requested) {
            self.save_config(cfg, password);
        }
    }

    /// 「网络与目标不符」提示框；返回用户是否选择了「彻底关闭」
    fn handle_mismatch(&self, ctx: &egui::Context, prompt: Option<Mismatch>) -> bool {
        let Some(prompt) = prompt else {
            return false;
        };
        let mut quit = false;
        match dialogs::mismatch(ctx, &prompt.current, &prompt.target) {
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
        quit
    }

    /// 「彻底关闭」确认框；返回是否确认退出
    fn handle_exit_confirm(&mut self, ctx: &egui::Context, cfg: &mut Config, password: &str) -> bool {
        if !self.ui_state.confirming_exit {
            return false;
        }
        match dialogs::exit_confirm(ctx, &mut self.ui_state.no_more_prompt) {
            Some(ExitAnswer::Confirm { no_more_prompt }) => {
                self.ui_state.confirming_exit = false;
                if no_more_prompt && !cfg.no_exit_confirm {
                    cfg.no_exit_confirm = true;
                    lock(&self.shared).cfg.no_exit_confirm = true;
                    self.save_config(cfg, password);
                }
                true
            }
            Some(ExitAnswer::Cancel) => {
                self.ui_state.confirming_exit = false;
                false
            }
            None => false,
        }
    }

    /// 真正退出：停掉托盘并关闭窗口
    fn shutdown(&mut self, ctx: &egui::Context) {
        lock(&self.shared).quit = true;
        self.tray = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(REFRESH);

        // ① 取一份快照，避免渲染期间长时间持锁
        let (mut cfg, mut password, status, level, mut paused, logs, saved, visible, ssid, quote) = {
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
                guard.quote.clone(),
            )
        };
        let (mismatch, update) = {
            let guard = lock(&self.shared);
            (guard.mismatch.clone(), guard.update.clone())
        };
        let snapshot = Snapshot {
            status: &status,
            level,
            ssid: &ssid,
            quote: &quote,
            saved: &saved,
            visible: &visible,
            logs: &logs,
            config_path: &self.config_path,
            update: update.as_ref(),
        };

        // ② 画界面，拿回本帧的用户操作
        let actions = egui::CentralPanel::default()
            .show(ui, |ui| {
                ui::draw(
                    ui,
                    &snapshot,
                    &mut cfg,
                    &mut password,
                    &mut self.ui_state,
                    &mut paused,
                )
            })
            .inner;

        // ③ 写回状态、处理各类交互
        if self.apply(actions, &cfg, &password, paused) {
            self.apply_autostart(cfg.boot);
        }
        if actions.connect {
            self.save_config(&cfg, &password);
        }
        crate::tray::poll(&mut self.tray, &self.shared);
        self.handle_show_window(&ctx);
        self.handle_save_request(&cfg, &password);
        if actions.check_update {
            spawn_update_check(self.shared.clone(), ctx.clone());
        }
        if actions.apply_update {
            spawn_update_apply(self.shared.clone(), ctx.clone());
        }

        let quit = actions.quit
            || self.handle_mismatch(&ctx, mismatch)
            || self.handle_exit_confirm(&ctx, &mut cfg, &password);
        if quit {
            self.shutdown(&ctx);
        }
    }
}

/// 加载中文字体（egui 内置字体不含 CJK，缺失时界面会显示方框）
fn install_font(shared: &Arc<Mutex<Shared>>, cc: &eframe::CreationContext<'_>) {
    let mut guard = lock(shared);
    match crate::fonts::install(&cc.egui_ctx) {
        Some(path) => guard.log(format!("已加载中文字体：{path}")),
        None => guard.log("警告：未找到系统中文字体，界面中文可能显示为方框"),
    }
}

/// 以系统实际状态为准同步「开机自启」配置（用户可能在系统里手动改过）
fn sync_autostart(shared: &Arc<Mutex<Shared>>) {
    let actual = crate::autostart::is_enabled();
    let mut guard = lock(shared);
    if guard.cfg.boot != actual {
        guard.cfg.boot = actual;
        guard.log(format!(
            "开机自启实际状态为{}，已同步配置",
            if actual { "已启用" } else { "未启用" }
        ));
    }
}

/// 后台获取每日名言（不阻塞界面启动），取到后写入共享状态
fn spawn_quote_fetch(shared: Arc<Mutex<Shared>>, config_path: &Path, ctx: egui::Context) {
    let cache_path = config_path.with_file_name(quotes::CACHE_FILE);
    std::thread::spawn(move || {
        let (quote, log) = quotes::load_daily(&cache_path);
        let mut guard = lock(&shared);
        guard.quote = quote;
        if let Some(message) = log {
            guard.log(message);
        }
        drop(guard);
        ctx.request_repaint();
    });
}

/// 后台检查新版本（启动时与「检查更新」按钮共用）
fn spawn_update_check(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    {
        let mut guard = lock(&shared);
        // 已有检查或下载在进行时不重复触发
        if matches!(
            guard.update,
            Some(UpdateState::Checking | UpdateState::Downloading(..))
        ) {
            return;
        }
        guard.update = Some(UpdateState::Checking);
    }
    std::thread::spawn(move || {
        let result = crate::update::check(env!("CARGO_PKG_VERSION"));
        let mut guard = lock(&shared);
        guard.update = match result {
            Ok(Some(update)) => {
                guard.log(format!("发现新版本 v{}", update.version));
                Some(UpdateState::Available(update))
            }
            Ok(None) => {
                guard.log("已是最新版本");
                None
            }
            Err(err) => Some(UpdateState::Failed(err.to_string())),
        };
        drop(guard);
        ctx.request_repaint();
    });
}

/// 后台下载并替换可执行文件，完成后提示重启
fn spawn_update_apply(shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    let update = match lock(&shared).update.clone() {
        Some(UpdateState::Available(update)) => update,
        _ => return,
    };
    lock(&shared).update = Some(UpdateState::Downloading(0, update.size));

    std::thread::spawn(move || {
        let progress = {
            let shared = shared.clone();
            let ctx = ctx.clone();
            move |done: u64, total: u64| {
                lock(&shared).update = Some(UpdateState::Downloading(done, total));
                ctx.request_repaint();
            }
        };
        let result = crate::update::apply(&update, progress);
        let mut guard = lock(&shared);
        guard.update = Some(match result {
            Ok(()) => {
                guard.log(format!("已更新到 v{}，重启程序后生效", update.version));
                UpdateState::Ready(update.version.clone())
            }
            Err(err) => {
                guard.log(format!("更新失败：{err}"));
                UpdateState::Failed(err.to_string())
            }
        });
        drop(guard);
        ctx.request_repaint();
    });
}
