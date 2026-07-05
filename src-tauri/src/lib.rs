//! OpenMind Desktop — Rust core entry point.
//!
//! This is where the four architecture pieces from the spec (§7) get
//! constructed and registered as Tauri-managed state:
//!
//!   ModelRouter ← LazyAgent
//!   MemoryTree
//!   ConnectorRegistry
//!
//! `run()` here, not `main.rs` — see that file's comment for why
//! (mobile builds need the entry point in the library crate).

mod commands;
mod error;
mod lazy_agent;
mod mcp;
mod memory;
mod model_router;
mod oauth;

use lazy_agent::LazyAgent;
use mcp::ConnectorRegistry;
use memory::MemoryTree;
use model_router::ModelRouter;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            // Initialize the OS keychain store for OAuth token storage (spec §6).
            // Must happen before any oauth::store_token / load_token calls.
            // keyring-core requires set_default_store() to be called once at startup.
            #[cfg(target_os = "macos")]
            {
                use apple_native_keyring_store::AppleKeyringStore;
                keyring_core::set_default_store(std::sync::Arc::new(AppleKeyringStore::default()));
            }
            #[cfg(target_os = "windows")]
            {
                use windows_native_keyring_store::WindowsKeyringStore;
                keyring_core::set_default_store(std::sync::Arc::new(WindowsKeyringStore::default()));
            }
            // Linux: keyring-core's built-in file-based store is the default
            // when no explicit store is set. It encrypts tokens at rest
            // using a machine-derived key. Secret Service integration
            // (KDE Wallet / GNOME Keyring) can be added as a later improvement.

            let model_router = ModelRouter::new();
            let lazy_agent = LazyAgent::new();

            // Real on-disk path — this is what makes Milestone 3 work:
            // data survives restarts. Tauri's app_data_dir() resolves to:
            //   macOS:   ~/Library/Application Support/com.openmind.desktop/
            //   Linux:   ~/.local/share/com.openmind.desktop/
            //   Windows: %APPDATA%\com.openmind.desktop\
            //
            // Falls back to in-memory only if the path can't be determined
            // (shouldn't happen in a real Tauri app, but better than crashing
            // on an edge case at startup).
            let memory_tree = {
                let db_path = app
                    .path()
                    .app_data_dir()
                    .ok()
                    .map(|d| d.join("memory.db"));

                match db_path {
                    Some(path) => {
                        // Ensure the directory exists before opening.
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        MemoryTree::open(path)
                            .unwrap_or_else(|e| {
                                eprintln!("MemoryTree: failed to open on-disk db ({e}), \
                                           falling back to in-memory");
                                MemoryTree::open_in_memory()
                                    .expect("in-memory fallback should always work")
                            })
                    }
                    None => {
                        eprintln!("MemoryTree: could not determine app data dir, \
                                   using in-memory database (data will not persist)");
                        MemoryTree::open_in_memory()
                            .expect("in-memory fallback should always work")
                    }
                }
            };

            let connector_registry = ConnectorRegistry::new();

            app.manage(model_router);
            app.manage(lazy_agent);
            app.manage(memory_tree);
            app.manage(connector_registry);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_model_status,
            commands::send_chat_message,
            commands::get_token_savings,
            commands::add_memory,
            commands::list_memory_tree,
            commands::search_memory,
            commands::get_background_loop_status,
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
