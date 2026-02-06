use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use glob::glob;
use serde_json::Value;

use crate::pricing::{
	calculate_codex_cost_from_pricing, find_model_pricing, CodexTokens, LiteLLMModelPricing,
};
use crate::time_parse::parse_js_timestamp;
use crate::time_range::DateRange;
use crate::usage::UsageTotals;

const CODEX_HOME_ENV: &str = "CODEX_HOME";
const DEFAULT_CODEX_DIR: &str = ".codex";
const DEFAULT_SESSION_SUBDIR: &str = "sessions";
const LEGACY_FALLBACK_MODEL: &str = "gpt-5";
const CODEX_PROVIDER_PREFIXES: [&str; 3] = ["openai/", "azure/", "openrouter/openai/"];
const SESSION_FILES_TTL: Duration = Duration::from_secs(60 * 5);

#[derive(Debug, Clone, Copy, Default)]
struct RawUsage {
	input_tokens: u64,
	cached_input_tokens: u64,
	output_tokens: u64,
	reasoning_output_tokens: u64,
	total_tokens: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct DeltaUsage {
	input_tokens: u64,
	cached_input_tokens: u64,
	output_tokens: u64,
	reasoning_output_tokens: u64,
	total_tokens: u64,
}

fn parse_yyyymmdd(value: &str) -> Option<NaiveDate> {
	NaiveDate::parse_from_str(value, "%Y%m%d").ok()
}

fn parse_local_date_if_in_range(
	timestamp_rfc3339: &str,
	since: NaiveDate,
	until: NaiveDate,
) -> Option<NaiveDate> {
	let parsed = parse_js_timestamp(timestamp_rfc3339)?;
	let local_date = parsed.local_date;
	if local_date < since || local_date > until {
		return None;
	}
	Some(local_date)
}

fn ensure_u64(value: Option<&Value>) -> u64 {
	let Some(value) = value else {
		return 0;
	};
	let Some(number) = value.as_number() else {
		return 0;
	};
	if let Some(u) = number.as_u64() {
		return u;
	}
	if let Some(i) = number.as_i64() {
		return if i >= 0 { i as u64 } else { 0 };
	}
	if let Some(f) = number.as_f64() {
		if f.is_finite() && f >= 0.0 {
			let rounded = f.round();
			if (f - rounded).abs() < 1e-9 && rounded <= (u64::MAX as f64) {
				return rounded as u64;
			}
		}
	}
	0
}

fn as_non_empty_string(value: Option<&Value>) -> Option<String> {
	let s = value.and_then(|v| v.as_str())?;
	let trimmed = s.trim();
	if trimmed.is_empty() {
		None
	} else {
		Some(trimmed.to_string())
	}
}

fn extract_model(value: &Value) -> Option<String> {
	// Prefer info.model / info.model_name / info.metadata.model
	if let Some(info) = value.get("info") {
		if let Some(model) = as_non_empty_string(info.get("model")) {
			return Some(model);
		}
		if let Some(model) = as_non_empty_string(info.get("model_name")) {
			return Some(model);
		}
		if let Some(metadata) = info.get("metadata") {
			if let Some(model) = as_non_empty_string(metadata.get("model")) {
				return Some(model);
			}
		}
	}

	// Fallback: payload.model / payload.metadata.model
	if let Some(model) = as_non_empty_string(value.get("model")) {
		return Some(model);
	}
	if let Some(metadata) = value.get("metadata") {
		if let Some(model) = as_non_empty_string(metadata.get("model")) {
			return Some(model);
		}
	}

	None
}

fn normalize_raw_usage(value: Option<&Value>) -> Option<RawUsage> {
	let value = value?;
	let obj = value.as_object()?;

	let input = ensure_u64(obj.get("input_tokens"));
	let cached = ensure_u64(obj.get("cached_input_tokens").or(obj.get("cache_read_input_tokens")));
	let output = ensure_u64(obj.get("output_tokens"));
	let reasoning = ensure_u64(obj.get("reasoning_output_tokens"));
	let total = ensure_u64(obj.get("total_tokens"));

	Some(RawUsage {
		input_tokens: input,
		cached_input_tokens: cached,
		output_tokens: output,
		reasoning_output_tokens: reasoning,
		total_tokens: if total > 0 { total } else { input + output },
	})
}

fn subtract_raw_usage(current: RawUsage, previous: Option<RawUsage>) -> RawUsage {
	RawUsage {
		input_tokens: current
			.input_tokens
			.saturating_sub(previous.map(|p| p.input_tokens).unwrap_or(0)),
		cached_input_tokens: current
			.cached_input_tokens
			.saturating_sub(previous.map(|p| p.cached_input_tokens).unwrap_or(0)),
		output_tokens: current
			.output_tokens
			.saturating_sub(previous.map(|p| p.output_tokens).unwrap_or(0)),
		reasoning_output_tokens: current
			.reasoning_output_tokens
			.saturating_sub(previous.map(|p| p.reasoning_output_tokens).unwrap_or(0)),
		total_tokens: current
			.total_tokens
			.saturating_sub(previous.map(|p| p.total_tokens).unwrap_or(0)),
	}
}

fn convert_to_delta(raw: RawUsage) -> DeltaUsage {
	let total = if raw.total_tokens > 0 {
		raw.total_tokens
	} else {
		raw.input_tokens + raw.output_tokens
	};

	let cached = std::cmp::min(raw.cached_input_tokens, raw.input_tokens);

	DeltaUsage {
		input_tokens: raw.input_tokens,
		cached_input_tokens: cached,
		output_tokens: raw.output_tokens,
		reasoning_output_tokens: raw.reasoning_output_tokens,
		total_tokens: total,
	}
}

fn model_alias(model: &str) -> Option<&'static str> {
	match model {
		"gpt-5-codex" => Some("gpt-5"),
		_ => None,
	}
}

fn pricing_for_model(
	dataset: &HashMap<String, LiteLLMModelPricing>,
	model: &str,
) -> Option<LiteLLMModelPricing> {
	find_model_pricing(dataset, model, &CODEX_PROVIDER_PREFIXES).or_else(|| {
		model_alias(model)
			.and_then(|alias| find_model_pricing(dataset, alias, &CODEX_PROVIDER_PREFIXES))
	})
}

fn cost_for_tokens(
	tokens: CodexTokens,
	model: &str,
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> f64 {
	let pricing = pricing_for_model(dataset, model);
	let Some(pricing) = pricing else {
		return 0.0;
	};

	calculate_codex_cost_from_pricing(tokens, &pricing)
}

#[derive(Debug, Default)]
struct SessionFilesCache {
	session_dirs: Vec<PathBuf>,
	scanned_at: Option<Instant>,
	files: Vec<PathBuf>,
}

static SESSION_FILES_CACHE: OnceLock<Mutex<SessionFilesCache>> = OnceLock::new();

fn session_files_cache() -> &'static Mutex<SessionFilesCache> {
	SESSION_FILES_CACHE.get_or_init(|| Mutex::new(SessionFilesCache::default()))
}

pub fn session_files_from_dirs(session_dirs: &[PathBuf]) -> Vec<PathBuf> {
	if session_dirs.is_empty() {
		return Vec::new();
	}

	{
		let guard = session_files_cache()
			.lock()
			.expect("session_files_cache lock poisoned");
		if guard.session_dirs == session_dirs {
			if let Some(scanned_at) = guard.scanned_at {
				if Instant::now().duration_since(scanned_at) < SESSION_FILES_TTL {
					return guard.files.clone();
				}
			}
		}
	}

	let mut files = Vec::new();
	for dir in session_dirs {
		let pattern = dir
			.join("**")
			.join("*.jsonl")
			.to_string_lossy()
			.to_string();
		for entry in glob(&pattern).unwrap_or_else(|_| glob("").expect("glob fallback failed")) {
			if let Ok(path) = entry {
				files.push(path);
			}
		}
	}

	{
		let mut guard = session_files_cache()
			.lock()
			.expect("session_files_cache lock poisoned");
		guard.session_dirs = session_dirs.to_vec();
		guard.scanned_at = Some(Instant::now());
		guard.files = files.clone();
	}
	files
}

pub fn default_codex_session_dirs() -> Vec<PathBuf> {
	fn is_dir(path: &Path) -> bool {
		std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
	}

	fn resolve_like_node(raw: &str) -> PathBuf {
		let base = PathBuf::from(raw);
		if base.is_absolute() {
			return base;
		}
		std::env::current_dir()
			.unwrap_or_else(|_| PathBuf::from("."))
			.join(base)
	}

	let home = std::env::var("HOME").unwrap_or_default();
	if home.is_empty() {
		return Vec::new();
	}

	let codex_home = std::env::var(CODEX_HOME_ENV)
		.ok()
		.map(|v| v.trim().to_string())
		.filter(|v| !v.is_empty())
		.map(|v| resolve_like_node(&v))
		.unwrap_or_else(|| PathBuf::from(format!("{home}/{DEFAULT_CODEX_DIR}")));

	let default_sessions = codex_home.join(DEFAULT_SESSION_SUBDIR);
	if is_dir(&default_sessions) {
		vec![default_sessions]
	} else {
		Vec::new()
	}
}

pub fn load_codex_totals_from_files_with_pricing(
	files: &[PathBuf],
	range: &DateRange,
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> UsageTotals {
	let Some(since) = parse_yyyymmdd(&range.since_yyyymmdd) else {
		return UsageTotals::default();
	};
	let Some(until) = parse_yyyymmdd(&range.until_yyyymmdd) else {
		return UsageTotals::default();
	};

	let should_calculate_cost = !dataset.is_empty();

	let mut totals = UsageTotals::default();
	let mut model_tokens: HashMap<String, CodexTokens> = HashMap::new();

	for file_path in files {
		let Ok(file) = File::open(file_path) else {
			continue;
		};
		let reader = BufReader::new(file);

		let mut previous_totals: Option<RawUsage> = None;
		let mut current_model: Option<String> = None;
		let mut current_model_is_fallback = false;

		for line in reader.lines().flatten() {
			let trimmed = line.trim();
			if trimmed.is_empty() {
				continue;
			}
			if !trimmed.contains("\"event_msg\"") && !trimmed.contains("\"turn_context\"") {
				continue;
			}

			let Ok(entry) = serde_json::from_str::<Value>(trimmed) else {
				continue;
			};

			let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
			let payload = entry.get("payload").unwrap_or(&Value::Null);
			let timestamp = entry.get("timestamp").and_then(|v| v.as_str());

			if entry_type == "turn_context" {
				if let Some(model) = extract_model(payload) {
					current_model = Some(model);
					current_model_is_fallback = false;
				}
				continue;
			}

			if entry_type != "event_msg" {
				continue;
			}

			if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
				continue;
			}

			let Some(timestamp) = timestamp else {
				continue;
			};

			let info = payload.get("info").unwrap_or(&Value::Null);
			let last_usage = normalize_raw_usage(info.get("last_token_usage"));
			let total_usage = normalize_raw_usage(info.get("total_token_usage"));

			let mut raw = last_usage;
			if raw.is_none() {
				if let Some(total_usage) = total_usage {
					raw = Some(subtract_raw_usage(total_usage, previous_totals));
				}
			}

			if let Some(total_usage) = total_usage {
				previous_totals = Some(total_usage);
			}

			let Some(raw) = raw else {
				continue;
			};

			let delta = convert_to_delta(raw);
			if delta.input_tokens == 0
				&& delta.cached_input_tokens == 0
				&& delta.output_tokens == 0
				&& delta.reasoning_output_tokens == 0
			{
				continue;
			}

			let extracted = extract_model(payload);
			let extracted_is_none = extracted.is_none();
			let mut is_fallback_model = false;

			if let Some(extracted_model) = extracted.clone() {
				current_model = Some(extracted_model);
				current_model_is_fallback = false;
			}

			let mut model = extracted.or_else(|| current_model.clone());
			if model.is_none() {
				model = Some(LEGACY_FALLBACK_MODEL.to_string());
				is_fallback_model = true;
				current_model = model.clone();
				current_model_is_fallback = true;
			} else if extracted_is_none && current_model_is_fallback {
				is_fallback_model = true;
			}

				let model = model.unwrap_or_else(|| LEGACY_FALLBACK_MODEL.to_string());
				let _ = is_fallback_model; // reserved for later surfacing/annotation
				if parse_local_date_if_in_range(timestamp, since, until).is_none() {
					continue;
				}

				totals.total_tokens = totals.total_tokens.saturating_add(delta.total_tokens);
				if should_calculate_cost {
					let entry = model_tokens.entry(model.clone()).or_default();
					entry.input_tokens = entry.input_tokens.saturating_add(delta.input_tokens);
					entry.cached_input_tokens = entry
						.cached_input_tokens
						.saturating_add(delta.cached_input_tokens);
					entry.output_tokens = entry.output_tokens.saturating_add(delta.output_tokens);
				}
			}
		}

		if should_calculate_cost {
			for (model, tokens) in model_tokens {
				totals.cost_usd += cost_for_tokens(tokens, &model, dataset);
		}
	}

		totals
	}

	pub fn load_codex_totals_from_files_all_time_with_pricing(
		files: &[PathBuf],
		dataset: &HashMap<String, LiteLLMModelPricing>,
	) -> UsageTotals {
		let should_calculate_cost = !dataset.is_empty();

		let mut totals = UsageTotals::default();
		let mut model_tokens: HashMap<String, CodexTokens> = HashMap::new();

		for file_path in files {
			let Ok(file) = File::open(file_path) else {
				continue;
			};
			let reader = BufReader::new(file);

			let mut previous_totals: Option<RawUsage> = None;
			let mut current_model: Option<String> = None;
			let mut current_model_is_fallback = false;

			for line in reader.lines().flatten() {
				let trimmed = line.trim();
				if trimmed.is_empty() {
					continue;
				}
				if !trimmed.contains("\"event_msg\"") && !trimmed.contains("\"turn_context\"") {
					continue;
				}

				let Ok(entry) = serde_json::from_str::<Value>(trimmed) else {
					continue;
				};

				let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
				let payload = entry.get("payload").unwrap_or(&Value::Null);

				if entry_type == "turn_context" {
					if let Some(model) = extract_model(payload) {
						current_model = Some(model);
						current_model_is_fallback = false;
					}
					continue;
				}

				if entry_type != "event_msg" {
					continue;
				}

				if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
					continue;
				}

				let info = payload.get("info").unwrap_or(&Value::Null);
				let last_usage = normalize_raw_usage(info.get("last_token_usage"));
				let total_usage = normalize_raw_usage(info.get("total_token_usage"));

				let mut raw = last_usage;
				if raw.is_none() {
					if let Some(total_usage) = total_usage {
						raw = Some(subtract_raw_usage(total_usage, previous_totals));
					}
				}

				if let Some(total_usage) = total_usage {
					previous_totals = Some(total_usage);
				}

				let Some(raw) = raw else {
					continue;
				};

				let delta = convert_to_delta(raw);
				if delta.input_tokens == 0
					&& delta.cached_input_tokens == 0
					&& delta.output_tokens == 0
					&& delta.reasoning_output_tokens == 0
				{
					continue;
				}

				let extracted = extract_model(payload);
				let extracted_is_none = extracted.is_none();
				let mut is_fallback_model = false;

				if let Some(extracted_model) = extracted.clone() {
					current_model = Some(extracted_model);
					current_model_is_fallback = false;
				}

				let mut model = extracted.or_else(|| current_model.clone());
				if model.is_none() {
					model = Some(LEGACY_FALLBACK_MODEL.to_string());
					is_fallback_model = true;
					current_model = model.clone();
					current_model_is_fallback = true;
				} else if extracted_is_none && current_model_is_fallback {
					is_fallback_model = true;
				}

				let model = model.unwrap_or_else(|| LEGACY_FALLBACK_MODEL.to_string());
				let _ = is_fallback_model; // reserved for later surfacing/annotation

				totals.total_tokens = totals.total_tokens.saturating_add(delta.total_tokens);
				if should_calculate_cost {
					let entry = model_tokens.entry(model.clone()).or_default();
					entry.input_tokens = entry.input_tokens.saturating_add(delta.input_tokens);
					entry.cached_input_tokens = entry
						.cached_input_tokens
						.saturating_add(delta.cached_input_tokens);
					entry.output_tokens = entry.output_tokens.saturating_add(delta.output_tokens);
				}
			}
		}

		if should_calculate_cost {
			for (model, tokens) in model_tokens {
				totals.cost_usd += cost_for_tokens(tokens, &model, dataset);
			}
		}

		totals
	}

pub fn load_codex_totals_from_session_dirs_with_pricing(
	session_dirs: &[PathBuf],
	range: &DateRange,
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> UsageTotals {
	let files = session_files_from_dirs(session_dirs);
	load_codex_totals_from_files_with_pricing(&files, range, dataset)
}

pub fn load_codex_totals_from_session_dirs_all_time_with_pricing(
	session_dirs: &[PathBuf],
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> UsageTotals {
	let files = session_files_from_dirs(session_dirs);
	load_codex_totals_from_files_all_time_with_pricing(&files, dataset)
}

	#[cfg(test)]
	mod tests {
		use super::*;
		use chrono::Local;
		use chrono::TimeZone;

	struct RestoreCwd {
		original: PathBuf,
	}

	impl RestoreCwd {
		fn new() -> Self {
			Self {
				original: std::env::current_dir().expect("current_dir"),
			}
		}
	}

	impl Drop for RestoreCwd {
		fn drop(&mut self) {
			let _ = std::env::set_current_dir(&self.original);
		}
	}

	struct RestoreEnvVar {
		key: &'static str,
		original: Option<String>,
	}

	impl RestoreEnvVar {
		fn new(key: &'static str) -> Self {
			Self {
				key,
				original: std::env::var(key).ok(),
			}
		}
	}

	impl Drop for RestoreEnvVar {
		fn drop(&mut self) {
			match &self.original {
				Some(value) => std::env::set_var(self.key, value),
				None => std::env::remove_var(self.key),
			}
		}
	}

	#[test]
	fn parses_token_count_events_and_sums_cost() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let sessions = tmp.path().join("sessions");
		std::fs::create_dir_all(&sessions).expect("mkdir");

		let file_path = sessions.join("s1.jsonl");
		let day = Local
			.with_ymd_and_hms(2026, 2, 6, 12, 0, 0)
			.single()
			.expect("local dt")
			.to_rfc3339();

		// First event uses total_token_usage without last_token_usage -> delta = totals - previous (0).
		// Second event uses last_token_usage directly.
		let lines = vec![
			serde_json::json!({
				"type": "turn_context",
				"payload": { "info": { "model": "gpt-5" } }
			}),
			serde_json::json!({
				"type": "event_msg",
				"timestamp": day,
				"payload": {
					"type": "token_count",
					"info": {
						"total_token_usage": {
							"input_tokens": 1000.0,
							"cached_input_tokens": 200.0,
							"output_tokens": 500.0,
							"reasoning_output_tokens": 50,
							"total_tokens": 1500
						}
					}
				}
			}),
			serde_json::json!({
				"type": "event_msg",
				"timestamp": day,
				"payload": {
					"type": "token_count",
					"info": {
						"last_token_usage": {
							"input_tokens": 100,
							"cached_input_tokens": 9999, // should be clamped to <= input
							"output_tokens": 50,
							"reasoning_output_tokens": 0,
							"total_tokens": 150
						}
					}
				}
			}),
		];

		let content = lines
			.into_iter()
			.map(|v| v.to_string())
			.collect::<Vec<_>>()
			.join("\n");
		std::fs::write(&file_path, content).expect("write");

		let range = DateRange {
			since_yyyymmdd: "20260206".to_string(),
			until_yyyymmdd: "20260206".to_string(),
			label: "Today",
		};

		let mut dataset = HashMap::new();
		dataset.insert(
			"gpt-5".to_string(),
			LiteLLMModelPricing {
				input_cost_per_token: Some(1.25e-6),
				cache_read_input_token_cost: Some(1.25e-7),
				output_cost_per_token: Some(1e-5),
				..Default::default()
			},
		);

		let totals = load_codex_totals_from_files_with_pricing(&[file_path], &range, &dataset);
		assert_eq!(totals.total_tokens, 1500 + 150);

		let cost1 = (800.0 * 1.25e-6) + (200.0 * 1.25e-7) + (500.0 * 1e-5);
		let cost2 = (0.0 * 1.25e-6) + (100.0 * 1.25e-7) + (50.0 * 1e-5); // cached clamped to 100
		assert!((totals.cost_usd - (cost1 + cost2)).abs() < 1e-12);
	}

	#[test]
		fn codex_home_resolves_relative_paths_like_node() {
		let _lock = crate::test_util::env_cwd_lock()
			.lock()
			.expect("env/cwd lock poisoned");
		let _restore_cwd = RestoreCwd::new();
		let _restore_env = RestoreEnvVar::new("CODEX_HOME");

		let tmp = tempfile::tempdir().expect("tempdir");
		std::env::set_current_dir(tmp.path()).expect("set_current_dir");

		let relative = PathBuf::from("rel").join("codex");
		std::fs::create_dir_all(relative.join("sessions")).expect("mkdir");
		std::env::set_var("CODEX_HOME", relative.to_string_lossy().to_string());

		let dirs = default_codex_session_dirs();
		assert_eq!(dirs.len(), 1);
		assert!(dirs[0].is_absolute());
		let expected = std::env::current_dir()
			.expect("current_dir")
			.join("rel")
			.join("codex")
			.join("sessions");
			assert_eq!(dirs[0], expected);
		}

		#[test]
		fn all_time_includes_token_count_events_without_timestamp() {
			let tmp = tempfile::tempdir().expect("tempdir");
			let sessions = tmp.path().join("sessions");
			std::fs::create_dir_all(&sessions).expect("mkdir");

			let file_path = sessions.join("s1.jsonl");
			let lines = vec![serde_json::json!({
				"type": "event_msg",
				"payload": {
					"type": "token_count",
					"info": {
						"last_token_usage": {
							"input_tokens": 1,
							"cached_input_tokens": 0,
							"output_tokens": 2,
							"reasoning_output_tokens": 0,
							"total_tokens": 3
						}
					}
				}
			})];

			let content = lines
				.into_iter()
				.map(|v| v.to_string())
				.collect::<Vec<_>>()
				.join("\n");
			std::fs::write(&file_path, content).expect("write");

			let dataset = HashMap::<String, LiteLLMModelPricing>::new();
			let totals = load_codex_totals_from_files_all_time_with_pricing(&[file_path], &dataset);
			assert_eq!(totals.total_tokens, 3);
		}
	}
