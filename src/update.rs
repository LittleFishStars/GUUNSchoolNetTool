//! 自动更新：向 GitHub Releases 查询新版本，下载并替换自身。
//!
//! - **检查**：读取 `releases/latest`，用 semver 比较版本号；
//! - **下载**：按平台挑选资产（Windows 取 `.exe`，Linux 取 `.tar.gz` 并解包）；
//! - **替换**：交给 `self-replace`，它在 Windows 上也能替换正在运行的程序；
//! - 替换完成后需要重启程序才会运行新版本。

use std::io::Read;
use std::path::PathBuf;
#[cfg(not(windows))]
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

/// 查询最新版本的接口
const LATEST_API: &str =
    "https://api.github.com/repos/LittleFishStars/GUUNSchoolNetTool/releases/latest";
/// 检查阶段的超时
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// 下载阶段的超时（安装包有几十 MB，给足时间）
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// GitHub 要求请求方声明 User-Agent
const USER_AGENT: &str = concat!("guunnet/", env!("CARGO_PKG_VERSION"));

/// 一个可用的更新
#[derive(Debug, Clone)]
pub struct Update {
    /// 新版本号（已去掉 `v` 前缀）
    pub version: String,
    /// 更新说明（Release 正文）
    pub notes: String,
    /// 安装包下载地址
    pub url: String,
    /// 安装包字节数（0 表示未知）
    pub size: u64,
}

/// GitHub Release 响应中我们关心的字段
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// Release 中的一个资产
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// 查询是否有新版本；已是最新时返回 `Ok(None)`
pub fn check(current: &str) -> Result<Option<Update>> {
    let release: Release = get_json(LATEST_API)?;
    let latest = release.tag_name.trim_start_matches('v').to_string();
    if !is_newer(current, &latest)? {
        return Ok(None);
    }
    let asset = pick_asset(&release.assets)
        .ok_or_else(|| anyhow!("新版本 v{latest} 没有适用于本平台的安装包"))?;
    Ok(Some(Update {
        version: latest,
        notes: release.body.trim().to_string(),
        url: asset.browser_download_url.clone(),
        size: asset.size,
    }))
}

/// 下载更新并替换当前可执行文件。
///
/// `on_progress(已下载, 总字节)` 用于向界面汇报进度；成功后需要提示用户重启。
pub fn apply(update: &Update, mut on_progress: impl FnMut(u64, u64)) -> Result<()> {
    let payload = download(&update.url, update.size, &mut on_progress)?;
    let new_executable = write_executable(&payload)?;
    let result = self_replace::self_replace(&new_executable).context("替换可执行文件失败");
    let _ = std::fs::remove_file(&new_executable);
    result
}

/// 比较版本号，判断远端是否更新
fn is_newer(current: &str, latest: &str) -> Result<bool> {
    let current = semver::Version::parse(current).context("当前版本号无法解析")?;
    let latest = semver::Version::parse(latest).context("远端版本号无法解析")?;
    Ok(latest > current)
}

/// 按平台挑选安装包
fn pick_asset(assets: &[Asset]) -> Option<&Asset> {
    let (keyword, suffix) = if cfg!(windows) {
        ("windows", ".exe")
    } else {
        ("linux", ".tar.gz")
    };
    assets
        .iter()
        .find(|asset| asset.name.contains(keyword) && asset.name.ends_with(suffix))
}

/// 发一次 GET 并把响应体解析为 JSON
fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T> {
    let agent = crate::http::agent(CHECK_TIMEOUT);
    let mut response = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("请求 GitHub 失败")?;
    let text = response
        .body_mut()
        .read_to_string()
        .context("读取响应失败")?;
    serde_json::from_str(&text).context("解析响应失败")
}

/// 流式下载，边下边汇报进度
fn download(url: &str, size: u64, on_progress: &mut impl FnMut(u64, u64)) -> Result<Vec<u8>> {
    let agent = crate::http::agent(DOWNLOAD_TIMEOUT);
    let mut response = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .context("下载安装包失败")?;
    let total = if size > 0 {
        size
    } else {
        response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    };

    let mut reader = response.body_mut().as_reader();
    let mut payload = Vec::with_capacity(total as usize);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut chunk).context("下载中断")?;
        if read == 0 {
            break;
        }
        payload.extend_from_slice(&chunk[..read]);
        on_progress(payload.len() as u64, total);
    }
    Ok(payload)
}

/// 把下载到的数据落成可执行文件。
///
/// Windows 的资产本身就是 exe；Linux 的资产是 tar.gz，需要先取出其中的 `guunnet`。
fn write_executable(payload: &[u8]) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("guunnet-update{}", std::env::consts::EXE_SUFFIX));
    #[cfg(windows)]
    std::fs::write(&path, payload).context("写入新版本失败")?;

    #[cfg(not(windows))]
    {
        extract_guunnet(payload, &path)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .context("设置可执行权限失败")?;
    }
    Ok(path)
}

/// 从 tar.gz 中取出名为 `guunnet` 的可执行文件
#[cfg(not(windows))]
fn extract_guunnet(payload: &[u8], destination: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(payload));
    for entry in archive.entries().context("读取压缩包失败")? {
        let mut entry = entry.context("读取压缩包条目失败")?;
        let path = entry.path().context("读取条目名失败")?.into_owned();
        if path.file_name().and_then(|name| name.to_str()) == Some("guunnet") {
            entry.unpack(destination).context("解压可执行文件失败")?;
            return Ok(());
        }
    }
    anyhow::bail!("压缩包里没有找到 guunnet 可执行文件")
}


#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_string(),
            browser_download_url: format!("https://example.com/{name}"),
            size: 1,
        }
    }

    #[test]
    fn 版本比较() {
        assert!(is_newer("1.2.1", "1.2.2").unwrap());
        assert!(is_newer("1.2.1", "1.3.0").unwrap());
        assert!(is_newer("1.2.9", "1.2.10").unwrap(), "应按数字而非字符串比较");
        assert!(!is_newer("1.2.1", "1.2.1").unwrap());
        assert!(!is_newer("1.2.1", "1.2.0").unwrap());
    }

    #[test]
    fn 版本号非法时报错而非误判() {
        assert!(is_newer("1.2.1", "不是版本号").is_err());
    }

    #[test]
    fn 挑选本平台安装包() {
        let assets = vec![
            asset("guunnet-v1.3.0-windows-x86_64.exe"),
            asset("guunnet-v1.3.0-linux-x86_64.tar.gz"),
            asset("source.zip"),
        ];
        let picked = pick_asset(&assets).expect("应能挑到本平台安装包");
        let expected = if cfg!(windows) { "windows" } else { "linux" };
        assert!(picked.name.contains(expected));
        assert!(picked.name.ends_with(".exe") || picked.name.ends_with(".tar.gz"));
    }

    #[test]
    fn 没有匹配资产时返回空() {
        let assets = vec![asset("source.zip")];
        assert!(pick_asset(&assets).is_none());
    }
}
