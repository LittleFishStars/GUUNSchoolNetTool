# ============================================================
#  校园网自动连接工具 (ePortal)   —   by opencode
#  基于同学源码修正协议，解决：记住密码 / 回车连接 / 断线休眠自动重连
# ============================================================
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Security

# 声明 DPI 感知：让 Windows 不再对窗口做位图拉伸 → 字体变清晰（不模糊）
try {
    Add-Type @"
using System;
using System.Runtime.InteropServices;
public class DpiHelper {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int value);
    public static void Enable() {
        try { SetProcessDpiAwareness(2); }   // 2 = PER_MONITOR_DPI_AWARE（Win8.1+）
        catch { try { SetProcessDPIAware(); } catch {} }
    }
}
"@
    [DpiHelper]::Enable()
} catch {}

[System.Windows.Forms.Application]::EnableVisualStyles()

# 设置进程的 AppUserModelID → 让 Windows 把它当成独立程序，
# 任务栏/最小化按钮就用我们的图标，而不是宿主的 PowerShell 图标
try {
    Add-Type @"
using System;
using System.Runtime.InteropServices;
public class AppIdHelper {
    [DllImport("shell32.dll", SetLastError=true)]
    public static extern int SetCurrentProcessExplicitAppUserModelID([MarshalAs(UnmanagedType.LPWStr)] string AppID);
}
"@
    [void][AppIdHelper]::SetCurrentProcessExplicitAppUserModelID('opencode.GUUNNet.Tool')
} catch {}

# 关键：把控制台输出编码设为 UTF-8，否则 netsh 返回的中文 WiFi 名会乱码
# （乱码会导致中文热点名扫到了却认不出来）
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

# ---------------- 应用目录（兼容 .ps1 运行 与 .exe 运行）----------------
$script:isExe = $false
function Get-AppDir {
    try {
        $p = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
        if ($p -and ($p -notmatch 'powershell\.exe$') -and ($p -notmatch 'pwsh\.exe$')) {
            return (Split-Path -Parent $p)
        }
    } catch {}
    if ($PSScriptRoot) { return $PSScriptRoot }
    try { return (Split-Path -Parent $MyInvocation.MyCommand.Definition) } catch {}
    return (Get-Location).Path
}
$ScriptDir = Get-AppDir
try {
    $m = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    if ($m -and ($m -notmatch 'powershell\.exe$') -and ($m -notmatch 'pwsh\.exe$')) { $script:isExe = $true; $script:exePath = $m }
} catch {}
$CfgFile = Join-Path $ScriptDir 'config.json'
$LogFile = Join-Path $ScriptDir 'log.txt'
$VbsFile = Join-Path $ScriptDir 'GUUNNet-Silent.vbs'
$IcoFile = Join-Path $ScriptDir 'GUUNNet.ico'

# 取程序图标（窗口/托盘/快捷方式共用）：exe 用自身嵌入图标，脚本用同目录 .ico
function Get-AppIcon {
    try {
        $ico = New-Object System.Drawing.Icon($IcoFile)
        if ($ico) { return $ico }
    } catch {}
    try {
        if ($script:isExe -and $script:exePath -and (Test-Path -LiteralPath $script:exePath)) {
            $ic = [System.Drawing.Icon]::ExtractAssociatedIcon($script:exePath)
            if ($ic) { return $ic }
        }
    } catch {}
    return [System.Drawing.SystemIcons]::Application
}

# Wlan 辅助类：内嵌 C# 源码（打包成 exe 后不再依赖外部 .cs 文件）
$script:canForceScan = $null   # $null=未尝试, $true/$false=已确定
$script:WlanCsSource = @'
using System;
using System.Runtime.InteropServices;

public class Wlan
{
    [StructLayout(LayoutKind.Sequential)]
    public struct WLAN_INTERFACE_INFO
    {
        public Guid InterfaceGuid;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)]
        public string strInterfaceDescription;
        public int isState;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct WLAN_INTERFACE_INFO_LIST
    {
        public int dwNumberOfItems;
        public int dwIndex;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 8)]
        public WLAN_INTERFACE_INFO[] InterfaceInfo;
    }

    [DllImport("wlanapi.dll")]
    public static extern int WlanOpenHandle(int dwClientVersion, IntPtr pReserved, out int pdwNegotiatedVersion, out IntPtr phClientHandle);

    [DllImport("wlanapi.dll")]
    public static extern int WlanEnumInterfaces(IntPtr hClientHandle, IntPtr pReserved, out IntPtr ppInterfaceList);

    [DllImport("wlanapi.dll")]
    public static extern int WlanScan(IntPtr hClientHandle, ref Guid pInterfaceGuid, IntPtr pDot11Ssid, IntPtr pIeData, IntPtr pReserved);

    [DllImport("wlanapi.dll")]
    public static extern int WlanCloseHandle(IntPtr hClientHandle, IntPtr pReserved);

    [DllImport("wlanapi.dll")]
    public static extern void WlanFreeMemory(IntPtr pMemory);

    public static bool ForceScan()
    {
        int neg = 0;
        IntPtr h = IntPtr.Zero;
        if (WlanOpenHandle(2, IntPtr.Zero, out neg, out h) != 0) return false;
        try
        {
            IntPtr listPtr = IntPtr.Zero;
            if (WlanEnumInterfaces(h, IntPtr.Zero, out listPtr) != 0) return false;
            try
            {
                var list = (WLAN_INTERFACE_INFO_LIST)Marshal.PtrToStructure(listPtr, typeof(WLAN_INTERFACE_INFO_LIST));
                if (list.dwNumberOfItems < 1) return false;
                Guid g = list.InterfaceInfo[0].InterfaceGuid;
                int r = WlanScan(h, ref g, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
                return r == 0;
            }
            finally { WlanFreeMemory(listPtr); }
        }
        finally { WlanCloseHandle(h, IntPtr.Zero); }
    }
}
'@
function Ensure-WlanClass {
    if ($null -ne $script:canForceScan) { return $script:canForceScan }
    try {
        Add-Type -TypeDefinition $script:WlanCsSource -ErrorAction Stop
        $script:canForceScan = $true
    } catch { $script:canForceScan = $false }
    return $script:canForceScan
}

# ---------------- 参数 ----------------
$PortalBase  = 'http://10.10.90.2:801/eportal/portal/login'
$SsidKeyword = 'CMCC-GNNUN'                                     # 校园网 SSID 前缀
$CheckUrl    = 'http://www.baidu.com'                            # 联网检测
$CheckUrl2   = 'http://www.msftconnecttest.com/connecttest.txt'
$CheckExpect = 'Microsoft Connect Test'
# 自适应轮询：需要登录时快速重试，稳定在线时放慢以省电
$PollFastSec = 2      # 校园网但未登录 / 正在切换 → 快速
$PollSlowSec = 10     # 已联网稳定 / 非校园网     → 慢速省电
$LoginCooldownSec = 3

# 运营商 → ePortal 后缀（与同学源码一致）
$CarrierMap = [ordered]@{ '移动' = 'cmcc'; '电信' = 'dx'; '联通' = 'unicom' }

# ---------------- 单实例 + 再次启动时唤出已有窗口 ----------------
# 命名事件：用于让"已在运行的实例"把窗口显示到前台
$createdNew = $false
$script:showEvent = New-Object System.Threading.EventWaitHandle(
    $false, [System.Threading.EventResetMode]::AutoReset, 'Global\GUUNNetTool_Show', [ref]$createdNew)
$script:mutex = New-Object System.Threading.Mutex($false, 'Global\GUUNNetTool_OpenCode')
if (-not $script:mutex.WaitOne(0)) {
    # 已有实例在运行 → 通知它把窗口唤到前台，然后本进程退出
    try { [void]$script:showEvent.Set() } catch {}
    Start-Sleep -Milliseconds 400
    exit
}

# ---------------- 工具函数 ----------------
$script:LogMaxBytes = 1024 * 1024    # 日志超过 1MB 才精简（每行仅约 55 字节，足够久）
$script:LogKeepLines = 1000          # 精简后保留最近 1000 行
function Write-Log([string]$msg) {
    try {
        Add-Content -LiteralPath $LogFile -Value ("[{0}] {1}" -f (Get-Date).ToString('MM-dd HH:mm:ss'), $msg) -Encoding UTF8
        # 日志轮转：超出上限时只保留最近若干行，避免无限增长
        $fi = Get-Item -LiteralPath $LogFile -ErrorAction SilentlyContinue
        if ($fi -and $fi.Length -gt $script:LogMaxBytes) {
            $keep = Get-Content -LiteralPath $LogFile -Tail $script:LogKeepLines -ErrorAction SilentlyContinue
            $header = "[{0}] ---- 日志已精简（保留最近 {1} 行）----" -f (Get-Date).ToString('MM-dd HH:mm:ss'), $script:LogKeepLines
            ($header, $keep) | Set-Content -LiteralPath $LogFile -Encoding UTF8
        }
    } catch {}
}
function Protect-Text([string]$s) {
    if ([string]::IsNullOrEmpty($s)) { return '' }
    try {
        $b = [Text.Encoding]::UTF8.GetBytes($s)
        $e = [Security.Cryptography.ProtectedData]::Protect($b, $null, 'CurrentUser')
        return [Convert]::ToBase64String($e)
    } catch { return '' }
}
function Unprotect-Text([string]$s) {
    if ([string]::IsNullOrEmpty($s)) { return '' }
    try {
        $e = [Convert]::FromBase64String($s)
        $b = [Security.Cryptography.ProtectedData]::Unprotect($e, $null, 'CurrentUser')
        return [Text.Encoding]::UTF8.GetString($b)
    } catch { return '' }
}
# 无窗口执行外部命令
#  1) 用 cmd 强制 UTF-8 代码页(65001) → 无论父进程有无控制台，netsh 都按 UTF-8 输出（中文不乱码）
#  2) CreateNoWindow → 不闪黑框
#  3) 严格 UTF-8 解码，非法字节则回退 GBK（双保险）
function Invoke-Exe([string]$file, [string]$arguments) {
    try {
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $env:ComSpec          # cmd.exe
        $psi.Arguments = '/c chcp 65001>nul & ' + $file + ' ' + $arguments
        $psi.UseShellExecute = $false
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.CreateNoWindow = $true
        $p = [System.Diagnostics.Process]::Start($psi)
        $ms = New-Object System.IO.MemoryStream
        $p.StandardOutput.BaseStream.CopyTo($ms)
        $p.StandardError.ReadToEnd() | Out-Null
        $p.WaitForExit()
        $bytes = $ms.ToArray()
        try {
            # 严格模式：遇到非法 UTF-8 字节会抛异常
            $strict = New-Object System.Text.UTF8Encoding($false, $true)
            return $strict.GetString($bytes)
        } catch {
            # 回退：系统 ANSI(中文=GBK)
            return [System.Text.Encoding]::GetEncoding(936).GetString($bytes)
        }
    } catch { return '' }
}
function Get-CurSSID {
    try {
        $txt = Invoke-Exe 'netsh.exe' 'wlan show interfaces'
        foreach ($line in ($txt -split "`r?`n")) {
            if ($line -match '^\s*SSID\s*:\s*(.+?)\s*$') { return $Matches[1] }
        }
    } catch {}
    return ''
}
# 去抖版：容忍"瞬时读不到 SSID"（开机/重连时会闪断），避免误判为断开而反复重连
$script:ssidLast = ''
$script:ssidLastAt = [datetime]::MinValue
function Get-CurSSIDStable {
    $raw = Get-CurSSID
    if ($raw) {
        $script:ssidLast = $raw
        $script:ssidLastAt = Get-Date
        return $raw
    }
    # 读到空：若 6 秒内还读到过某个 SSID，就当作"还在那个网络"（容错）
    if ($script:ssidLast -and (((Get-Date) - $script:ssidLastAt).TotalSeconds -lt 6)) {
        return $script:ssidLast
    }
    $script:ssidLast = ''
    return ''
}
# 取已保存的 WiFi 配置文件（名字都是 ASCII 或可正常读取）
function Get-WlanProfiles {
    $names = @()
    try {
        $txt = Invoke-Exe 'netsh.exe' 'wlan show profiles'
        foreach ($line in ($txt -split "`r?`n")) {
            $m = [regex]::Match($line, ':\s*(.+)$')
            if ($m.Success) {
                $n = $m.Groups[1].Value.Trim()
                if ($n) { $names += $n }
            }
        }
    } catch {}
    return ($names | Sort-Object -Unique)
}
# 扫描附近可见的 WiFi（含未保存的）。返回 SSID 数组。
function Get-VisibleSsids {
    $names = @()
    try {
        $txt = Invoke-Exe 'netsh.exe' 'wlan show networks'
        foreach ($line in ($txt -split "`r?`n")) {
            # 只匹配 "SSID N : 名称"，不匹配 "BSSID N : ..."
            $m = [regex]::Match($line, '^\s*SSID\s+\d+\s*:\s*(.+?)\s*$')
            if ($m.Success) {
                $n = $m.Groups[1].Value.Trim()
                if ($n) { $names += $n }
            }
        }
    } catch {}
    return ($names | Sort-Object -Unique)
}
# 连接到指定的 WiFi 配置文件
function Connect-ToSsid([string]$name) {
    try {
        $safe = $name -replace '"', ''
        return (Invoke-Exe 'netsh.exe' ('wlan connect name="' + $safe + '"'))
    } catch { return '' }
}
# 等待（期间保持界面响应）
function Wait-UI([int]$ms) {
    $end = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $end) {
        try { [System.Windows.Forms.Application]::DoEvents() } catch {}
        Start-Sleep -Milliseconds 100
    }
}
# 附近可见网络缓存（避免每次检测都强制扫描）
$script:visCache = @()
$script:visCacheAt = [datetime]::MinValue
$script:visCacheTtlSec = 60
function Get-VisibleSsidsCached([bool]$force) {
    $age = ((Get-Date) - $script:visCacheAt).TotalSeconds
    if ((-not $force) -and ($age -lt $script:visCacheTtlSec)) { return $script:visCache }
    if (Ensure-WlanClass) {
        try { [void][Wlan]::ForceScan(); Wait-UI 3500 } catch {}
    }
    $script:visCache = @(Get-VisibleSsids)
    $script:visCacheAt = Get-Date
    return $script:visCache
}
# 目标网络此刻是否在附近（可见）
function Test-SsidVisible([string]$name) {
    if ([string]::IsNullOrEmpty($name)) { return $false }
    $list = Get-VisibleSsidsCached $false
    return ($list -contains $name)
}
# 联网状态缓存（避免每次检测都做慢速外网请求）
$script:netCache = $false
$script:netCacheAt = [datetime]::MinValue
function Test-InternetCached([int]$ttlSec) {
    if (-not $ttlSec) { $ttlSec = 12 }
    if (((Get-Date) - $script:netCacheAt).TotalSeconds -lt $ttlSec) { return $script:netCache }
    $r = Test-Internet
    $script:netCache = $r
    $script:netCacheAt = Get-Date
    return $r
}
# 状态变化日志：只在"状态签名"变化时写日志，避免刷屏（用于诊断开机/断网问题）
$script:lastStateSig = ''
function Log-State([string]$sig) {
    if ($sig -ne $script:lastStateSig) {
        $script:lastStateSig = $sig
        Write-Log ('状态: ' + $sig)
    }
}
# 快速测试某主机端口是否可连（默认超时 800ms）
function Test-TcpPort([string]$host_, [int]$port, [int]$timeoutMs) {
    if (-not $timeoutMs) { $timeoutMs = 800 }
    $client = $null
    try {
        $client = New-Object System.Net.Sockets.TcpClient
        $iar = $client.BeginConnect($host_, $port, $null, $null)
        $ok = $iar.AsyncWaitHandle.WaitOne($timeoutMs, $false)
        if ($ok) { $client.EndConnect($iar) }
        return $ok
    } catch {
        return $false
    } finally {
        if ($client) { try { $client.Close() } catch {} }
    }
}
# 认证服务器是否已可达（= 网络真正就绪，可以登录了）
$script:PortalHost = '10.10.90.2'
$script:PortalPort = 801
function Test-PortalReachable {
    return (Test-TcpPort $script:PortalHost $script:PortalPort 800)
}
# 等待网络就绪：反复探测认证服务器，最多等 $maxSec 秒
# 返回 $true=已就绪；$false=超时
function Wait-NetReady([int]$maxSec, [string]$statusPrefix) {
    $start = Get-Date
    while (((Get-Date) - $start).TotalSeconds -lt $maxSec) {
        if (Test-PortalReachable) { return $true }
        if ($statusPrefix) {
            $el = [int]((Get-Date) - $start).TotalSeconds
            Set-Status ($statusPrefix + '… ' + $el + 's') 'OrangeRed'
        }
        Wait-UI 700
    }
    return (Test-PortalReachable)
}
function Get-LocalIP {
    # 取 10.x 开头的本机 IP（与同学源码一致）
    try {
        $addrs = [System.Net.Dns]::GetHostAddresses([System.Net.Dns]::GetHostName())
        foreach ($a in $addrs) {
            if ($a.AddressFamily -eq 'InterNetwork' -and $a.IPAddressToString -match '^10\.') { return $a.IPAddressToString }
        }
        foreach ($a in $addrs) { if ($a.AddressFamily -eq 'InterNetwork') { return $a.IPAddressToString } }
    } catch {}
    return ''
}
function Test-Internet {
    # 严格判断"真联网"：必须拿到预期结果，
    # 否则门户重定向页也会返回 200，被误判成已联网 → 导致不登录。
    # 优先用最快的 generate_204（约 150ms），后再用内容校验兜底。
    try {
        $r1 = Invoke-WebRequest -Uri 'http://connectivitycheck.gstatic.com/generate_204' -TimeoutSec 3 -UseBasicParsing -ErrorAction Stop
        if ($r1.StatusCode -eq 204) { return $true }
    } catch {}
    try {
        $r = Invoke-WebRequest -Uri $CheckUrl2 -TimeoutSec 3 -UseBasicParsing -ErrorAction Stop
        if ($r.Content -and $r.Content.Trim() -eq $CheckExpect) { return $true }
    } catch {}
    return $false
}

function Invoke-PortalLogin([string]$schoolId, [string]$pass, [string]$carrierName) {
    # 返回 @{ ok=[bool]; authfail=[bool]; msg=[string] }
    if ([string]::IsNullOrEmpty($schoolId) -or [string]::IsNullOrEmpty($pass)) {
        return @{ ok = $false; authfail = $false; msg = '学号或密码为空' }
    }
    $carrier = $CarrierMap[$carrierName]
    if (-not $carrier) { $carrier = 'dx' }
    $ip = Get-LocalIP
    $acct = ',0,' + $schoolId + '@' + $carrier          # 关键：账号格式
    $url = $PortalBase +
           '?callback=jsonpReturn' +
           '&user_account=' + [Uri]::EscapeDataString($acct) +
           '&user_password=' + [Uri]::EscapeDataString($pass) +
           '&wlan_user_ip=' + [Uri]::EscapeDataString($ip) +
           '&_=' + [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    try {
        $r = Invoke-WebRequest -Uri $url -TimeoutSec 10 -UseBasicParsing -ErrorAction Stop
        $txt = $r.Content
        $json = $txt
        $m = [regex]::Match($json, '\{.*\}')
        if ($m.Success) { $json = $m.Value }
        try {
            $obj = $json | ConvertFrom-Json
            $res  = [string]$obj.result
            $code = [string]$obj.ret_code
            $msg  = [string]$obj.msg
            # result:1 = 认证成功；result:0 + ret_code:2 = 已经在线（也算成功）
            if ($res -eq '1' -or $code -eq '2') {
                return @{ ok = $true; authfail = $false; msg = ($(if($msg){$msg}else{'认证成功'})) }
            }
            # 账号或密码错误（用于提示重新输入）
            $authfail = ($msg -match '密码错误|账号密码|账户密码|统一身份认证|用户名或密码')
            return @{ ok = $false; authfail = $authfail; msg = $(if($msg){$msg}else{('未知响应: ' + $txt)}) }
        } catch {
            return @{ ok = $false; authfail = $false; msg = '解析响应失败: ' + $txt }
        }
    } catch {
        return @{ ok = $false; authfail = $false; msg = '请求失败: ' + $_.Exception.Message }
    }
}

# ---------------- 配置读写 ----------------
$script:cfg = [ordered]@{
    schoolId = ''; passEnc = ''; carrier = '电信'; remember = $true; auto = $true; boot = $false
    targetSsid = ''; switchNetwork = $true
}
function Load-Config {
    if (Test-Path -LiteralPath $CfgFile) {
        try {
            $o = Get-Content -LiteralPath $CfgFile -Raw -Encoding UTF8 | ConvertFrom-Json
            foreach ($k in @($script:cfg.Keys)) { if ($null -ne $o.$k) { $script:cfg[$k] = $o.$k } }
        } catch {}
    }
}
function Save-Config {
    try { ($script:cfg | ConvertTo-Json) | Set-Content -LiteralPath $CfgFile -Encoding UTF8 } catch {}
}
Load-Config

$script:lastLoginAt = [datetime]::MinValue         # 上次"登录成功"时间
$script:lastLoginAttemptAt = [datetime]::MinValue  # 上次"登录尝试"时间（失败也计，用于退避）
$script:loginFailCount = 0                         # 连续失败次数（用于指数退避）
$script:busy = $false
$script:lastSwitchAt = [datetime]::MinValue
$script:paused = $false                            # 暂停状态（仅本次运行有效）
$script:mismatchPrompted = $false                  # "目标不符"提示是否已弹过
$script:mismatchLastSsid = ''                      # 上次弹窗时的当前网络

# ---------------- 开机自启 ----------------
function Set-Autostart([bool]$on) {
    $startup = [Environment]::GetFolderPath('Startup')
    $lnkPath = Join-Path $startup '校园网自连.lnk'
    if ($on) {
        $ws = New-Object -ComObject WScript.Shell
        $sc = $ws.CreateShortcut($lnkPath)
        if ($script:isExe) {
            # 打包成 exe 后：直接用 exe 自身启动（无窗口，ps2exe 用 -WindowStyle Hidden 编译）
            $sc.TargetPath = $script:exePath
            $sc.Arguments = ''
        } else {
            # 脚本形式：用 wscript + VBS 静默启动（不闪黑窗）
            $vbs = 'CreateObject("WScript.Shell").Run "powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""' +
                   ($PSCommandPath -replace '"','""') + '""", 0, False'
            Set-Content -LiteralPath $VbsFile -Value $vbs -Encoding Default
            $sc.TargetPath = 'wscript.exe'
            $sc.Arguments = '"' + $VbsFile + '"'
        }
        $sc.WorkingDirectory = $ScriptDir
        $sc.Description = '校园网自动连接'
        # 快捷方式图标：exe 用自身，脚本用 ico 文件
        if ($script:isExe -and $script:exePath) { $sc.IconLocation = $script:exePath + ',0' }
        elseif (Test-Path -LiteralPath $IcoFile) { $sc.IconLocation = $IcoFile + ',0' }
        $sc.Save()
    } else {
        if (Test-Path -LiteralPath $lnkPath) { Remove-Item -LiteralPath $lnkPath -Force -ErrorAction SilentlyContinue }
    }
}

# ---------------- 每日励志名言 ----------------
$script:Quotes = @(
    '天行健，君子以自强不息。——《周易》',
    '纸上得来终觉浅，绝知此事要躬行。——陆游',
    '宝剑锋从磨砺出，梅花香自苦寒来。',
    '不积跬步，无以至千里；不积小流，无以成江海。——荀子',
    '绳锯木断，水滴石穿。——《汉书》',
    '长风破浪会有时，直挂云帆济沧海。——李白',
    '有志者，事竟成。——《后汉书》',
    '博观而约取，厚积而薄发。——苏轼',
    '千磨万击还坚劲，任尔东西南北风。——郑燮',
    '路漫漫其修远兮，吾将上下而求索。——屈原',
    '骐骥一跃，不能十步；驽马十驾，功在不舍。——荀子',
    '盛年不重来，一日难再晨。——陶渊明',
    '会当凌绝顶，一览众山小。——杜甫',
    '锲而不舍，金石可镂。——荀子',
    '莫等闲，白了少年头，空悲切。——岳飞',
    '业精于勤，荒于嬉；行成于思，毁于随。——韩愈',
    '读书破万卷，下笔如有神。——杜甫',
    '黑发不知勤学早，白首方悔读书迟。——颜真卿',
    '志不强者智不达。——墨子',
    '天将降大任于是人也，必先苦其心志。——《孟子》',
    '少壮不努力，老大徒伤悲。——《长歌行》',
    '苟有恒，何必三更眠五更起。——颜真卿',
    '山重水复疑无路，柳暗花明又一村。——陆游',
    '积土成山，风雨兴焉；积水成渊，蛟龙生焉。——荀子',
    '纸上画饼终难饱，实干方能有所成。',
    '锲而不舍，朽木不折；锲而不舍，金石可镂。',
    '穷且益坚，不坠青云之志。——王勃',
    '宝剑不磨要生锈，人不学习要落后。',
    '天生我材必有用。——李白',
    '世上无难事，只要肯登攀。——毛泽东'
)
function Get-TodayQuote {
    $doy = (Get-Date).DayOfYear
    return $script:Quotes[($doy - 1) % $script:Quotes.Count]
}
# 刷新名言（日期变化时更新；也可手动调用）
$script:quoteDay = -1
function Update-Quote {
    try {
        $doy = (Get-Date).DayOfYear
        if ($doy -ne $script:quoteDay) {
            $script:quoteDay = $doy
            if ($script:lblQuote) { $script:lblQuote.Text = Get-TodayQuote }
        }
    } catch {}
}

# ---------------- 主窗口 ----------------
$form = New-Object System.Windows.Forms.Form
$script:form = $form
$form.Text = '校园网自连'
$form.StartPosition = 'CenterScreen'
$form.FormBorderStyle = 'Sizable'      # 可调整大小 / 可最大化
$form.MaximizeBox = $true
$form.MinimizeBox = $true
try { $form.Icon = Get-AppIcon } catch {}   # 窗口/任务栏图标（替换默认的 PowerShell 图标）

# DPI 缩放系数（进程已声明 DPI 感知，这里手动缩放控件，保证清晰且大小合适）
$script:dpiScale = 1.0
try {
    $applied = (Get-ItemProperty 'HKCU:\Control Panel\Desktop\WindowMetrics' -Name AppliedDPI -EA SilentlyContinue).AppliedDPI
    if ($applied -and [int]$applied -gt 0) { $script:dpiScale = [int]$applied / 96.0 }
} catch {}
if ($script:dpiScale -lt 1) { $script:dpiScale = 1 }
function S([double]$v) { return [int][math]::Round($v * $script:dpiScale) }

$form.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', (9 * $script:dpiScale))
$form.ClientSize = New-Object System.Drawing.Size((S 430), (S 556))
$form.MinimumSize = New-Object System.Drawing.Size((S 410), (S 520))
$form.Add_Shown({ $script:form.Activate(); $script:form.TopMost = $true; $script:form.TopMost = $false })

function New-Label($text, $x, $y, $w, $h) {
    $l = New-Object System.Windows.Forms.Label
    $l.Text = $text
    $l.Location = New-Object System.Drawing.Point((S $x), (S $y))
    if ($h) { $l.Size = New-Object System.Drawing.Size((S $w), (S $h)) }
    else { $l.Size = New-Object System.Drawing.Size((S $w), (S 24)) }
    $l.Anchor = 'Top,Left'
    return $l
}

$lblCarrier = New-Label '运营商' 24 20 66
$cmbCarrier = New-Object System.Windows.Forms.ComboBox
$cmbCarrier.Location = New-Object System.Drawing.Point((S 96), (S 18))
$cmbCarrier.Size = New-Object System.Drawing.Size((S 120), (S 28))
$cmbCarrier.DropDownStyle = 'DropDownList'
[void]$cmbCarrier.Items.AddRange(@('电信','移动','联通'))
$cmbCarrier.SelectedItem = '电信'

$lblUser = New-Label '学号' 24 58 66
$txtUser = New-Object System.Windows.Forms.TextBox
$txtUser.Location = New-Object System.Drawing.Point((S 96), (S 56))
$txtUser.Size = New-Object System.Drawing.Size((S 286), (S 28))

$lblPass = New-Label '密码' 24 96 66
$txtPass = New-Object System.Windows.Forms.TextBox
$txtPass.Location = New-Object System.Drawing.Point((S 96), (S 94))
$txtPass.Size = New-Object System.Drawing.Size((S 218), (S 28))
$txtPass.UseSystemPasswordChar = $true

# 显示/隐藏密码
$chkShowPass = New-Object System.Windows.Forms.CheckBox
$chkShowPass.Text = '显示'
$chkShowPass.Location = New-Object System.Drawing.Point((S 322), (S 97))
$chkShowPass.Size = New-Object System.Drawing.Size((S 62), (S 24))
$chkShowPass.Add_Click({
    $txtPass.UseSystemPasswordChar = -not $chkShowPass.Checked
})

$chkRemember = New-Object System.Windows.Forms.CheckBox
$chkRemember.Text = '记住密码（加密保存）'
$chkRemember.Location = New-Object System.Drawing.Point((S 96), (S 126))
$chkRemember.Size = New-Object System.Drawing.Size((S 220), (S 24))

$chkAuto = New-Object System.Windows.Forms.CheckBox
$chkAuto.Text = '自动重连（断网/休眠后自动登录）'
$chkAuto.Location = New-Object System.Drawing.Point((S 96), (S 152))
$chkAuto.Size = New-Object System.Drawing.Size((S 330), (S 24))

$chkBoot = New-Object System.Windows.Forms.CheckBox
$chkBoot.Text = '开机自动启动'
$chkBoot.Location = New-Object System.Drawing.Point((S 96), (S 178))
$chkBoot.Size = New-Object System.Drawing.Size((S 330), (S 24))

# 自动切换到目标网络（与上面三个勾选框放在一起）
$chkSwitch = New-Object System.Windows.Forms.CheckBox
$chkSwitch.Text = '自动切换到目标网络'
$chkSwitch.Location = New-Object System.Drawing.Point((S 96), (S 204))
$chkSwitch.Size = New-Object System.Drawing.Size((S 330), (S 24))

# 目标网络：选择要连接/登录的 WiFi
$lblTarget = New-Label '目标网络' 24 236 66
$cmbTarget = New-Object System.Windows.Forms.ComboBox
$cmbTarget.Location = New-Object System.Drawing.Point((S 96), (S 234))
$cmbTarget.Size = New-Object System.Drawing.Size((S 244), (S 28))
$cmbTarget.DropDownStyle = 'DropDownList'

# 按钮统一字体（显式指定同一个 Font 对象，避免各按钮渲染不一致导致某些字看起来不一样）
$script:btnFont = New-Object System.Drawing.Font('微软雅黑', (10 * $script:dpiScale), [System.Drawing.FontStyle]::Regular)

$btnRefresh = New-Object System.Windows.Forms.Button
$btnRefresh.Text = '刷新'
$btnRefresh.Location = New-Object System.Drawing.Point((S 348), (S 233))
$btnRefresh.Size = New-Object System.Drawing.Size((S 70), (S 29))
$btnRefresh.FlatStyle = 'Flat'
$btnRefresh.Font = $script:btnFont

# 说明：本工具只做校园网(ePortal)登录，不负责连接手机热点
$lblHint = New-Object System.Windows.Forms.Label
$lblHint.Text = '提示：仅用于校园网登录，不支持热点。'
$lblHint.Location = New-Object System.Drawing.Point((S 24), (S 268))
$lblHint.Size = New-Object System.Drawing.Size((S 394), (S 24))
$lblHint.ForeColor = [System.Drawing.Color]::FromArgb(150, 110, 40)
$lblHint.Anchor = 'Top,Left,Right'
$lblHint.AutoSize = $false

$btnConnect = New-Object System.Windows.Forms.Button
$btnConnect.Text = '连接'
$btnConnect.Location = New-Object System.Drawing.Point((S 96), (S 298))
$btnConnect.Size = New-Object System.Drawing.Size((S 132), (S 40))
$btnConnect.BackColor = [System.Drawing.Color]::FromArgb(99,102,241)
$btnConnect.ForeColor = [System.Drawing.Color]::White
$btnConnect.FlatStyle = 'Flat'
$btnConnect.Font = $script:btnFont

$btnReset = New-Object System.Windows.Forms.Button
$btnReset.Text = '重置'
$btnReset.Location = New-Object System.Drawing.Point((S 244), (S 298))
$btnReset.Size = New-Object System.Drawing.Size((S 132), (S 40))
$btnReset.FlatStyle = 'Flat'
$btnReset.Font = $script:btnFont
$btnReset.Add_Click({
    $ans = [System.Windows.Forms.MessageBox]::Show('清空已保存的账号密码？之后可重新输入。', '校园网自连', 'YesNo', 'Question')
    if ($ans -ne 'Yes') { return }
    $script:cfg.passEnc = ''
    $script:cfg.schoolId = ''
    Save-Config
    $txtUser.Text = ''; $txtPass.Text = ''
    Set-Status '已重置，请重新输入账号密码' 'DimGray'
    $txtUser.Focus()
})

# 暂停 / 继续（一个按钮，切换）
$script:btnPause = New-Object System.Windows.Forms.Button
$script:btnPause.Text = '暂停'
$script:btnPause.Location = New-Object System.Drawing.Point((S 96), (S 342))
$script:btnPause.Size = New-Object System.Drawing.Size((S 132), (S 30))
$script:btnPause.FlatStyle = 'Flat'
$script:btnPause.Font = $script:btnFont
$script:btnPause.Add_Click({ Toggle-Pause })

# 查看日志
$btnLog = New-Object System.Windows.Forms.Button
$btnLog.Text = '查看日志'
$btnLog.Location = New-Object System.Drawing.Point((S 244), (S 342))
$btnLog.Size = New-Object System.Drawing.Size((S 132), (S 30))
$btnLog.FlatStyle = 'Flat'
$btnLog.Font = $script:btnFont
$btnLog.Add_Click({
    try {
        if (Test-Path -LiteralPath $LogFile) { Start-Process notepad.exe -ArgumentList ('"' + $LogFile + '"') }
        else { Set-Status '暂无日志文件' 'DimGray' }
    } catch {}
})

# 彻底关闭（杀死后台进程）
$btnExit = New-Object System.Windows.Forms.Button
$btnExit.Text = '彻底关闭'
$btnExit.Location = New-Object System.Drawing.Point((S 96), (S 384))
$btnExit.Size = New-Object System.Drawing.Size((S 280), (S 30))
$btnExit.FlatStyle = 'Flat'
$btnExit.ForeColor = [System.Drawing.Color]::FromArgb(200, 40, 40)
$btnExit.Font = $script:btnFont
$btnExit.Add_Click({
    $ans = [System.Windows.Forms.MessageBox]::Show(
        '彻底关闭？程序将退出，开机/断网时不再自动连接。' + "`n" + '（下次开机仍会自动启动）',
        '校园网自连', 'YesNo', 'Question')
    if ($ans -ne 'Yes') { return }
    Stop-App
})

# 底部面板：状态栏 + 每日名言（随窗口缩放，始终贴底）
$panelBottom = New-Object System.Windows.Forms.Panel
$panelBottom.Dock = 'Bottom'
$panelBottom.Height = (S 96)
$panelBottom.Padding = New-Object System.Windows.Forms.Padding((S 16), (S 4), (S 16), (S 6))

$lblStatus = New-Object System.Windows.Forms.Label
$lblStatus.Text = '就绪'
$lblStatus.Dock = 'Top'
$lblStatus.Height = (S 40)
$lblStatus.ForeColor = [System.Drawing.Color]::DimGray
$lblStatus.AutoEllipsis = $true

$script:lblQuote = New-Object System.Windows.Forms.Label
$script:lblQuote.Text = Get-TodayQuote
$script:quoteDay = (Get-Date).DayOfYear
$script:lblQuote.Dock = 'Fill'
$script:lblQuote.AutoEllipsis = $true
$script:lblQuote.ForeColor = [System.Drawing.Color]::FromArgb(120, 90, 30)
$script:lblQuote.TextAlign = 'MiddleLeft'
$script:lblQuote.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', (9 * $script:dpiScale), [System.Drawing.FontStyle]::Italic)

$panelBottom.Controls.Add($script:lblQuote)
$panelBottom.Controls.Add($lblStatus)

$form.Controls.AddRange(@($lblCarrier,$cmbCarrier,$lblUser,$txtUser,$lblPass,$txtPass,$chkShowPass,$chkRemember,$chkAuto,$chkBoot,$lblTarget,$cmbTarget,$btnRefresh,$lblHint,$chkSwitch,$btnConnect,$btnReset,$script:btnPause,$btnLog,$btnExit))
$form.Controls.Add($panelBottom)
$form.AcceptButton = $btnConnect   # 回车即触发

function Set-Status([string]$text, [string]$color) {
    $lblStatus.Text = $text
    if ($color) { $lblStatus.ForeColor = [System.Drawing.Color]::FromName($color) }
}

# 彻底退出：停掉所有定时器、移除托盘图标、释放互斥锁，然后强制结束进程
function Stop-App {
    try { Write-Log '==== 用户彻底关闭程序 ====' } catch {}
    $script:reallyExit = $true
    # 停掉所有定时器，避免退出过程中还在跑检测
    foreach ($t in @($script:timer, $hideTimer, $showCheck, $tRefresh, $t0)) {
        try { if ($t) { $t.Stop() } } catch {}
    }
    # 移除托盘图标
    try { if ($ni) { $ni.Visible = $false; $ni.Dispose() } } catch {}
    # 释放单实例互斥锁
    try { $script:mutex.ReleaseMutex() } catch {}
    try { $script:mutex.Dispose() } catch {}
    # 关闭窗口并退出消息循环
    try { $script:form.Close() } catch {}
    try { [System.Windows.Forms.Application]::Exit() } catch {}
    # 最后强制结束本进程（确保后台被彻底杀死）
    try { [Environment]::Exit(0) } catch {}
}

# 暂停 / 继续（字面意义的暂停：不检测、不登录、不切换网络）
function Set-PauseUI {
    try {
        if ($script:btnPause) { $script:btnPause.Text = if ($script:paused) { '继续' } else { '暂停' } }
        if ($script:trayPauseItem) { $script:trayPauseItem.Text = if ($script:paused) { '继续' } else { '暂停' } }
    } catch {}
}
function Pause-App {
    $script:paused = $true          # 仅本次运行有效，重启后自动恢复
    Set-PauseUI
    Set-Status '已暂停：暂不检测网络、不自动登录、不切换网络。点「继续」恢复。' 'DimGray'
    Write-Log '已暂停'
}
function Resume-App {
    $script:paused = $false
    Set-PauseUI
    Set-Status ('已恢复 · ' + (Get-Date).ToString('HH:mm:ss')) 'Green'
    Write-Log '已恢复'
    PollFast
    try { $script:form.BeginInvoke([Action]{ try { Monitor-Tick } catch {} }) | Out-Null } catch {}
}
function Toggle-Pause {
    if ($script:paused) { Resume-App } else { Pause-App }
}

# 网络与目标不一致时的弹窗（暂停 / 忽略 / 彻底关闭）
function Show-MismatchDialog([string]$cur, [string]$target) {
    $dlg = New-Object System.Windows.Forms.Form
    $dlg.Text = '校园网自连'
    $dlg.StartPosition = 'CenterScreen'
    $dlg.FormBorderStyle = 'FixedDialog'
    $dlg.MaximizeBox = $false; $dlg.MinimizeBox = $false
    $dlg.TopMost = $true
    $dlg.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', (10 * $script:dpiScale))
    $dlg.ClientSize = New-Object System.Drawing.Size((S 420), (S 190))

    $lbl = New-Object System.Windows.Forms.Label
    $lbl.Location = New-Object System.Drawing.Point((S 18), (S 16))
    $lbl.Size = New-Object System.Drawing.Size((S 384), (S 96))
    $lbl.Text = "当前连接的网络是「$cur」，" + "`n" +
                "与设定的目标网络「$target」不一致，" + "`n" +
                "且目标网络当前不在附近。" + "`n`n" +
                "要不要暂停自动连接？"
    $dlg.Controls.Add($lbl)

    $bPause = New-Object System.Windows.Forms.Button
    $bPause.Text = '暂停程序'
    $bPause.Location = New-Object System.Drawing.Point((S 18), (S 128))
    $bPause.Size = New-Object System.Drawing.Size((S 120), (S 40))
    $bPause.DialogResult = 'Yes'

    $bIgnore = New-Object System.Windows.Forms.Button
    $bIgnore.Text = '忽略'
    $bIgnore.Location = New-Object System.Drawing.Point((S 150), (S 128))
    $bIgnore.Size = New-Object System.Drawing.Size((S 120), (S 40))
    $bIgnore.DialogResult = 'No'

    $bExit = New-Object System.Windows.Forms.Button
    $bExit.Text = '彻底关闭'
    $bExit.Location = New-Object System.Drawing.Point((S 282), (S 128))
    $bExit.Size = New-Object System.Drawing.Size((S 120), (S 40))
    $bExit.ForeColor = [System.Drawing.Color]::FromArgb(200, 40, 40)
    $bExit.DialogResult = 'Cancel'

    $dlg.Controls.AddRange(@($bPause, $bIgnore, $bExit))
    $dlg.AcceptButton = $bIgnore
    $dlg.CancelButton = $bIgnore

    $r = $dlg.ShowDialog()
    $dlg.Dispose()
    switch ($r) {
        'Yes'    { return 'pause' }
        'Cancel' { return 'exit' }
        default  { return 'ignore' }
    }
}

# 未保存网络的标记后缀 / 分组标题（供选择时识别）
$script:UNSAVED_TAG = '（未保存）'
$script:VISIBLE_HEADER = '── 附近可见（未保存，仅供参考）──'

# 刷新"目标网络"下拉列表：已保存(可选为目标) + 附近可见(仅参考)
# $forceScan=$true 时先强制触发一次 WiFi 扫描（否则 netsh 只回显缓存）
function Refresh-TargetList([bool]$forceScan) {
    $want  = [string]$script:cfg.targetSsid
    $saved = @(Get-WlanProfiles)
    if ($forceScan -and (Ensure-WlanClass)) {
        try { [void][Wlan]::ForceScan(); Wait-UI 3500 } catch {}
    }
    $visible = @(Get-VisibleSsids)

    $cmbTarget.BeginUpdate()
    $cmbTarget.Items.Clear()
    [void]$cmbTarget.Items.Add('自动（任意校园网）')                 # 0 号：不加限制
    foreach ($p in $saved) { [void]$cmbTarget.Items.Add($p) }        # 已保存 → 可选为目标

    # 附近可见但未保存的
    $extra = @($visible | Where-Object { $saved -notcontains $_ })
    if ($extra.Count -gt 0) {
        [void]$cmbTarget.Items.Add($script:VISIBLE_HEADER)
        foreach ($e in $extra) { [void]$cmbTarget.Items.Add($e + $script:UNSAVED_TAG) }
    }

    if ($want -and $cmbTarget.Items.Contains($want)) { $cmbTarget.SelectedItem = $want }
    else { $cmbTarget.SelectedIndex = 0 }
    $cmbTarget.EndUpdate()

    Write-Log ("refresh: 已保存=" + $saved.Count + " 可见=" + $visible.Count + " 新增未保存=" + $extra.Count)
    return @{ saved = $saved.Count; visible = $extra.Count }
}
# 读取当前选择的目标网络（'' = 自动 / 未保存项）
function Get-SelectedTarget {
    $sel = [string]$cmbTarget.SelectedItem
    if ($cmbTarget.SelectedIndex -le 0) { return '' }
    if ($sel -eq $script:VISIBLE_HEADER) { return '' }
    if ($sel.EndsWith($script:UNSAVED_TAG)) { return '' }
    return $sel
}
# 选到"未保存/分组标题"时 → 提示并退回"自动"
$cmbTarget.Add_SelectedIndexChanged({
    $sel = [string]$cmbTarget.SelectedItem
    if ($sel -and ($sel -eq $script:VISIBLE_HEADER -or $sel.EndsWith($script:UNSAVED_TAG))) {
        Set-Status '该网络未保存，不能作为自动目标。请先用系统 WiFi 列表连它一次（输密码），再点刷新。' 'OrangeRed'
        $cmbTarget.SelectedIndex = 0
    }
})
$btnRefresh.Add_Click({
    Set-Status '正在扫描附近网络…' 'DimGray'
    [void][System.Windows.Forms.Application]::DoEvents()
    $r = Refresh-TargetList $true      # 刷新时强制扫描
    Set-Status ('已刷新：已保存 ' + $r.saved + ' 个，附近可见(未保存) ' + $r.visible + ' 个') 'DimGray'
})

# ---------------- 托盘 ----------------
$ni = New-Object System.Windows.Forms.NotifyIcon
$script:appIcon = Get-AppIcon
$ni.Icon = $script:appIcon
$ni.Text = '校园网自连'
$ni.Visible = $true
$menu = New-Object System.Windows.Forms.ContextMenuStrip
[void]$menu.Items.Add('显示主界面')
[void]$menu.Items.Add('立即连接')
[void]$menu.Items.Add('重置账号密码')
$script:trayPauseItem = $menu.Items.Add('暂停')
[void]$menu.Items.Add('彻底关闭')
$ni.ContextMenuStrip = $menu
$menu.Items[0].Add_Click({ $form.Show(); $form.WindowState='Normal'; $form.Activate() })
$menu.Items[1].Add_Click({ try { Connect-Now } catch {} })
$menu.Items[2].Add_Click({ try { $btnReset.PerformClick() } catch {} })
$menu.Items[3].Add_Click({ Toggle-Pause })
$ni.Add_DoubleClick({ $form.Show(); $form.WindowState='Normal'; $form.Activate() })

# ---------------- 核心：登录 ----------------
function Connect-Now {
    if ($script:busy) { return }
    $script:busy = $true
    try {
        $user = $txtUser.Text.Trim()
        $pass = $txtPass.Text
        $carrier = [string]$cmbCarrier.SelectedItem
        if ([string]::IsNullOrEmpty($user) -or [string]::IsNullOrEmpty($pass)) {
            Set-Status '请先填写学号和密码' 'Red'; return
        }
        if ($chkRemember.Checked) {
            $script:cfg.schoolId = $user
            $script:cfg.passEnc = Protect-Text $pass
            $script:cfg.remember = $true
        } else {
            $script:cfg.remember = $false
            $script:cfg.passEnc = ''
            $script:cfg.schoolId = $user
        }
        $script:cfg.carrier = $carrier
        $script:cfg.auto = $chkAuto.Checked
        $script:cfg.boot = $chkBoot.Checked
        $script:cfg.switchNetwork = $chkSwitch.Checked
        $target = Get-SelectedTarget
        $script:cfg.targetSsid = $target
        Save-Config
        Set-Autostart $chkBoot.Checked

        # 若选了目标网络且当前不在该网络 → 先切过去
        if ($target) {
            $cur = Get-CurSSID
            if ($cur -ne $target) {
                Set-Status ('正在查找目标网络「' + $target + '」…') 'DimGray'
                if (Test-SsidVisible $target) {
                    Set-Status ('正在切换到「' + $target + '」…') 'DimGray'
                    Write-Log ("connect: 切换到目标网络 " + $target + " (当前 " + $cur + ")")
                    Connect-ToSsid $target | Out-Null
                } else {
                    Set-Status ('目标网络「' + $target + '」不在附近，无法切换（当前：' + $cur + '）') 'Red'
                    return
                }
            }
        }

        # 判断当前是否校园网：只有校园网才需要等门户 + 登录
        $curSsid = Get-CurSSID
        $isCampus = ($curSsid -like ('*' + $SsidKeyword + '*'))

        if (-not $isCampus) {
            # 非校园网（如手机热点）→ 无需等门户、无需登录，直接报告
            Set-Status ('已连接「' + $(if($curSsid){$curSsid}else{'你的网络'}) + '」（非校园网，无需登录）') 'Green'
            Write-Log ("connect: 当前非校园网[" + $curSsid + "]，跳过登录")
            if ($form.Visible) { $hideTimer.Start() }
            return
        }

        # 校园网：等网络真正就绪（认证服务器可达）再登录，避免"无法连接到远程服务器"
        if (-not (Test-PortalReachable)) {
            Set-Status '网络准备中，等待连接就绪…' 'OrangeRed'
            if (-not (Wait-NetReady 25 '正在等待网络就绪')) {
                Set-Status '✘ 网络迟迟未就绪，请检查 WiFi 是否正常' 'Red'
                return
            }
        }

        Set-Status '正在登录…' 'DimGray'
        $r = Invoke-PortalLogin $user $pass $carrier
        Write-Log ("login id=$user carrier=$carrier => ok=" + $r.ok + " msg=" + $r.msg)
        # 连接类错误（网络抖动）→ 短暂等待后重试一次，避免误报
        if (-not $r.ok -and -not $r.authfail -and ($r.msg -match '无法连接|超时|timed out|远程服务器')) {
            Set-Status '网络抖动，正在重试…' 'OrangeRed'
            Wait-UI 2500
            if (Test-PortalReachable) {
                $r = Invoke-PortalLogin $user $pass $carrier
                Write-Log ("login retry => ok=" + $r.ok + " msg=" + $r.msg)
            }
        }
        if ($r.ok) {
            $script:lastLoginAt = Get-Date
            $script:loginFailCount = 0        # 成功 → 清零退避
            # 登录成功 → 立刻把联网缓存置为"已联网"，避免因旧缓存而重复登录
            $script:netCache = $true
            $script:netCacheAt = Get-Date
            Set-Status ('✔ ' + (Get-Date).ToString('HH:mm:ss') + ' 连接成功 · ' + $r.msg) 'Green'
            # 连接成功后，若窗口可见则自动最小化到托盘
            if ($form.Visible) { $hideTimer.Start() }
        } else {
            Set-Status ('✘ 连接失败：' + $r.msg) 'Red'
            # 账号/密码错误 → 清空保存的密码，便于重新输入
            if ($r.authfail) {
                $script:cfg.passEnc = ''
                Save-Config
                Set-Status ('✘ 账号或密码错误，已清空保存的密码，请重新输入') 'Red'
                Show-MainWindow
                $txtPass.SelectAll(); $txtPass.Focus()
            } elseif ($r.msg -match '繁忙|稍后|请稍|too many|busy') {
                Write-Log ('手动登录遇到服务器繁忙: ' + $r.msg)
            }
        }
    } finally { $script:busy = $false }
}
$btnConnect.Add_Click({ Connect-Now })

# 显示并激活主窗口（后台静默运行时也可调用）
function Show-MainWindow {
    try {
        $script:form.Show()
        $script:form.WindowState = [System.Windows.Forms.FormWindowState]::Normal
        Update-Quote                    # 每次显示窗口也刷新名言（兜底）
        $script:form.Activate()
        $script:form.TopMost = $true
        $script:form.TopMost = $false
        $script:form.BringToFront()
    } catch {}
}

# ---------------- 后台自动检测 ----------------
# 自适应设置轮询间隔（快=需要抢救，慢=稳定省电）
function Set-Poll([int]$ms) {
    try {
        if ($script:timer -and $script:timer.Interval -ne $ms) {
            $script:timer.Interval = $ms
            Write-Log ('轮询间隔 → ' + [int]($ms / 1000) + 's')
        }
    } catch {}
}
function PollFast { Set-Poll ($PollFastSec * 1000) }
function PollSlow { Set-Poll ($PollSlowSec * 1000) }

function Monitor-Tick {
    # 已暂停 → 什么都不做（字面意义的暂停）
    if ($script:paused) { PollSlow; return }

    $target = [string]$script:cfg.targetSsid
    $ssid = Get-CurSSIDStable      # 去抖：容忍开机/重连时的瞬时断连
    $curName = if ($ssid) { $ssid } else { '无' }

    # 网络切换：只要当前网络变了，就允许重新弹一次"目标不符"提示
    if ($ssid -ne $script:mismatchLastSsid) { $script:mismatchPrompted = $false }

    # 设了目标网络但当前不在其上
    if ($target -and ($ssid -ne $target)) {
        if (-not [bool]$script:cfg.switchNetwork) {
            # 用户关闭了"自动切换" → 如实报告当前，不抢网
            $on = Test-InternetCached 12
            Log-State ("非目标网络[" + $curName + "] 自动切换已关 联网=" + $on)
            if ($on) {
                Set-Status ('已联网 · ' + $curName + '（自动切换已关闭）') 'Green'
            } else {
                Set-Status ('当前：' + $curName + '（自动切换已关闭）') 'DimGray'
            }
            PollSlow
            return
        }
        if (Test-SsidVisible $target) {
            # 目标在附近 → 切换过去（带冷却，防抖动）
            Log-State ("目标[" + $target + "]可见 当前[" + $curName + "] 准备切换")
            if ((Get-Date) - $script:lastSwitchAt -gt [TimeSpan]::FromSeconds(20)) {
                $script:lastSwitchAt = Get-Date
                Write-Log ("切换目标网络: " + $target + " (当前 " + $curName + ")")
                Connect-ToSsid $target | Out-Null
            }
            Set-Status ('正在切换到「' + $target + '」…') 'OrangeRed'
            PollFast
        } else {
            # 目标不在附近（如断电后连了热点）→ 不折腾、不抢网
            $on = Test-InternetCached 12
            Log-State ("目标[" + $target + "]不在附近 当前[" + $curName + "] 联网=" + $on)
            if ($on) {
                Set-Status ('已联网 · ' + $curName + '（目标网「' + $target + '」不在附近）') 'Green'
            } else {
                Set-Status ('当前：' + $curName + '（目标网「' + $target + '」不在附近）') 'DimGray'
            }
            PollSlow
            # 网络与目标不符且目标不在附近 → 弹一次提示（暂停/忽略/彻底关闭）
            if ((-not $script:mismatchPrompted) -and $ssid) {
                $script:mismatchPrompted = $true
                $script:mismatchLastSsid = $ssid
                Write-Log ("提示弹窗: 当前[" + $curName + "] ≠ 目标[" + $target + "] 且目标不在附近")
                $act = Show-MismatchDialog $curName $target
                switch ($act) {
                    'pause' { Pause-App }
                    'exit'  { Stop-App }
                    default { Write-Log '用户选择忽略' }
                }
            }
        }
        return
    }

    # 判定是否为"校园网"：只看 SSID 关键词（不能因为"当前==目标"就当成校园网，
    # 否则把手机热点设为目标时，会误以为在校园网上而一直等门户）
    $onCampus = ($ssid -like ('*' + $SsidKeyword + '*'))
    if (-not $ssid -or -not $onCampus) {
        $on = ($ssid -and (Test-InternetCached 12))
        Log-State ("非校园网[" + $curName + "] 联网=" + $on)
        if ($on) {
            Set-Status ('已联网 · ' + $curName + '（非校园网，无需登录）') 'Green'
        } else {
            Set-Status ('未连接校园网（当前：' + $curName + '）') 'DimGray'
        }
        PollSlow
        return
    }
    if (-not $chkAuto.Checked) { PollSlow; return }
    if ($script:busy) { return }

    # ① 最快门：认证服务器 TCP 是否可达（约 20ms）
    #    不可达 = 网络还没准备好，快速重试（尽快抢到登录时机）
    if (-not (Test-PortalReachable)) {
        Log-State ("校园网[" + $ssid + "]认证服务器不可达(网络准备中)")
        Set-Status ('网络准备中（' + $ssid + '）…') 'OrangeRed'
        PollFast
        return
    }
    # ② 门户可达 → 已联网吗？（缓存 15 秒，命中则几乎瞬时）
    if (Test-InternetCached 15) {
        Log-State ("已联网[" + $ssid + "]")
        if ($lblStatus.Text -notmatch '已联网|连接成功') { Set-Status ('已联网 · ' + $ssid + ' · ' + (Get-Date).ToString('HH:mm:ss')) 'Green' }
        PollSlow        # 已稳定联网 → 放慢省电
        return
    }
    # ③ 未联网 → 冷却后登录（保持快速轮询，尽快连上）
    PollFast
    # 冷却按"上次尝试"计时（失败也计），并随连续失败次数指数退避，避免猛敲认证服务器
    $backoff = [Math]::Min($LoginCooldownSec * [Math]::Pow(2, [Math]::Min($script:loginFailCount, 4)), 60)
    if ((Get-Date) - $script:lastLoginAttemptAt -lt [TimeSpan]::FromSeconds($backoff)) { return }
    Log-State ("校园网[" + $ssid + "]未联网 门户可达 → 准备登录")
    # 未联网 → 自动登录
    $user = $txtUser.Text.Trim()
    if ([string]::IsNullOrEmpty($user) -and $script:cfg.schoolId) { $user = [string]$script:cfg.schoolId }
    $pass = $txtPass.Text
    if ([string]::IsNullOrEmpty($pass) -and $script:cfg.passEnc) { $pass = Unprotect-Text $script:cfg.passEnc }
    $carrier = [string]$cmbCarrier.SelectedItem
    if ([string]::IsNullOrEmpty($user) -or [string]::IsNullOrEmpty($pass)) {
        Log-State ("校园网[" + $ssid + "]未联网 但无可用账号密码")
        Set-Status '检测到断网，但没有可用账号密码，请填写' 'OrangeRed'
        # 数据不完整 → 弹出窗口让用户输入
        if (-not $form.Visible) { Show-MainWindow }
        return
    }
    $script:busy = $true
    try {
        Set-Status ('检测到断网，正在自动重连… ' + (Get-Date).ToString('HH:mm:ss')) 'OrangeRed'
        $script:lastLoginAttemptAt = Get-Date       # 每次尝试都计时（失败也退避）
        $r = Invoke-PortalLogin $user $pass $carrier
        Write-Log ("auto-login => ok=" + $r.ok + " msg=" + $r.msg)
        if ($r.ok) {
            $script:lastLoginAt = Get-Date
            $script:loginFailCount = 0               # 成功 → 清零退避
            # 登录成功 → 立刻把联网缓存置为"已联网"，避免因旧缓存而重复登录
            $script:netCache = $true
            $script:netCacheAt = Get-Date
            Set-Status ('✔ ' + (Get-Date).ToString('HH:mm:ss') + ' 自动重连成功') 'Green'
            if ($form.Visible) { $hideTimer.Start() }
        } elseif ($r.authfail) {
            # 账号密码错误 → 清空保存的密码，并弹出窗口让人重新输入
            $script:cfg.passEnc = ''
            Save-Config
            Set-Status '✘ 账号或密码错误，已清空保存的密码，请重新输入' 'Red'
            Show-MainWindow
            $txtPass.SelectAll(); $txtPass.Focus()
        } else {
            # 其他失败（网络抖动 / 服务器繁忙）→ 累加退避，下次间隔更久再试
            $script:loginFailCount++
            if ($r.msg -match '繁忙|稍后|请稍|too many|busy') {
                Write-Log ('服务器繁忙，退避第 ' + $script:loginFailCount + ' 次')
                Set-Status ('认证服务器繁忙，将退避重试（第 ' + $script:loginFailCount + ' 次）…') 'OrangeRed'
            } else {
                Set-Status '网络未就绪，稍后自动重试…' 'OrangeRed'
            }
        }
    } finally { $script:busy = $false }
}

$script:timer = New-Object System.Windows.Forms.Timer
$script:timer.Interval = $PollFastSec * 1000     # 先按"快"启动，首次检测后自适应
$script:timer.Add_Tick({ Monitor-Tick })
$script:timer.Start()

# 连接成功后延时自动最小化到托盘（留 2 秒让你看到"连接成功"）
$hideTimer = New-Object System.Windows.Forms.Timer
$hideTimer.Interval = 2000
$hideTimer.Add_Tick({ $hideTimer.Stop(); $form.Hide() })

# 需要在 UI 线程里立即跑一次检测（网络变化时先切回快速轮询）
# 节流：网络事件可能在短时间内连发多次（开机/重连），限制最小间隔，避免抖动
$script:lastTriggerAt = [datetime]::MinValue
function Trigger-CheckNow {
    if (((Get-Date) - $script:lastTriggerAt).TotalSeconds -lt 2) { return }
    $script:lastTriggerAt = Get-Date
    $script:lastLoginAt = [datetime]::MinValue
    PollFast
    try { $script:form.BeginInvoke([Action]{ try { Monitor-Tick } catch {} }) | Out-Null } catch {}
}
# 网络恢复 / 休眠唤醒 → 立即重试（不等下一次定时）
try {
    [System.Net.NetworkInformation.NetworkChange]::NetworkAvailabilityChanged.Add({
        param($s,$e) if ($e.IsAvailable) { Trigger-CheckNow }
    })
} catch {}
try {
    [Microsoft.Win32.SystemEvents]::PowerModeChanged.Add({
        param($s,$e)
        if ($e.Mode -eq [Microsoft.Win32.PowerModes]::Resume) { Trigger-CheckNow }
    })
} catch {}

# ---------------- 关闭 = 最小化到托盘 ----------------
$form.Add_FormClosing({
    param($s,$e)
    if (-not $script:reallyExit) { $e.Cancel = $true; $form.Hide() }
})
$menu.Items[4].Add_Click({ Stop-App })

# ---------------- 初始化界面 ----------------
$txtUser.Text = [string]$script:cfg.schoolId
$chkRemember.Checked = [bool]$script:cfg.remember
$chkAuto.Checked = [bool]$script:cfg.auto
$chkBoot.Checked = [bool]$script:cfg.boot
$chkSwitch.Checked = [bool]$script:cfg.switchNetwork
Set-PauseUI    # 同步"暂停/继续"按钮文字
if ($script:cfg.carrier -and $cmbCarrier.Items.Contains([string]$script:cfg.carrier)) { $cmbCarrier.SelectedItem = [string]$script:cfg.carrier }
if ($script:cfg.passEnc) { try { $txtPass.Text = Unprotect-Text $script:cfg.passEnc } catch {} }

# 启动时"快速"填充下拉（不扫描，避免 netsh 拖慢启动 → 影响连接速度）
$cmbTarget.BeginUpdate()
$cmbTarget.Items.Clear()
[void]$cmbTarget.Items.Add('自动（任意校园网）')
if ($script:cfg.targetSsid) { [void]$cmbTarget.Items.Add([string]$script:cfg.targetSsid); $cmbTarget.SelectedItem = [string]$script:cfg.targetSsid }
else { $cmbTarget.SelectedIndex = 0 }
$cmbTarget.EndUpdate()

Write-Log '==== 程序启动 ===='

# 后台监听"再次启动"信号 → 唤出窗口
# 用 Timer 在主线程轮询信号（比跨线程 Invoke 更稳）
$showCheck = New-Object System.Windows.Forms.Timer
$showCheck.Interval = 500
$showCheck.Add_Tick({
    try {
        if ($script:showEvent.WaitOne(0)) {
            Write-Log '收到唤起信号，显示窗口'
            $script:form.Show()
            $script:form.WindowState = [System.Windows.Forms.FormWindowState]::Normal
            $script:form.Activate()
            $script:form.TopMost = $true
            $script:form.TopMost = $false
            $script:form.BringToFront()
        }
    } catch {}
})
$showCheck.Start()

$t0 = New-Object System.Windows.Forms.Timer
$t0.Interval = 300
$t0.Add_Tick({ $t0.Stop(); Monitor-Tick })
$t0.Start()

# 稍后再做"完整"网络列表刷新（含扫描），不拖慢启动与首次登录
$tRefresh = New-Object System.Windows.Forms.Timer
$tRefresh.Interval = 6000
$tRefresh.Add_Tick({ $tRefresh.Stop(); try { [void](Refresh-TargetList $false) } catch {} })
$tRefresh.Start()

# 每日名言：每分钟检查一次日期变化（跨午夜后自动更换，无需重启）
$tQuote = New-Object System.Windows.Forms.Timer
$tQuote.Interval = 60000
$tQuote.Add_Tick({ Update-Quote })
$tQuote.Start()

# ---------------- 决定启动方式 ----------------
# 有可用的账号+密码 → 后台静默启动（不弹窗）
# 没有/不完整 → 显示窗口让人填写
$hasValidCred = (-not [string]::IsNullOrEmpty([string]$script:cfg.schoolId)) -and
                (-not [string]::IsNullOrEmpty([string]$script:cfg.passEnc))

if ($hasValidCred) {
    Write-Log '有已保存的账号密码 → 后台静默启动'
    # 不传 $form，消息循环照常运行，托盘与定时器都有效，窗口保持隐藏
    [System.Windows.Forms.Application]::Run()
} else {
    Write-Log '无可用账号密码 → 显示窗口'
    [System.Windows.Forms.Application]::Run($form)
}
