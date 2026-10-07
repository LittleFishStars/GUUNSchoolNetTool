# 校园网自连（Rust + egui）

赣南师范大学校园网自动登录工具，使用 **Rust + egui** 实现，跨平台：
Windows 与 Linux 均可运行，Windows 专属能力（DPAPI 密码保护、`netsh` WiFi 操作）
通过条件编译启用。

> 仓库早期版本的 Python（Tkinter）与 PowerShell（WinForms）实现已移除，
> 由本实现整体替代。

## 功能

- **自动登录**：ePortal 认证，支持 电信(`@dx`) / 移动(无后缀) / 联通(`@lt`)
- **断线自动重连**：网络恢复或休眠唤醒后自动重新登录
- **自适应轮询**：需要登录时 2 秒一次，稳定在线时 10 秒一次（省电）
- **指数退避**：连续失败按 3s → 6s → 12s → 24s → 48s 递增，避免猛敲认证服务器
- **目标网络**：只列系统「已保存」的网络；目标不在附近时不会抢网
- **暂停 / 继续**：临时停止检测与自动登录
- **记住密码**：Windows 用 DPAPI 加密（仅当前用户可解密），其他平台为 base64 混淆
- **门户优先探测**：先测认证服务器 TCP 是否可达，避免「无法连接到远程服务器」
- **WiFi 自愈**：启动与切换网络时自动把校园网配置设为「自动连接」，修复开机不自动连校园网
- **开机自动启动**：勾选即写入系统；首次登录成功会自动开启
- **系统托盘**：显示窗口 / 暂停继续 / 彻底退出
- **每日名言**：底部按日轮换（30 条文案）
- **不符提示**：连的网络不是目标网且目标不在附近时弹窗，可选 暂停 / 忽略 / 彻底关闭
- **彻底关闭确认**：退出前确认，可勾选「不再提示」
- **运行日志**：界面内实时查看，含每次登录的账号、运营商与响应消息

## 平台差异

| 能力 | Windows | Linux |
| --- | --- | --- |
| WiFi 读取 / 切换 | `netsh wlan` | `nmcli`（NetworkManager） |
| WiFi 自愈 | `netsh wlan set profileparameter` | `nmcli connection modify` |
| 密码保护 | DPAPI（仅当前用户可解密） | base64 混淆（**非加密**） |
| 系统托盘 | `tray-icon`（原生托盘） | `ksni`（StatusNotifierItem；无 SNI 宿主时自动降级为无托盘） |
| 开机自启 | 注册表 Run 项 | XDG Autostart（`~/.config/autostart/`） |

调用外部命令时，Windows 下会隐藏控制台窗口（`CREATE_NO_WINDOW`）并把代码页切到
UTF-8，否则中文 WiFi 名会乱码。

## 构建与运行

```bash
cargo run --release          # 直接运行
cargo build --release        # 产物在 target/release/guunnet
cargo test                   # 单元测试（协议解析、退避策略、名言轮换）
```

Linux 需要 `nmcli`（NetworkManager）读取与切换 WiFi：

```bash
sudo pacman -S networkmanager   # 已安装可忽略
```

Windows 直接 `cargo build --release` 即可，无额外系统依赖（WiFi 走系统自带 `netsh`）。

## 配置文件

查找顺序：

1. `--config <路径>` 或 `-c <路径>` 命令行参数
2. 可执行文件同目录的 `config.json`（便携模式）
3. 用户配置目录：Linux `~/.config/guunnet/config.json`，Windows `%APPDATA%\guunnet\config.json`

密码字段 `passEnc` 带前缀标识：`dpapi:` 为 DPAPI 密文，`b64:` 为混淆文本。

## 实现说明

- 认证地址默认 `10.10.90.2:801`，校园网 SSID 关键词默认 `CMCC-GNNUN`；
  其他学校可通过配置文件的 `portalHost` / `portalPort` / `ssidKeyword` 修改。
- 联网判定使用 `generate_204` 与 `msftconnecttest`：门户重定向页同样返回 200，
  因此必须校验响应内容，否则会把「未认证」误判为「已联网」而不去登录。
- egui 内置字体不含 CJK，程序启动时会从系统字体（Noto Sans CJK / 微软雅黑等）
  加载一份回退字体，缺失时界面中文会显示为方框。
- 注：Dr.COM 门户在**设备已在线时不校验密码**（一律回 `IP 已经在线`），
  所以「在线时测试登录」无法判断密码对错。
