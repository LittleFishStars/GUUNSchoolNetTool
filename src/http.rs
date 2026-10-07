//! 统一的 HTTP 客户端构造。
//!
//! 证书校验使用**系统根证书**而非内置的 Mozilla 根证书：公司代理或本地加速器
//! （例如 Watt Toolkit）会替换 TLS 证书，只有信任系统 CA 才能正常访问。

use std::time::Duration;

/// 浏览器 UA：部分站点（如古文岛）会拒绝没有 UA 的请求
pub const BROWSER_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// 构造一个带全局超时、且信任系统证书的 HTTP 客户端
pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}
