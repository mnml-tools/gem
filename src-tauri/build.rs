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

/// tauri-build embeds the Windows application manifest (Common Controls v6, needed by the dialog
/// APIs) into the app binary only, so the `cargo test` executables die at load with
/// STATUS_ENTRYPOINT_NOT_FOUND. Embed it through the linker instead, for every target.
/// Returns whether it did, so tauri-build can skip its own copy (a duplicate fails to link).
fn embed_windows_manifest() -> bool {
    let msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !msvc {
        return false;
    }
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    true
}

fn main() {
    check_main_capability();
    let mut attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if embed_windows_manifest() {
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    }
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build failed: {error:#}");
    }
}
