//! 网络探测与 WiFi 操作。
//!
//! 跨平台差异集中在本模块：
//! - Linux：`nmcli`（NetworkManager），回退 `iwgetid`
//! - Windows：`netsh wlan`，经 `cmd /c chcp 65001` 调用以避免中文 SSID 乱码

use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::process::Command;
use std::time::Duration;

/// 门户 TCP 探测超时（对应 PowerShell 版的 800ms）
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);
/// 联网检测的 HTTP 超时
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

/// 构造子进程命令；Windows 下隐藏控制台窗口，避免黑框闪烁
fn command(program: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 执行命令并返回标准输出（宽松 UTF-8 解码）
fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = command(program, args).output().ok()?;
    if output.stdout.is_empty() && !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Windows 下调用 netsh：先切到 UTF-8 代码页，否则中文 WiFi 名会乱码
#[cfg(windows)]
fn netsh(args: &[&str]) -> Option<String> {
    let line = format!("chcp 65001>nul & netsh {}", args.join(" "));
    run("cmd", &["/c", line.as_str()])
}

/// 获取本机出口 IP。门户登记的是校园网地址，因此优先返回 `10.` 开头的地址。
pub fn local_ip(portal_host: &str, portal_port: u16) -> String {
    let candidates = [(portal_host, portal_port), ("10.10.90.2", 801)];
    let mut fallback = String::new();
    for (host, port) in candidates {
        let ip = udp_local_ip(host, port);
        if ip.is_empty() {
            continue;
        }
        if ip.starts_with("10.") {
            return ip;
        }
        if fallback.is_empty() {
            fallback = ip;
        }
    }
    fallback
}

/// 向目标地址"连接"一次 UDP（不实际发包），由内核选出出口网卡的源地址
fn udp_local_ip(host: &str, port: u16) -> String {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return String::new();
    };
    if sock.connect((host, port)).is_err() {
        return String::new();
    }
    sock.local_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_default()
}

/// 认证服务器是否可达（= 网络真正就绪，可以登录了）
pub fn portal_reachable(host: &str, port: u16) -> bool {
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs.any(|addr| TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok())
}

/// 联网检测。
///
/// 必须拿到预期结果才算联网：门户重定向页同样返回 200，
/// 只看状态码会把"未认证"误判成"已联网"，从而不再登录。
pub fn internet_ok() -> bool {
    if let Some((status, _)) = http_get("http://connectivitycheck.gstatic.com/generate_204")
        && status == 204
    {
        return true;
    }
    http_get("http://www.msftconnecttest.com/connecttest.txt")
        .is_some_and(|(_, body)| body.trim() == "Microsoft Connect Test")
}

/// 发起一次 GET，返回状态码与响应体
fn http_get(url: &str) -> Option<(u16, String)> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into();
    let mut resp = agent.get(url).call().ok()?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().ok()?;
    Some((status, body))
}

/// 当前连接的 WiFi 名称；未连接时返回空串
pub fn current_ssid() -> String {
    #[cfg(target_os = "linux")]
    {
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
        if let Some(out) = run("iwgetid", &["-r"]) {
            let ssid = out.trim();
            if !ssid.is_empty() {
                return ssid.to_string();
            }
        }
        String::new()
    }
    #[cfg(windows)]
    {
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
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        String::new()
    }
}

/// 附近可见的 WiFi 名称（去重排序）。`rescan` 为真时强制重新扫描（较慢）。
pub fn visible_ssids(rescan: bool) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    #[cfg(target_os = "linux")]
    {
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
    }
    #[cfg(windows)]
    {
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
    }
    names.sort();
    names.dedup();
    names
}

/// 连接到指定 WiFi（需要系统已保存该网络的配置）
pub fn connect_ssid(ssid: &str) -> bool {
    if ssid.is_empty() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        // 优先启用已保存的连接；否则现场连接
        if run("nmcli", &["connection", "up", "id", ssid]).is_some() {
            return true;
        }
        run("nmcli", &["device", "wifi", "connect", ssid]).is_some()
    }
    #[cfg(windows)]
    {
        let arg = format!("name=\"{ssid}\"");
        netsh(&["wlan", "connect", arg.as_str()]).is_some()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        false
    }
}
