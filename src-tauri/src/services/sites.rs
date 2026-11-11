//! Saved sites: validation, CRUD, duplicate and reorder.
//!
//! Secrets live in the [`SecretStore`]; the `has_secret` column mirrors whether one is stored
//! so the Sites page can show it without touching the keyring. Keyring work happens before the
//! database change it belongs to, so a keyring failure never leaves a row claiming a secret.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use super::secrets::SecretStore;
use crate::domain::site::{
    AuthMethod, FtpsMode, Protocol, SecretUpdate, Site, SiteColor, SiteInput,
};
use crate::error::{AppError, AppResult};
use crate::storage::Database;

/// Gap between consecutive `sort_order` values.
const ORDER_STEP: i64 = 1024;
const MAX_NAME: usize = 200;
const MAX_HOST: usize = 253;
const MAX_USER: usize = 255;

/// Validates and canonicalises what the site dialog, quick connect or an import produced.
pub fn normalise(mut input: SiteInput) -> AppResult<SiteInput> {
    input.name = input.name.trim().to_owned();
    let name_len = input.name.chars().count();
    if name_len == 0 || name_len > MAX_NAME {
        return Err(AppError::validation(format!(
            "the name must have 1 to {MAX_NAME} characters"
        )));
    }
    input.host = input.host.trim().to_owned();
    if input.host.is_empty() {
        return Err(AppError::validation("the host is required"));
    }
    if input.host.contains("://") {
        return Err(AppError::validation(
            "enter the host without a scheme (choose the protocol instead)",
        ));
    }
    if input.host.chars().any(|c| c.is_whitespace() || c == '/') || input.host.len() > MAX_HOST {
        return Err(AppError::validation("the host is not valid"));
    }
    if input.port == 0 {
        return Err(AppError::validation("the port must be between 1 and 65535"));
    }
    input.user = input.user.trim().to_owned();
    if input.user.len() > MAX_USER || input.user.contains(['\r', '\n', '\0']) {
        return Err(AppError::validation("the user name is not valid"));
    }
    input.key_path = input
        .key_path
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty());
    match input.protocol {
        // FTP and FTPS only know passwords (an empty user means anonymous).
        Protocol::Ftp | Protocol::Ftps => {
            input.auth = AuthMethod::Password;
            input.key_path = None;
        }
        Protocol::Sftp => {
            if input.auth == AuthMethod::Key && input.key_path.is_none() {
                return Err(AppError::validation("choose a private key file"));
            }
            if input.auth != AuthMethod::Key {
                input.key_path = None;
            }
        }
    }
    input.initial_path = input
        .initial_path
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty());
    if let Some(path) = &input.initial_path
        && (!path.starts_with('/') || path.contains('\0'))
    {
        return Err(AppError::validation(
            "the initial path must be absolute (start with /)",
        ));
    }
    Ok(input)
}

const COLUMNS: &str = "id, name, protocol, host, port, user, auth, key_path, ftps_mode, passive, \
                       initial_path, color, sort_order, has_secret, created_at, updated_at";

fn from_row(row: &Row<'_>) -> rusqlite::Result<Site> {
    // CHECK constraints guarantee the enum columns; the fallbacks are never used in practice.
    let protocol: String = row.get(2)?;
    let auth: String = row.get(6)?;
    let ftps_mode: String = row.get(8)?;
    let color: Option<String> = row.get(11)?;
    Ok(Site {
        id: row.get(0)?,
        name: row.get(1)?,
        protocol: Protocol::parse(&protocol).unwrap_or(Protocol::Sftp),
        host: row.get(3)?,
        port: row.get(4)?,
        user: row.get(5)?,
        auth: AuthMethod::parse(&auth).unwrap_or(AuthMethod::Password),
        key_path: row.get(7)?,
        ftps_mode: FtpsMode::parse(&ftps_mode).unwrap_or(FtpsMode::Explicit),
        passive: row.get(9)?,
        initial_path: row.get(10)?,
        color: color.as_deref().and_then(SiteColor::parse),
        sort_order: row.get(12)?,
        has_secret: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

pub fn list(conn: &Connection) -> AppResult<Vec<Site>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM sites ORDER BY sort_order, name COLLATE NOCASE"
    ))?;
    let sites = stmt
        .query_map([], from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(sites)
}

pub fn get(conn: &Connection, id: &str) -> AppResult<Site> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM sites WHERE id = ?1"),
        [id],
        from_row,
    )
    .optional()?
    .ok_or_else(|| AppError::not_found("site"))
}

fn next_order(conn: &Connection) -> AppResult<i64> {
    let max: Option<i64> = conn.query_row("SELECT MAX(sort_order) FROM sites", [], |r| r.get(0))?;
    Ok(max.map_or(0, |m| m + ORDER_STEP))
}

fn insert(tx: &Transaction<'_>, id: &str, input: &SiteInput, has_secret: bool) -> AppResult<Site> {
    let now = Utc::now().to_rfc3339();
    let order = next_order(tx)?;
    tx.execute(
        "INSERT INTO sites (id, name, protocol, host, port, user, auth, key_path, ftps_mode,
                            passive, initial_path, color, sort_order, has_secret, created_at,
                            updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
        params![
            id,
            input.name,
            input.protocol.as_str(),
            input.host,
            input.port,
            input.user,
            input.auth.as_str(),
            input.key_path,
            input.ftps_mode.as_str(),
            input.passive,
            input.initial_path,
            input.color.map(SiteColor::as_str),
            order,
            has_secret,
            now
        ],
    )?;
    get(tx, id)
}

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// An empty secret is the same as none.
fn non_empty(secret: Option<String>) -> Option<String> {
    secret.filter(|s| !s.is_empty())
}

pub fn create(
    db: &Database,
    secrets: &dyn SecretStore,
    input: SiteInput,
    secret: Option<String>,
) -> AppResult<Site> {
    let input = normalise(input)?;
    let id = new_id();
    let secret = non_empty(secret);
    if let Some(secret) = &secret {
        secrets
            .set(&id, secret)
            .map_err(|e| super::secrets::remember_error(secrets, e))?;
    }
    let created = db.write(|tx| insert(tx, &id, &input, secret.is_some()));
    if created.is_err() && secret.is_some() {
        let _ = secrets.delete(&id);
    }
    created
}

pub fn update(
    db: &Database,
    secrets: &dyn SecretStore,
    id: &str,
    input: SiteInput,
    secret: SecretUpdate,
) -> AppResult<Site> {
    let input = normalise(input)?;
    let current = db.read(|c| get(c, id))?;
    let has_secret = match secret {
        SecretUpdate::Keep => current.has_secret,
        SecretUpdate::Clear => {
            secrets.delete(id)?;
            false
        }
        SecretUpdate::Set { value } if value.is_empty() => {
            secrets.delete(id)?;
            false
        }
        SecretUpdate::Set { value } => {
            secrets
                .set(id, &value)
                .map_err(|e| super::secrets::remember_error(secrets, e))?;
            true
        }
    };
    db.write(|tx| {
        tx.execute(
            "UPDATE sites SET name = ?2, protocol = ?3, host = ?4, port = ?5, user = ?6,
                              auth = ?7, key_path = ?8, ftps_mode = ?9, passive = ?10,
                              initial_path = ?11, color = ?12, has_secret = ?13, updated_at = ?14
             WHERE id = ?1",
            params![
                id,
                input.name,
                input.protocol.as_str(),
                input.host,
                input.port,
                input.user,
                input.auth.as_str(),
                input.key_path,
                input.ftps_mode.as_str(),
                input.passive,
                input.initial_path,
                input.color.map(SiteColor::as_str),
                has_secret,
                Utc::now().to_rfc3339()
            ],
        )?;
        get(tx, id)
    })
}

/// Deletes a site and its secret. A keyring failure keeps the site when a secret is known to
/// be stored (deleting the row would orphan it in the keyring).
pub fn delete(db: &Database, secrets: &dyn SecretStore, id: &str) -> AppResult<()> {
    let site = db.read(|c| get(c, id))?;
    if let Err(error) = secrets.delete(id) {
        if site.has_secret {
            return Err(error);
        }
        log::warn!("could not clear the keyring entry of a deleted site: {error}");
    }
    db.write(|tx| {
        tx.execute("DELETE FROM sites WHERE id = ?1", [id])?;
        Ok(())
    })
}

/// Copies a site (and its stored secret, best effort) under the name "<name> copy".
pub fn duplicate(db: &Database, secrets: &dyn SecretStore, id: &str) -> AppResult<Site> {
    let original = db.read(|c| get(c, id))?;
    let mut input = original.input();
    let name: String = format!("{} copy", original.name)
        .chars()
        .take(MAX_NAME)
        .collect();
    input.name = name;
    let new_id = new_id();
    let mut has_secret = false;
    if original.has_secret {
        match secrets.get(id).and_then(|secret| match secret {
            Some(secret) => secrets.set(&new_id, &secret).map(|()| true),
            None => Ok(false),
        }) {
            Ok(copied) => has_secret = copied,
            Err(error) => log::warn!("could not copy the secret of a duplicated site: {error}"),
        }
    }
    db.write(|tx| insert(tx, &new_id, &input, has_secret))
}

/// Puts the given sites first, in this order; the others keep their relative order after them.
pub fn reorder(tx: &Transaction<'_>, ids: &[String]) -> AppResult<()> {
    let existing = list(tx)?;
    for id in ids {
        if !existing.iter().any(|s| &s.id == id) {
            return Err(AppError::not_found("site"));
        }
    }
    let rest = existing
        .iter()
        .filter(|s| !ids.contains(&s.id))
        .map(|s| &s.id);
    for (index, id) in ids.iter().chain(rest).enumerate() {
        let order = i64::try_from(index).unwrap_or(i64::MAX / ORDER_STEP) * ORDER_STEP;
        tx.execute(
            "UPDATE sites SET sort_order = ?2 WHERE id = ?1",
            params![id, order],
        )?;
    }
    Ok(())
}

/// Records that a secret was stored (or removed) for a site, e.g. after "Remember" on connect.
pub fn set_has_secret(tx: &Transaction<'_>, id: &str, has_secret: bool) -> AppResult<()> {
    tx.execute(
        "UPDATE sites SET has_secret = ?2 WHERE id = ?1",
        params![id, has_secret],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::services::secrets::MemoryStore;
    use crate::services::test_support::sftp_input;

    #[test]
    fn validation_rules() {
        let ok = normalise(SiteInput {
            name: "  prod  ".into(),
            initial_path: Some("  ".into()),
            ..sftp_input()
        })
        .unwrap();
        assert_eq!(ok.name, "prod");
        assert_eq!(ok.initial_path, None);
        let bad = |input: SiteInput| normalise(input).unwrap_err().code();
        assert_eq!(
            bad(SiteInput {
                name: " ".into(),
                ..sftp_input()
            }),
            ErrorCode::Validation
        );
        assert_eq!(
            bad(SiteInput {
                name: "x".repeat(201),
                ..sftp_input()
            }),
            ErrorCode::Validation
        );
        for host in ["", "sftp://example.test", "exa mple.test", "a/b"] {
            assert_eq!(
                bad(SiteInput {
                    host: host.into(),
                    ..sftp_input()
                }),
                ErrorCode::Validation,
                "{host}"
            );
        }
        assert_eq!(
            bad(SiteInput {
                port: 0,
                ..sftp_input()
            }),
            ErrorCode::Validation
        );
        assert_eq!(
            bad(SiteInput {
                auth: AuthMethod::Key,
                key_path: None,
                ..sftp_input()
            }),
            ErrorCode::Validation
        );
        assert_eq!(
            bad(SiteInput {
                initial_path: Some("var/www".into()),
                ..sftp_input()
            }),
            ErrorCode::Validation
        );
    }

    #[test]
    fn ftp_forces_password_auth() {
        let input = normalise(SiteInput {
            protocol: Protocol::Ftps,
            auth: AuthMethod::Key,
            key_path: Some("/k".into()),
            ..sftp_input()
        })
        .unwrap();
        assert_eq!(input.auth, AuthMethod::Password);
        assert_eq!(input.key_path, None);
    }

    #[test]
    fn crud_keeps_has_secret_in_sync() {
        let db = Database::open_in_memory().unwrap();
        let store = MemoryStore::default();
        let site = create(&db, &store, sftp_input(), Some("pw".into())).unwrap();
        assert!(site.has_secret);
        assert_eq!(store.get(&site.id).unwrap().as_deref(), Some("pw"));

        let kept = update(&db, &store, &site.id, sftp_input(), SecretUpdate::Keep).unwrap();
        assert!(kept.has_secret);
        let cleared = update(&db, &store, &site.id, sftp_input(), SecretUpdate::Clear).unwrap();
        assert!(!cleared.has_secret);
        assert_eq!(store.get(&site.id).unwrap(), None);
        let set = update(
            &db,
            &store,
            &site.id,
            SiteInput {
                name: "renamed".into(),
                ..sftp_input()
            },
            SecretUpdate::Set {
                value: "new".into(),
            },
        )
        .unwrap();
        assert!(set.has_secret);
        assert_eq!(set.name, "renamed");

        delete(&db, &store, &site.id).unwrap();
        assert_eq!(store.get(&site.id).unwrap(), None);
        assert_eq!(
            db.read(|c| get(c, &site.id)).unwrap_err().code(),
            ErrorCode::NotFound
        );
    }

    #[test]
    fn keyring_failures_change_nothing() {
        let db = Database::open_in_memory().unwrap();
        let store = MemoryStore::default();
        store.set_broken(true);
        let refused = create(&db, &store, sftp_input(), Some("pw".into())).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::Platform);
        assert!(
            refused.to_string().contains("Untick \"Remember\""),
            "{refused}"
        );
        assert!(!refused.to_string().contains("pw"));
        assert!(db.read(list).unwrap().is_empty());
        // Without a secret the keyring is not needed.
        let site = create(&db, &store, sftp_input(), None).unwrap();
        assert!(!site.has_secret);
        let set = update(
            &db,
            &store,
            &site.id,
            sftp_input(),
            SecretUpdate::Set { value: "x".into() },
        );
        assert_eq!(set.unwrap_err().code(), ErrorCode::Platform);
        assert!(!db.read(|c| get(c, &site.id)).unwrap().has_secret);
    }

    #[test]
    fn duplicate_copies_the_secret_and_appends() {
        let db = Database::open_in_memory().unwrap();
        let store = MemoryStore::default();
        let site = create(&db, &store, sftp_input(), Some("pw".into())).unwrap();
        let copy = duplicate(&db, &store, &site.id).unwrap();
        assert_ne!(copy.id, site.id);
        assert_eq!(copy.name, "prod copy");
        assert!(copy.has_secret);
        assert_eq!(store.get(&copy.id).unwrap().as_deref(), Some("pw"));
        assert!(copy.sort_order > site.sort_order);
    }

    #[test]
    fn reorder_puts_given_ids_first() {
        let db = Database::open_in_memory().unwrap();
        let store = MemoryStore::default();
        let names = ["a", "b", "c"];
        let ids: Vec<String> = names
            .iter()
            .map(|n| {
                create(
                    &db,
                    &store,
                    SiteInput {
                        name: (*n).into(),
                        ..sftp_input()
                    },
                    None,
                )
                .unwrap()
                .id
            })
            .collect();
        db.write(|tx| reorder(tx, &[ids[2].clone()])).unwrap();
        let order: Vec<String> = db.read(list).unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(order, ["c", "a", "b"]);
        assert_eq!(
            db.write(|tx| reorder(tx, &["missing".into()]))
                .unwrap_err()
                .code(),
            ErrorCode::NotFound
        );
    }
}
