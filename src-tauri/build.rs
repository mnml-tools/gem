//! Build script: Tauri codegen plus the app command manifest.
//!
//! Declaring every app command here makes Tauri generate one `allow-<command>` permission per
//! command, so the window's capability (capabilities/main.json) grants only what it needs
//! (least privilege; see docs/architecture.md#security-boundaries). A command missing here is
//! denied at runtime.

const COMMANDS: &[&str] = &[
    "app_status",
    "get_settings",
    "update_settings",
    "choose_download_folder",
    "open_download_folder",
    "save_log",
    "list_sites",
    "create_site",
    "update_site",
    "delete_site",
    "duplicate_site",
    "reorder_sites",
    "choose_key_file",
    "forget_host_key",
    "import_ssh_config",
    "import_filezilla",
    "export_sites",
    "import_sites_file",
    "connect",
    "test_connection",
    "trust_host_key",
    "disconnect",
    "reconnect",
    "list_dir",
    "stat_path",
    "make_dir",
    "make_file",
    "rename_path",
    "delete_paths",
    "chmod_path",
    "open_file",
    "read_more",
    "save_file",
    "download_paths",
    "upload_files",
    "upload_dropped",
    "cancel_transfer",
    "retry_transfer",
    "clear_transfers",
    "resolve_conflict",
    "list_snippets",
    "create_snippet",
    "update_snippet",
    "delete_snippet",
    "reorder_snippets",
    "run_command",
    "cancel_command",
];

/// The main window is the full UI: fail the build if its capability misses a command.
fn check_main_capability() {
    println!("cargo:rerun-if-changed=capabilities/main.json");
    let main = std::fs::read_to_string("capabilities/main.json").unwrap_or_default();
    let missing: Vec<&str> = COMMANDS
        .iter()
        .copied()
        .filter(|c| !main.contains(&format!("\"allow-{}\"", c.replace('_', "-"))))
        .collect();
    assert!(
        missing.is_empty(),
        "capabilities/main.json does not grant: {missing:?}"
    );
}

fn main() {
    check_main_capability();
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build failed: {error:#}");
    }
}
