//! 单实例保护：重复启动时只唤出已有窗口，保证同时只存在一个窗口。
//!
//! 做法：运行中的实例监听回环地址上一个由内核分配的端口，并把端口号写到
//! 配置文件同目录的 `instance.port`；新启动的进程读到端口后尝试握手，
//! 握手成功说明已有实例在跑，于是把它的窗口唤到前台，自己立刻退出。
//! 只监听 `127.0.0.1`，不对外开放，也不需要任何额外依赖。
//!
//! 唤醒窗口既置 [`crate::state::Shared::show_window`] 标志，也直接发视口命令：
//! 窗口隐藏时 eframe 不跑界面而是走 [`eframe::App::logic`]，两条路径都能生效。

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::state::{Shared, update};

/// 记录监听端口的文件名（与 config.json 同目录）
pub const PORT_FILE: &str = "instance.port";
/// 握手超时：过大会明显拖慢启动
const TIMEOUT: Duration = Duration::from_millis(600);
/// 「唤出窗口」请求口令
const HELLO: &str = "GUUNNET SHOW";
/// 已有实例的应答口令（用来确认端口后面确实是本程序）
const ACK: &str = "GUUNNET OK";

/// 启动时的身份判定
pub enum Startup {
    /// 本进程是唯一实例：界面就绪后把该监听器交给 [`watch`]
    Primary(TcpListener),
    /// 已有实例在运行（窗口已唤出），本进程应直接退出
    AlreadyRunning,
    /// 拿不到可用端口：放弃单实例保护，照常启动（绝不因此打不开程序）
    NoProtection,
}

/// 判定本次启动是「唯一实例」还是「重复启动」
pub fn start(config_path: &Path) -> Startup {
    let port_file = port_path(config_path);
    if let Some(port) = read_port(&port_file)
        && notify_existing(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
    {
        return Startup::AlreadyRunning;
    }

    // 端口交给内核分配：既避开固定端口被别的程序占用，
    // 也避开上一次握手连接留下的 TIME_WAIT 让 bind 失败
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) else {
        return Startup::NoProtection;
    };
    if let Ok(addr) = listener.local_addr() {
        // 写失败只影响下次的探测，不影响本次运行
        let _ = write_port(&port_file, addr.port());
    }
    Startup::Primary(listener)
}

/// 端口记录文件路径（与配置文件同目录）
pub fn port_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name(PORT_FILE)
}

/// 常驻监听后续启动请求，收到握手就把窗口唤到前台
pub fn watch(listener: TcpListener, shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(TIMEOUT));
            let _ = stream.set_write_timeout(Some(TIMEOUT));

            let mut request = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut request).is_err() || request.trim() != HELLO {
                continue; // 不是本程序的请求，忽略
            }
            drop(reader);

            // 先应答再唤醒：新进程拿到应答就退出，不必等窗口真的显示出来
            let _ = stream.write_all(format!("{ACK}\n").as_bytes());
            let _ = stream.flush();

            update(&shared, |state| {
                state.show_window = true;
                state.log("检测到重复启动，已唤出已有窗口");
            });
            show_window(&ctx);
        }
    });
}

/// 把窗口从隐藏/最小化状态恢复到前台
pub fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

/// 尝试唤醒该端口上的既有实例；只有握手成功才算数
fn notify_existing(addr: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, TIMEOUT) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    // 对端用 read_line 收请求，必须带换行，否则会等到读超时
    if stream.write_all(format!("{HELLO}\n").as_bytes()).is_err() {
        return false;
    }
    let _ = stream.flush();

    let mut reply = String::new();
    let mut reader = BufReader::new(&stream);
    // 读失败或对端直接关闭（Ok(0)）都不算数：端口后面可能只是别的程序
    if reader.read_line(&mut reply).is_err() {
        return false;
    }
    reply.trim() == ACK
}

/// 读取上次运行记录下来的端口
fn read_port(path: &Path) -> Option<u16> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// 记录本次运行的端口（自动创建配置目录）
fn write_port(path: &Path, port: u16) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{port}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::lock;
    /// 每个用例独占一个子目录（端口文件名固定，只能靠目录隔离），
    /// 放在 target/ 下以免污染用户配置目录
    fn test_config(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("single-test")
            .join(name);
        std::fs::create_dir_all(&dir).ok();
        dir.join("config.json")
    }

    #[test]
    fn 第二个实例会认出已有实例() {
        let config = test_config("primary");
        let port_file = port_path(&config);
        let _ = std::fs::remove_file(&port_file);

        let Startup::Primary(listener) = start(&config) else {
            panic!("首次启动应当是唯一实例");
        };
        assert!(read_port(&port_file).is_some(), "应当记下监听端口");

        let shared = Arc::new(Mutex::new(Shared::new(Config::default(), String::new())));
        watch(listener, shared.clone(), egui::Context::default());

        assert!(matches!(start(&config), Startup::AlreadyRunning));
        // 唤醒标志由监听线程设置，稍等片刻
        for _ in 0..50 {
            if lock(&shared).show_window {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(lock(&shared).show_window, "应当请求唤出已有窗口");
        let _ = std::fs::remove_file(&port_file);
    }

    #[test]
    fn 端口文件损坏时照常启动() {
        let config = test_config("broken");
        let port_file = port_path(&config);
        std::fs::write(&port_file, "不是端口\n").ok();
        assert!(matches!(start(&config), Startup::Primary(_)));
        let _ = std::fs::remove_file(&port_file);
    }
}
