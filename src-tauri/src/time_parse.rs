use chrono::{DateTime, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedTimestamp {
	pub millis: i64,
	pub local_date: NaiveDate,
}

fn from_rfc3339(value: &str) -> Option<ParsedTimestamp> {
	let dt = DateTime::parse_from_rfc3339(value).ok()?;
	let millis = dt.timestamp_millis();
	Some(ParsedTimestamp {
		millis,
		local_date: dt.with_timezone(&Local).date_naive(),
	})
}

fn from_local_naive(dt: NaiveDateTime) -> Option<ParsedTimestamp> {
	let local = match Local.from_local_datetime(&dt) {
		LocalResult::Single(value) => value,
		LocalResult::Ambiguous(earliest, _) => earliest,
		LocalResult::None => return None,
	};

	Some(ParsedTimestamp {
		millis: local.with_timezone(&Utc).timestamp_millis(),
		local_date: local.date_naive(),
	})
}

fn from_utc_date_only(date: NaiveDate) -> Option<ParsedTimestamp> {
	let dt = date.and_hms_opt(0, 0, 0)?;
	let utc = Utc.from_utc_datetime(&dt);
	let millis = utc.timestamp_millis();
	Some(ParsedTimestamp {
		millis,
		local_date: utc.with_timezone(&Local).date_naive(),
	})
}

pub fn parse_js_timestamp(value: &str) -> Option<ParsedTimestamp> {
	let trimmed = value.trim();
	if trimmed.is_empty() {
		return None;
	}

	// JS `new Date("1700000000000")` yields Invalid Date, so do not accept pure numeric strings.
	if trimmed.chars().all(|c| c.is_ascii_digit()) {
		return None;
	}

	// RFC3339 / ISO with timezone.
	if let Some(parsed) = from_rfc3339(trimmed) {
		return Some(parsed);
	}

	// ISO date-time without timezone: treat as local time (JS does this for date-time forms).
	const LOCAL_DT_FORMATS: [&str; 4] = [
		"%Y-%m-%dT%H:%M:%S%.f",
		"%Y-%m-%dT%H:%M:%S",
		"%Y-%m-%d %H:%M:%S%.f",
		"%Y-%m-%d %H:%M:%S",
	];
	for fmt in LOCAL_DT_FORMATS {
		if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, fmt) {
			if let Some(parsed) = from_local_naive(dt) {
				return Some(parsed);
			}
		}
	}

	// Date-only: treat as UTC midnight (JS treats YYYY-MM-DD as UTC).
	if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
		return from_utc_date_only(date);
	}
	if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y/%m/%d") {
		// Common non-ISO input: interpret like JS in local time for slash forms.
		let dt = date.and_hms_opt(0, 0, 0)?;
		return from_local_naive(dt);
	}

	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_rfc3339() {
		let parsed = parse_js_timestamp("2026-02-06T12:00:00-08:00").expect("parsed");
		assert!(parsed.millis > 0);
	}

	#[test]
	fn parses_local_datetime_without_timezone() {
		let parsed = parse_js_timestamp("2026-02-06T12:00:00").expect("parsed");
		assert_eq!(parsed.local_date, NaiveDate::from_ymd_opt(2026, 2, 6).expect("date"));
	}

	#[test]
	fn parses_utc_date_only_to_expected_millis() {
		let parsed = parse_js_timestamp("2026-02-06").expect("parsed");
		let expected = Utc
			.with_ymd_and_hms(2026, 2, 6, 0, 0, 0)
			.single()
			.expect("utc dt")
			.timestamp_millis();
		assert_eq!(parsed.millis, expected);
	}

	#[test]
	fn rejects_numeric_strings_like_js() {
		assert!(parse_js_timestamp("1700000000000").is_none());
		assert!(parse_js_timestamp("1700000000").is_none());
		assert!(parse_js_timestamp("1").is_none());
	}

	#[test]
	fn parses_slash_date_as_local_midnight() {
		let parsed = parse_js_timestamp("2026/02/06").expect("parsed");
		assert_eq!(parsed.local_date, NaiveDate::from_ymd_opt(2026, 2, 6).expect("date"));
	}
}
