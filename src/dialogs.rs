//! 模态对话框：彻底关闭确认、网络与目标不符提示。
//!
//! 对应 PowerShell 版的 `Show-ExitConfirm` 与 `Show-MismatchDialog`。

/// 「彻底关闭」确认框的结果
pub enum ExitAnswer {
    /// 确认退出；`no_more_prompt` 表示用户勾选了「不再提示」
    Confirm { no_more_prompt: bool },
    /// 取消
    Cancel,
}

/// 显示退出确认框；返回 `None` 表示用户尚未作出选择
pub fn exit_confirm(ctx: &egui::Context, no_more_prompt: &mut bool) -> Option<ExitAnswer> {
    let mut answer = None;
    egui::Window::new("彻底关闭？")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("程序将退出，本次不再自动重连。");
            ui.label("（下次开机仍会自动启动并登录）");
            ui.add_space(8.0);
            ui.checkbox(no_more_prompt, "不再提示（以后点「彻底关闭」直接退出）");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("是(Y)").clicked() {
                    answer = Some(ExitAnswer::Confirm {
                        no_more_prompt: *no_more_prompt,
                    });
                }
                if ui.button("否(N)").clicked() {
                    answer = Some(ExitAnswer::Cancel);
                }
            });
        });
    answer
}

/// 「网络与目标不符」提示框的结果
pub enum MismatchAnswer {
    /// 暂停自动连接
    Pause,
    /// 忽略本次提示
    Ignore,
    /// 彻底关闭程序
    Exit,
}

/// 显示「当前网络与目标网络不符」提示框
pub fn mismatch(ctx: &egui::Context, current: &str, target: &str) -> Option<MismatchAnswer> {
    let mut answer = None;
    egui::Window::new("网络与目标不符")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("当前连接的网络是「{current}」，"));
            ui.label(format!("与设定的目标网络「{target}」不一致，"));
            ui.label("且目标网络当前不在附近。");
            ui.add_space(6.0);
            ui.label("要不要暂停自动连接？");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("暂停程序").clicked() {
                    answer = Some(MismatchAnswer::Pause);
                }
                if ui.button("忽略").clicked() {
                    answer = Some(MismatchAnswer::Ignore);
                }
                if ui.button("彻底关闭").clicked() {
                    answer = Some(MismatchAnswer::Exit);
                }
            });
        });
    answer
}
