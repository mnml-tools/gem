//! IPC contract tests: real commands, JSON in camelCase exactly as `src/lib/ipc/api.ts` sends
//! it, through Tauri's mock runtime and the in-memory remote. They catch argument-name and
//! serialisation mismatches the type checker cannot. Commands that open native dialogs are not
//! covered here.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{
    INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewWindow, WebviewWindowBuilder};

use super::support::DataState;
use crate::platform::drag_drop::DropRegistry;
use crate::remote::memory::MemoryServer;
use crate::services::secrets::{MemoryStore, SecretStore};
use crate::services::sessions::{Connector, SessionManager};
use crate::services::test_support::{MemoryConnector, PASSWORD};
use crate::storage::Database;

struct Harness {
    app: App<MockRuntime>,
    window: WebviewWindow<MockRuntime>,
    server: Arc<MemoryServer>,
    db: Arc<Database>,
}

fn harness() -> Harness {
    let server = MemoryServer::new();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let manager = SessionManager::new(
        Arc::new(MemoryConnector::new(Arc::clone(&server))) as Arc<dyn Connector>,
        Arc::new(MemoryStore::default()) as Arc<dyn SecretStore>,
    );
    let app = mock_builder()
        .manage(DataState(Ok(Arc::clone(&db))))
        .manage(manager)
        .manage(DropRegistry::default())
        .manage(crate::services::secrets::KeyringProbe::default())
        .invoke_handler(crate::app_commands!())
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let window = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock window");
    Harness {
        app,
        window,
        server,
        db,
    }
}

fn site_input() -> Value {
    json!({
        "name": "prod",
        "protocol": "sftp",
        "host": "example.test",
        "port": 22,
        "user": "deploy",
        "auth": "password",
        "keyPath": null,
        "ftpsMode": "explicit",
        "passive": true,
        "initialPath": null,
        "color": "blue"
    })
}

impl Harness {
    fn invoke(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        let request = InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: self.window.url().expect("window url"),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        get_ipc_response(&self.window, request).map(|body| body.deserialize::<Value>().unwrap())
    }

    fn ok(&self, cmd: &str, args: Value) -> Value {
        self.invoke(cmd, args)
            .unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
    }

    fn connect(&self) -> String {
        let info = self.ok(
            "connect",
            json!({
                "target": { "kind": "adhoc", "site": site_input() },
                "secret": PASSWORD,
                "remember": false,
                "events": "__CHANNEL__:1"
            }),
        );
        info["id"].as_str().unwrap().to_owned()
    }

    fn wait_transfer(&self, session_id: &str, id: &str, state: &str) {
        let manager = self.app.state::<SessionManager>();
        let session = manager.get(session_id).unwrap();
        for _ in 0..1000 {
            let done = session
                .transfers
                .snapshot()
                .iter()
                .any(|t| t.id == id && serde_json::to_value(t.state).unwrap() == state);
            if done {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("transfer {id} never reached {state}");
    }
}

#[test]
fn app_status_and_settings() {
    let h = harness();
    let status = h.ok("app_status", json!({}));
    assert_eq!(status["version"], "0.1.0");
    assert!(["linux", "macos", "windows"].contains(&status["platform"].as_str().unwrap()));
    assert!(status["dataError"].is_null());
    assert_eq!(status["keyringAvailable"], true);

    let mut settings = h.ok("get_settings", json!({}));
    assert_eq!(settings["previewWindowKib"], 2048);
    settings["transferConcurrency"] = json!(99);
    settings["conflictPolicy"] = json!("rename");
    let saved = h.ok("update_settings", json!({ "settings": settings }));
    assert_eq!(saved["transferConcurrency"], 8);
    assert_eq!(saved["conflictPolicy"], "rename");
    settings["downloadFolder"] = json!("/etc");
    let refused = h
        .invoke("update_settings", json!({ "settings": settings }))
        .unwrap_err();
    assert_eq!(refused["code"], "validation");
}

#[test]
fn sites_crud_with_camel_case_arguments() {
    let h = harness();
    let site = h.ok(
        "create_site",
        json!({ "input": site_input(), "secret": "s3cret" }),
    );
    assert_eq!(site["hasSecret"], true);
    assert_eq!(site["color"], "blue");
    let id = site["id"].as_str().unwrap();
    let mut input = site_input();
    input["name"] = json!("renamed");
    let updated = h.ok(
        "update_site",
        json!({ "id": id, "input": input, "secret": { "kind": "clear" } }),
    );
    assert_eq!(updated["name"], "renamed");
    assert_eq!(updated["hasSecret"], false);
    h.ok(
        "update_site",
        json!({ "id": id, "input": input, "secret": { "kind": "set", "value": "x" } }),
    );
    let copy = h.ok("duplicate_site", json!({ "id": id }));
    h.ok("reorder_sites", json!({ "ids": [copy["id"], id] }));
    let listed = h.ok("list_sites", json!({}));
    assert_eq!(listed[0]["id"], copy["id"]);
    h.ok("delete_site", json!({ "id": id }));
    assert_eq!(h.ok("list_sites", json!({})).as_array().unwrap().len(), 1);
    let mut nameless = site_input();
    nameless["name"] = json!(" ");
    let invalid = h
        .invoke("create_site", json!({ "input": nameless, "secret": null }))
        .unwrap_err();
    assert_eq!(invalid["code"], "validation");
}

#[test]
fn host_keys_trust_and_forget() {
    let h = harness();
    let key = json!({
        "host": "example.test",
        "port": 22,
        "keyType": "ssh-ed25519",
        "fingerprint": "SHA256:abc",
        "previousFingerprint": null
    });
    h.ok("trust_host_key", json!({ "key": key }));
    let mut other = key.clone();
    other["fingerprint"] = json!("SHA256:other");
    let refused = h
        .invoke("trust_host_key", json!({ "key": other }))
        .unwrap_err();
    assert_eq!(refused["code"], "conflict");
    assert_eq!(
        h.ok(
            "forget_host_key",
            json!({ "host": "example.test", "port": 22 })
        ),
        true
    );
}

#[test]
fn sessions_browse_and_edit() {
    let h = harness();
    let tested = h.ok(
        "test_connection",
        json!({ "site": site_input(), "secret": PASSWORD, "siteId": null }),
    );
    assert_eq!(tested["ok"], true);
    assert_eq!(tested["home"], "/home/user");
    assert!(tested["error"].is_null());
    assert!(!tested["log"].as_array().unwrap().is_empty());
    let failed = h.ok(
        "test_connection",
        json!({ "site": site_input(), "secret": "wrong", "siteId": null }),
    );
    assert_eq!(failed["ok"], false);
    assert!(failed["home"].is_null());
    assert_eq!(failed["error"]["code"], "auth_failed");
    assert!(failed["error"]["hostKey"].is_null());
    assert!(!failed["log"].as_array().unwrap().is_empty());

    let session = h.connect();
    let sid = session.as_str();
    let made = h.ok(
        "make_dir",
        json!({ "sessionId": sid, "path": "/home/user/app" }),
    );
    assert_eq!(made["kind"], "dir");
    h.ok(
        "make_file",
        json!({ "sessionId": sid, "path": "/home/user/app/a.txt" }),
    );
    let renamed = h.ok(
        "rename_path",
        json!({ "sessionId": sid, "from": "/home/user/app/a.txt", "to": "/home/user/app/b.txt" }),
    );
    assert_eq!(renamed["path"], "/home/user/app/b.txt");
    let listing = h.ok(
        "list_dir",
        json!({ "sessionId": sid, "path": "/home/user/app/" }),
    );
    assert_eq!(listing["path"], "/home/user/app");
    assert_eq!(listing["entries"][0]["name"], "b.txt");
    let entry = h.ok(
        "chmod_path",
        json!({ "sessionId": sid, "path": "/home/user/app/b.txt", "mode": 0o600 }),
    );
    assert_eq!(entry["mode"], 0o600);
    h.ok(
        "stat_path",
        json!({ "sessionId": sid, "path": "/home/user/app/b.txt" }),
    );

    h.server.add_file(
        "/home/user/app/big.log",
        "line\r\n".repeat(30_000).as_bytes(),
    );
    let opened = h.ok(
        "open_file",
        json!({ "sessionId": sid, "path": "/home/user/app/big.log", "whole": false }),
    );
    // Settings default: 2048 KiB window; the file is smaller, so it is complete.
    assert_eq!(opened["complete"], true);
    assert_eq!(opened["lineEnding"], "crlf");
    let chunk = h.ok(
        "read_more",
        json!({
            "sessionId": sid,
            "path": opened["path"],
            "offset": 0,
            "encoding": opened["encoding"],
            "expectedSize": opened["size"],
            "expectedMtime": opened["mtime"],
            "toEnd": false
        }),
    );
    assert_eq!(chunk["complete"], true);
    let saved = h.ok(
        "save_file",
        json!({
            "sessionId": sid,
            "path": "/home/user/app/big.log",
            "request": {
                "text": "one\ntwo\n",
                "encoding": opened["encoding"],
                "bom": opened["bom"],
                "lineEnding": opened["lineEnding"],
                "expectedSize": opened["size"],
                "expectedMtime": opened["mtime"],
                "force": false,
                "allowInPlace": false
            }
        }),
    );
    assert_eq!(saved["atomic"], true);
    assert_eq!(
        h.server.file("/home/user/app/big.log").unwrap(),
        b"one\r\ntwo\r\n"
    );

    let report = h.ok(
        "delete_paths",
        json!({ "sessionId": sid, "paths": ["/home/user/app", "/home/user"] }),
    );
    assert_eq!(report["deleted"], 3);
    assert_eq!(report["failed"][0]["path"], "/home/user");

    let again = h.ok("reconnect", json!({ "sessionId": sid, "secret": PASSWORD }));
    assert_eq!(again["id"], sid);
    h.ok("disconnect", json!({ "sessionId": sid }));
    let gone = h
        .invoke("list_dir", json!({ "sessionId": sid, "path": "/" }))
        .unwrap_err();
    assert_eq!(gone["code"], "not_found");
}

#[test]
fn transfers_with_camel_case_arguments() {
    let h = harness();
    let downloads = tempfile::tempdir().unwrap();
    let folder = downloads.path().display().to_string();
    h.db.write(|tx| crate::services::settings::set_download_folder(tx, Some(folder)))
        .unwrap();
    let session = h.connect();
    let sid = session.as_str();
    h.server.add_file("/home/user/a.txt", b"remote");

    let queued = h.ok(
        "download_paths",
        json!({ "sessionId": sid, "paths": ["/home/user/a.txt"], "chooseFolder": false }),
    );
    let first = queued[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(queued[0]["direction"], "download");
    h.wait_transfer(sid, &first, "done");
    assert_eq!(
        std::fs::read(downloads.path().join("a.txt")).unwrap(),
        b"remote"
    );

    // Same file again with the default policy (ask): resolve it.
    let queued = h.ok(
        "download_paths",
        json!({ "sessionId": sid, "paths": ["/home/user/a.txt"], "chooseFolder": false }),
    );
    let second = queued[0]["id"].as_str().unwrap().to_owned();
    h.wait_transfer(sid, &second, "conflict");
    h.ok(
        "resolve_conflict",
        json!({ "id": second, "decision": "rename", "applyToAll": false }),
    );
    h.wait_transfer(sid, &second, "done");
    assert!(downloads.path().join("a (1).txt").exists());
    let not_retryable = h
        .invoke("retry_transfer", json!({ "id": second }))
        .unwrap_err();
    assert_eq!(not_retryable["code"], "validation");
    h.ok("cancel_transfer", json!({ "id": second }));

    // An OS drop: the UI only has the drop id.
    let local = tempfile::tempdir().unwrap();
    let file = local.path().join("up.txt");
    std::fs::write(&file, b"up").unwrap();
    let drop_id = h.app.state::<DropRegistry>().store(vec![file]);
    let uploads = h.ok(
        "upload_dropped",
        json!({ "sessionId": sid, "dropId": drop_id, "remoteDir": "/home/user" }),
    );
    let upload = uploads[0]["id"].as_str().unwrap().to_owned();
    h.wait_transfer(sid, &upload, "done");
    assert_eq!(h.server.file("/home/user/up.txt").unwrap(), b"up");
    let reused = h
        .invoke(
            "upload_dropped",
            json!({ "sessionId": sid, "dropId": drop_id, "remoteDir": "/home/user" }),
        )
        .unwrap_err();
    assert_eq!(reused["code"], "not_found");

    h.ok("clear_transfers", json!({ "sessionId": sid }));
    let unknown = h
        .invoke("cancel_transfer", json!({ "id": first }))
        .unwrap_err();
    assert_eq!(unknown["code"], "not_found");
}

#[test]
fn snippets_and_commands_with_camel_case_arguments() {
    let h = harness();
    let site = h.ok(
        "create_site",
        json!({ "input": site_input(), "secret": null }),
    );
    let site_id = site["id"].as_str().unwrap();
    let global = h.ok(
        "create_snippet",
        json!({ "input": { "siteId": null, "name": "uptime", "command": "uptime", "confirm": false } }),
    );
    let local = h.ok(
        "create_snippet",
        json!({ "input": { "siteId": site_id, "name": "logs", "command": "tail {dir}", "confirm": true } }),
    );
    assert_eq!(local["siteId"], site_id);
    assert_eq!(local["confirm"], true);
    let listed = h.ok("list_snippets", json!({ "siteId": site_id }));
    assert_eq!(listed[0]["id"], global["id"]);
    assert_eq!(listed[1]["id"], local["id"]);
    assert_eq!(
        h.ok("list_snippets", json!({ "siteId": null }))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let moved = h.ok(
        "update_snippet",
        json!({ "id": local["id"], "input": { "siteId": null, "name": "logs", "command": "tail {dir}", "confirm": false } }),
    );
    assert!(moved["siteId"].is_null());
    h.ok(
        "reorder_snippets",
        json!({ "ids": [moved["id"], global["id"]] }),
    );
    h.ok("delete_snippet", json!({ "id": global["id"] }));

    h.server.enable_shell();
    h.server.script(
        "tail '/home/user'",
        crate::remote::memory::Script::stdout("x\n", 0),
    );
    let session = h.connect();
    let started = h.ok(
        "run_command",
        json!({
            "sessionId": session,
            "command": "tail {dir}",
            "dir": "/home/user",
            "events": "__CHANNEL__:2"
        }),
    );
    assert_eq!(started["command"], "tail '/home/user'");
    h.ok("cancel_command", json!({ "runId": started["runId"] }));
    h.ok("cancel_command", json!({ "runId": "unknown" }));
    let refused = h
        .invoke(
            "run_command",
            json!({ "sessionId": session, "command": "ls {dir}", "dir": null, "events": "__CHANNEL__:3" }),
        )
        .unwrap_err();
    assert_eq!(refused["code"], "validation");
}
