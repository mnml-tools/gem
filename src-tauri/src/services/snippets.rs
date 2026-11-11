//! Saved remote commands (ADR 0014): global (`site_id` NULL) or per site.
//!
//! A site's snippets go with it (`ON DELETE CASCADE`). Lists show the globals first, then the
//! site's, each in its saved order. Moving a snippet to another scope (update with another
//! `site_id`) puts it at the end of that scope.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::domain::command::{Snippet, SnippetInput};
use crate::error::{AppError, AppResult};

const ORDER_STEP: i64 = 1024;
const MAX_NAME: usize = 100;
const MAX_COMMAND: usize = 4000;

const COLUMNS: &str = "id, site_id, name, command, confirm, sort_order, created_at, updated_at";

fn from_row(row: &Row<'_>) -> rusqlite::Result<Snippet> {
    Ok(Snippet {
        id: row.get(0)?,
        site_id: row.get(1)?,
        name: row.get(2)?,
        command: row.get(3)?,
        confirm: row.get(4)?,
        sort_order: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

/// Validates a snippet; the name is trimmed, the command kept as typed.
fn normalise(conn: &Connection, mut input: SnippetInput) -> AppResult<SnippetInput> {
    input.name = input.name.trim().to_owned();
    let name_len = input.name.chars().count();
    if name_len == 0 || name_len > MAX_NAME {
        return Err(AppError::validation(format!(
            "the name must have 1 to {MAX_NAME} characters"
        )));
    }
    if input.command.trim().is_empty() || input.command.chars().count() > MAX_COMMAND {
        return Err(AppError::validation(format!(
            "the command must have 1 to {MAX_COMMAND} characters"
        )));
    }
    if input.name.contains('\0') || input.command.contains('\0') {
        return Err(AppError::validation(
            "snippets cannot contain NUL characters",
        ));
    }
    if let Some(site_id) = &input.site_id {
        let exists: Option<String> = conn
            .query_row("SELECT id FROM sites WHERE id = ?1", [site_id], |r| {
                r.get(0)
            })
            .optional()?;
        if exists.is_none() {
            return Err(AppError::not_found("site"));
        }
    }
    Ok(input)
}

/// Global snippets plus those of `site_id` (only globals when `None`).
pub fn list(conn: &Connection, site_id: Option<&str>) -> AppResult<Vec<Snippet>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM snippets
         WHERE site_id IS NULL OR site_id = ?1
         ORDER BY site_id IS NOT NULL, sort_order, name COLLATE NOCASE"
    ))?;
    let snippets = stmt
        .query_map([site_id], from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(snippets)
}

pub fn get(conn: &Connection, id: &str) -> AppResult<Snippet> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM snippets WHERE id = ?1"),
        [id],
        from_row,
    )
    .optional()?
    .ok_or_else(|| AppError::not_found("snippet"))
}

fn next_order(conn: &Connection, site_id: Option<&str>) -> AppResult<i64> {
    let max: Option<i64> = conn.query_row(
        "SELECT MAX(sort_order) FROM snippets WHERE site_id IS ?1",
        [site_id],
        |r| r.get(0),
    )?;
    Ok(max.map_or(0, |m| m + ORDER_STEP))
}

pub fn create(tx: &Transaction<'_>, input: SnippetInput) -> AppResult<Snippet> {
    let input = normalise(tx, input)?;
    let id = uuid::Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO snippets (id, site_id, name, command, confirm, sort_order, created_at,
                               updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![
            id,
            input.site_id,
            input.name,
            input.command,
            input.confirm,
            next_order(tx, input.site_id.as_deref())?,
            now
        ],
    )?;
    get(tx, &id)
}

pub fn update(tx: &Transaction<'_>, id: &str, input: SnippetInput) -> AppResult<Snippet> {
    let current = get(tx, id)?;
    let input = normalise(tx, input)?;
    let order = if current.site_id == input.site_id {
        current.sort_order
    } else {
        next_order(tx, input.site_id.as_deref())?
    };
    tx.execute(
        "UPDATE snippets SET site_id = ?2, name = ?3, command = ?4, confirm = ?5,
                             sort_order = ?6, updated_at = ?7
         WHERE id = ?1",
        params![
            id,
            input.site_id,
            input.name,
            input.command,
            input.confirm,
            order,
            Utc::now().to_rfc3339()
        ],
    )?;
    get(tx, id)
}

pub fn delete(tx: &Transaction<'_>, id: &str) -> AppResult<()> {
    if tx.execute("DELETE FROM snippets WHERE id = ?1", [id])? == 0 {
        return Err(AppError::not_found("snippet"));
    }
    Ok(())
}

/// Puts the given snippets first in their scope, in this order; all must share one scope.
pub fn reorder(tx: &Transaction<'_>, ids: &[String]) -> AppResult<()> {
    let Some(first) = ids.first() else {
        return Ok(());
    };
    let scope = get(tx, first)?.site_id;
    for id in ids {
        if get(tx, id)?.site_id != scope {
            return Err(AppError::validation(
                "snippets can only be reordered within one scope",
            ));
        }
    }
    let mut stmt = tx.prepare(
        "SELECT id FROM snippets WHERE site_id IS ?1 ORDER BY sort_order, name COLLATE NOCASE",
    )?;
    let existing: Vec<String> = stmt
        .query_map([scope.as_deref()], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let rest = existing.iter().filter(|id| !ids.contains(id));
    for (index, id) in ids.iter().chain(rest).enumerate() {
        let order = i64::try_from(index).unwrap_or(i64::MAX / ORDER_STEP) * ORDER_STEP;
        tx.execute(
            "UPDATE snippets SET sort_order = ?2 WHERE id = ?1",
            params![id, order],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::services::secrets::MemoryStore;
    use crate::services::sites;
    use crate::services::test_support::sftp_input;
    use crate::storage::Database;

    fn input(site_id: Option<&str>, name: &str, command: &str) -> SnippetInput {
        SnippetInput {
            site_id: site_id.map(str::to_owned),
            name: name.into(),
            command: command.into(),
            confirm: false,
        }
    }

    fn setup() -> (Database, String) {
        let db = Database::open_in_memory().unwrap();
        let site = sites::create(&db, &MemoryStore::default(), sftp_input(), None).unwrap();
        (db, site.id)
    }

    fn names(db: &Database, site_id: Option<&str>) -> Vec<String> {
        db.read(|c| list(c, site_id))
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect()
    }

    #[test]
    fn crud_and_listing_order() {
        let (db, site) = setup();
        let site = Some(site.as_str());
        let mut created = Vec::new();
        for (scope, name) in [
            (site, "logs"),
            (None, "uptime"),
            (site, "cron"),
            (None, "df"),
        ] {
            created.push(
                db.write(|tx| create(tx, input(scope, name, "true")))
                    .unwrap(),
            );
        }
        // Globals first, then the site's, each in creation (saved) order.
        assert_eq!(names(&db, site), ["uptime", "df", "logs", "cron"]);
        assert_eq!(names(&db, None), ["uptime", "df"]);

        let edited = db
            .write(|tx| {
                update(
                    tx,
                    &created[1].id,
                    SnippetInput {
                        confirm: true,
                        ..input(None, "  load  ", "uptime; cat /proc/loadavg")
                    },
                )
            })
            .unwrap();
        assert_eq!(edited.name, "load");
        assert!(edited.confirm);
        assert_eq!(edited.sort_order, created[1].sort_order);

        // Moving to the site's scope puts it last there.
        db.write(|tx| update(tx, &created[1].id, input(site, "load", "uptime")))
            .unwrap();
        assert_eq!(names(&db, site), ["df", "logs", "cron", "load"]);

        db.write(|tx| delete(tx, &created[3].id)).unwrap();
        assert_eq!(
            db.write(|tx| delete(tx, &created[3].id))
                .unwrap_err()
                .code(),
            ErrorCode::NotFound
        );
        assert_eq!(names(&db, None), Vec::<String>::new());
    }

    #[test]
    fn validation() {
        let (db, site) = setup();
        let bad = |i: SnippetInput| db.write(|tx| create(tx, i)).unwrap_err().code();
        assert_eq!(bad(input(None, " ", "ls")), ErrorCode::Validation);
        assert_eq!(
            bad(input(None, &"n".repeat(101), "ls")),
            ErrorCode::Validation
        );
        assert_eq!(bad(input(None, "x", "  ")), ErrorCode::Validation);
        assert_eq!(
            bad(input(None, "x", &"a".repeat(4001))),
            ErrorCode::Validation
        );
        assert_eq!(bad(input(None, "x", "ls\0rm")), ErrorCode::Validation);
        assert_eq!(bad(input(Some("missing"), "x", "ls")), ErrorCode::NotFound);
        let ok = db
            .write(|tx| create(tx, input(Some(&site), &"é".repeat(100), &"a".repeat(4000))))
            .unwrap();
        assert_eq!(ok.command.len(), 4000);
    }

    #[test]
    fn reorder_within_one_scope() {
        let (db, site) = setup();
        let ids: Vec<String> = ["a", "b", "c"]
            .iter()
            .map(|n| {
                db.write(|tx| create(tx, input(None, n, "true")))
                    .unwrap()
                    .id
            })
            .collect();
        let local = db
            .write(|tx| create(tx, input(Some(&site), "local", "true")))
            .unwrap();
        db.write(|tx| reorder(tx, &[ids[2].clone(), ids[0].clone()]))
            .unwrap();
        assert_eq!(names(&db, Some(&site)), ["c", "a", "b", "local"]);
        assert_eq!(
            db.write(|tx| reorder(tx, &[ids[0].clone(), local.id.clone()]))
                .unwrap_err()
                .code(),
            ErrorCode::Validation
        );
        db.write(|tx| reorder(tx, &[])).unwrap();
    }

    #[test]
    fn deleting_a_site_deletes_its_snippets() {
        let (db, site) = setup();
        let store = MemoryStore::default();
        db.write(|tx| create(tx, input(Some(&site), "logs", "true")))
            .unwrap();
        db.write(|tx| create(tx, input(None, "global", "true")))
            .unwrap();
        sites::delete(&db, &store, &site).unwrap();
        let left: i64 = db
            .read(|c| Ok(c.query_row("SELECT count(*) FROM snippets", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(left, 1, "only the global snippet remains");
        assert_eq!(names(&db, Some(&site)), ["global"]);
    }
}
