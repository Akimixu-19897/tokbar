use crate::claude;
use crate::codex;
use crate::pricing::LiteLLMModelPricing;
use crate::time_range::DateRange;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const ALL_TIME_TTL: Duration = Duration::from_secs(60 * 5);

#[derive(Debug, Clone, Copy, Default)]
pub struct UsageTotals {
	pub total_tokens: u64,
	pub cost_usd: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum UsageError {
	#[error("{0}")]
	ClaudePaths(#[from] claude::ClaudePathError),
}

#[derive(Debug, Default)]
struct CachedTotals {
	computed_at: Option<Instant>,
	totals: UsageTotals,
}

static CX_ALL_TIME_CACHE: OnceLock<Mutex<CachedTotals>> = OnceLock::new();
static CX_ALL_TIME_CACHE_WITH_COST: OnceLock<Mutex<CachedTotals>> = OnceLock::new();

fn cx_all_time_cache() -> &'static Mutex<CachedTotals> {
	CX_ALL_TIME_CACHE.get_or_init(|| Mutex::new(CachedTotals::default()))
}

fn cx_all_time_cache_with_cost() -> &'static Mutex<CachedTotals> {
	CX_ALL_TIME_CACHE_WITH_COST.get_or_init(|| Mutex::new(CachedTotals::default()))
}

#[derive(Debug, Default)]
struct CachedTotalsMaybe {
	computed_at: Option<Instant>,
	totals: Option<UsageTotals>,
}

static CC_ALL_TIME_CACHE: OnceLock<Mutex<CachedTotalsMaybe>> = OnceLock::new();
static CC_ALL_TIME_CACHE_WITH_COST: OnceLock<Mutex<CachedTotalsMaybe>> = OnceLock::new();

fn cc_all_time_cache() -> &'static Mutex<CachedTotalsMaybe> {
	CC_ALL_TIME_CACHE.get_or_init(|| Mutex::new(CachedTotalsMaybe::default()))
}

fn cc_all_time_cache_with_cost() -> &'static Mutex<CachedTotalsMaybe> {
	CC_ALL_TIME_CACHE_WITH_COST.get_or_init(|| Mutex::new(CachedTotalsMaybe::default()))
}

pub fn load_cc_totals_with_pricing(
	range: &DateRange,
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> Result<UsageTotals, UsageError> {
	let base_dirs = claude::default_claude_base_dirs()?;

	Ok(claude::load_claude_totals_from_base_dirs_with_pricing(
		&base_dirs,
		range,
		dataset,
	))
}

pub fn load_cx_totals_with_pricing(
	range: &DateRange,
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> UsageTotals {
	let session_dirs = codex::default_codex_session_dirs();
	if session_dirs.is_empty() {
		return UsageTotals::default();
	}

	codex::load_codex_totals_from_session_dirs_with_pricing(
		&session_dirs,
		range,
		dataset,
	)
}

pub fn load_cx_totals_all_time_cached_with_pricing(
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> UsageTotals {
	let should_calculate_cost = !dataset.is_empty();
	let cache = if should_calculate_cost {
		cx_all_time_cache_with_cost()
	} else {
		cx_all_time_cache()
	};

	{
		let guard = cache.lock().expect("cx_all_time_cache lock poisoned");
		if let Some(at) = guard.computed_at {
			if Instant::now().duration_since(at) < ALL_TIME_TTL {
				return guard.totals;
			}
		}
	}

	let session_dirs = codex::default_codex_session_dirs();
	let totals = if session_dirs.is_empty() {
		UsageTotals::default()
	} else {
		codex::load_codex_totals_from_session_dirs_all_time_with_pricing(&session_dirs, dataset)
	};

	let mut guard = cache.lock().expect("cx_all_time_cache lock poisoned");
	guard.computed_at = Some(Instant::now());
	guard.totals = totals;
	totals
}

pub fn load_cc_totals_all_time_cached_with_pricing(
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> Result<UsageTotals, UsageError> {
	let should_calculate_cost = !dataset.is_empty();
	let cache = if should_calculate_cost {
		cc_all_time_cache_with_cost()
	} else {
		cc_all_time_cache()
	};

	{
		let guard = cache.lock().expect("cc_all_time_cache lock poisoned");
		if let (Some(at), Some(totals)) = (guard.computed_at, guard.totals) {
			if Instant::now().duration_since(at) < ALL_TIME_TTL {
				return Ok(totals);
			}
		}
	}

	let base_dirs = claude::default_claude_base_dirs()?;
	let totals =
		claude::load_claude_totals_from_base_dirs_all_time_with_pricing(&base_dirs, dataset);

	let mut guard = cache.lock().expect("cc_all_time_cache lock poisoned");
	guard.computed_at = Some(Instant::now());
	guard.totals = Some(totals);
	Ok(totals)
}
