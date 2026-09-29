# 校园网自连 (PowerShell 版)

赣南师范大学校园网自动登录工具，使用 **PowerShell + WinForms** 实现（无需 Python 环境）。

## 功能

- **自动登录**：ePortal 认证，支持 电信 / 移动 / 联通
- **记住密码**：使用 Windows DPAPI 加密保存（仅当前用户可解密）
- **断线自动重连**：断网、休眠唤醒后自动重新登录
- **自适应轮询**：需要登录时 2 秒一次，稳定在线时 10 秒一次（省电）
- **目标网络**：可指定固定连接的 WiFi；目标不在附近时不会抢网
- **网络与目标不符提示**：连了别的网络（如热点）时弹窗，可选暂停/忽略/关闭
- **暂停/继续**：临时停止自动连接
- **每日名言**：底部显示，自动按日更换
- **托盘常驻**：关闭窗口 = 最小化到托盘

## 运行要求

- Windows 10 / 11（系统自带 PowerShell 5.1）
- 无需安装任何依赖

## 使用方法

1. 将 `GUUNNet.ps1` 和 `GUUNNet.ico` 放在同一文件夹
2. 右键 `GUUNNet.ps1` → 使用 PowerShell 运行（或用下面的方式静默启动）

**静默启动**（不闪黑窗）：在同目录新建 `GUUNNet-Silent.vbs`：

```vbs
Set fso = CreateObject("Scripting.FileSystemObject")
scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
CreateObject("WScript.Shell").Run "powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File """ & scriptDir & "\GUUNNet.ps1""", 0, False
```

3. 填写运营商 / 学号 / 密码 → 点「连接」
4. 勾选「开机自动启动」可加入开机启动（程序会自动生成启动项）

## 打包成 exe（可选）

用 [ps2exe](https://github.com/MScholtes/PS2EXE)：

```powershell
Invoke-ps2exe -inputFile GUUNNet.ps1 -outputFile GUUNNet.exe -iconFile GUUNNet.ico -noConsole
```

## 版本

当前版本：**v1.0.0**（窗口标题、托盘提示会显示版本号）。

版本号唯一来源是 `GUUNNet.ps1` 顶部的 `$script:AppVer`；打包成 exe 时，exe 的文件版本应与它保持一致。

## 说明

- 本版本与仓库中的 Python 版（`main.py`）功能对应，是另一种技术栈的实现
- 认证服务器地址针对 **赣南师范大学**（`10.10.90.2:801`），其他学校需自行修改
