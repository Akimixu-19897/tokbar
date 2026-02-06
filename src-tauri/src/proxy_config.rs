use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProxyConfig {
	pub aggregated: Option<String>,
	pub http: Option<String>,
	pub https: Option<String>,
	pub socks5: Option<String>,
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
	let Some(value) = value else {
		return None;
	};
	let trimmed = value.trim();
	if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

impl ProxyConfig {
	pub fn normalized(self) -> Self {
		Self {
			aggregated: normalize_optional_string(self.aggregated),
			http: normalize_optional_string(self.http),
			https: normalize_optional_string(self.https),
			socks5: normalize_optional_string(self.socks5),
		}
	}

	pub fn is_empty(&self) -> bool {
		self.aggregated.is_none()
			&& self.http.is_none()
			&& self.https.is_none()
			&& self.socks5.is_none()
	}
}

fn default_config_path() -> Option<PathBuf> {
	let home = std::env::var("HOME").ok()?;
	if home.trim().is_empty() {
		return None;
	}
	Some(PathBuf::from(home).join(".tokbar").join("proxy.json"))
}

pub fn load_proxy_config() -> ProxyConfig {
	let Some(path) = default_config_path() else {
		return ProxyConfig::default();
	};
	let Ok(body) = fs::read_to_string(path) else {
		return ProxyConfig::default();
	};
	serde_json::from_str::<ProxyConfig>(&body)
		.unwrap_or_default()
		.normalized()
}

pub fn save_proxy_config(config: ProxyConfig) -> Result<(), String> {
	let Some(path) = default_config_path() else {
		return Err("HOME is not set".to_string());
	};
	let Some(parent) = path.parent() else {
		return Err("invalid proxy config path".to_string());
	};

	let config = config.normalized();
	let body = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;

	fs::create_dir_all(parent).map_err(|e| e.to_string())?;
	fs::write(path, body).map_err(|e| e.to_string())?;
	Ok(())
}

