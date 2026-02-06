use crate::usage::UsageTotals;

pub fn format_cost_usd(cost: f64) -> String {
	format!("${:.2}", cost)
}

pub fn format_tokens_compact(tokens: u64) -> String {
	const K: f64 = 1000.0;
	const M: f64 = 1_000_000.0;
	const B: f64 = 1_000_000_000.0;

	let value = tokens as f64;
	if value < K {
		return tokens.to_string();
	}
	if value < 100_000.0 {
		return format!("{:.1}k", value / K);
	}
	if value < M {
		return format!("{:.0}k", value / K);
	}
	if value < 100_000_000.0 {
		return format!("{:.1}m", value / M);
	}
	if value < B {
		return format!("{:.0}m", value / M);
	}
	format!("{:.1}b", value / B)
}

pub fn format_single_title(
	period: &str,
	source_abbr: &str,
	totals: UsageTotals,
	show_cost: bool,
) -> String {
	if show_cost {
		return format!(
			"{period} {source_abbr} {tokens}({cost})",
			tokens = format_tokens_compact(totals.total_tokens),
			cost = format_cost_usd(totals.cost_usd)
		);
	}

	format!(
		"{period} {source_abbr} {tokens}",
		tokens = format_tokens_compact(totals.total_tokens),
	)
}

pub fn format_both_title_one_line(
	period: &str,
	cx: UsageTotals,
	cc: UsageTotals,
	show_cost: bool,
) -> String {
	if show_cost {
		return format!(
			"{period} | cx {cx_tokens}({cx_cost}) | cc {cc_tokens}({cc_cost})",
			cx_tokens = format_tokens_compact(cx.total_tokens),
			cx_cost = format_cost_usd(cx.cost_usd),
			cc_tokens = format_tokens_compact(cc.total_tokens),
			cc_cost = format_cost_usd(cc.cost_usd),
		);
	}

	format!(
		"{period} | cx {cx_tokens} | cc {cc_tokens}",
		cx_tokens = format_tokens_compact(cx.total_tokens),
		cc_tokens = format_tokens_compact(cc.total_tokens),
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn tokens_compact_formats_expected() {
		assert_eq!(format_tokens_compact(0), "0");
		assert_eq!(format_tokens_compact(999), "999");
		assert_eq!(format_tokens_compact(1_000), "1.0k");
		assert_eq!(format_tokens_compact(12_300), "12.3k");
		assert_eq!(format_tokens_compact(123_000), "123k");
		assert_eq!(format_tokens_compact(1_234_000), "1.2m");
	}

	#[test]
	fn both_title_one_line_has_separators() {
		let title = format_both_title_one_line(
			"Today",
			UsageTotals {
				total_tokens: 12_300,
				cost_usd: 0.45,
			},
			UsageTotals {
				total_tokens: 8_100,
				cost_usd: 0.30,
			},
			true,
		);
		assert!(title.contains("Today | cx"));
		assert!(title.contains(" | cc "));
		assert!(!title.contains('\n'));
	}
}
