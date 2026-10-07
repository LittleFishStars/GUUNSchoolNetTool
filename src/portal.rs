//! ePortal 登录协议：请求构造与响应解析。
//!
//! 认证地址形如：
//! `http://10.10.90.2:801/eportal/portal/login?callback=jsonpReturn
//!  &user_account=,0,学号@后缀&user_password=密码&wlan_user_ip=本机IP&_=时间戳`
//!
//! 响应是 JSONP：`jsonpReturn({"result":1,"msg":"Portal协议认证成功！"});`

use std::time::Duration;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;

use crate::config::Config;

/// 单次登录的结果
#[derive(Debug, Clone)]
pub struct LoginResult {
    /// 是否认证成功（"已经在线"也算成功）
    pub ok: bool,
    /// 是否明确为账号/密码错误
    pub authfail: bool,
    /// 面向用户的说明文本
    pub msg: String,
}

impl LoginResult {
    /// 构造一个普通失败结果
    fn failure(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            authfail: false,
            msg: msg.into(),
        }
    }
}

/// 发起一次门户登录
pub fn login(cfg: &Config, school_id: &str, password: &str, local_ip: &str) -> LoginResult {
    if school_id.is_empty() || password.is_empty() {
        return LoginResult::failure("学号或密码为空");
    }
    // 账号格式：,0,学号@后缀（移动无后缀）
    let account = format!(",0,{}{}", school_id, cfg.carrier.suffix());
    let url = format!(
        "{}/eportal/portal/login?callback=jsonpReturn&user_account={}&user_password={}&wlan_user_ip={}&_={}",
        cfg.portal_base(),
        escape(&account),
        escape(password),
        escape(local_ip),
        chrono::Utc::now().timestamp_millis(),
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    match agent.get(&url).call() {
        Ok(mut resp) => match resp.body_mut().read_to_string() {
            Ok(text) => parse(&text),
            Err(err) => LoginResult::failure(format!("读取响应失败: {err}")),
        },
        Err(err) => LoginResult::failure(format!("请求失败: {err}")),
    }
}

/// 解析门户响应，兼容 JSONP 包裹与纯 JSON
pub fn parse(text: &str) -> LoginResult {
    let Some(json) = extract_object(text) else {
        return LoginResult::failure(format!("解析响应失败: {text}"));
    };
    let Ok(value) = serde_json::from_str::<Value>(&json) else {
        return LoginResult::failure(format!("解析响应失败: {text}"));
    };
    let result = as_text(&value["result"]);
    let code = as_text(&value["ret_code"]);
    let msg = value["msg"].as_str().unwrap_or_default().to_string();
    // result=1 表示认证成功；result=0 且 ret_code=2 表示设备已经在线，同样视为成功
    if result == "1" || code == "2" {
        let msg = if msg.is_empty() {
            "认证成功".to_string()
        } else {
            msg
        };
        return LoginResult {
            ok: true,
            authfail: false,
            msg,
        };
    }
    let authfail = [
        "密码错误",
        "账号密码",
        "账户密码",
        "统一身份认证",
        "用户名或密码",
    ]
    .iter()
    .any(|keyword| msg.contains(keyword));
    let msg = if msg.is_empty() {
        format!("未知响应: {text}")
    } else {
        msg
    };
    LoginResult {
        ok: false,
        authfail,
        msg,
    }
}

/// 截取响应中的第一个 JSON 对象
fn extract_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| text[start..=end].to_string())
}

/// 把 JSON 标量统一转成字符串，兼容 `"1"` 与 `1` 两种写法
fn as_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// URL 百分号编码（等价于 .NET 的 `Uri.EscapeDataString` 语义）
fn escape(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 成功响应() {
        let r = parse(r#"jsonpReturn({"result":1,"msg":"Portal协议认证成功！"});"#);
        assert!(r.ok);
        assert_eq!(r.msg, "Portal协议认证成功！");
    }

    #[test]
    fn 已经在线也算成功() {
        let r = parse(r#"jsonpReturn({"result":0,"msg":"IP: 10.16.120.151 已经在线！","ret_code":2});"#);
        assert!(r.ok);
        assert!(!r.authfail);
    }

    #[test]
    fn 密码错误识别为认证失败() {
        let r = parse(
            r#"jsonpReturn({"result":0,"msg":"您的统一身份认证账号密码错误，请检查账号密码","ret_code":1});"#,
        );
        assert!(!r.ok);
        assert!(r.authfail);
    }

    #[test]
    fn 非json响应不报成功() {
        let r = parse("<html>portal</html>");
        assert!(!r.ok);
    }

    #[test]
    fn 数字与字符串ret_code等价() {
        assert!(parse(r#"{"result":0,"ret_code":"2"}"#).ok);
        assert!(parse(r#"{"result":"1"}"#).ok);
    }
}
