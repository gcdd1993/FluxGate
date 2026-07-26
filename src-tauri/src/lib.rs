mod db;
mod gateway;
mod models;
mod rectifier;
mod takeover;

use db::Database;
use gateway::GatewayState;
use models::{DashboardData, GatewayConfig, ModelAlias, Provider, UsageFilter};
use std::sync::{atomic::Ordering, Arc};
use tauri::{Manager, State};

struct AppState {
    gateway: Arc<GatewayState>,
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
async fn save_config(state: State<'_, AppState>, config: GatewayConfig) -> Result<(), String> {
    let previous = state.gateway.config.read().await.clone();
    takeover::apply_changes(&previous, &config).map_err(|e| e.to_string())?;
    state
        .gateway
        .db
        .save_config(&config)
        .map_err(|e| e.to_string())?;
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
fn seed_demo_data(state: State<'_, AppState>) -> Result<(), String> {
    state.gateway.db.seed_demo().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let db = Arc::new(Database::open(&data_dir.join("fluxgate.db"))?);
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
            app.manage(AppState { gateway });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_dashboard,
            save_config,
            save_provider,
            fetch_provider_models,
            delete_provider,
            save_alias,
            delete_alias,
            reset_circuit,
            seed_demo_data
        ])
        .run(tauri::generate_context!())
        .expect("error while running FluxGate");
}
