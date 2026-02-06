#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
pub fn env_cwd_lock() -> &'static Mutex<()> {
	static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
	LOCK.get_or_init(|| Mutex::new(()))
}

