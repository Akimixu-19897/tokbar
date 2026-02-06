use crate::usage::UsageTotals;

pub fn format_u64_with_commas(value: u64) -> String {
	let s = value.to_string();
	let mut out = String::with_capacity(s.len() + s.len() / 3);
	let mut count = 0usize;
	for ch in s.chars().rev() {
		if count == 3 {
			out.push(',');
			count = 0;
		}
		out.push(ch);
		count += 1;
	}
	out.chars().rev().collect()
}

pub fn format_single_title_raw(
	period: &str,
	source_abbr: &str,
	totals: UsageTotals,
	show_cost: bool,
) -> String {
	if show_cost {
		return format!(
			"{period} {source_abbr} {tokens}({cost})",
			tokens = format_u64_with_commas(totals.total_tokens),
			cost = format!("${:.2}", totals.cost_usd),
		);
	}

	format!(
		"{period} {source_abbr} {tokens}",
		tokens = format_u64_with_commas(totals.total_tokens),
	)
}

pub fn format_both_title_raw(
	period: &str,
	cx: UsageTotals,
	cc: UsageTotals,
	show_cost: bool,
) -> String {
	let left = format!("{period} |");
	let cx_line = if show_cost {
		format!(
			"cx {tokens}({cost})",
			tokens = format_u64_with_commas(cx.total_tokens),
			cost = format!("${:.2}", cx.cost_usd)
		)
	} else {
		format!("cx {tokens}", tokens = format_u64_with_commas(cx.total_tokens))
	};
	let cc_line = if show_cost {
		format!(
			"cc {tokens}({cost})",
			tokens = format_u64_with_commas(cc.total_tokens),
			cost = format!("${:.2}", cc.cost_usd)
		)
	} else {
		format!("cc {tokens}", tokens = format_u64_with_commas(cc.total_tokens))
	};
	format!("{left}\t{cx_line}\n\t{cc_line}")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn raw_single_title_prints_full_tokens() {
		let title = format_single_title_raw(
			"Today",
			"cx",
			UsageTotals {
				total_tokens: 12345,
				cost_usd: 0.45,
			},
			true,
		);
		assert_eq!(title, "Today cx 12,345($0.45)");
	}

	#[test]
	fn raw_both_title_prints_two_lines() {
		let title = format_both_title_raw(
			"Today",
			UsageTotals {
				total_tokens: 123,
				cost_usd: 0.01,
			},
			UsageTotals {
				total_tokens: 456,
				cost_usd: 0.02,
			},
			true,
		);
		assert!(title.contains("Today |"));
		assert!(title.contains('\n'));
		assert!(title.contains("cx 123($0.01)"));
		assert!(title.contains("cc 456($0.02)"));
	}

	#[test]
	fn comma_formatter_handles_large_numbers() {
		let title = format_single_title_raw(
			"Today",
			"cx",
			UsageTotals {
				total_tokens: 113_577_339,
				cost_usd: 0.0,
			},
			true,
		);
		assert_eq!(title, "Today cx 113,577,339($0.00)");
	}
}
