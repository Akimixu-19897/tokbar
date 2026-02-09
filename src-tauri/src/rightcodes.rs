use serde_json::Value;

/// Right.codes 展示用的最小摘要（仅满足 tokbar 需求）。
#[derive(Debug, Clone, PartialEq)]
pub struct RcSummary {
	/// 状态栏展示片段：`rc $已用/$总 R` 或 `rc $已用/$总 NR`
	pub title_part: String,
	/// 菜单里展示的状态文案（不含任何敏感信息）。
	pub menu_status: String,
}

/// 从 `/auth/login` 响应中提取 token（兼容 `user_token` / `userToken` 变体）。
///
/// 说明：
/// - 该函数只做 JSON 解析，不做网络请求。
/// - token 属于敏感信息：调用方严禁把返回值直接写入日志/错误信息。
pub fn extract_user_token(payload: &Value) -> Option<String> {
	let obj = payload.as_object()?;
	let token = obj.get("user_token").and_then(|v| v.as_str()).map(str::trim);
	if let Some(t) = token {
		if !t.is_empty() {
			return Some(t.to_string());
		}
	}
	let token2 = obj.get("userToken").and_then(|v| v.as_str()).map(str::trim);
	if let Some(t) = token2 {
		if !t.is_empty() {
			return Some(t.to_string());
		}
	}
	None
}

/// 从 `/subscriptions/list` 响应中抽取“一个套餐包”的额度与 reset 状态，生成 tokbar 所需的展示摘要。
///
/// 约束：
/// - 当前按“用户只购买一个套餐包”的前提处理：从数组中挑选第一个可计算的包。
/// - 若无法计算（字段缺失/类型不对），返回 None；上层应当“状态栏不显示 rc”，只在菜单里提示失败原因。
pub fn summarize_single_subscription(payload: &Value) -> Option<RcSummary> {
	let subs = payload
		.as_object()?
		.get("subscriptions")?
		.as_array()?;

	for item in subs {
		let obj = match item.as_object() {
			Some(v) => v,
			None => continue,
		};
		let total = obj.get("total_quota").and_then(_to_f64)?;
		let remaining = obj.get("remaining_quota").and_then(_to_f64)?;
		let used = (total - remaining).max(0.0);
		let reset_today = obj.get("reset_today").and_then(|v| v.as_bool()).unwrap_or(false);

		let used_text = fmt_money_quota(used);
		let total_text = fmt_money_quota(total);
		let reset_text = if reset_today { "R" } else { "NR" };

		let title_part = format!("rc {used}/{total} {reset}", used = used_text, total = total_text, reset = reset_text);
		let menu_status = format!("rc：{used}/{total} {reset}", used = used_text, total = total_text, reset = reset_text);
		return Some(RcSummary { title_part, menu_status });
	}

	None
}

fn _to_f64(v: &Value) -> Option<f64> {
	if let Some(n) = v.as_f64() {
		return Some(n);
	}
	// 兼容一些后端把数字编码成字符串的情况（尽量容错，不引入额外规则）。
	let s = v.as_str()?.trim();
	if s.is_empty() {
		return None;
	}
	s.parse::<f64>().ok()
}

/// 格式化“套餐额度”金额显示（对齐 rightcodes-tui-dashboard 的口径）：
/// - 整数：不带小数
/// - 非整数：保留 5 位小数
/// - 统一带 `$`，并使用千分位分隔
pub fn fmt_money_quota(value: f64) -> String {
	// 说明：额度展示更偏“面板读数”，与成本/余额不同；这里严格仿照 Python 侧实现以便用户核对。
	let rounded = value.round();
	if (value - rounded).abs() < 1e-9 {
		return format!("${}", format_int_with_commas(rounded as i64));
	}
	format!("${}", format_f64_with_commas(value, 5))
}

fn format_int_with_commas(value: i64) -> String {
	let sign = if value < 0 { "-" } else { "" };
	let mut digits = value.abs().to_string();
	let mut out = String::new();
	while digits.len() > 3 {
		let split = digits.len() - 3;
		let tail = digits.split_off(split);
		if out.is_empty() {
			out = tail;
		} else {
			out = format!("{tail},{out}");
		}
	}
	if out.is_empty() {
		format!("{sign}{digits}")
	} else {
		format!("{sign}{digits},{out}")
	}
}

fn format_f64_with_commas(value: f64, decimals: usize) -> String {
	let sign = if value < 0.0 { "-" } else { "" };
	let abs = value.abs();
	let fixed = format!("{abs:.*}", decimals, abs = abs);
	let (int_part, frac_part) = fixed.split_once('.').unwrap_or((fixed.as_str(), ""));
	let int_val = int_part.parse::<i64>().unwrap_or(0);
	if frac_part.is_empty() {
		format!("{sign}{}", format_int_with_commas(int_val))
	} else {
		format!("{sign}{}.{}", format_int_with_commas(int_val), frac_part)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn extract_user_token_accepts_user_token_and_user_token_camel() {
		let a = json!({"user_token":"abc"});
		assert_eq!(extract_user_token(&a), Some("abc".to_string()));

		let b = json!({"userToken":"def"});
		assert_eq!(extract_user_token(&b), Some("def".to_string()));
	}

	#[test]
	fn fmt_money_quota_formats_int_and_decimal_like_dashboard() {
		assert_eq!(fmt_money_quota(10.0), "$10".to_string());
		assert_eq!(fmt_money_quota(12345.0), "$12,345".to_string());
		assert_eq!(fmt_money_quota(1.234567), "$1.23457".to_string());
	}

	#[test]
	fn summarize_single_subscription_builds_title_and_menu_status() {
		let payload = json!({
			"subscriptions": [
				{"total_quota": 20, "remaining_quota": 10, "reset_today": true}
			]
		});
		let s = summarize_single_subscription(&payload).expect("should summarize");
		assert_eq!(s.title_part, "rc $10/$20 R".to_string());
		assert_eq!(s.menu_status, "rc：$10/$20 R".to_string());
	}

	#[test]
	fn summarize_single_subscription_skips_unusable_items_and_returns_none() {
		let payload = json!({
			"subscriptions": [
				{"total_quota": 20},
				{"tier_id":"x"}
			]
		});
		assert_eq!(summarize_single_subscription(&payload), None);
	}
}

