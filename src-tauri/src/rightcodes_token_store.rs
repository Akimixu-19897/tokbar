/// Right.codes token 存储（keyring 优先，本地文件兜底）。
///
/// 注意：
/// - 该模块后续会被托盘逻辑与登录命令使用。
/// - token 属于敏感信息：严禁把 token 写入日志/错误字符串；菜单提示只允许输出“状态”，不允许输出值。
///
/// 这里先提供骨架以满足编译；具体实现会在后续步骤补齐。

// 说明：按 TDD 流程，先让依赖方可以编译运行测试；实现细节在对应步骤补齐。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Right.codes token store（keyring 优先，本地文件兜底）。
///
/// 说明：
/// - token 属于敏感信息：任何错误字符串/菜单状态都不得包含 token 明文。
/// - 密码不落盘：本模块只存 token，不接触密码。
pub struct RightcodesTokenStore {
	/// 文件兜底路径（默认 `~/.tokbar/rightcodes-token.json`）。
	file_path: PathBuf,
	/// 是否禁用 keyring（用于测试/无 keyring 环境的兜底路径验证）。
	disable_keyring: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredIn {
	Keyring,
	File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenFilePayload {
	token: String,
	/// 仅用于排障（不包含敏感信息）。
	saved_at: String,
}

impl RightcodesTokenStore {
	pub fn new() -> Self {
		Self {
			file_path: default_token_path(),
			disable_keyring: false,
		}
	}

	#[cfg(test)]
	fn new_for_test(file_path: PathBuf) -> Self {
		Self {
			file_path,
			disable_keyring: true,
		}
	}

	/// 读取 token（keyring 优先；失败则读取文件兜底）。
	pub fn load_token(&self) -> Option<String> {
		if !self.disable_keyring {
			if let Some(t) = load_from_keyring() {
				return Some(t);
			}
		}
		load_from_file(&self.file_path)
	}

	/// 保存 token（优先 keyring；失败则降级写入文件）。
	pub fn save_token(&self, token: &str) -> Result<StoredIn, String> {
		if !self.disable_keyring {
			if try_save_to_keyring(token).is_ok() {
				return Ok(StoredIn::Keyring);
			}
		}
		save_to_file(&self.file_path, token)?;
		Ok(StoredIn::File)
	}
}

impl Default for RightcodesTokenStore {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn file_store_roundtrip_saves_and_loads_token() {
		let dir = tempfile::tempdir().expect("tempdir");
		let path = dir.path().join("rightcodes-token.json");
		let store = RightcodesTokenStore::new_for_test(path.clone());

		// 说明：测试只覆盖“文件兜底”逻辑；keyring 在 CI/本地环境差异较大，不做强依赖测试。
		store.save_token("abc").expect("save token");
		let loaded = load_from_file(&path).expect("load token");
		assert_eq!(loaded, "abc".to_string());
	}

	#[test]
	fn load_from_file_returns_none_for_missing_or_empty_token() {
		let dir = tempfile::tempdir().expect("tempdir");
		let missing = dir.path().join("missing.json");
		assert_eq!(load_from_file(&missing), None);

		let path = dir.path().join("empty.json");
		fs::write(
			&path,
			r#"{"token":"   ","saved_at":"2026-02-09 00:00:00"}"#,
		)
		.expect("write");
		assert_eq!(load_from_file(&path), None);
	}
}

fn default_token_path() -> PathBuf {
	// 说明：tokbar 现有设置都放在 ~/.tokbar/ 下，Right.codes token 也统一放这里。
	let home = std::env::var("HOME").unwrap_or_default();
	PathBuf::from(home).join(".tokbar").join("rightcodes-token.json")
}

fn load_from_file(path: &Path) -> Option<String> {
	let body = fs::read_to_string(path).ok()?;
	let payload = serde_json::from_str::<TokenFilePayload>(&body).ok()?;
	let token = payload.token.trim();
	if token.is_empty() {
		return None;
	}
	Some(token.to_string())
}

fn save_to_file(path: &Path, token: &str) -> Result<(), String> {
	let parent = path.parent().ok_or("invalid token path")?;
	fs::create_dir_all(parent).map_err(|e| e.to_string())?;
	let payload = TokenFilePayload {
		token: token.to_string(),
		saved_at: chrono::Local::now()
			.format("%Y-%m-%d %H:%M:%S")
			.to_string(),
	};
	let body = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
	fs::write(path, body).map_err(|e| e.to_string())?;
	// 尽量设置 0600，避免误泄露。
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
	}
	Ok(())
}

fn load_from_keyring() -> Option<String> {
	let client = tmuntaner_keyring::KeyringClient::new("user_token", "rightcodes", "tokbar").ok()?;
	let token = client.get_password().ok()??;
	let t = token.trim();
	if t.is_empty() {
		return None;
	}
	Some(t.to_string())
}

fn try_save_to_keyring(token: &str) -> Result<(), ()> {
	let client = tmuntaner_keyring::KeyringClient::new("user_token", "rightcodes", "tokbar").map_err(|_| ())?;
	client
		.set_password(token.to_string())
		.map_err(|_| ())?;
	Ok(())
}
