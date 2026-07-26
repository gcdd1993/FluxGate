mod balance;
mod cache_injector;
mod db;
mod gateway;
mod models;
mod rectifier;
mod storage;
mod takeover;
mod webdav;

use db::Database;
use gateway::GatewayState;
use models::{
    BalanceInfo, DashboardData, GatewayConfig, ModelAlias, Provider, UsageFilter, WebdavConfig,
};
use storage::{BackupEntry, StorageInfo, StoragePaths};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, State, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

struct AppState {
    gateway: Arc<GatewayState>,
    /// Mirrors `GatewayConfig::minimize_to_tray` for the synchronous
    /// window-close handler, which cannot await the config lock.
    minimize_to_tray: Arc<AtomicBool>,
    paths: StoragePaths,
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn apply_autostart(app: &AppHandle, enabled: bool) {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(error) = result {
        eprintln!("FluxGate autostart update failed: {error}");
    }
}

#[tauri::command]
async fn get_dashboard(
    state: State<'_, AppState>,
    filter: UsageFilter,
) -> Result<DashboardData, String> {
    let (summary, trend, provider_stats, model_stats, logs) = state
        .gateway
        .db
        .analytics(&filter)
        .map_err(|e| e.to_string())?;
    Ok(DashboardData {
        config: state.gateway.config.read().await.clone(),
        providers: state.gateway.providers.read().await.clone(),
        aliases: state.gateway.aliases.read().await.clone(),
        summary,
        trend,
        provider_stats,
        model_stats,
        logs,
        uptime_seconds: state.gateway.started.elapsed().as_secs(),
        active_connections: state.gateway.active.load(Ordering::SeqCst),
    })
}
#[tauri::command]
async fn save_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: GatewayConfig,
) -> Result<(), String> {
    let previous = state.gateway.config.read().await.clone();
    takeover::apply_changes(&previous, &config).map_err(|e| e.to_string())?;
    state
        .gateway
        .db
        .save_config(&config)
        .map_err(|e| e.to_string())?;
    state
        .minimize_to_tray
        .store(config.minimize_to_tray, Ordering::SeqCst);
    if previous.launch_at_login != config.launch_at_login {
        apply_autostart(&app, config.launch_at_login);
    }
    *state.gateway.config.write().await = config;
    Ok(())
}
#[tauri::command]
async fn save_provider(state: State<'_, AppState>, provider: Provider) -> Result<(), String> {
    state
        .gateway
        .db
        .save_provider(&provider)
        .map_err(|e| e.to_string())?;
    let mut values = state.gateway.providers.write().await;
    if let Some(i) = values.iter().position(|x| x.id == provider.id) {
        values[i] = provider
    } else {
        values.push(provider)
    }
    Ok(())
}
#[tauri::command]
async fn fetch_provider_models(
    state: State<'_, AppState>,
    provider: Provider,
) -> Result<Vec<String>, String> {
    state
        .gateway
        .fetch_provider_models(&provider)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn fetch_provider_balance(
    state: State<'_, AppState>,
    provider: Provider,
) -> Result<BalanceInfo, String> {
    balance::fetch(state.gateway.http(), &provider)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn delete_provider(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .gateway
        .db
        .delete_provider(&id)
        .map_err(|e| e.to_string())?;
    state.gateway.providers.write().await.retain(|x| x.id != id);
    state.gateway.aliases.write().await.retain_mut(|alias| {
        alias.normalize();
        alias.targets.retain(|target| target.provider_id != id);
        !alias.targets.is_empty()
    });
    Ok(())
}
#[tauri::command]
async fn save_alias(state: State<'_, AppState>, alias: ModelAlias) -> Result<(), String> {
    state
        .gateway
        .db
        .save_alias(&alias)
        .map_err(|e| e.to_string())?;
    let mut values = state.gateway.aliases.write().await;
    if let Some(i) = values.iter().position(|x| x.id == alias.id) {
        values[i] = alias
    } else {
        values.push(alias)
    }
    Ok(())
}
#[tauri::command]
async fn delete_alias(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .gateway
        .db
        .delete_alias(&id)
        .map_err(|e| e.to_string())?;
    state.gateway.aliases.write().await.retain(|x| x.id != id);
    Ok(())
}
#[tauri::command]
async fn reset_circuit(state: State<'_, AppState>, provider_id: String) -> Result<(), String> {
    state.gateway.reset_circuit(&provider_id).await;
    Ok(())
}
#[tauri::command]
fn storage_info(state: State<'_, AppState>) -> StorageInfo {
    storage::info(&state.paths)
}
#[tauri::command]
async fn choose_directory(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let folder = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().blocking_pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;
    match folder {
        Some(path) => Ok(Some(
            path.into_path().map_err(|e| e.to_string())?.display().to_string(),
        )),
        None => Ok(None),
    }
}
#[tauri::command]
fn set_data_directory(state: State<'_, AppState>, path: String) -> Result<(), String> {
    storage::set_data_dir(&state.paths, &state.gateway.db, &path).map_err(|e| e.to_string())
}
#[tauri::command]
fn reset_data_directory(state: State<'_, AppState>) -> Result<(), String> {
    storage::reset_data_dir(&state.paths, &state.gateway.db).map_err(|e| e.to_string())
}
#[tauri::command]
fn open_data_directory(state: State<'_, AppState>) -> Result<(), String> {
    let dir = state.paths.data_dir.clone();
    #[cfg(target_os = "windows")]
    let command = "explorer";
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let command = "xdg-open";
    std::process::Command::new(command)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn backup_now(state: State<'_, AppState>) -> Result<BackupEntry, String> {
    storage::create_backup(&state.paths.data_dir, &state.gateway.db).map_err(|e| e.to_string())
}
#[tauri::command]
fn backup_list(state: State<'_, AppState>) -> Vec<BackupEntry> {
    storage::list_backups(&state.paths.data_dir)
}
#[tauri::command]
fn backup_delete(state: State<'_, AppState>, file_name: String) -> Result<(), String> {
    storage::delete_backup(&state.paths.data_dir, &file_name).map_err(|e| e.to_string())
}
#[tauri::command]
fn backup_restore(state: State<'_, AppState>, file_name: String) -> Result<(), String> {
    storage::stage_restore_backup(&state.paths.data_dir, &file_name).map_err(|e| e.to_string())
}
#[tauri::command]
async fn webdav_test(state: State<'_, AppState>, config: WebdavConfig) -> Result<(), String> {
    webdav::test(state.gateway.http(), &config)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn webdav_upload(
    state: State<'_, AppState>,
    config: WebdavConfig,
) -> Result<webdav::SyncReport, String> {
    webdav::upload(
        state.gateway.http(),
        &config,
        &state.gateway.db,
        &state.paths.data_dir,
    )
    .await
    .map_err(|e| e.to_string())
}
#[tauri::command]
async fn webdav_download(
    state: State<'_, AppState>,
    config: WebdavConfig,
) -> Result<webdav::SyncReport, String> {
    webdav::download(state.gateway.http(), &config, &state.paths.data_dir)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn restart_app(app: AppHandle) {
    app.restart();
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateInfo {
    version: String,
    notes: String,
}
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(UpdateInfo {
            version: update.version.clone(),
            notes: update.body.clone().unwrap_or_default(),
        })),
        Ok(None) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("当前已是最新版本")?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let paths = storage::resolve(app.path().app_data_dir()?);
            // A staged restore (local backup or WebDAV download) replaces the
            // database now, before any connection opens it.
            storage::apply_pending_restore(&paths.data_dir)?;
            let db = Arc::new(Database::open(&paths.data_dir.join(storage::DB_FILE))?);
            storage::auto_backup(&paths.data_dir, &db);
            let config = db.config();
            let gateway = Arc::new(GatewayState::new(
                db.clone(),
                config.clone(),
                db.providers()?,
                db.aliases()?,
            ));
            let server = gateway.clone();
            let host = config.host.clone();
            let port = config.port;
            tauri::async_runtime::spawn(async move {
                if let Err(e) = gateway::serve(server, host, port).await {
                    eprintln!("FluxGate server stopped: {e:#}");
                }
            });
            apply_autostart(app.handle(), config.launch_at_login);
            let show = MenuItem::with_id(app, "show", "显示主界面", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出 FluxGate", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("fluxgate-tray")
                .tooltip("FluxGate · 本地 AI 网关")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            app.manage(AppState {
                gateway,
                minimize_to_tray: Arc::new(AtomicBool::new(config.minimize_to_tray)),
                paths,
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let minimize = window
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|state| state.minimize_to_tray.load(Ordering::SeqCst))
                    .unwrap_or(false);
                if minimize {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_dashboard,
            save_config,
            save_provider,
            fetch_provider_models,
            fetch_provider_balance,
            delete_provider,
            save_alias,
            delete_alias,
            reset_circuit,
            storage_info,
            choose_directory,
            set_data_directory,
            reset_data_directory,
            open_data_directory,
            backup_now,
            backup_list,
            backup_delete,
            backup_restore,
            webdav_test,
            webdav_upload,
            webdav_download,
            restart_app,
            check_update,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running FluxGate");
}
