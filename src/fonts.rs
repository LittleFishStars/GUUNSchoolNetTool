//! 中文字体加载。
//!
//! egui 内置字体（Ubuntu-Light 等）不含 CJK 字形，不额外加载中文时界面中文会显示成方框。
//! 这里从系统字体中按候选列表挑一款装上，作为内置字体之后的回退字体。

use std::path::Path;
use std::sync::Arc;

/// 候选字体：(路径, 字体文件内的字面序号)。
/// TTC 文件里含有多个字面，序号决定用哪一个（例如 Noto Sans CJK 的 SC 是第 2 个）。
#[cfg(target_os = "linux")]
const CANDIDATES: &[(&str, u32)] = &[
    // Noto Sans CJK SC
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
    ("/usr/share/fonts/noto-cjk/NotoSansCJKsc-Regular.otf", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf", 0),
    // 文泉驿微米黑
    ("/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc", 0),
    ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
    ("/usr/share/fonts/wqy-microhei/wqy-microhei.ttc", 0),
    // 微软雅黑（部分发行版装了 Windows 字体）
    ("/usr/share/fonts/winfonts/msyh.ttc", 0),
];

#[cfg(windows)]
const CANDIDATES: &[(&str, u32)] = &[
    // 微软雅黑、黑体、宋体
    ("C:/Windows/Fonts/msyh.ttc", 0),
    ("C:/Windows/Fonts/msyh.ttf", 0),
    ("C:/Windows/Fonts/simhei.ttf", 0),
    ("C:/Windows/Fonts/simsun.ttc", 0),
];

#[cfg(not(any(target_os = "linux", windows)))]
const CANDIDATES: &[(&str, u32)] = &[];

/// 安装中文字体；返回实际使用的字体路径，找不到时返回 `None`
pub fn install(ctx: &egui::Context) -> Option<&'static str> {
    let (path, index) = CANDIDATES
        .iter()
        .find(|(path, _)| Path::new(path).is_file())?;
    let bytes = std::fs::read(path).ok()?;

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk".to_owned(),
        Arc::new(egui::FontData {
            font: bytes.into(),
            index: *index,
            tweak: Default::default(),
        }),
    );
    // 追加到两种字族的末尾：ASCII 仍用内置字体，缺字形时回退到中文字体
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
    Some(path)
}
