//! Tauri GUI/Tray 入口（仅在 `cfg(not(test))` 下编译）。
//!
//! 这里承载应用的窗口、托盘菜单、命令绑定等逻辑。

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

use crate::{
	app_settings, format, litellm, proxy_config, raw_format, rightcodes, rightcodes_api,
	rightcodes_token_store, time_range, usage,
};

const REFRESH_INTERVAL_SECS: u64 = 30;
type Runtime = Wry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Period {
	Today,
	Week,
	Month,
	Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Source {
	Cx,
	Cc,
	Both,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Settings {
	period: Period,
	source: Source,
}

impl Default for Settings {
	fn default() -> Self {
		Self {
			period: Period::Today,
			source: Source::Both,
		}
	}
}

#[derive(Clone)]
struct AppState {
	settings: Arc<Mutex<Settings>>,
	prefs: Arc<Mutex<app_settings::AppSettings>>,
	menu: MenuHandles,
	last_ui: Arc<Mutex<LastUiState>>,
}

#[derive(Clone)]
struct MenuHandles {
	stats_cx_full: MenuItem<Runtime>,
	stats_cc_full: MenuItem<Runtime>,
	totals_cx_all: MenuItem<Runtime>,
	totals_cc_all: MenuItem<Runtime>,
	rightcodes_status: MenuItem<Runtime>,
	dock_icon: CheckMenuItem<Runtime>,
	autostart: CheckMenuItem<Runtime>,
	pricing_status: MenuItem<Runtime>,
	period_today: CheckMenuItem<Runtime>,
	period_week: CheckMenuItem<Runtime>,
	period_month: CheckMenuItem<Runtime>,
	period_year: CheckMenuItem<Runtime>,
	source_cx: CheckMenuItem<Runtime>,
	source_cc: CheckMenuItem<Runtime>,
	source_both: CheckMenuItem<Runtime>,
}

#[derive(Debug, Default)]
struct LastUiState {
	title: Option<String>,
	tooltip: Option<String>,
	stats_cx_full: Option<String>,
	stats_cc_full: Option<String>,
	totals_cx_all: Option<String>,
	totals_cc_all: Option<String>,
	pricing_status: Option<String>,
	rightcodes_status: Option<String>,
}

fn load_tray_icon_image() -> Option<tauri::image::Image<'static>> {
	// Windows 托盘区不支持 “title 文本常驻显示”，并且如果没有 icon，托盘图标会不可见。
	// 因此这里明确设置一个 32x32 png 作为托盘 icon，确保 Windows 下能看到可点击的托盘图标。
	//
	// 说明：
	// - `icon.icns` 仅用于 macOS，Windows 不会使用它。
	// - Windows 托盘更推荐使用 `icon.ico`（多尺寸），其次才是 png。
	tauri::image::Image::from_bytes(include_bytes!("../icons/icon.ico"))
		.ok()
		.or_else(|| tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png")).ok())
		.or_else(|| tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png")).ok())
}

fn apply_dock_icon_preference(app: &AppHandle, show_dock_icon: bool) {
	#[cfg(target_os = "macos")]
	{
		let policy = if show_dock_icon {
			tauri::ActivationPolicy::Regular
		} else {
			tauri::ActivationPolicy::Accessory
		};
		let _ = app.set_activation_policy(policy);
	}
	#[cfg(not(target_os = "macos"))]
	{
		let _ = (app, show_dock_icon);
	}
}

fn range_for_period(period: Period) -> time_range::DateRange {
	match period {
		Period::Today => time_range::range_today(),
		Period::Week => time_range::range_week_monday(),
		Period::Month => time_range::range_month(),
		Period::Year => time_range::range_year(),
	}
}

fn compute_title(_app: &AppHandle, settings: Settings) -> String {
	let range = range_for_period(settings.period);
	let period = range.label;

	let show_cost = false;
	let dataset = std::collections::HashMap::new();

	let cx = usage::load_cx_totals_with_pricing(&range, &dataset);
	let cc_result = usage::load_cc_totals_with_pricing(&range, &dataset);
	let cc_available = cc_result.is_ok();

	match settings.source {
		Source::Cx => format::format_single_title(period, "cx", cx, show_cost),
		Source::Cc => match cc_result {
			Ok(totals) => format::format_single_title(period, "cc", totals, show_cost),
			// 本机没有 Claude Code 日志目录时，不展示“0”，给出明确提示。
			Err(_) => format!("{period} cc N/A"),
		},
		Source::Both => {
			// 当本机没有 cc 数据来源时（通常是未安装 Claude Code / 无日志目录），
			// “Both” 也只展示 cx，避免出现 “cc 0” 的误导。
			if !cc_available {
				return format::format_single_title(period, "cx", cx, show_cost);
			}

			let cc = cc_result.unwrap_or_default();
			format::format_both_title_one_line(period, cx, cc, show_cost)
		}
	}
}

fn build_menu(
	app: &AppHandle,
	settings: Settings,
	prefs: &app_settings::AppSettings,
) -> tauri::Result<(Menu<Runtime>, MenuHandles)> {
	let stats_cx_full =
		MenuItem::with_id(app, "stats.cx_full", "正在加载 cx…", false, None::<&str>)?;
	let stats_cc_full =
		MenuItem::with_id(app, "stats.cc_full", "正在加载 cc…", false, None::<&str>)?;
	let totals_cx_all =
		MenuItem::with_id(app, "totals.cx_all", "全部 cx：加载中…", false, None::<&str>)?;
	let totals_cc_all =
		MenuItem::with_id(app, "totals.cc_all", "全部 cc：加载中…", false, None::<&str>)?;
	let dock_icon = CheckMenuItem::with_id(
		app,
		"dock.icon",
		"显示程序坞图标",
		true,
		prefs.show_dock_icon,
		None::<&str>,
	)?;
	let autostart = CheckMenuItem::with_id(
		app,
		"autostart",
		"开机启动",
		true,
		prefs.autostart,
		None::<&str>,
	)?;
	let pricing_status = MenuItem::with_id(app, "pricing.status", "模型价格：检查中…", true, None::<&str>)?;
	let proxy_open = MenuItem::with_id(app, "proxy.open", "代理设置…", true, None::<&str>)?;
	let rightcodes_status =
		MenuItem::with_id(app, "rightcodes.status", "rc：未登录（点击登录）", false, None::<&str>)?;
	let rightcodes_login =
		MenuItem::with_id(app, "rightcodes.login", "Right.codes 登录…", true, None::<&str>)?;

	let period_today = CheckMenuItem::with_id(
		app,
		"period.today",
		"今天",
		true,
		settings.period == Period::Today,
		None::<&str>,
	)?;
	let period_week = CheckMenuItem::with_id(
		app,
		"period.week",
		"本周",
		true,
		settings.period == Period::Week,
		None::<&str>,
	)?;
	let period_month = CheckMenuItem::with_id(
		app,
		"period.month",
		"本月",
		true,
		settings.period == Period::Month,
		None::<&str>,
	)?;
	let period_year = CheckMenuItem::with_id(
		app,
		"period.year",
		"本年",
		true,
		settings.period == Period::Year,
		None::<&str>,
	)?;

	let source_cx = CheckMenuItem::with_id(
		app,
		"source.cx",
		"cx（Codex）",
		true,
		settings.source == Source::Cx,
		None::<&str>,
	)?;
	let source_cc = CheckMenuItem::with_id(
		app,
		"source.cc",
		"cc（Claude Code）",
		true,
		settings.source == Source::Cc,
		None::<&str>,
	)?;
	let source_both = CheckMenuItem::with_id(
		app,
		"source.both",
		"cx + cc",
		true,
		settings.source == Source::Both,
		None::<&str>,
	)?;

	let period_menu = Submenu::with_id_and_items(
		app,
		"period",
		"统计周期",
		true,
		&[&period_today, &period_week, &period_month, &period_year],
	)?;
	let source_menu =
		Submenu::with_id_and_items(app, "source", "数据来源", true, &[&source_cx, &source_cc, &source_both])?;

	let menu = Menu::with_items(
		app,
		&[
			&stats_cx_full,
			&stats_cc_full,
			&PredefinedMenuItem::separator(app)?,
			&totals_cx_all,
			&totals_cc_all,
			&PredefinedMenuItem::separator(app)?,
			&dock_icon,
			&autostart,
			&pricing_status,
			&proxy_open,
			&rightcodes_status,
			&rightcodes_login,
			&PredefinedMenuItem::separator(app)?,
			&MenuItem::with_id(app, "refresh", "立即刷新", true, None::<&str>)?,
			&period_menu,
			&source_menu,
			&PredefinedMenuItem::separator(app)?,
			&MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?,
		],
	)?;

	Ok((
		menu,
		MenuHandles {
			stats_cx_full,
			stats_cc_full,
			totals_cx_all,
			totals_cc_all,
			rightcodes_status,
			dock_icon,
			autostart,
			pricing_status,
			period_today,
			period_week,
			period_month,
			period_year,
			source_cx,
			source_cc,
			source_both,
		},
	))
}

fn sync_menu_checks(menu: &MenuHandles, settings: Settings) {
	let _ = menu
		.period_today
		.set_checked(settings.period == Period::Today);
	let _ = menu.period_week.set_checked(settings.period == Period::Week);
	let _ = menu
		.period_month
		.set_checked(settings.period == Period::Month);
	let _ = menu.period_year.set_checked(settings.period == Period::Year);

	let _ = menu.source_cx.set_checked(settings.source == Source::Cx);
	let _ = menu.source_cc.set_checked(settings.source == Source::Cc);
	let _ = menu.source_both.set_checked(settings.source == Source::Both);
}

fn update_tray_title(app: &AppHandle, settings: Settings) {
	if let Some(tray) = app.tray_by_id("tokbar-tray") {
		let state = app.try_state::<AppState>();
		let mut settings = settings;
		let range = range_for_period(settings.period);
		let period = range.label;
		let pricing = litellm::get_pricing_context();
		let show_cost = pricing.available;
		let dataset = &pricing.dataset;

		let cx = usage::load_cx_totals_with_pricing(&range, dataset);
		let cc_result = usage::load_cc_totals_with_pricing(&range, dataset);
		let cc_available = cc_result.is_ok();
		let cc_for_both = cc_result.as_ref().copied().unwrap_or_default();
		let all_label = "All";
		let show_all_cost = pricing.available;
		let cx_all = usage::load_cx_totals_all_time_cached_with_pricing(dataset);
		let cc_all_result = usage::load_cc_totals_all_time_cached_with_pricing(dataset);

		// 当本机没有 cc 数据来源时，强制把 source 降级为 Cx（即使用户选了 Both）。
		// 这样避免展示误导性的 “cc 0”，并让菜单勾选状态保持一致。
		if !cc_available && settings.source != Source::Cx {
			settings.source = Source::Cx;
			if let Some(state) = state.as_ref() {
				if let Ok(mut guard) = state.settings.lock() {
					guard.source = Source::Cx;
				}
				sync_menu_checks(&state.menu, settings);
			}
		}

		let base_title = match settings.source {
			Source::Cx => format::format_single_title(period, "cx", cx, show_cost),
			Source::Cc => match cc_result {
				Ok(totals) => format::format_single_title(period, "cc", totals, show_cost),
				Err(_) => format!("{period} cc ERR"),
			},
			Source::Both => format::format_both_title_one_line(period, cx, cc_for_both, show_cost),
		};

		// Right.codes：只有当拉取成功且可计算套餐额度时，才在状态栏追加 `rc ...`；
		// 任何失败/未登录/字段缺失，都只在菜单里提示原因，避免在状态栏制造噪音。
		let (rc_title_part, rc_menu_text) = compute_rightcodes_ui();
		let title = if let Some(rc) = rc_title_part {
			format!("{base} {rc}", base = base_title, rc = rc)
		} else {
			base_title
		};

		let mut last_ui = state
			.as_ref()
			.map(|s| s.last_ui.lock().expect("last_ui lock poisoned"));

		let should_set_title = last_ui
			.as_ref()
			.and_then(|v| v.title.as_deref())
			!= Some(title.as_str());
		if should_set_title {
			let _ = tray.set_title(Some(&title));
			if let Some(ref mut ui) = last_ui {
				ui.title = Some(title.clone());
			}
		}

		#[cfg(target_os = "macos")]
		{
			let should_set_tooltip = last_ui
				.as_ref()
				.and_then(|v| v.tooltip.as_deref())
				!= Some(title.as_str());
			if should_set_tooltip {
				let _ = tray.set_tooltip(Some(&title));
				let _ = tray.set_icon(None::<tauri::image::Image<'_>>);
				if let Some(ref mut ui) = last_ui {
					ui.tooltip = Some(title.clone());
				}
			}
		}

		// 同步更新菜单中的“完整统计”文本（不做 compact）。
		if let Some(state) = state.as_ref() {
			let full_cx = raw_format::format_single_title_raw(period, "cx", cx, show_cost);
			let full_cc = if cc_available {
				raw_format::format_single_title_raw(period, "cc", cc_for_both, show_cost)
			} else {
				// 本机没有 cc：菜单中不展示具体数值（避免 0 误导），并禁用相关项。
				"cc：未检测到（本机无 Claude Code 日志）".to_string()
			};
			let all_cx =
				raw_format::format_single_title_raw(all_label, "cx", cx_all, show_all_cost);
			let all_cc = if cc_available {
				match cc_all_result {
					Ok(totals) => raw_format::format_single_title_raw(
						all_label,
						"cc",
						totals,
						show_all_cost,
					),
					Err(_) => format!("{all_label} cc ERR"),
				}
			} else {
				"All cc：未检测到".to_string()
			};

			let pricing_text = if pricing.available && pricing.last_error.is_none() {
				"模型价格：可用".to_string()
			} else if pricing.available {
				"模型价格：使用缓存（离线）".to_string()
			} else {
				"无法获取模型价格，请设置魔法代理（点击打开设置）".to_string()
			};

			let ui = last_ui
				.as_mut()
				.expect("AppState exists but last_ui lock missing");
			if ui.stats_cx_full.as_deref() != Some(full_cx.as_str()) {
				let _ = state.menu.stats_cx_full.set_text(full_cx.clone());
				ui.stats_cx_full = Some(full_cx);
			}
			if ui.stats_cc_full.as_deref() != Some(full_cc.as_str()) {
				let _ = state.menu.stats_cc_full.set_text(full_cc.clone());
				ui.stats_cc_full = Some(full_cc);
			}
			if ui.totals_cx_all.as_deref() != Some(all_cx.as_str()) {
				let _ = state.menu.totals_cx_all.set_text(all_cx.clone());
				ui.totals_cx_all = Some(all_cx);
			}
			if ui.totals_cc_all.as_deref() != Some(all_cc.as_str()) {
				let _ = state.menu.totals_cc_all.set_text(all_cc.clone());
				ui.totals_cc_all = Some(all_cc);
			}
			if ui.pricing_status.as_deref() != Some(pricing_text.as_str()) {
				let _ = state.menu.pricing_status.set_text(pricing_text.clone());
				ui.pricing_status = Some(pricing_text);
			}

			if ui.rightcodes_status.as_deref() != Some(rc_menu_text.as_str()) {
				let _ = state.menu.rightcodes_status.set_text(rc_menu_text.clone());
				ui.rightcodes_status = Some(rc_menu_text);
			}

			// 没有 cc 数据来源时禁用 cc/both 相关菜单项，避免用户选择后产生困惑。
			let _ = state.menu.stats_cc_full.set_enabled(cc_available);
			let _ = state.menu.totals_cc_all.set_enabled(cc_available);
			let _ = state.menu.source_cc.set_enabled(cc_available);
			let _ = state.menu.source_both.set_enabled(cc_available);
		}

	}
}

fn compute_rightcodes_ui() -> (Option<String>, String) {
	let store = rightcodes_token_store::RightcodesTokenStore::new();
	let Some(token) = store.load_token() else {
		return (
			None,
			"rc：未登录（点击 Right.codes 登录…）".to_string(),
		);
	};

	let client = rightcodes_api::RightcodesApiClient::new("https://right.codes");
	let payload = match client.list_subscriptions(&token) {
		Ok(v) => v,
		Err(e) => {
			// 失败只显示在菜单里（标题不显示 rc）。
			return (None, e.to_menu_text());
		}
	};

	let Some(summary) = rightcodes::summarize_single_subscription(&payload) else {
		return (
			None,
			"rc：套餐数据缺失（无法计算额度）".to_string(),
		);
	};
	(Some(summary.title_part), summary.menu_status)
}

fn spawn_refresh_loop(app: AppHandle, settings: Arc<Mutex<Settings>>) {
	std::thread::spawn(move || loop {
		let settings = *settings.lock().expect("settings lock poisoned");
		update_tray_title(&app, settings);
		std::thread::sleep(std::time::Duration::from_secs(REFRESH_INTERVAL_SECS));
	});
}

fn open_proxy_window(app: &AppHandle) {
	if let Some(window) = app.get_webview_window("proxy") {
		let _ = window.show();
		let _ = window.set_focus();
		return;
	}

	let builder = tauri::WebviewWindowBuilder::new(
		app,
		"proxy",
		tauri::WebviewUrl::App("index.html?view=proxy".into()),
	)
	.title("代理设置")
	.inner_size(640.0, 520.0)
	.resizable(true)
	.maximizable(true)
	.minimizable(true)
	.closable(true);

	let _ = builder.build();
}

fn open_rightcodes_login_window(app: &AppHandle) {
	if let Some(window) = app.get_webview_window("rightcodes_login") {
		let _ = window.show();
		let _ = window.set_focus();
		return;
	}

	// 说明：使用 Webview 窗口承载登录 UI（支持用户名+密码输入）。
	// 密码只用于换取 token；不会落盘；token 会按“keyring 优先、文件兜底”策略保存。
	let builder = tauri::WebviewWindowBuilder::new(
		app,
		"rightcodes_login",
		tauri::WebviewUrl::App("index.html?view=rightcodes_login".into()),
	)
	.title("Right.codes 登录")
	.inner_size(520.0, 360.0)
	.resizable(true)
	.maximizable(false)
	.minimizable(true)
	.closable(true);

	let _ = builder.build();
}

#[derive(Debug, Clone, Serialize)]
struct ProxySaveResult {
	available: bool,
	last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RightcodesLoginResult {
	stored_in: String,
}

#[tauri::command]
fn tokbar_get_proxy_config() -> proxy_config::ProxyConfig {
	litellm::current_proxy_config()
}

#[tauri::command]
fn tokbar_set_proxy_config(
	app: AppHandle,
	config: proxy_config::ProxyConfig,
) -> Result<ProxySaveResult, String> {
	litellm::update_proxy_config(config)?;
	let pricing = litellm::get_pricing_context();

	if let Some(state) = app.try_state::<AppState>() {
		let settings = *state.settings.lock().expect("settings lock poisoned");
		update_tray_title(&app, settings);
	}

	Ok(ProxySaveResult {
		available: pricing.available,
		last_error: pricing.last_error,
	})
}

#[tauri::command]
fn tokbar_rightcodes_login(app: AppHandle, username: String, password: String) -> Result<RightcodesLoginResult, String> {
	let user = username.trim();
	if user.is_empty() || password.is_empty() {
		return Err("请输入用户名和密码。".to_string());
	}

	let client = rightcodes_api::RightcodesApiClient::new("https://right.codes");
	let token = client.login(user, &password).map_err(|e| match e {
		rightcodes_api::RightcodesApiError::Auth => "认证失败：请检查账号/密码。".to_string(),
		rightcodes_api::RightcodesApiError::RateLimited { retry_after_seconds } => {
			if let Some(s) = retry_after_seconds {
				format!("触发限流（429），请 {s}s 后重试。")
			} else {
				"触发限流（429），请稍后重试。".to_string()
			}
		}
		rightcodes_api::RightcodesApiError::Network => "网络错误：请检查网络后重试。".to_string(),
		rightcodes_api::RightcodesApiError::HttpStatus(code) => format!("登录失败：接口错误（HTTP {code}）。"),
		rightcodes_api::RightcodesApiError::BadPayload => "登录失败：接口返回异常（无法解析）。".to_string(),
	})?;

	let store = rightcodes_token_store::RightcodesTokenStore::new();
	let stored_in = store.save_token(&token).map_err(|e| {
		// 说明：错误信息不得包含任何敏感信息（token/密码）。
		format!("保存 token 失败：{e}")
	})?;

	// 登录成功后立即刷新一次，确保状态栏/菜单立刻更新（而不是等 30s 刷新线程）。
	if let Some(state) = app.try_state::<AppState>() {
		let settings = *state.settings.lock().expect("settings lock poisoned");
		update_tray_title(&app, settings);
	}

	let stored_in_text = match stored_in {
		rightcodes_token_store::StoredIn::Keyring => "keyring",
		rightcodes_token_store::StoredIn::File => "file",
	};

	Ok(RightcodesLoginResult {
		stored_in: stored_in_text.to_string(),
	})
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
	tauri::Builder::default()
		.plugin(tauri_plugin_opener::init())
		.plugin(tauri_plugin_autostart::init(
			tauri_plugin_autostart::MacosLauncher::LaunchAgent,
			None,
		))
		.invoke_handler(tauri::generate_handler![
			tokbar_get_proxy_config,
			tokbar_set_proxy_config,
			tokbar_rightcodes_login
		])
		.setup(|app| {
			use tauri_plugin_autostart::ManagerExt as _;

			let settings = Settings::default();
			let prefs = app_settings::load_settings();

			apply_dock_icon_preference(&app.handle(), prefs.show_dock_icon);
			if prefs.autostart {
				let _ = app.handle().autolaunch().enable();
			} else {
				let _ = app.handle().autolaunch().disable();
			}

			let (menu, menu_handles) = build_menu(&app.handle(), settings, &prefs)?;

			let state = AppState {
				settings: Arc::new(Mutex::new(settings)),
				prefs: Arc::new(Mutex::new(prefs)),
				menu: menu_handles,
				last_ui: Arc::new(Mutex::new(LastUiState::default())),
			};
			app.manage(state.clone());

			let title = compute_title(&app.handle(), settings);
			let mut tray_builder = TrayIconBuilder::with_id("tokbar-tray")
				.menu(&menu)
				.title(&title);

			if let Some(icon) = load_tray_icon_image() {
				tray_builder = tray_builder.icon(icon);
			}

			tray_builder.on_menu_event(|app, event| {
					let Some(state) = app.try_state::<AppState>() else {
						return;
					};
					let mut settings = state.settings.lock().expect("settings lock poisoned");

					match event.id().as_ref() {
						"rightcodes.login" => {
							open_rightcodes_login_window(app);
							return;
						}
						"refresh" => {
							let app = app.clone();
							let settings = *settings;
							std::thread::spawn(move || update_tray_title(&app, settings));
							return;
						}
						"dock.icon" => {
							let mut prefs = state.prefs.lock().expect("prefs lock poisoned");
							prefs.show_dock_icon = !prefs.show_dock_icon;
							let _ = app_settings::save_settings(prefs.clone());
							apply_dock_icon_preference(app, prefs.show_dock_icon);
							let _ = state.menu.dock_icon.set_checked(prefs.show_dock_icon);
							return;
						}
						"autostart" => {
							use tauri_plugin_autostart::ManagerExt as _;
							let mut prefs = state.prefs.lock().expect("prefs lock poisoned");
							let next = !prefs.autostart;
							let result = if next {
								app.autolaunch().enable()
							} else {
								app.autolaunch().disable()
							};
							if result.is_ok() {
								prefs.autostart = next;
								let _ = app_settings::save_settings(prefs.clone());
								let _ = state.menu.autostart.set_checked(prefs.autostart);
							}
							return;
						}
						"pricing.status" | "proxy.open" => {
							open_proxy_window(app);
							return;
						}
						"quit" => app.exit(0),
						"period.today" => settings.period = Period::Today,
						"period.week" => settings.period = Period::Week,
						"period.month" => settings.period = Period::Month,
						"period.year" => settings.period = Period::Year,
						"source.cx" => settings.source = Source::Cx,
						"source.cc" => settings.source = Source::Cc,
						"source.both" => settings.source = Source::Both,
						_ => {}
					}

					let updated = *settings;
					drop(settings);
					sync_menu_checks(&state.menu, updated);
					let app = app.clone();
					std::thread::spawn(move || update_tray_title(&app, updated));
				}).build(app)?;

			{
				let app = app.handle().clone();
				std::thread::spawn(move || update_tray_title(&app, settings));
			}
			sync_menu_checks(&state.menu, settings);

			spawn_refresh_loop(app.handle().clone(), state.settings.clone());

			Ok(())
		})
		.run(tauri::generate_context!())
		.expect("error while running tauri application");
}
