//! 系统托盘。
//!
//! - Windows：用 `tray-icon` 创建原生托盘图标，菜单事件在界面循环里轮询；
//! - Linux：用 `ksni` 实现 freedesktop StatusNotifierItem（走 D-Bus）。
//!   若桌面没有 SNI 宿主（例如只有 niri 而没有 waybar 一类面板），启动会失败，
//!   此时静默降级为「无托盘」，不影响主程序。

use std::sync::{Arc, Mutex};

use crate::state::{Shared, lock};

/// 托盘句柄；环境不支持时为 `None`
pub struct Tray {
    #[cfg(target_os = "linux")]
    handle: ksni::blocking::Handle<LinuxTray>,
    /// 上次同步给菜单的暂停状态，避免每帧都刷新菜单
    #[cfg(target_os = "linux")]
    last_paused: bool,
    #[cfg(windows)]
    inner: windows_impl::WindowsTray,
}

/// 启动托盘；失败时返回 `None`
pub fn spawn(shared: Arc<Mutex<Shared>>) -> Option<Tray> {
    #[cfg(target_os = "linux")]
    {
        use ksni::blocking::TrayMethods as _;
        let handle = LinuxTray { shared }.spawn().ok()?;
        Some(Tray {
            handle,
            last_paused: false,
        })
    }
    #[cfg(windows)]
    {
        Some(Tray {
            inner: windows_impl::spawn(shared)?,
        })
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = shared;
        None
    }
}

/// 处理托盘交互，并在需要时刷新菜单勾选状态
pub fn poll(tray: &mut Option<Tray>, shared: &Arc<Mutex<Shared>>) {
    #[cfg(target_os = "linux")]
    if let Some(tray) = tray {
        let paused = lock(shared).paused;
        if tray.last_paused != paused {
            tray.last_paused = paused;
            // 让 ksni 重新拉取菜单，"暂停"的勾选状态才会跟上
            let _ = tray.handle.update(|_| {});
        }
    }
    #[cfg(windows)]
    if let Some(tray) = tray {
        windows_impl::poll(&tray.inner, shared);
        let paused = lock(shared).paused;
        if tray.inner.pause_item.is_checked() != paused {
            tray.inner.pause_item.set_checked(paused);
        }
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (tray, shared);
    }
}

/// Linux 托盘（StatusNotifierItem）
#[cfg(target_os = "linux")]
struct LinuxTray {
    shared: Arc<Mutex<Shared>>,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for LinuxTray {
    fn id(&self) -> String {
        "guunnet".into()
    }

    fn title(&self) -> String {
        format!("校园网自连 v{}", env!("CARGO_PKG_VERSION"))
    }

    fn icon_name(&self) -> String {
        // 用系统主题图标，免去额外打包图片资源
        "network-wireless".into()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let (status, paused) = {
            let guard = lock(&self.shared);
            (guard.status.clone(), guard.paused)
        };
        vec![
            StandardItem {
                label: status,
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "显示窗口".into(),
                activate: Box::new(|tray: &mut Self| {
                    lock(&tray.shared).show_window = true;
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "暂停自动连接".into(),
                checked: paused,
                activate: Box::new(|tray: &mut Self| {
                    let mut guard = lock(&tray.shared);
                    guard.paused = !guard.paused;
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "彻底退出".into(),
                activate: Box::new(|tray: &mut Self| {
                    lock(&tray.shared).quit = true;
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Windows 托盘（`tray-icon`）
#[cfg(windows)]
mod windows_impl {
    use super::*;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    /// 保存菜单项句柄：既用于识别事件，也用于同步勾选状态
    pub struct WindowsTray {
        _icon: TrayIcon,
        show_item: MenuItem,
        pub pause_item: CheckMenuItem,
        quit_item: MenuItem,
    }

    pub fn spawn(shared: Arc<Mutex<Shared>>) -> Option<WindowsTray> {
        let paused = lock(&shared).paused;
        let show_item = MenuItem::new("显示窗口", true, None);
        let pause_item = CheckMenuItem::new("暂停自动连接", true, paused, None);
        let quit_item = MenuItem::new("彻底退出", true, None);
        let menu = Menu::new();
        menu.append(&show_item).ok()?;
        menu.append(&pause_item).ok()?;
        menu.append(&quit_item).ok()?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(format!("校园网自连 v{}", env!("CARGO_PKG_VERSION")))
            .with_icon(load_icon()?)
            .build()
            .ok()?;
        Some(WindowsTray {
            _icon: icon,
            show_item,
            pause_item,
            quit_item,
        })
    }

    /// 轮询托盘菜单事件
    pub fn poll(tray: &WindowsTray, shared: &Arc<Mutex<Shared>>) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            if id == tray.show_item.id() {
                lock(shared).show_window = true;
            } else if id == tray.pause_item.id() {
                let mut guard = lock(shared);
                guard.paused = !guard.paused;
            } else if id == tray.quit_item.id() {
                lock(shared).quit = true;
            }
        }
    }

    /// 从仓库自带的 icon.ico 解码托盘图标
    fn load_icon() -> Option<Icon> {
        const ICO: &[u8] = include_bytes!("../icon.ico");
        let image = image::load_from_memory_with_format(ICO, image::ImageFormat::Ico).ok()?;
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        Icon::from_rgba(rgba.into_raw(), width, height).ok()
    }
}
