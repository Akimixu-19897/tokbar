use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::pricing::{LiteLLMModelPricing, LITELLM_PRICING_URL};
use crate::proxy_config::{self, ProxyConfig};

const PRICING_CHECK_TTL: Duration = Duration::from_secs(25);
const PRICING_DATASET_TTL: Duration = Duration::from_secs(60 * 60 * 12);
const NETWORK_TIMEOUT_CONNECT: Duration = Duration::from_secs(3);
const NETWORK_TIMEOUT_TOTAL: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Default)]
pub struct PricingContext {
	pub available: bool,
	pub last_error: Option<String>,
	pub dataset: Arc<HashMap<String, LiteLLMModelPricing>>,
}

#[derive(Default)]
struct PricingCache {
	checked_at: Option<Instant>,
	fetched_at: Option<Instant>,
	last_error: Option<String>,
	dataset: Arc<HashMap<String, LiteLLMModelPricing>>,
	proxy: ProxyConfig,
	consecutive_failures: u32,
	next_retry_at: Option<Instant>,
}

static CACHE: OnceLock<Mutex<PricingCache>> = OnceLock::new();

fn cache() -> &'static Mutex<PricingCache> {
	CACHE.get_or_init(|| {
		let proxy = proxy_config::load_proxy_config();
		let (dataset, loaded_err) = load_dataset_from_disk();

		Mutex::new(PricingCache {
			checked_at: None,
			fetched_at: dataset.as_ref().map(|_| Instant::now()),
			last_error: loaded_err,
			dataset: Arc::new(dataset.unwrap_or_default()),
			proxy,
			consecutive_failures: 0,
			next_retry_at: None,
		})
	})
}

fn parse_dataset(json: &str) -> HashMap<String, LiteLLMModelPricing> {
	let Ok(value) = serde_json::from_str::<Value>(json) else {
		return HashMap::new();
	};
	let Some(obj) = value.as_object() else {
		return HashMap::new();
	};

	let mut out = HashMap::new();
	for (key, raw) in obj {
		if !raw.is_object() {
			continue;
		}
		let Ok(pricing) = serde_json::from_value::<LiteLLMModelPricing>(raw.clone()) else {
			continue;
		};
		out.insert(key.clone(), pricing);
	}
	out
}

fn default_cache_path() -> Option<PathBuf> {
	let home = std::env::var("HOME").ok()?;
	if home.trim().is_empty() {
		return None;
	}
	Some(
		PathBuf::from(home)
			.join(".tokbar")
			.join("litellm")
			.join("model_prices_and_context_window.json"),
	)
}

fn load_dataset_from_disk() -> (Option<HashMap<String, LiteLLMModelPricing>>, Option<String>) {
	let Some(path) = default_cache_path() else {
		return (None, None);
	};
	let Ok(body) = fs::read_to_string(&path) else {
		return (None, None);
	};
	let dataset = parse_dataset(&body);
	if dataset.is_empty() {
		return (
			None,
			Some("pricing cache exists but failed to parse or is empty".to_string()),
		);
	}
	(Some(dataset), None)
}

fn save_dataset_to_disk(body: &str) {
	let Some(path) = default_cache_path() else {
		return;
	};
	let Some(parent) = path.parent() else {
		return;
	};
	let _ = fs::create_dir_all(parent);
	let _ = fs::write(path, body);
}

fn normalize_proxy_url(raw: &str, default_scheme: &str) -> String {
	let trimmed = raw.trim();
	if trimmed.contains("://") {
		return trimmed.to_string();
	}
	format!("{default_scheme}://{trimmed}")
}

fn proxy_for_pricing_https(proxy: &ProxyConfig) -> Option<ureq::Proxy> {
	let aggregated = proxy.aggregated.as_deref();
	let https = proxy.https.as_deref();
	let http = proxy.http.as_deref();
	let socks5 = proxy.socks5.as_deref();

	let (raw, scheme) = if let Some(v) = aggregated {
		(v, "http")
	} else if let Some(v) = https {
		(v, "http")
	} else if let Some(v) = http {
		(v, "http")
	} else if let Some(v) = socks5 {
		(v, "socks5")
	} else {
		return None;
	};

	let proxy_url = normalize_proxy_url(raw, scheme);
	ureq::Proxy::new(proxy_url).ok()
}

fn agent_for_proxy(proxy: Option<ureq::Proxy>) -> ureq::Agent {
	let mut builder = ureq::builder()
		.timeout_connect(NETWORK_TIMEOUT_CONNECT)
		.timeout(NETWORK_TIMEOUT_TOTAL);

	if let Some(proxy) = proxy {
		builder = builder.proxy(proxy);
	}

	builder.build()
}

fn backoff_for_failures(failures: u32) -> Duration {
	match failures {
		0 => Duration::from_secs(0),
		1 => Duration::from_secs(60),
		2 => Duration::from_secs(60 * 5),
		_ => Duration::from_secs(60 * 30),
	}
}

fn check_pricing_url(agent: &ureq::Agent) -> Result<(), String> {
	agent
		.head(LITELLM_PRICING_URL)
		.set("User-Agent", "tokbar/0.1.0")
		.call()
		.map(|_| ())
		.map_err(|e| e.to_string())
}

fn fetch_pricing_body(agent: &ureq::Agent) -> Result<String, String> {
	let response = agent
		.get(LITELLM_PRICING_URL)
		.set("User-Agent", "tokbar/0.1.0")
		.call()
		.map_err(|e| e.to_string())?;
	response.into_string().map_err(|e| e.to_string())
}

pub fn get_pricing_context() -> PricingContext {
	let now = Instant::now();
	let (
		cached_checked_at,
		cached_fetched_at,
		cached_err,
		cached_dataset,
		cached_proxy,
		cached_next_retry_at,
	) = {
		let guard = cache().lock().expect("pricing cache lock poisoned");
		(
			guard.checked_at,
			guard.fetched_at,
			guard.last_error.clone(),
			guard.dataset.clone(),
			guard.proxy.clone(),
			guard.next_retry_at,
		)
	};

	let cached_has_dataset = !cached_dataset.is_empty();

	if let Some(next_retry_at) = cached_next_retry_at {
		if now < next_retry_at {
			return PricingContext {
				available: cached_has_dataset,
				last_error: cached_err,
				dataset: if cached_has_dataset {
					cached_dataset
				} else {
					Arc::new(HashMap::new())
				},
			};
		}
	}

	if let Some(checked_at) = cached_checked_at {
		if now.duration_since(checked_at) < PRICING_CHECK_TTL {
			return PricingContext {
				available: cached_has_dataset,
				last_error: cached_err,
				dataset: if cached_has_dataset {
					cached_dataset
				} else {
					Arc::new(HashMap::new())
				},
			};
		}
	}

	let proxy = proxy_for_pricing_https(&cached_proxy);
	let agent = agent_for_proxy(proxy);

	let check = check_pricing_url(&agent);
	if let Err(err) = check {
		let mut guard = cache().lock().expect("pricing cache lock poisoned");
		guard.checked_at = Some(now);
		guard.last_error = Some(err.clone());
		guard.consecutive_failures = guard.consecutive_failures.saturating_add(1);
		let backoff = backoff_for_failures(guard.consecutive_failures);
		guard.next_retry_at = Some(now + backoff);
		return PricingContext {
			available: cached_has_dataset,
			last_error: Some(err),
			dataset: if cached_has_dataset {
				cached_dataset
			} else {
				Arc::new(HashMap::new())
			},
		};
	}

	let should_fetch = match cached_fetched_at {
		Some(fetched_at) => cached_dataset.is_empty() || now.duration_since(fetched_at) > PRICING_DATASET_TTL,
		None => true,
	};

	if should_fetch {
		match fetch_pricing_body(&agent) {
			Ok(body) => {
				let dataset = parse_dataset(&body);
				if dataset.is_empty() {
					let err = "pricing json parsed but dataset is empty".to_string();
					let mut guard = cache().lock().expect("pricing cache lock poisoned");
					guard.checked_at = Some(now);
					guard.last_error = Some(err.clone());
					guard.consecutive_failures = guard.consecutive_failures.saturating_add(1);
					let backoff = backoff_for_failures(guard.consecutive_failures);
					guard.next_retry_at = Some(now + backoff);
					return PricingContext {
						available: cached_has_dataset,
						last_error: Some(err),
						dataset: if cached_has_dataset {
							cached_dataset
						} else {
							Arc::new(HashMap::new())
						},
					};
				}

				save_dataset_to_disk(&body);
				let mut guard = cache().lock().expect("pricing cache lock poisoned");
				guard.checked_at = Some(now);
				guard.fetched_at = Some(now);
				guard.last_error = None;
				guard.dataset = Arc::new(dataset);
				guard.consecutive_failures = 0;
				guard.next_retry_at = None;
				return PricingContext {
					available: true,
					last_error: None,
					dataset: guard.dataset.clone(),
				};
			}
			Err(err) => {
				let mut guard = cache().lock().expect("pricing cache lock poisoned");
				guard.checked_at = Some(now);
				guard.last_error = Some(err.clone());
				guard.consecutive_failures = guard.consecutive_failures.saturating_add(1);
				let backoff = backoff_for_failures(guard.consecutive_failures);
				guard.next_retry_at = Some(now + backoff);
				return PricingContext {
					available: cached_has_dataset,
					last_error: Some(err),
					dataset: if cached_has_dataset {
						cached_dataset
					} else {
						Arc::new(HashMap::new())
					},
				};
			}
		}
	}

	// Pricing URL is reachable and cached dataset is fresh enough.
	let mut guard = cache().lock().expect("pricing cache lock poisoned");
	guard.checked_at = Some(now);
	guard.last_error = None;
	guard.consecutive_failures = 0;
	guard.next_retry_at = None;

	PricingContext {
		available: cached_has_dataset,
		last_error: None,
		dataset: cached_dataset,
	}
}

pub fn update_proxy_config(config: ProxyConfig) -> Result<(), String> {
	proxy_config::save_proxy_config(config.clone())?;
	let mut guard = cache().lock().expect("pricing cache lock poisoned");
	guard.proxy = config.normalized();
	guard.checked_at = None;
	guard.fetched_at = None;
	guard.last_error = None;
	guard.dataset = Arc::new(HashMap::new());
	guard.consecutive_failures = 0;
	guard.next_retry_at = None;
	Ok(())
}

pub fn current_proxy_config() -> ProxyConfig {
	let guard = cache().lock().expect("pricing cache lock poisoned");
	guard.proxy.clone()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn normalize_proxy_url_adds_scheme() {
		assert_eq!(
			normalize_proxy_url("127.0.0.1:7897", "http"),
			"http://127.0.0.1:7897"
		);
		assert_eq!(
			normalize_proxy_url("socks5://127.0.0.1:7897", "http"),
			"socks5://127.0.0.1:7897"
		);
	}
}
