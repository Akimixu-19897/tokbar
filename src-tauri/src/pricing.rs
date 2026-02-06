use std::collections::HashMap;

use serde::Deserialize;

pub const LITELLM_PRICING_URL: &str =
	"https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LiteLLMModelPricing {
	pub input_cost_per_token: Option<f64>,
	pub output_cost_per_token: Option<f64>,
	pub cache_creation_input_token_cost: Option<f64>,
	pub cache_read_input_token_cost: Option<f64>,
	#[allow(dead_code)]
	pub max_input_tokens: Option<u64>,
	pub input_cost_per_token_above_200k_tokens: Option<f64>,
	pub output_cost_per_token_above_200k_tokens: Option<f64>,
	pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
	pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClaudeTokens {
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub cache_creation_input_tokens: u64,
	pub cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CodexTokens {
	pub input_tokens: u64,
	pub cached_input_tokens: u64,
	pub output_tokens: u64,
}

pub fn find_model_pricing(
	dataset: &HashMap<String, LiteLLMModelPricing>,
	model_name: &str,
	provider_prefixes: &[&str],
) -> Option<LiteLLMModelPricing> {
	let mut candidates = Vec::with_capacity(1 + provider_prefixes.len());
	candidates.push(model_name.to_string());
	for prefix in provider_prefixes {
		candidates.push(format!("{prefix}{model_name}"));
	}

	for candidate in candidates {
		if let Some(pricing) = dataset.get(&candidate) {
			return Some(pricing.clone());
		}
	}

	let lower = model_name.to_ascii_lowercase();
	for (key, value) in dataset {
		let comparison = key.to_ascii_lowercase();
		if comparison.contains(&lower) || lower.contains(&comparison) {
			return Some(value.clone());
		}
	}

	None
}

pub fn calculate_claude_cost_from_pricing(tokens: ClaudeTokens, pricing: &LiteLLMModelPricing) -> f64 {
	const DEFAULT_TIERED_THRESHOLD: u64 = 200_000;

	fn tiered_cost(total_tokens: u64, base: Option<f64>, above: Option<f64>) -> f64 {
		if total_tokens == 0 {
			return 0.0;
		}

		if total_tokens > DEFAULT_TIERED_THRESHOLD {
			if let Some(above_price) = above {
				let below_tokens = DEFAULT_TIERED_THRESHOLD as f64;
				let above_tokens = (total_tokens - DEFAULT_TIERED_THRESHOLD) as f64;
				let mut cost = above_tokens * above_price;
				if let Some(base_price) = base {
					cost += below_tokens * base_price;
				}
				return cost;
			}
		}

		base.unwrap_or(0.0) * (total_tokens as f64)
	}

	let input = tiered_cost(
		tokens.input_tokens,
		pricing.input_cost_per_token,
		pricing.input_cost_per_token_above_200k_tokens,
	);
	let output = tiered_cost(
		tokens.output_tokens,
		pricing.output_cost_per_token,
		pricing.output_cost_per_token_above_200k_tokens,
	);
	let cache_creation = tiered_cost(
		tokens.cache_creation_input_tokens,
		pricing.cache_creation_input_token_cost,
		pricing.cache_creation_input_token_cost_above_200k_tokens,
	);
	let cache_read = tiered_cost(
		tokens.cache_read_input_tokens,
		pricing.cache_read_input_token_cost,
		pricing.cache_read_input_token_cost_above_200k_tokens,
	);

	input + output + cache_creation + cache_read
}

pub fn calculate_codex_cost_from_pricing(tokens: CodexTokens, pricing: &LiteLLMModelPricing) -> f64 {
	let non_cached_input_tokens = tokens
		.input_tokens
		.saturating_sub(tokens.cached_input_tokens) as f64;
	let cached_input_tokens = tokens.cached_input_tokens as f64;
	let output_tokens = tokens.output_tokens as f64;

	let input_cost = pricing.input_cost_per_token.unwrap_or(0.0);
	let cache_read_cost = pricing
		.cache_read_input_token_cost
		.or(pricing.input_cost_per_token)
		.unwrap_or(0.0);
	let output_cost = pricing.output_cost_per_token.unwrap_or(0.0);

	(non_cached_input_tokens * input_cost)
		+ (cached_input_tokens * cache_read_cost)
		+ (output_tokens * output_cost)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn model_pricing_matches_provider_prefix() {
		let mut dataset = HashMap::new();
		dataset.insert(
			"anthropic/claude-opus-4-20250514".to_string(),
			LiteLLMModelPricing {
				input_cost_per_token: Some(3e-6),
				output_cost_per_token: Some(1.5e-5),
				..Default::default()
			},
		);

		let pricing = find_model_pricing(
			&dataset,
			"claude-opus-4-20250514",
			&["anthropic/", "claude-"],
		);
		assert!(pricing.is_some());
	}

	#[test]
	fn model_pricing_falls_back_to_substring_match() {
		let mut dataset = HashMap::new();
		dataset.insert(
			"gpt-5".to_string(),
			LiteLLMModelPricing {
				input_cost_per_token: Some(1.25e-6),
				output_cost_per_token: Some(1e-5),
				cache_read_input_token_cost: Some(1.25e-7),
				..Default::default()
			},
		);

		let pricing = find_model_pricing(&dataset, "gpt-5.2", &["openai/"]);
		assert!(pricing.is_some());
	}

	#[test]
	fn claude_tiered_cost_matches_ccusage_logic() {
		let pricing = LiteLLMModelPricing {
			input_cost_per_token: Some(3e-6),
			input_cost_per_token_above_200k_tokens: Some(6e-6),
			..Default::default()
		};

		let tokens = ClaudeTokens {
			input_tokens: 300_000,
			..Default::default()
		};

		let cost = calculate_claude_cost_from_pricing(tokens, &pricing);
		let expected = 200_000.0 * 3e-6 + 100_000.0 * 6e-6;
		assert!((cost - expected).abs() < 1e-9);
	}

	#[test]
	fn claude_tiered_cost_charges_only_above_threshold_if_base_missing() {
		let pricing = LiteLLMModelPricing {
			input_cost_per_token: None,
			input_cost_per_token_above_200k_tokens: Some(6e-6),
			..Default::default()
		};

		let tokens = ClaudeTokens {
			input_tokens: 300_000,
			..Default::default()
		};

		let cost = calculate_claude_cost_from_pricing(tokens, &pricing);
		let expected = 100_000.0 * 6e-6;
		assert!((cost - expected).abs() < 1e-9);
	}

	#[test]
	fn codex_cost_splits_cached_and_non_cached_input() {
		let pricing = LiteLLMModelPricing {
			input_cost_per_token: Some(1.25e-6),
			cache_read_input_token_cost: Some(1.25e-7),
			output_cost_per_token: Some(1e-5),
			..Default::default()
		};

		let tokens = CodexTokens {
			input_tokens: 1_000,
			cached_input_tokens: 200,
			output_tokens: 500,
		};

		let cost = calculate_codex_cost_from_pricing(tokens, &pricing);
		let expected = 800.0 * 1.25e-6 + 200.0 * 1.25e-7 + 500.0 * 1e-5;
		assert!((cost - expected).abs() < 1e-12);
	}
}
