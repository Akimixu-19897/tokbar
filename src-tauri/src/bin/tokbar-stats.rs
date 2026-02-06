use tokbar_lib::raw_format::{format_both_title_raw, format_single_title_raw};
use tokbar_lib::time_range;
use tokbar_lib::usage;
use tokbar_lib::litellm;

#[derive(Debug, Clone, Copy)]
enum Period {
	Today,
	Week,
	Month,
	Year,
}

#[derive(Debug, Clone, Copy)]
enum Source {
	Cx,
	Cc,
	Both,
}

fn usage_and_exit() -> ! {
	eprintln!(
		"Usage: tokbar-stats [--period today|week|month|year] [--source cx|cc|both]\n\
Examples:\n\
  tokbar-stats --source cx\n\
  tokbar-stats --source cc\n\
  tokbar-stats --period week --source both"
	);
	std::process::exit(2);
}

fn parse_args() -> (Period, Source) {
	let mut period = Period::Today;
	let mut source = Source::Both;

	let mut args = std::env::args().skip(1);
	while let Some(arg) = args.next() {
		match arg.as_str() {
			"--period" => {
				let Some(value) = args.next() else {
					usage_and_exit();
				};
				period = match value.as_str() {
					"today" => Period::Today,
					"week" => Period::Week,
					"month" => Period::Month,
					"year" => Period::Year,
					_ => usage_and_exit(),
				};
			}
			"--source" => {
				let Some(value) = args.next() else {
					usage_and_exit();
				};
				source = match value.as_str() {
					"cx" => Source::Cx,
					"cc" => Source::Cc,
					"both" => Source::Both,
					_ => usage_and_exit(),
				};
			}
			"-h" | "--help" => usage_and_exit(),
			_ => usage_and_exit(),
		}
	}

	(period, source)
}

fn range_for_period(period: Period) -> time_range::DateRange {
	match period {
		Period::Today => time_range::range_today(),
		Period::Week => time_range::range_week_monday(),
		Period::Month => time_range::range_month(),
		Period::Year => time_range::range_year(),
	}
}

fn main() {
	let (period, source) = parse_args();
	let range = range_for_period(period);
	let period_label = range.label;
	let pricing = litellm::get_pricing_context();
	let show_cost = pricing.available;
	let dataset = &pricing.dataset;

	match source {
		Source::Cx => {
			let totals = usage::load_cx_totals_with_pricing(&range, dataset);
			println!("{}", format_single_title_raw(period_label, "cx", totals, show_cost));
		}
		Source::Cc => match usage::load_cc_totals_with_pricing(&range, dataset) {
			Ok(totals) => println!("{}", format_single_title_raw(period_label, "cc", totals, show_cost)),
			Err(err) => {
				eprintln!("ERR: {err}");
				std::process::exit(1);
			}
		},
		Source::Both => {
			let cx = usage::load_cx_totals_with_pricing(&range, dataset);
			let cc = usage::load_cc_totals_with_pricing(&range, dataset).unwrap_or_default();
			println!("{}", format_both_title_raw(period_label, cx, cc, show_cost));
		}
	}
}
