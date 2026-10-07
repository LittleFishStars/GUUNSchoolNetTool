//! 密码保护。
//!
//! - Windows：DPAPI（`CryptProtectData`，CurrentUser 范围），密文带 `dpapi:` 前缀，
//!   仅当前 Windows 用户可解密。
//! - 其他平台：base64 混淆（**不是加密**，仅避免明文直接可读），密文带 `b64:` 前缀。
//!
//! 同时兼容两种情况：PowerShell 版遗留的无前缀 `base64(DPAPI(...))` 密文，
//! 以及配置文件里误填的明文。

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

const DPAPI_PREFIX: &str = "dpapi:";
const B64_PREFIX: &str = "b64:";

/// 加密/混淆明文密码，返回可写入配置文件的字符串
pub fn protect(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    #[cfg(windows)]
    if let Some(blob) = dpapi::protect(plain.as_bytes()) {
        return format!("{DPAPI_PREFIX}{}", B64.encode(blob));
    }
    format!("{B64_PREFIX}{}", B64.encode(plain.as_bytes()))
}

/// 还原 [`protect`] 写入的密文；无法识别时原样返回
pub fn unprotect(stored: &str) -> String {
    if stored.is_empty() {
        return String::new();
    }
    if let Some(rest) = stored.strip_prefix(DPAPI_PREFIX) {
        return blob_to_string(&B64.decode(rest).unwrap_or_default());
    }
    if let Some(rest) = stored.strip_prefix(B64_PREFIX) {
        return String::from_utf8_lossy(&B64.decode(rest).unwrap_or_default()).into_owned();
    }
    // 无前缀：优先按 PowerShell 版遗留的 base64(DPAPI) 处理，失败再按明文
    if let Ok(raw) = B64.decode(stored) {
        let decoded = blob_to_string(&raw);
        if !decoded.is_empty() {
            return decoded;
        }
    }
    stored.to_string()
}

/// 解 DPAPI 密文；非 Windows 平台或解密失败时返回空串
fn blob_to_string(blob: &[u8]) -> String {
    #[cfg(windows)]
    if let Some(raw) = dpapi::unprotect(blob) {
        return String::from_utf8_lossy(&raw).into_owned();
    }
    String::from_utf8_lossy(blob).into_owned()
}

#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;
    use std::ptr;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
    };

    /// 调用 DPAPI 时的公共封装：把输入交给 `f`，成功后拷出输出缓冲并释放
    unsafe fn with_blob<F>(data: &[u8], f: F) -> Option<Vec<u8>>
    where
        F: FnOnce(*const CRYPT_INTEGER_BLOB, *mut CRYPT_INTEGER_BLOB) -> i32,
    {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: ptr::null_mut(),
            };
            if f(&input, &mut output) == 0 {
                return None;
            }
            let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(output.pbData as *mut c_void);
            Some(result)
        }
    }

    /// 用当前用户的凭据加密
    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            with_blob(data, |input, output| {
                CryptProtectData(
                    input,
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    output,
                )
            })
        }
    }

    /// 解密当前用户加密的数据
    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            with_blob(data, |input, output| {
                CryptUnprotectData(
                    input,
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    output,
                )
            })
        }
    }
}
