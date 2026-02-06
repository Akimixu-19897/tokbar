use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};

#[derive(Debug, Clone)]
pub struct DateRange {
	pub since_yyyymmdd: String,
	pub until_yyyymmdd: String,
	pub label: &'static str,
}

fn yyyymmdd(date: NaiveDate) -> String {
	format!("{:04}{:02}{:02}", date.year(), date.month(), date.day())
}

pub fn range_today() -> DateRange {
	let today = Local::now().date_naive();
	let today_str = yyyymmdd(today);
	DateRange {
		since_yyyymmdd: today_str.clone(),
		until_yyyymmdd: today_str,
		label: "Today",
	}
}

pub fn range_week_monday() -> DateRange {
	let today = Local::now().date_naive();
	let weekday = today.weekday();
	let days_from_monday = match weekday {
		Weekday::Mon => 0,
		Weekday::Tue => 1,
		Weekday::Wed => 2,
		Weekday::Thu => 3,
		Weekday::Fri => 4,
		Weekday::Sat => 5,
		Weekday::Sun => 6,
	};
	let since = today - Duration::days(days_from_monday);

	DateRange {
		since_yyyymmdd: yyyymmdd(since),
		until_yyyymmdd: yyyymmdd(today),
		label: "Week",
	}
}

pub fn range_month() -> DateRange {
	let today = Local::now().date_naive();
	let since = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today);

	DateRange {
		since_yyyymmdd: yyyymmdd(since),
		until_yyyymmdd: yyyymmdd(today),
		label: "Month",
	}
}

pub fn range_year() -> DateRange {
	let today = Local::now().date_naive();
	let since = NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap_or(today);

	DateRange {
		since_yyyymmdd: yyyymmdd(since),
		until_yyyymmdd: yyyymmdd(today),
		label: "Year",
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn week_range_is_monday_start() {
		let today = Local::now().date_naive();
		let range = range_week_monday();
		let since = NaiveDate::parse_from_str(&range.since_yyyymmdd, "%Y%m%d").unwrap();
		let delta = today.signed_duration_since(since);
		assert!(delta.num_days() >= 0 && delta.num_days() <= 6);
		assert_eq!(since.weekday(), Weekday::Mon);
	}
}

