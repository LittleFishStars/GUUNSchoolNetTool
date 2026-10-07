# 校园网自连（Rust + egui 版）

赣南师范大学校园网自动登录工具的 **Rust + egui** 重写版，跨平台实现：
核心功能在 Windows 与 Linux 上均可运行，Windows 专属能力通过条件编译启用。

> 本目录是 `master` 分支 `main.py`（Python/Tkinter）与 PR #2 `powershell/GUUNNet.ps1`
> （PowerShell/WinForms）之后的新技术栈实现，功能以 PowerShell 版 v1.0.6 为基准。

## 功能（核心）

- **自动登录**：ePortal 认证，支持 电信(`@dx`) / 移动(无后缀) / 联通(`@lt`)
- **断线自动重连**：网络恢复或休眠唤醒后自动重新登录
- **自适应轮询**：需要登录时 2 秒一次，稳定在线时 10 秒一次（省电）
- **指数退避**：连续失败按 3s → 6s → 12s → 24s → 60s 递增，避免猛敲认证服务器
- **目标网络**：可指定固定连接的 WiFi；目标不在附近时不会抢网
- **暂停 / 继续**：临时停止检测与自动登录
- **记住密码**：Windows 用 DPAPI 加密（仅当前用户可解密），其他平台为 base64 混淆
- **门户优先探测**：先测认证服务器 TCP 是否可达，避免"无法连接到远程服务器"
- **运行日志**：界面内实时查看，含每次登录的账号、运营商与响应消息
- **系统托盘**：显示窗口 / 暂停继续 / 彻底退出；Linux 走 StatusNotifierItem（无 SNI 宿主时自动降级为无托盘，不影响主程序）

## 与 PowerShell 版的差异

| 能力 | PowerShell 版 | Rust 版 |
| --- | --- | --- |
| 运行环境 | Windows + PowerShell 5.1 | Windows / Linux，单文件可执行 |
| 密码保护 | DPAPI | Windows DPAPI；其他平台 base64 混淆 |
| WiFi 自愈（设为自动连接） | 支持 | 暂未实现 |
| 系统托盘 | 支持 | 支持（Windows 用 `tray-icon`，Linux 用 `ksni`/SNI） |
| 每日名言 | 支持 | 暂未实现 |
| 开机自启 | 支持 | Windows 上界面已预留开关，暂未接线 |
| 网络与目标不符弹窗 | 支持 | 状态栏提示，不弹窗 |

## 构建与运行

```bash
cd rust
cargo run --release          # 直接运行
cargo build --release        # 产物在 target/release/guunnet
cargo test                   # 运行单元测试（协议解析、退避策略）
```

Linux 需要 `nmcli`（NetworkManager）读取与切换 WiFi：

```bash
sudo pacman -S networkmanager   # 已安装可忽略
```

Windows 可直接 `cargo build --release`，不需要额外依赖（WiFi 走系统自带 `netsh`）。

## 配置文件

查找顺序：

1. `--config <路径>` 或 `-c <路径>` 命令行参数
2. 可执行文件同目录的 `config.json`（便携模式，与 PowerShell 版一致）
3. 用户配置目录：Linux `~/.config/guunnet/config.json`，Windows `%APPDATA%\guunnet\config.json`

字段名与 PowerShell 版一致（`schoolId` / `carrier` / `targetSsid` 等），可直接复用既有配置。
密码字段 `passEnc` 带前缀标识：`dpapi:` 为 DPAPI 密文，`b64:` 为混淆文本。

## 说明

- 认证服务器默认 `10.10.90.2:801`，校园网 SSID 关键词默认 `CMCC-GNNUN`；
  其他学校可通过配置文件的 `portalHost` / `portalPort` / `ssidKeyword` 修改。
- 联网判定使用 `generate_204` 与 `msftconnecttest`：门户重定向页同样返回 200，
  因此必须校验响应内容，否则会把"未认证"误判为"已联网"而不去登录。
- 注：Dr.COM 门户在**设备已在线时不校验密码**（一律回 `IP 已经在线`），
  所以"在线时测试登录"无法判断密码对错。
