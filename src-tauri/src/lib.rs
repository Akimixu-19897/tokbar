mod claude;
mod app_settings;
mod format;
pub mod raw_format;
mod codex;
mod pricing;
mod proxy_config;
pub mod litellm;

#[cfg(test)]
mod test_util;
mod time_parse;
pub mod time_range;
pub mod usage;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

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

	match settings.source {
		Source::Cx => format::format_single_title(period, "cx", cx, show_cost),
		Source::Cc => match cc_result {
			Ok(totals) => format::format_single_title(period, "cc", totals, show_cost),
			Err(_) => format!("{period} cc ERR"),
		},
		Source::Both => {
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
		MenuItem::with_id(app, "stats.cx_full", "Loading cx…", false, None::<&str>)?;
	let stats_cc_full =
		MenuItem::with_id(app, "stats.cc_full", "Loading cc…", false, None::<&str>)?;
	let totals_cx_all =
		MenuItem::with_id(app, "totals.cx_all", "All cx: Loading…", false, None::<&str>)?;
	let totals_cc_all =
		MenuItem::with_id(app, "totals.cc_all", "All cc: Loading…", false, None::<&str>)?;
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
	let pricing_status = MenuItem::with_id(
		app,
		"pricing.status",
		"模型价格：检查中…",
		true,
		None::<&str>,
	)?;
	let proxy_open = MenuItem::with_id(app, "proxy.open", "Proxy…", true, None::<&str>)?;

	let period_today = CheckMenuItem::with_id(
		app,
		"period.today",
		"Today",
		true,
		settings.period == Period::Today,
		None::<&str>,
	)?;
	let period_week = CheckMenuItem::with_id(
		app,
		"period.week",
		"Week",
		true,
		settings.period == Period::Week,
		None::<&str>,
	)?;
	let period_month = CheckMenuItem::with_id(
		app,
		"period.month",
		"Month",
		true,
		settings.period == Period::Month,
		None::<&str>,
	)?;
	let period_year = CheckMenuItem::with_id(
		app,
		"period.year",
		"Year",
		true,
		settings.period == Period::Year,
		None::<&str>,
	)?;

	let source_cx = CheckMenuItem::with_id(
		app,
		"source.cx",
		"cx (Codex)",
		true,
		settings.source == Source::Cx,
		None::<&str>,
	)?;
	let source_cc = CheckMenuItem::with_id(
		app,
		"source.cc",
		"cc (Claude Code)",
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
		"Period",
		true,
		&[&period_today, &period_week, &period_month, &period_year],
	)?;
	let source_menu = Submenu::with_id_and_items(
		app,
		"source",
		"Source",
		true,
		&[&source_cx, &source_cc, &source_both],
	)?;

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
			&PredefinedMenuItem::separator(app)?,
			&MenuItem::with_id(app, "refresh", "Refresh Now", true, None::<&str>)?,
			&period_menu,
			&source_menu,
			&PredefinedMenuItem::separator(app)?,
			&MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
		],
	)?;

	Ok((
		menu,
		MenuHandles {
			stats_cx_full,
			stats_cc_full,
			totals_cx_all,
			totals_cc_all,
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
		let range = range_for_period(settings.period);
		let period = range.label;
		let pricing = litellm::get_pricing_context();
		let show_cost = pricing.available;
		let dataset = &pricing.dataset;

			let cx = usage::load_cx_totals_with_pricing(&range, dataset);
			let cc_result = usage::load_cc_totals_with_pricing(&range, dataset);
			let cc_for_both = cc_result.as_ref().copied().unwrap_or_default();
			let all_label = "All";
			let show_all_cost = pricing.available;
			let cx_all = usage::load_cx_totals_all_time_cached_with_pricing(dataset);
			let cc_all_result = usage::load_cc_totals_all_time_cached_with_pricing(dataset);

			let title = match settings.source {
				Source::Cx => format::format_single_title(period, "cx", cx, show_cost),
				Source::Cc => match cc_result {
				Ok(totals) => format::format_single_title(period, "cc", totals, show_cost),
				Err(_) => format!("{period} cc ERR"),
			},
			Source::Both => {
				format::format_both_title_one_line(period, cx, cc_for_both, show_cost)
			}
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

			// Also update menu items with full (non-compacted) totals.
			if let Some(state) = state.as_ref() {
				let full_cx = raw_format::format_single_title_raw(period, "cx", cx, show_cost);
				let full_cc = if cc_result.is_ok() {
					raw_format::format_single_title_raw(period, "cc", cc_for_both, show_cost)
				} else {
					format!("{period} cc ERR")
				};
				let all_cx = raw_format::format_single_title_raw(
					all_label,
					"cx",
					cx_all,
					show_all_cost,
				);
				let all_cc = match cc_all_result {
					Ok(totals) => raw_format::format_single_title_raw(
						all_label,
						"cc",
						totals,
						show_all_cost,
					),
					Err(_) => format!("{all_label} cc ERR"),
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
		}
	}
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
	.title("Proxy Settings")
	.inner_size(640.0, 520.0)
	.resizable(true)
	.maximizable(true)
	.minimizable(true)
	.closable(true);

	let _ = builder.build();
}

#[derive(Debug, Clone, Serialize)]
struct ProxySaveResult {
	available: bool,
	last_error: Option<String>,
}

#[tauri::command]
fn tokbar_get_proxy_config() -> proxy_config::ProxyConfig {
	litellm::current_proxy_config()
}

#[tauri::command]
fn tokbar_set_proxy_config(app: AppHandle, config: proxy_config::ProxyConfig) -> Result<ProxySaveResult, String> {
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
			tokbar_set_proxy_config
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
			TrayIconBuilder::with_id("tokbar-tray")
				.menu(&menu)
				.title(&title)
				.on_menu_event(|app, event| {
					let Some(state) = app.try_state::<AppState>() else {
						return;
					};
					let mut settings = state.settings.lock().expect("settings lock poisoned");

						match event.id().as_ref() {
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
				})
				.build(app)?;

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
