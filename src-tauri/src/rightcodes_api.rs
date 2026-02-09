use std::time::Duration;

use serde_json::{json, Value};

use crate::rightcodes::extract_user_token;

/// Right.codes API 访问错误（只包含可展示信息，不包含任何敏感数据）。
#[derive(Debug, Clone, PartialEq)]
pub enum RightcodesApiError {
	/// 网络错误（DNS/超时/断连等）。
	Network,
	/// 认证失败（401/403）。
	Auth,
	/// 触发限流（429），可选给出 Retry-After 秒数或 next retry 文案。
	RateLimited { retry_after_seconds: Option<u64> },
	/// 其它 HTTP 非 2xx。
	HttpStatus(u16),
	/// 响应 JSON 解析失败或结构不符合预期。
	BadPayload,
}

impl RightcodesApiError {
	/// 转为菜单可展示的简短文案（不含敏感信息）。
	pub fn to_menu_text(&self) -> String {
		match self {
			RightcodesApiError::Network => "rc：网络错误（请检查网络）".to_string(),
			RightcodesApiError::Auth => "rc：认证失败（请重新登录）".to_string(),
			RightcodesApiError::RateLimited { retry_after_seconds } => {
				if let Some(s) = retry_after_seconds {
					format!("rc：触发限流（429），请 {s}s 后重试")
				} else {
					"rc：触发限流（429），请稍后重试".to_string()
				}
			}
			RightcodesApiError::HttpStatus(code) => format!("rc：接口错误（HTTP {code}）"),
			RightcodesApiError::BadPayload => "rc：接口返回异常（无法解析）".to_string(),
		}
	}
}

/// Right.codes 最小 HTTP 客户端（仅满足 tokbar：login + subscriptions/list）。
///
/// 约束：
/// - 不在错误信息里包含 token/用户名/密码
/// - 超时要短（避免刷新线程长时间卡住）
pub struct RightcodesApiClient {
	base_url: String,
	agent: ureq::Agent,
}

impl RightcodesApiClient {
	pub fn new(base_url: &str) -> Self {
		let agent = ureq::AgentBuilder::new()
			.timeout_connect(Duration::from_secs(8))
			.timeout_read(Duration::from_secs(12))
			.timeout_write(Duration::from_secs(12))
			.build();
		Self {
			base_url: base_url.trim_end_matches('/').to_string(),
			agent,
		}
	}

	pub fn login(&self, username: &str, password: &str) -> Result<String, RightcodesApiError> {
		let url = format!("{}/auth/login", self.base_url);
		let body = json!({
			"username": username,
			"password": password,
		});

		let resp = self
			.agent
			.post(&url)
			.set("Accept", "application/json")
			.send_json(body);

		let payload = match parse_json_or_map_error(resp) {
			Ok(v) => v,
			Err(e) => return Err(e),
		};

		let token = extract_user_token(&payload).ok_or(RightcodesApiError::BadPayload)?;
		Ok(token)
	}

	pub fn list_subscriptions(&self, token: &str) -> Result<Value, RightcodesApiError> {
		let url = format!("{}/subscriptions/list", self.base_url);
		let resp = self
			.agent
			.get(&url)
			.set("Accept", "application/json")
			.set("Authorization", &format!("Bearer {token}"))
			.call();

		parse_json_or_map_error(resp)
	}
}

fn parse_json_or_map_error(resp: Result<ureq::Response, ureq::Error>) -> Result<Value, RightcodesApiError> {
	match resp {
		Ok(r) => r
			.into_json::<Value>()
			.map_err(|_| RightcodesApiError::BadPayload),
		Err(ureq::Error::Status(code, r)) => {
			let status = code as u16;
			if status == 401 || status == 403 {
				return Err(RightcodesApiError::Auth);
			}
			if status == 429 {
				let retry_after = parse_retry_after_seconds(&r);
				return Err(RightcodesApiError::RateLimited {
					retry_after_seconds: retry_after,
				});
			}
			Err(RightcodesApiError::HttpStatus(status))
		}
		Err(ureq::Error::Transport(_)) => Err(RightcodesApiError::Network),
	}
}

fn parse_retry_after_seconds(resp: &ureq::Response) -> Option<u64> {
	// 说明：我们只解析“秒数”这种常见形态；HTTP-date 解析在 tokbar 场景收益不高，先不做。
	let raw = resp.header("Retry-After")?.trim();
	if raw.is_empty() {
		return None;
	}
	raw.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rate_limited_error_formats_retry_after_seconds() {
		let e = RightcodesApiError::RateLimited {
			retry_after_seconds: Some(12),
		};
		assert_eq!(e.to_menu_text(), "rc：触发限流（429），请 12s 后重试".to_string());
	}

	#[test]
	fn auth_error_formats_menu_text() {
		assert_eq!(
			RightcodesApiError::Auth.to_menu_text(),
			"rc：认证失败（请重新登录）".to_string()
		);
	}
}

