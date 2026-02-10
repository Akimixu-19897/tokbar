use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{NaiveDate};
use glob::glob;
use serde_json::Value;

use crate::pricing::{
	calculate_claude_cost_from_pricing, find_model_pricing, ClaudeTokens, LiteLLMModelPricing,
};
use crate::time_parse::parse_js_timestamp;
use crate::time_range::DateRange;
use crate::usage::UsageTotals;

const CLAUDE_PROVIDER_PREFIXES: [&str; 7] = [
	"anthropic/",
	"claude-3-5-",
	"claude-3-",
	"claude-",
	"openai/",
	"azure/",
	"openrouter/openai/",
];
const CLAUDE_FILES_TTL: Duration = Duration::from_secs(60 * 5);

#[derive(Debug, Default)]
struct ClaudeFilesCache {
	base_dirs: Vec<PathBuf>,
	scanned_at: Option<Instant>,
	files: Vec<PathBuf>,
}

static CLAUDE_FILES_CACHE: OnceLock<Mutex<ClaudeFilesCache>> = OnceLock::new();

fn claude_files_cache() -> &'static Mutex<ClaudeFilesCache> {
	CLAUDE_FILES_CACHE.get_or_init(|| Mutex::new(ClaudeFilesCache::default()))
}

#[derive(Debug, thiserror::Error)]
pub enum ClaudePathError {
	#[error("no valid Claude data directories found in CLAUDE_CONFIG_DIR: {env_paths}")]
	NoValidEnvPaths { env_paths: String },
	#[error("no valid Claude data directories found in default locations")]
	NoValidDefaultPaths,
}

fn parse_yyyymmdd(value: &str) -> Option<NaiveDate> {
	NaiveDate::parse_from_str(value, "%Y%m%d").ok()
}

fn date_in_range_local(timestamp_rfc3339: &str, since: NaiveDate, until: NaiveDate) -> bool {
	let Some(parsed) = parse_js_timestamp(timestamp_rfc3339) else {
		return false;
	};
	parsed.local_date >= since && parsed.local_date <= until
}

fn as_non_empty_string(value: Option<&Value>) -> Option<String> {
	let raw = value.and_then(|v| v.as_str())?;
	let trimmed = raw.trim();
	if trimmed.is_empty() {
		None
	} else {
		Some(trimmed.to_string())
	}
}

fn as_u64_token(value: Option<&Value>) -> Option<u64> {
	let number = value?.as_number()?;
	if let Some(u) = number.as_u64() {
		return Some(u);
	}
	if let Some(i) = number.as_i64() {
		return if i >= 0 { Some(i as u64) } else { None };
	}
	if let Some(f) = number.as_f64() {
		if f.is_finite() && f >= 0.0 {
			let rounded = f.round();
			if (f - rounded).abs() < 1e-9 && rounded <= (u64::MAX as f64) {
				return Some(rounded as u64);
			}
		}
	}
	None
}

fn as_f64(value: Option<&Value>) -> Option<f64> {
	value.and_then(|v| v.as_f64())
}

#[derive(Debug, Clone)]
struct ClaudeUsageEntry {
	timestamp: String,
	message_id: Option<String>,
	request_id: Option<String>,
	model: Option<String>,
	input_tokens: u64,
	output_tokens: u64,
	cache_creation_input_tokens: u64,
	cache_read_input_tokens: u64,
	cost_usd: Option<f64>,
}

fn parse_usage_entry(value: &Value) -> Option<ClaudeUsageEntry> {
	let timestamp = as_non_empty_string(value.get("timestamp"))?;

	let message = value.get("message")?.as_object()?;

	// 说明：
	// - Claude Code 的 usage 形态可能随“接入不同提供商模型”而变化。
	// - 这里兼容两类常见字段名：
	//   - Anthropic 风格：input_tokens / output_tokens
	//   - OpenAI 风格：prompt_tokens / completion_tokens
	let usage = message
		.get("usage")
		.or_else(|| value.get("usage"))
		.and_then(|v| v.as_object())?;

	let input_tokens = first_u64_token(usage, &["input_tokens", "prompt_tokens"])?;
	let output_tokens = first_u64_token(usage, &["output_tokens", "completion_tokens"])?;
	let cache_creation_input_tokens =
		as_u64_token(usage.get("cache_creation_input_tokens")).unwrap_or(0);
	let cache_read_input_tokens = as_u64_token(usage.get("cache_read_input_tokens")).unwrap_or(0);

	let message_id = as_non_empty_string(message.get("id"));
	let request_id = as_non_empty_string(value.get("requestId"));
	let model = as_non_empty_string(message.get("model")).or_else(|| as_non_empty_string(value.get("model")));
	let cost_usd = as_f64(value.get("costUSD"));

	Some(ClaudeUsageEntry {
		timestamp,
		message_id,
		request_id,
		model,
		input_tokens,
		output_tokens,
		cache_creation_input_tokens,
		cache_read_input_tokens,
		cost_usd,
	})
}

fn first_u64_token(usage: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
	for k in keys {
		if let Some(v) = as_u64_token(usage.get(*k)) {
			return Some(v);
		}
	}
	None
}

fn unique_hash(entry: &ClaudeUsageEntry) -> Option<String> {
	Some(format!(
		"{}:{}",
		entry.message_id.as_deref()?,
		entry.request_id.as_deref()?
	))
}

fn earliest_timestamp_millis(file_path: &Path) -> Option<i64> {
	let file = File::open(file_path).ok()?;
	let reader = BufReader::new(file);

	let mut earliest: Option<i64> = None;
	for line in reader.lines().flatten() {
		if line.trim().is_empty() {
			continue;
		}

		let Ok(value) = serde_json::from_str::<Value>(&line) else {
			continue;
		};

		let Some(timestamp) = value.get("timestamp").and_then(|v| v.as_str()) else {
			continue;
		};
		let Some(parsed) = parse_js_timestamp(timestamp) else {
			continue;
		};
		let millis = parsed.millis;
		earliest = Some(earliest.map(|prev| prev.min(millis)).unwrap_or(millis));
	}

	earliest
}

fn sort_files_by_timestamp(files: &[PathBuf]) -> Vec<PathBuf> {
	let mut enriched: Vec<(PathBuf, Option<i64>)> = files
		.iter()
		.cloned()
		.map(|path| {
			let ts = earliest_timestamp_millis(&path);
			(path, ts)
		})
		.collect();

	enriched.sort_by(|a, b| match (a.1, b.1) {
		(None, None) => std::cmp::Ordering::Equal,
		(None, Some(_)) => std::cmp::Ordering::Greater,
		(Some(_), None) => std::cmp::Ordering::Less,
		(Some(at), Some(bt)) => at.cmp(&bt),
	});

	enriched.into_iter().map(|(path, _)| path).collect()
}

pub fn usage_files_from_claude_base_dirs(base_dirs: &[PathBuf]) -> Vec<PathBuf> {
	if base_dirs.is_empty() {
		return Vec::new();
	}

	{
		let guard = claude_files_cache()
			.lock()
			.expect("claude_files_cache lock poisoned");
		if guard.base_dirs == base_dirs {
			if let Some(scanned_at) = guard.scanned_at {
				if Instant::now().duration_since(scanned_at) < CLAUDE_FILES_TTL {
					return guard.files.clone();
				}
			}
		}
	}

	let mut files = Vec::new();
	for base_dir in base_dirs {
		let pattern = base_dir
			.join("projects")
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
		let mut guard = claude_files_cache()
			.lock()
			.expect("claude_files_cache lock poisoned");
		guard.base_dirs = base_dirs.to_vec();
		guard.scanned_at = Some(Instant::now());
		guard.files = files.clone();
	}
	files
}

pub fn load_claude_totals_from_files_with_pricing(
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

	let mut processed_hashes: HashSet<String> = HashSet::new();
	let mut totals = UsageTotals::default();

	let sorted_files = sort_files_by_timestamp(files);
	for file_path in &sorted_files {
		let Ok(file) = File::open(file_path) else {
			continue;
		};
		let reader = BufReader::new(file);
		for line in reader.lines().flatten() {
			let trimmed = line.trim();
			if trimmed.is_empty() {
				continue;
			}
			if !trimmed.contains("\"usage\"") {
				continue;
			}

			let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
				continue;
			};

			let Some(entry) = parse_usage_entry(&value) else {
				continue;
			};

			if !date_in_range_local(&entry.timestamp, since, until) {
				continue;
			}

			if let Some(hash) = unique_hash(&entry) {
				if processed_hashes.contains(&hash) {
					continue;
				}
				processed_hashes.insert(hash);
			}

			let input = entry.input_tokens;
			let output = entry.output_tokens;
			let cache_creation = entry.cache_creation_input_tokens;
			let cache_read = entry.cache_read_input_tokens;

			totals.total_tokens = totals
				.total_tokens
				.saturating_add(input + output + cache_creation + cache_read);

			if let Some(cost_usd) = entry.cost_usd {
				totals.cost_usd += cost_usd;
			} else if let Some(model) = entry.model {
				if let Some(pricing) = find_model_pricing(dataset, &model, &CLAUDE_PROVIDER_PREFIXES) {
					totals.cost_usd += calculate_claude_cost_from_pricing(
						ClaudeTokens {
							input_tokens: input,
							output_tokens: output,
							cache_creation_input_tokens: cache_creation,
							cache_read_input_tokens: cache_read,
						},
						&pricing,
					);
				}
			}
		}
	}

	totals
}

pub fn load_claude_totals_from_files_all_time_with_pricing(
	files: &[PathBuf],
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> UsageTotals {
	let mut processed_hashes: HashSet<String> = HashSet::new();
	let mut totals = UsageTotals::default();

	for file_path in files {
		let Ok(file) = File::open(file_path) else {
			continue;
		};
		let reader = BufReader::new(file);
		for line in reader.lines().flatten() {
			let trimmed = line.trim();
			if trimmed.is_empty() {
				continue;
			}
			if !trimmed.contains("\"usage\"") {
				continue;
			}

			let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
				continue;
			};

			let Some(entry) = parse_usage_entry(&value) else {
				continue;
			};

			if let Some(hash) = unique_hash(&entry) {
				if processed_hashes.contains(&hash) {
					continue;
				}
				processed_hashes.insert(hash);
			}

			let input = entry.input_tokens;
			let output = entry.output_tokens;
			let cache_creation = entry.cache_creation_input_tokens;
			let cache_read = entry.cache_read_input_tokens;

			totals.total_tokens = totals
				.total_tokens
				.saturating_add(input + output + cache_creation + cache_read);

			if let Some(cost_usd) = entry.cost_usd {
				totals.cost_usd += cost_usd;
			} else if let Some(model) = entry.model {
				if let Some(pricing) = find_model_pricing(dataset, &model, &CLAUDE_PROVIDER_PREFIXES) {
					totals.cost_usd += calculate_claude_cost_from_pricing(
						ClaudeTokens {
							input_tokens: input,
							output_tokens: output,
							cache_creation_input_tokens: cache_creation,
							cache_read_input_tokens: cache_read,
						},
						&pricing,
					);
				}
			}
		}
	}

	totals
}

pub fn load_claude_totals_from_base_dirs_with_pricing(
	base_dirs: &[PathBuf],
	range: &DateRange,
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> UsageTotals {
	let files = usage_files_from_claude_base_dirs(base_dirs);
	load_claude_totals_from_files_with_pricing(&files, range, dataset)
}

pub fn load_claude_totals_from_base_dirs_all_time_with_pricing(
	base_dirs: &[PathBuf],
	dataset: &HashMap<String, LiteLLMModelPricing>,
) -> UsageTotals {
	let files = usage_files_from_claude_base_dirs(base_dirs);
	load_claude_totals_from_files_all_time_with_pricing(&files, dataset)
}

pub fn default_claude_base_dirs() -> Result<Vec<PathBuf>, ClaudePathError> {
	const ENV: &str = "CLAUDE_CONFIG_DIR";

	fn is_dir(path: &Path) -> bool {
		std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
	}

	fn has_projects_dir(base: &Path) -> bool {
		is_dir(&base.join("projects"))
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

	let env_paths = std::env::var(ENV).unwrap_or_default();
	if !env_paths.trim().is_empty() {
		let mut out = Vec::new();
		let mut seen = HashSet::<PathBuf>::new();
		for raw in env_paths.split(',').map(|p| p.trim()).filter(|p| !p.is_empty()) {
			let base = resolve_like_node(raw);
			if !is_dir(&base) || !has_projects_dir(&base) {
				continue;
			}
			if seen.insert(base.clone()) {
				out.push(base);
			}
		}
		if out.is_empty() {
			return Err(ClaudePathError::NoValidEnvPaths {
				env_paths: env_paths.trim().to_string(),
			});
		}
		return Ok(out);
	}

	let home = std::env::var("HOME").unwrap_or_default();
	if home.is_empty() {
		return Err(ClaudePathError::NoValidDefaultPaths);
	}

	let xdg_config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{home}/.config"));
	let candidates = [
		PathBuf::from(format!("{xdg_config}/claude")),
		PathBuf::from(format!("{home}/.claude")),
	];

	let mut out = Vec::new();
	for base in candidates {
		if is_dir(&base) && has_projects_dir(&base) {
			out.push(base);
		}
	}

	if out.is_empty() {
		return Err(ClaudePathError::NoValidDefaultPaths);
	}

	Ok(out)
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
	fn aggregates_tokens_cost_filters_range_and_dedupes() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let base = tmp.path().join(".claude");
		let projects = base.join("projects").join("p1");
		std::fs::create_dir_all(&projects).expect("mkdir");

		let file_path = projects.join("session.jsonl");
		let day = Local
			.with_ymd_and_hms(2026, 2, 6, 12, 0, 0)
			.single()
			.expect("local dt")
			.to_rfc3339();
		let other_day = Local
			.with_ymd_and_hms(2026, 2, 5, 12, 0, 0)
			.single()
			.expect("local dt")
			.to_rfc3339();

		let lines = vec![
			serde_json::json!({
				"timestamp": day,
				"message": { "id": "m1", "usage": { "input_tokens": 100, "output_tokens": 50 } },
				"requestId": "r1",
				"costUSD": 0.10
			}),
			// Duplicate (same message.id + requestId) should be skipped
			serde_json::json!({
				"timestamp": day,
				"message": { "id": "m1", "usage": { "input_tokens": 999, "output_tokens": 999 } },
				"requestId": "r1",
				"costUSD": 9.99
			}),
			// Missing requestId => no dedupe, should count
			serde_json::json!({
				"timestamp": day,
				"message": { "id": "m2", "usage": { "input_tokens": 10, "output_tokens": 5, "cache_creation_input_tokens": 2, "cache_read_input_tokens": 3 } },
				"costUSD": 0.01
			}),
			// Outside date range => skipped
			serde_json::json!({
				"timestamp": other_day,
				"message": { "id": "m3", "usage": { "input_tokens": 500, "output_tokens": 500 } },
				"requestId": "r3",
				"costUSD": 1.00
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

		let totals =
			load_claude_totals_from_base_dirs_with_pricing(&[base], &range, &HashMap::new());
		assert_eq!(totals.total_tokens, 150 + (10 + 5 + 2 + 3));
		assert!((totals.cost_usd - (0.10 + 0.01)).abs() < 1e-9);
	}

	#[test]
	fn dedupe_is_stable_by_sorting_files_by_earliest_timestamp() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let base = tmp.path().join(".claude");
		let projects = base.join("projects").join("p1");
		std::fs::create_dir_all(&projects).expect("mkdir");

		let day_early = Local
			.with_ymd_and_hms(2026, 2, 6, 12, 0, 0)
			.single()
			.expect("local dt")
			.to_rfc3339();
		let day_late = Local
			.with_ymd_and_hms(2026, 2, 6, 13, 0, 0)
			.single()
			.expect("local dt")
			.to_rfc3339();

		let file_a = projects.join("a.jsonl");
		let file_b = projects.join("b.jsonl");

		std::fs::write(
			&file_a,
			serde_json::json!({
				"timestamp": day_late,
				"message": { "id": "m1", "usage": { "input_tokens": 999, "output_tokens": 1 } },
				"requestId": "r1",
				"costUSD": 0.99
			})
			.to_string(),
		)
		.expect("write a");
		std::fs::write(
			&file_b,
			serde_json::json!({
				"timestamp": day_early,
				"message": { "id": "m1", "usage": { "input_tokens": 100, "output_tokens": 50 } },
				"requestId": "r1",
				"costUSD": 0.10
			})
			.to_string(),
		)
		.expect("write b");

		let range = DateRange {
			since_yyyymmdd: "20260206".to_string(),
			until_yyyymmdd: "20260206".to_string(),
			label: "Today",
		};

		let totals =
			load_claude_totals_from_base_dirs_with_pricing(&[base], &range, &HashMap::new());
		assert_eq!(totals.total_tokens, 150);
		assert!((totals.cost_usd - 0.10).abs() < 1e-9);
	}

	#[test]
	fn skips_invalid_entries_that_fail_schema_validation() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let base = tmp.path().join(".claude");
		let projects = base.join("projects").join("p1");
		std::fs::create_dir_all(&projects).expect("mkdir");

		let file_path = projects.join("session.jsonl");
		let day = Local
			.with_ymd_and_hms(2026, 2, 6, 12, 0, 0)
			.single()
			.expect("local dt")
			.to_rfc3339();

		let line = serde_json::json!({
			"timestamp": day,
			"message": { "id": "m1", "usage": { "input_tokens": 100 } },
			"requestId": "r1",
			"costUSD": 0.10
		});
		std::fs::write(&file_path, line.to_string()).expect("write");

		let range = DateRange {
			since_yyyymmdd: "20260206".to_string(),
			until_yyyymmdd: "20260206".to_string(),
			label: "Today",
		};

		let totals =
			load_claude_totals_from_base_dirs_with_pricing(&[base], &range, &HashMap::new());
		assert_eq!(totals.total_tokens, 0);
		assert!((totals.cost_usd - 0.0).abs() < 1e-12);
	}

	#[test]
	fn accepts_token_numbers_encoded_as_integer_floats() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let base = tmp.path().join(".claude");
		let projects = base.join("projects").join("p1");
		std::fs::create_dir_all(&projects).expect("mkdir");

		let file_path = projects.join("session.jsonl");
		let day = Local
			.with_ymd_and_hms(2026, 2, 6, 12, 0, 0)
			.single()
			.expect("local dt")
			.to_rfc3339();

		let line = serde_json::json!({
			"timestamp": day,
			"message": { "id": "m1", "usage": { "input_tokens": 100.0, "output_tokens": 50.0 } },
			"requestId": "r1",
			"costUSD": 0.10
		});
		std::fs::write(&file_path, line.to_string()).expect("write");

		let range = DateRange {
			since_yyyymmdd: "20260206".to_string(),
			until_yyyymmdd: "20260206".to_string(),
			label: "Today",
		};

		let totals = load_claude_totals_from_base_dirs_with_pricing(
			&[base],
			&range,
			&HashMap::new(),
		);
		assert_eq!(totals.total_tokens, 150);
		assert!((totals.cost_usd - 0.10).abs() < 1e-9);
	}

	#[test]
	fn falls_back_to_pricing_when_cost_usd_missing() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let base = tmp.path().join(".claude");
		let projects = base.join("projects").join("p1");
		std::fs::create_dir_all(&projects).expect("mkdir");

		let file_path = projects.join("session.jsonl");
		let day = Local
			.with_ymd_and_hms(2026, 2, 6, 12, 0, 0)
			.single()
			.expect("local dt")
			.to_rfc3339();

		let line = serde_json::json!({
			"timestamp": day,
			"message": {
				"id": "m1",
				"model": "claude-opus-4-20250514",
				"usage": { "input_tokens": 100, "output_tokens": 50 }
			},
			"requestId": "r1"
		});

		std::fs::write(&file_path, line.to_string()).expect("write");

		let range = DateRange {
			since_yyyymmdd: "20260206".to_string(),
			until_yyyymmdd: "20260206".to_string(),
			label: "Today",
		};

		let mut dataset = HashMap::new();
		dataset.insert(
			"anthropic/claude-opus-4-20250514".to_string(),
			LiteLLMModelPricing {
				input_cost_per_token: Some(3e-6),
				output_cost_per_token: Some(1.5e-5),
				..Default::default()
			},
		);

		let totals = load_claude_totals_from_files_with_pricing(&[file_path], &range, &dataset);
		assert_eq!(totals.total_tokens, 150);
		let expected = 100.0 * 3e-6 + 50.0 * 1.5e-5;
		assert!((totals.cost_usd - expected).abs() < 1e-12);
	}

	#[test]
	fn accepts_openai_style_usage_keys_prompt_and_completion_tokens() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let base = tmp.path().join(".claude");
		let projects = base.join("projects").join("p1");
		std::fs::create_dir_all(&projects).expect("mkdir");

		let file_path = projects.join("session.jsonl");
		let day = Local
			.with_ymd_and_hms(2026, 2, 6, 12, 0, 0)
			.single()
			.expect("local dt")
			.to_rfc3339();

		let line = serde_json::json!({
			"timestamp": day,
			"message": {
				"id": "m1",
				"model": "gpt-4o",
				"usage": { "prompt_tokens": 100, "completion_tokens": 50 }
			},
			"requestId": "r1"
		});
		std::fs::write(&file_path, line.to_string()).expect("write");

		let range = DateRange {
			since_yyyymmdd: "20260206".to_string(),
			until_yyyymmdd: "20260206".to_string(),
			label: "Today",
		};

		let mut dataset = HashMap::new();
		dataset.insert(
			"openai/gpt-4o".to_string(),
			LiteLLMModelPricing {
				input_cost_per_token: Some(1e-6),
				output_cost_per_token: Some(2e-6),
				..Default::default()
			},
		);

		let totals = load_claude_totals_from_files_with_pricing(&[file_path], &range, &dataset);
		assert_eq!(totals.total_tokens, 150);
		let expected = 100.0 * 1e-6 + 50.0 * 2e-6;
		assert!((totals.cost_usd - expected).abs() < 1e-12);
	}

	#[test]
	fn claude_config_dir_resolves_relative_paths_like_node() {
		let _lock = crate::test_util::env_cwd_lock()
			.lock()
			.expect("env/cwd lock poisoned");
		let _restore_cwd = RestoreCwd::new();
		let _restore_env = RestoreEnvVar::new("CLAUDE_CONFIG_DIR");

		let tmp = tempfile::tempdir().expect("tempdir");
		std::env::set_current_dir(tmp.path()).expect("set_current_dir");

		let relative = PathBuf::from("rel").join("claude");
		std::fs::create_dir_all(relative.join("projects")).expect("mkdir");
		std::env::set_var("CLAUDE_CONFIG_DIR", relative.to_string_lossy().to_string());

		let dirs = default_claude_base_dirs();
		let dirs = dirs.expect("dirs");
		assert_eq!(dirs.len(), 1);
		assert!(dirs[0].is_absolute());
		let expected = std::env::current_dir()
			.expect("current_dir")
			.join("rel")
			.join("claude");
		assert_eq!(dirs[0], expected);
	}

	#[test]
	fn claude_config_dir_errors_when_set_but_invalid() {
		let _lock = crate::test_util::env_cwd_lock()
			.lock()
			.expect("env/cwd lock poisoned");
		let _restore_env = RestoreEnvVar::new("CLAUDE_CONFIG_DIR");

		std::env::set_var("CLAUDE_CONFIG_DIR", "/nonexistent/claude");
		let err = default_claude_base_dirs().expect_err("should error");
		let message = err.to_string();
		assert!(message.contains("CLAUDE_CONFIG_DIR"));
	}

	#[test]
	fn all_time_includes_entries_with_unparseable_timestamps() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let file_path = tmp.path().join("usage.jsonl");

		let lines = vec![serde_json::json!({
			"timestamp": "not-a-date",
			"requestId": "r1",
			"message": {
				"id": "m1",
				"model": "claude-3-5-sonnet",
				"usage": {
					"input_tokens": 1,
					"output_tokens": 2,
					"cache_creation_input_tokens": 0,
					"cache_read_input_tokens": 0
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
		let totals = load_claude_totals_from_files_all_time_with_pricing(&[file_path], &dataset);
		assert_eq!(totals.total_tokens, 3);
	}
}
