//! 网络探测与 WiFi 操作。
//!
//! 与平台无关的能力（本机 IP、门户可达性、联网判定）直接实现于此；
//! WiFi 相关操作按平台分发到子模块：
//! - Linux：`nmcli`（NetworkManager），回退 `iwgetid`
//! - Windows：`netsh wlan`，经 `cmd /c chcp 65001` 调用以避免中文 SSID 乱码

use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::process::Command;
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{connect_ssid, current_ssid, saved_ssids, set_profile_auto, visible_ssids};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{connect_ssid, current_ssid, saved_ssids, set_profile_auto, visible_ssids};

/// 其他平台：不支持 WiFi 操作，保持可编译但功能不可用
#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    /// 当前连接的 WiFi 名称
    pub fn current_ssid() -> String {
        String::new()
    }
    /// 附近可见的 WiFi 名称
    pub fn visible_ssids(_rescan: bool) -> Vec<String> {
        Vec::new()
    }
    /// 连接到指定 WiFi
    pub fn connect_ssid(_ssid: &str) -> bool {
        false
    }
    /// 系统已保存的 WiFi 名称
    pub fn saved_ssids() -> Vec<String> {
        Vec::new()
    }
    /// 把某个 WiFi 配置设为「自动连接」
    pub fn set_profile_auto(_ssid: &str) -> bool {
        false
    }
}
#[cfg(not(any(target_os = "linux", windows)))]
pub use platform::{connect_ssid, current_ssid, saved_ssids, set_profile_auto, visible_ssids};

/// 门户 TCP 探测超时
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

/// 获取本机出口 IP。
///
/// 门户登记的是校园网地址，因此优先返回 `10.` 开头的地址。
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
/// 只看状态码会把「未认证」误判成「已联网」，从而不再登录。
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
    let agent = crate::http::agent(HTTP_TIMEOUT);
    let mut resp = agent.get(url).call().ok()?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().ok()?;
    Some((status, body))
}

/// 把所有名称含 `keyword` 的已保存配置批量设为「自动连接」，返回处理数量
pub fn repair_profiles_auto(keyword: &str) -> usize {
    saved_ssids()
        .into_iter()
        .filter(|name| name.contains(keyword) && set_profile_auto(name))
        .count()
}
