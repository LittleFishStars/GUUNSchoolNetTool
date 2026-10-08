//! 统一图标：应用窗口、Windows 托盘、Linux 托盘共用同一套图标。
//!
//! 图标源是仓库根目录的 `icon.svg`，预先用 `rsvg-convert` 渲染成多个尺寸的
//! PNG 放在 `assets/`（再生成命令见 README），编译期嵌进二进制。
//! 因此运行时既不需要 SVG 渲染器，也不依赖系统图标主题，各处的图标必然一致。
//!
//! - 窗口图标：[`window_icon`]（X11 由窗口管理器显示；Wayland 下看 `.desktop`）
//! - Windows 托盘：[`rgba`] → `tray_icon::Icon`
//! - Linux 托盘：[`tray_argb32`]（freedesktop 的 IconPixmap，ARGB32 网络字节序）

/// 预渲染好的各尺寸 PNG（必须按边长升序）
const SIZES: &[(u32, &[u8])] = &[
    (16, include_bytes!("../assets/icon-16.png")),
    (22, include_bytes!("../assets/icon-22.png")),
    (24, include_bytes!("../assets/icon-24.png")),
    (32, include_bytes!("../assets/icon-32.png")),
    (48, include_bytes!("../assets/icon-48.png")),
    (64, include_bytes!("../assets/icon-64.png")),
    (128, include_bytes!("../assets/icon-128.png")),
    (256, include_bytes!("../assets/icon-256.png")),
];

/// 应用窗口图标
pub fn window_icon() -> Option<egui::IconData> {
    let (rgba, width, height) = rgba(256)?;
    Some(egui::IconData {
        rgba,
        width,
        height,
    })
}

/// 解码最接近 `size` 的一档图标，返回 RGBA 像素与边长
pub fn rgba(size: u32) -> Option<(Vec<u8>, u32, u32)> {
    let (_, bytes) = pick(size);
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some((rgba.into_raw(), width, height))
}

/// Linux 托盘用的 ARGB32 图标（可给多档尺寸，宿主自己挑最合适的）
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // 其他平台的托盘不用 ARGB32
pub fn tray_argb32(sizes: &[u32]) -> Vec<(Vec<u8>, u32, u32)> {
    sizes
        .iter()
        .filter_map(|size| {
            let (rgba, width, height) = rgba(*size)?;
            Some((rgba_to_argb32(&rgba), width, height))
        })
        .collect()
}

/// RGBA → ARGB32（网络字节序），freedesktop 托盘的像素格式
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rgba_to_argb32(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|pixel| [pixel[3], pixel[0], pixel[1], pixel[2]])
        .collect()
}

/// 挑一档最贴近 `size` 的预渲染图：优先不小于目标的，都没有就取最大的
fn pick(size: u32) -> (u32, &'static [u8]) {
    SIZES
        .iter()
        .find(|(side, _)| *side >= size)
        .copied()
        .unwrap_or_else(|| SIZES[SIZES.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 各档图标都能解码且尺寸正确() {
        for (side, _) in SIZES {
            let (rgba, width, height) = rgba(*side).expect("应当能解码内置图标");
            assert_eq!((width, height), (*side, *side));
            assert_eq!(rgba.len(), (side * side * 4) as usize);
        }
    }

    #[test]
    fn 尺寸取最接近的一档() {
        assert_eq!(pick(20).0, 22);
        assert_eq!(pick(200).0, 256);
        // 超过最大档时退回最大的一张，而不是没有图标
        assert_eq!(pick(9999).0, 256);
    }

    #[test]
    fn 转成托盘像素格式时通道顺序为argb() {
        let rgba = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(rgba_to_argb32(&rgba), vec![4, 1, 2, 3, 8, 5, 6, 7]);
    }

    #[test]
    fn 窗口图标取自最大档() {
        let icon = window_icon().expect("应当有窗口图标");
        assert_eq!((icon.width, icon.height), (256, 256));
    }
}
