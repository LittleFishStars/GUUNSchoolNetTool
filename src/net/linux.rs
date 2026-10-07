//! Linux 平台实现：优先 `nmcli`（NetworkManager），回退 `iwgetid`。

use super::run;

/// 当前连接的 WiFi 名称；未连接时返回空串
pub fn current_ssid() -> String {
    if let Some(out) = run("nmcli", &["-t", "-f", "ACTIVE,SSID", "dev", "wifi"]) {
        for line in out.lines() {
            if let Some(ssid) = line.strip_prefix("yes:") {
                let ssid = ssid.trim();
                if !ssid.is_empty() {
                    return ssid.to_string();
                }
            }
        }
    }
    // 没有 NetworkManager 时退回 wireless_tools
    if let Some(out) = run("iwgetid", &["-r"]) {
        let ssid = out.trim();
        if !ssid.is_empty() {
            return ssid.to_string();
        }
    }
    String::new()
}

/// 附近可见的 WiFi 名称（去重排序）；`rescan` 为真时强制重新扫描（较慢）
pub fn visible_ssids(rescan: bool) -> Vec<String> {
    let mut names = Vec::new();
    let mut args = vec!["-t", "-f", "SSID", "dev", "wifi", "list"];
    if rescan {
        args.extend_from_slice(&["--rescan", "yes"]);
    }
    if let Some(out) = run("nmcli", &args) {
        for line in out.lines() {
            let ssid = line.trim();
            if !ssid.is_empty() {
                names.push(ssid.to_string());
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
    // 优先启用已保存的连接，否则现场连接
    if run("nmcli", &["connection", "up", "id", ssid]).is_some() {
        return true;
    }
    run("nmcli", &["device", "wifi", "connect", ssid]).is_some()
}

/// 系统已保存的 WiFi 名称
pub fn saved_ssids() -> Vec<String> {
    let mut names = Vec::new();
    if let Some(out) = run("nmcli", &["-t", "-f", "NAME,TYPE", "connection", "show"]) {
        for line in out.lines() {
            // 形如 "CMCC-GNNUN:802-11-wireless"；名称可能含冒号，故从右侧切分
            if let Some((name, kind)) = line.rsplit_once(':')
                && kind.trim() == "802-11-wireless"
            {
                let name = name.trim();
                if !name.is_empty() {
                    names.push(name.to_string());
                }
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
    let safe = ssid.replace('"', "");
    run(
        "nmcli",
        &[
            "connection",
            "modify",
            safe.as_str(),
            "connection.autoconnect",
            "yes",
        ],
    )
    .is_some()
}
