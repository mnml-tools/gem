//! App settings, stored as one JSON value per key in the `settings` table.
//!
//! Reading merges the stored values over [`Settings::default`] and clamps them
//! ([`Settings::normalised`]); unknown keys are ignored and a malformed value falls back to its
//! default, so older and newer versions can share a database.

use rusqlite::{Connection, Transaction, params};
use serde_json::{Map, Value};

use crate::domain::settings::Settings;
use crate::error::{AppError, AppResult};

fn defaults_map() -> Map<String, Value> {
    match serde_json::to_value(Settings::default()) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

pub fn get(conn: &Connection) -> AppResult<Settings> {
    let defaults = defaults_map();
    let mut merged = defaults.clone();
    let mut stmt = conn.prepare_cached("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (key, value) = row?;
        if !merged.contains_key(&key) {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&value) else {
            continue;
        };
        // Check each value on its own so one bad row does not reset the others.
        let mut probe = defaults.clone();
        probe.insert(key.clone(), value.clone());
        if serde_json::from_value::<Settings>(Value::Object(probe)).is_ok() {
            merged.insert(key, value);
        } else {
            log::warn!("ignoring invalid stored setting {key}");
        }
    }
    let settings: Settings =
        serde_json::from_value(Value::Object(merged)).unwrap_or_else(|_| Settings::default());
    Ok(settings.normalised())
}

/// Saves the settings sent by the UI (clamped). The download folder is a local path, and local
/// paths never come from the webview: it can only be kept or reset here; choosing one goes
/// through [`set_download_folder`] after a native dialog.
pub fn update(tx: &Transaction<'_>, settings: Settings) -> AppResult<Settings> {
    let current = get(tx)?;
    if settings.download_folder.is_some() && settings.download_folder != current.download_folder {
        return Err(AppError::validation(
            "choose the download folder with the folder dialog",
        ));
    }
    write_all(tx, &settings.normalised())?;
    get(tx)
}

/// Stores a folder chosen in a native dialog (or `None` for the OS Downloads folder).
pub fn set_download_folder(tx: &Transaction<'_>, folder: Option<String>) -> AppResult<Settings> {
    let mut settings = get(tx)?;
    settings.download_folder = folder;
    write_all(tx, &settings)?;
    get(tx)
}

fn write_all(tx: &Transaction<'_>, settings: &Settings) -> AppResult<()> {
    let Value::Object(map) =
        serde_json::to_value(settings).map_err(|e| AppError::Internal(format!("settings: {e}")))?
    else {
        return Err(AppError::Internal("settings are not an object".into()));
    };
    for (key, value) in map {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value.to_string()],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::transfer::ConflictPolicy;
    use crate::storage::Database;

    #[test]
    fn defaults_when_empty() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.read(get).unwrap(), Settings::default());
    }

    #[test]
    fn update_clamps_and_persists() {
        let db = Database::open_in_memory().unwrap();
        let saved = db
            .write(|tx| {
                update(
                    tx,
                    Settings {
                        transfer_concurrency: 50,
                        keepalive_secs: 1,
                        preview_window_kib: 1,
                        conflict_policy: ConflictPolicy::Rename,
                        show_hidden: true,
                        check_updates: false,
                        ..Settings::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(saved.transfer_concurrency, Settings::MAX_CONCURRENCY);
        assert_eq!(saved.keepalive_secs, 5);
        assert_eq!(saved.preview_window_kib, 64);
        assert_eq!(db.read(get).unwrap(), saved);
        assert_eq!(saved.conflict_policy, ConflictPolicy::Rename);
        assert!(!saved.check_updates);
    }

    #[test]
    fn download_folder_only_from_the_dialog() {
        let db = Database::open_in_memory().unwrap();
        let attempt = db.write(|tx| {
            update(
                tx,
                Settings {
                    download_folder: Some("/etc".into()),
                    ..Settings::default()
                },
            )
        });
        assert!(attempt.is_err());
        let chosen = db
            .write(|tx| set_download_folder(tx, Some("/home/me/Downloads/gem".into())))
            .unwrap();
        // Sending the same value back (the UI echoes the settings) is fine; so is resetting it.
        db.write(|tx| update(tx, chosen.clone())).unwrap();
        let reset = db
            .write(|tx| {
                update(
                    tx,
                    Settings {
                        download_folder: None,
                        ..chosen
                    },
                )
            })
            .unwrap();
        assert_eq!(reset.download_folder, None);
    }

    #[test]
    fn invalid_stored_values_fall_back_per_key() {
        let db = Database::open_in_memory().unwrap();
        db.write(|tx| {
            tx.execute_batch(
                "INSERT INTO settings VALUES ('transferConcurrency', '\"many\"');
                 INSERT INTO settings VALUES ('showHidden', 'true');
                 INSERT INTO settings VALUES ('futureSetting', '1');
                 INSERT INTO settings VALUES ('keepaliveSecs', 'not json');",
            )?;
            Ok(())
        })
        .unwrap();
        let settings = db.read(get).unwrap();
        assert!(settings.show_hidden);
        assert_eq!(settings.transfer_concurrency, 2);
        assert_eq!(settings.keepalive_secs, 30);
    }
}
