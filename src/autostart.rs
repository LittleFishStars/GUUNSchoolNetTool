//! 开机自启。
//!
//! 用 `auto-launch` 统一平台差异：Windows 写注册表 Run 项，
//! Linux 生成 `~/.config/autostart/guunnet.desktop`（XDG Autostart）。

use anyhow::{Context, Result, anyhow};
use auto_launch::{AutoLaunch, AutoLaunchBuilder};

/// 应用标识：注册表项名 / desktop 文件名
const APP_NAME: &str = "guunnet";

/// 构造自启管理器（记录的是当前可执行文件的真实路径）
fn launcher() -> Result<AutoLaunch> {
    let exe = std::env::current_exe().context("取不到可执行文件路径")?;
    let path = exe.to_string_lossy().to_string();
    AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(&path)
        .build()
        .map_err(|err| anyhow!("构造自启项失败：{err}"))
}

/// 启用或停用开机自启
pub fn set_enabled(on: bool) -> Result<()> {
    let launcher = launcher()?;
    if on {
        launcher
            .enable()
            .map_err(|err| anyhow!("启用开机自启失败：{err}"))?;
    } else {
        launcher
            .disable()
            .map_err(|err| anyhow!("停用开机自启失败：{err}"))?;
    }
    Ok(())
}

/// 查询开机自启当前是否已启用
pub fn is_enabled() -> bool {
    launcher()
        .ok()
        .and_then(|launcher| launcher.is_enabled().ok())
        .unwrap_or(false)
}
