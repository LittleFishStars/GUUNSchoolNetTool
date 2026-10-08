//! 构建脚本：给 Windows 产物嵌入程序图标，让 exe 文件、任务栏与 Alt+Tab 的图标
//! 和窗口、托盘保持一致（都源自 `icon.ico`）。
//!
//! 非 Windows 目标直接跳过；找不到 `windres` / `rc.exe` 时也只告警不报错，
//! 免得因为一个图标把整个构建弄挂。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=icon.ico");

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("icon.ico");
    if let Err(err) = resource.compile() {
        println!("cargo:warning=嵌入程序图标失败（缺 windres 或 rc.exe？）：{err}");
    }
}
