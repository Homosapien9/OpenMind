mod commands;
mod error;
mod lazy_agent;
mod mcp;
mod memory;
mod model_router;
mod oauth;
mod settings;

use tauri::Manager;
use lazy_agent::LazyAgent;
use mcp::ConnectorRegistry;
use memory::MemoryTree;
use model_router::ModelRouter;
use settings::SettingsStore;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                // `apple_native_keyring_store::keychain::Store::new()` already returns an
                // `Arc<Store>`, so wrapping it again in `Arc::new(...)` produces an
                // `Arc<Arc<Store>>`, which does not satisfy `CredentialStoreApi`.
                // Pass it straight through so it coerces to `Arc<dyn CredentialStoreApi>`.
                let store = apple_native_keyring_store::keychain::Store::new()
                    .expect("failed to initialize macOS Keychain store");
                keyring_core::set_default_store(store);
            }
            #[cfg(target_os = "windows")]
            {
                // `windows_native_keyring_store::Store::new()` already returns an
                // `Arc<Store>`, so wrapping it again in `Arc::new(...)` produced an
                // `Arc<Arc<Store>>`, which does not satisfy `CredentialStoreApi`.
                // Pass it straight through so it coerces to `Arc<dyn CredentialStoreApi>`.
                let store = windows_native_keyring_store::Store::new()
                    .expect("failed to initialize Windows Credential store");
                keyring_core::set_default_store(store);
            }

            let app_data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("openmind-desktop"));
            let _ = std::fs::create_dir_all(&app_data_dir);

            let model_router = ModelRouter::new();
            let lazy_agent = LazyAgent::new();
            let memory_tree = MemoryTree::open(app_data_dir.join("memory.db")).unwrap_or_else(|_| {
                MemoryTree::open_in_memory().expect("in-memory fallback should always work")
            });
            let connector_registry = ConnectorRegistry::new();
            let settings_store = SettingsStore::load(app_data_dir.join("settings.json")).unwrap_or_else(|_| {
                SettingsStore::load(std::env::temp_dir().join("openmind-desktop-settings.json"))
                    .expect("temporary settings fallback should always work")
            });

            app.manage(model_router);
            app.manage(lazy_agent);
            app.manage(memory_tree);
            app.manage(connector_registry);
            app.manage(settings_store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_model_status,
            commands::get_app_settings,
            commands::update_app_settings,
            commands::send_chat_message,
            commands::send_chat_message_streaming,
            commands::get_token_savings,
            commands::add_memory,
            commands::list_memory_tree,
            commands::search_memory,
            commands::get_background_loop_status,
            commands::get_conversation_history,
            commands::list_conversation_threads,
            commands::list_connectors,
            commands::connect_integration,
            commands::disconnect_integration,
            commands::list_tools,
            commands::call_tool,
            commands::begin_oauth,
            commands::get_oauth_token,
            commands::revoke_oauth_token,
        ])
        .run(tauri::generate_context!())
        .expect("error while running OpenMind Desktop");
}
