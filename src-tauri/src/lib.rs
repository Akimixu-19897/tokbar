#![cfg_attr(test, allow(dead_code))]

// tokbar Rust 库入口。
//
// 说明：
// - 业务解析/统计逻辑（codex/claude/usage/pricing 等）需要在单元测试中可运行；
// - Tauri GUI/Tray 相关代码在 Windows 上跑测试时可能因为 WebView2 运行时环境差异导致测试可执行文件无法启动。
// 因此我们把 GUI 部分放到 `app.rs`，并在 `cfg(not(test))` 下才编译/链接它。

mod app_settings;
mod claude;
mod codex;
mod format;
pub mod litellm;
mod pricing;
mod proxy_config;
pub mod raw_format;
mod rightcodes;
mod rightcodes_api;
mod rightcodes_token_store;

#[cfg(test)]
mod test_util;

mod time_parse;
pub mod time_range;
pub mod usage;

#[cfg(not(test))]
mod app;

#[cfg(not(test))]
pub use app::run;

#[cfg(test)]
pub fn run() {
	// 测试环境不启动 GUI。
}
