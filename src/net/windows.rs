//! Windows 平台实现：使用系统自带的 `netsh wlan`。

use super::run;

/// 调用 netsh：先切到 UTF-8 代码页，否则中文 WiFi 名会乱码
fn netsh(args: &[&str]) -> Option<String> {
    let line = format!("chcp 65001>nul & netsh {}", args.join(" "));
    run("cmd", &["/c", line.as_str()])
}

/// 当前连接的 WiFi 名称；未连接时返回空串
pub fn current_ssid() -> String {
    if let Some(out) = netsh(&["wlan", "show", "interfaces"]) {
        for line in out.lines() {
            if let Some((key, value)) = line.split_once(':') {
                // 只认 "SSID : 名称"，"BSSID : ..." 不算
                if key.trim() == "SSID" {
                    let ssid = value.trim();
                    if !ssid.is_empty() {
                        return ssid.to_string();
                    }
                }
            }
        }
    }
    String::new()
}

/// 附近可见的 WiFi 名称（去重排序）。
///
/// `netsh` 没有强制扫描开关，因此该参数在 Windows 上无意义。
pub fn visible_ssids(_rescan: bool) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(out) = netsh(&["wlan", "show", "networks"]) {
        for line in out.lines() {
            let trimmed = line.trim();
            // "SSID 1 : 名称"；BSSID 行被 starts_with 排除
            if !trimmed.starts_with("SSID") {
                continue;
            }
            if let Some((_, value)) = trimmed.split_once(':') {
                let ssid = value.trim();
                if !ssid.is_empty() {
                    names.push(ssid.to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// 连接到指定 WiFi（需要系统已保存该网络）
pub fn connect_ssid(ssid: &str) -> bool {
    if ssid.is_empty() {
        return false;
    }
    // 先自愈为「自动连接」，顺带修复开机不自动连校园网
    set_profile_auto(ssid);
    let arg = format!("name=\"{ssid}\"");
    netsh(&["wlan", "connect", arg.as_str()]).is_some()
}

/// 系统已保存的 WiFi 名称
pub fn saved_ssids() -> Vec<String> {
    let mut names = Vec::new();
    if let Some(out) = netsh(&["wlan", "show", "profiles"]) {
        for line in out.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            // 中英文系统的行首分别是「所有用户配置文件」「All User Profile」
            if !(key.contains("配置文件") || key.contains("Profile")) {
                continue;
            }
            let name = value.trim();
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// 把某个 WiFi 配置设为「自动连接」
pub fn set_profile_auto(ssid: &str) -> bool {
    if ssid.is_empty() {
        return false;
    }
    // 名称里的半角双引号会破坏命令行参数，直接剔除
    let safe = ssid.replace('"', "");
    let arg = format!("name=\"{safe}\"");
    netsh(&[
        "wlan",
        "set",
        "profileparameter",
        arg.as_str(),
        "connectionmode=auto",
    ])
    .is_some()
}
