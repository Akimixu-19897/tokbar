use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
	pub show_dock_icon: bool,
	pub autostart: bool,
}

impl Default for AppSettings {
	fn default() -> Self {
		Self {
			show_dock_icon: true,
			autostart: false,
		}
	}
}

fn default_config_path() -> Option<PathBuf> {
	let home = std::env::var("HOME").ok()?;
	if home.trim().is_empty() {
		return None;
	}
	Some(PathBuf::from(home).join(".tokbar").join("settings.json"))
}

pub fn load_settings() -> AppSettings {
	let Some(path) = default_config_path() else {
		return AppSettings::default();
	};
	let Ok(body) = fs::read_to_string(path) else {
		return AppSettings::default();
	};
	serde_json::from_str::<AppSettings>(&body).unwrap_or_default()
}

pub fn save_settings(settings: AppSettings) -> Result<(), String> {
	let Some(path) = default_config_path() else {
		return Err("HOME is not set".to_string());
	};
	let Some(parent) = path.parent() else {
		return Err("invalid settings path".to_string());
	};

	let body = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
	fs::create_dir_all(parent).map_err(|e| e.to_string())?;
	fs::write(path, body).map_err(|e| e.to_string())?;
	Ok(())
}

