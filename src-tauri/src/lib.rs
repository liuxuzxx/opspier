pub mod connections;
#[cfg(test)]
mod credential_tests;
pub mod credentials;
pub mod db;
pub mod kube_import;
#[cfg(test)]
mod kube_import_tests;
pub mod kubernetes;
#[cfg(test)]
mod profile_tests;
pub mod profiles;
pub mod ssh;

use std::sync::Arc;
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(Arc::new(ssh::SshService::default()))
        .manage(db::DbService::default())
        .manage(kubernetes::KubeRuntimes::default())
        .setup(|app| {
            let connections = connections::Connections::open(
                app.path().app_config_dir()?.join("profiles-v1.json"),
            )
            .map_err(std::io::Error::other)?;
            app.manage(Arc::new(connections));
            if let Some(window) = app.get_webview_window("main") {
                window.set_title(&format!(
                    "{} · 运维工作台 {}",
                    app.config().product_name.as_deref().unwrap_or("OpsPier"),
                    env!("CARGO_PKG_VERSION")
                ))?;
            }
            app.state::<Arc<ssh::SshService>>()
                .initialize(app.path().app_config_dir()?.join("host-pins.json"));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connections::kube_test_connection,
            connections::profiles_list,
            connections::profiles_save,
            connections::profiles_remove,
            connections::profiles_retry_cleanup,
            connections::profiles_import_legacy,
            connections::profiles_import_kube,
            connections::profiles_discard_import,
            connections::profiles_connect_db,
            connections::profiles_test_db,
            connections::profiles_probe_ssh,
            connections::profiles_connect_ssh,
            connections::profiles_open_kube,
            ssh::ssh_test_probe,
            ssh::ssh_test_authenticate,
            ssh::ssh_probe,
            ssh::ssh_connect,
            ssh::ssh_poll,
            ssh::ssh_input,
            ssh::ssh_resize,
            ssh::ssh_disconnect,
            db::db_test_connection,
            db::db_connect,
            db::db_disconnect,
            db::db_databases,
            db::db_tables,
            db::db_describe,
            db::db_query,
            db::db_query_page,
            db::db_cancel,
            db::db_export_csv,
            db::db_forget_query,
            kubernetes::kube_open,
            kubernetes::kube_select,
            kubernetes::kube_namespaces,
            kubernetes::kube_list,
            kubernetes::query::kube_query,
            kubernetes::query::kube_cancel_query,
            kubernetes::query::kube_resources,
            kubernetes::query::kube_resource_detail,
            kubernetes::query::kube_resource_logs_start,
            kubernetes::kube_detail,
            kubernetes::kube_watch,
            kubernetes::kube_logs_start,
            kubernetes::kube_logs_poll,
            kubernetes::kube_logs_stop,
            kubernetes::kube_stop,
            kubernetes::kube_close,
        ])
        .run(tauri::generate_context!())
        .expect("OpsPier 桌面启动失败");
}
