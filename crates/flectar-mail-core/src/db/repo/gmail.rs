use crate::error::Result;
use crate::models::now_ms;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct SyncState {
    pub sync_cursor: Option<String>,
    pub history_id: Option<String>,
    pub history_page_token: Option<String>,
    pub generation: i64,
    pub full_started_at: Option<i64>,
    pub backfill_done: bool,
}

pub fn state(conn: &Connection, account_id: i64) -> Result<SyncState> {
    Ok(conn
        .query_row(
            "SELECT sync_cursor, history_id, history_page_token, generation,
                    full_started_at, backfill_done
             FROM gmail_sync_state WHERE account_id = ?1",
            params![account_id],
            |row| {
                Ok(SyncState {
                    sync_cursor: row.get(0)?,
                    history_id: row.get(1)?,
                    history_page_token: row.get(2)?,
                    generation: row.get(3)?,
                    full_started_at: row.get(4)?,
                    backfill_done: row.get::<_, i64>(5)? != 0,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

/// Start a new authoritative scan. The profile watermark is captured before
/// message listing so history reconciliation closes the race with live mail.
pub fn begin_full_sync(conn: &Connection, account_id: i64, history_id: &str) -> Result<i64> {
    let previous = state(conn, account_id)?;
    let generation = previous.generation.saturating_add(1).max(1);
    conn.execute(
        "INSERT INTO gmail_sync_state (
             account_id, sync_cursor, history_id, history_page_token,
             generation, full_started_at, backfill_done, updated_at
         ) VALUES (?1, NULL, ?2, NULL, ?3, ?4, 0, ?4)
         ON CONFLICT(account_id) DO UPDATE SET
             sync_cursor = NULL,
             history_id = excluded.history_id,
             history_page_token = NULL,
             generation = excluded.generation,
             full_started_at = excluded.full_started_at,
             backfill_done = 0,
             updated_at = excluded.updated_at",
        params![account_id, history_id, generation, now_ms()],
    )?;
    Ok(generation)
}

pub fn checkpoint_full_page(
    conn: &Connection,
    account_id: i64,
    next_cursor: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE gmail_sync_state
         SET sync_cursor = ?2, backfill_done = (?2 IS NULL), updated_at = ?3
         WHERE account_id = ?1",
        params![account_id, next_cursor, now_ms()],
    )?;
    Ok(())
}

pub fn checkpoint_history_page(
    conn: &Connection,
    account_id: i64,
    next_page_token: Option<&str>,
    completed_history_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE gmail_sync_state SET
             history_page_token = ?2,
             history_id = COALESCE(?3, history_id),
             updated_at = ?4
         WHERE account_id = ?1",
        params![account_id, next_page_token, completed_history_id, now_ms()],
    )?;
    Ok(())
}

/// The History API returns 404 when its start id has aged out. Keep local data
/// available, but restart an authoritative generation so stale rows can be
/// removed only after every replacement page has committed.
pub fn expire_history(conn: &Connection, account_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO gmail_sync_state (account_id, updated_at)
         VALUES (?1, ?2)
         ON CONFLICT(account_id) DO UPDATE SET
             sync_cursor = NULL, history_id = NULL, history_page_token = NULL,
             generation = 0, backfill_done = 0, full_started_at = NULL,
             updated_at = excluded.updated_at",
        params![account_id, now_ms()],
    )?;
    Ok(())
}

pub fn set_message_folders(conn: &Connection, message_id: i64, folder_ids: &[i64]) -> Result<()> {
    conn.execute(
        "DELETE FROM message_folders WHERE message_id = ?1",
        params![message_id],
    )?;
    for folder_id in folder_ids {
        conn.execute(
            "INSERT OR IGNORE INTO message_folders (message_id, folder_id) VALUES (?1, ?2)",
            params![message_id, folder_id],
        )?;
    }
    Ok(())
}

/// Apply a Gmail History `labelsAdded` / `labelsRemoved` delta without
/// re-fetching the thread. Returns the local thread id when a message was
/// updated, or `None` when the message is not cached yet (caller should fetch).
pub fn apply_label_delta(
    conn: &Connection,
    account_id: i64,
    gm_msgid: &str,
    add_labels: &[String],
    remove_labels: &[String],
) -> Result<Option<i64>> {
    use crate::models::roles;
    use std::collections::{HashMap, HashSet};

    let Some(message) = crate::db::repo::messages::by_gm_msgid(conn, account_id, gm_msgid)? else {
        return Ok(None);
    };
    let Some(thread_id) = message.thread_id else {
        return Ok(None);
    };

    let mut mappings = HashMap::<String, (Option<i64>, Option<i64>, Option<String>)>::new();
    {
        let mut stmt = conn.prepare(
            "SELECT gl.provider_id, gl.folder_id, gl.local_label_id, f.role
             FROM gmail_labels gl
             LEFT JOIN folders f ON f.id = gl.folder_id
             WHERE gl.account_id = ?1",
        )?;
        let rows = stmt.query_map(params![account_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ),
            ))
        })?;
        for row in rows {
            let (id, mapping) = row?;
            mappings.insert(id, mapping);
        }
    }

    let mut label_set = HashSet::<String>::new();
    {
        let mut stmt = conn.prepare(
            "SELECT gl.provider_id
             FROM message_folders mf
             JOIN gmail_labels gl
               ON gl.folder_id = mf.folder_id AND gl.account_id = ?1
             JOIN folders f ON f.id = mf.folder_id
             WHERE mf.message_id = ?2
               AND COALESCE(f.role, '') NOT IN ('archive', 'all')",
        )?;
        for row in stmt.query_map(params![account_id, message.id], |row| {
            row.get::<_, String>(0)
        })? {
            label_set.insert(row?);
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT gl.provider_id
             FROM message_labels ml
             JOIN gmail_labels gl
               ON gl.local_label_id = ml.label_id AND gl.account_id = ?1
             WHERE ml.message_id = ?2",
        )?;
        for row in stmt.query_map(params![account_id, message.id], |row| {
            row.get::<_, String>(0)
        })? {
            label_set.insert(row?);
        }
    }
    if !message.is_read {
        label_set.insert("UNREAD".into());
    }
    if message.is_starred {
        label_set.insert("STARRED".into());
    }
    let is_draft: bool = conn.query_row(
        "SELECT is_draft FROM messages WHERE id = ?1",
        params![message.id],
        |row| Ok(row.get::<_, i64>(0)? != 0),
    )?;
    if is_draft {
        label_set.insert("DRAFT".into());
    }

    for label in remove_labels {
        label_set.remove(label);
    }
    for label in add_labels {
        label_set.insert(label.clone());
    }

    let archive_id = crate::db::repo::folders::by_role(conn, account_id, roles::ARCHIVE)?
        .map(|folder| folder.id)
        .ok_or_else(|| crate::error::CoreError::NotFound("Gmail archive folder".into()))?;
    let all_id = crate::db::repo::folders::by_role(conn, account_id, roles::ALL)?
        .map(|folder| folder.id)
        .ok_or_else(|| crate::error::CoreError::NotFound("Gmail all-mail folder".into()))?;

    let mut folder_ids = Vec::<i64>::new();
    let mut role_folders = HashMap::<String, i64>::new();
    let mut present_local_labels = Vec::<i64>::new();
    for label in &label_set {
        if let Some((folder_id, local_label_id, role)) = mappings.get(label) {
            if let Some(folder_id) = *folder_id {
                folder_ids.push(folder_id);
                if let Some(role) = role {
                    role_folders.insert(role.clone(), folder_id);
                }
            }
            if let Some(local_id) = *local_label_id {
                present_local_labels.push(local_id);
            }
        }
    }
    let archived = !label_set.contains("INBOX")
        && !label_set.contains("SENT")
        && !label_set.contains("DRAFT")
        && !label_set.contains("TRASH")
        && !label_set.contains("SPAM");
    if archived {
        folder_ids.push(archive_id);
        role_folders.insert(roles::ARCHIVE.into(), archive_id);
    }
    folder_ids.push(all_id);
    role_folders.insert(roles::ALL.into(), all_id);
    folder_ids.sort_unstable();
    folder_ids.dedup();

    let canonical_folder = [
        roles::TRASH,
        roles::SPAM,
        roles::DRAFTS,
        roles::INBOX,
        roles::SENT,
        roles::ARCHIVE,
        roles::ALL,
    ]
    .into_iter()
    .find_map(|role| role_folders.get(role).copied())
    .unwrap_or(all_id);

    let is_read = !label_set.contains("UNREAD");
    let is_starred = label_set.contains("STARRED");
    let is_draft = label_set.contains("DRAFT");
    conn.execute(
        "UPDATE messages SET folder_id = ?2, is_read = ?3, is_starred = ?4, is_draft = ?5
         WHERE id = ?1",
        params![
            message.id,
            canonical_folder,
            is_read as i64,
            is_starred as i64,
            is_draft as i64
        ],
    )?;
    set_message_folders(conn, message.id, &folder_ids)?;

    let provider_local_labels = mappings
        .values()
        .filter_map(|(_, local_label_id, _)| *local_label_id)
        .collect::<HashSet<_>>();
    for local_id in &provider_local_labels {
        if !present_local_labels.contains(local_id) {
            crate::db::repo::labels::remove_from_message(conn, message.id, *local_id)?;
        }
    }
    for local_id in present_local_labels {
        crate::db::repo::labels::add_to_message(conn, message.id, local_id)?;
    }
    crate::db::repo::threads::recompute(conn, thread_id)?;
    Ok(Some(thread_id))
}

/// Drop stale local INBOX membership for messages whose Gmail ids are not in
/// `inbox_gm_msgids`. Messages are never deleted; they move to Archive/All Mail.
pub fn reconcile_inbox_membership(
    conn: &Connection,
    account_id: i64,
    inbox_gm_msgids: &HashSet<String>,
) -> Result<Vec<i64>> {
    use crate::models::roles;

    let Some(inbox) = crate::db::repo::folders::by_role(conn, account_id, roles::INBOX)? else {
        return Ok(Vec::new());
    };
    let rows = {
        let mut stmt = conn.prepare(
            "SELECT m.id, m.gm_msgid
             FROM message_folders mf
             JOIN messages m ON m.id = mf.message_id
             WHERE mf.folder_id = ?1 AND m.account_id = ?2 AND m.gm_msgid IS NOT NULL",
        )?;
        stmt.query_map(params![inbox.id, account_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut touched = Vec::new();
    for chunk in rows.chunks(500) {
        for (message_id, gm_msgid) in chunk {
            if inbox_gm_msgids.contains(gm_msgid) {
                continue;
            }
            if let Some(thread_id) =
                apply_label_delta(conn, account_id, gm_msgid, &[], &["INBOX".into()])?
            {
                let _ = message_id;
                if !touched.contains(&thread_id) {
                    touched.push(thread_id);
                }
            }
        }
    }
    Ok(touched)
}

pub fn provider_label_for_local(
    conn: &Connection,
    account_id: i64,
    local_label_id: i64,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT provider_id FROM gmail_labels
             WHERE account_id = ?1 AND local_label_id = ?2 LIMIT 1",
            params![account_id, local_label_id],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn provider_label_for_folder(
    conn: &Connection,
    account_id: i64,
    folder_id: i64,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT provider_id FROM gmail_labels
             WHERE account_id = ?1 AND folder_id = ?2 AND kind = 'user' LIMIT 1",
            params![account_id, folder_id],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn set_draft_ids(
    conn: &Connection,
    account_id: i64,
    gmail_message_id: &str,
    gmail_draft_id: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE messages SET gmail_draft_id = ?3
         WHERE account_id = ?1 AND gm_msgid = ?2",
        params![account_id, gmail_message_id, gmail_draft_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil;

    #[test]
    fn full_and_history_checkpoints_are_durable() {
        let conn = testutil::conn();
        conn.execute(
            "INSERT INTO accounts (id, email, provider, auth_kind, username,
             incoming_host, incoming_port, smtp_host, smtp_port, created_at, mail_protocol, settings_json)
             VALUES (1, 'me@gmail.com', 'gmail', 'oauth2', 'me@gmail.com',
                     'unused', 993, 'unused', 465, 0, 'gmail', '{\"connection\":{\"incomingSecurity\":\"tls\",\"smtpSecurity\":\"tls\"}}')",
            [],
        )
        .unwrap();
        let generation = begin_full_sync(&conn, 1, "100").unwrap();
        assert_eq!(generation, 1);
        checkpoint_full_page(&conn, 1, Some("page-2")).unwrap();
        let current = state(&conn, 1).unwrap();
        assert_eq!(current.sync_cursor.as_deref(), Some("page-2"));
        assert!(!current.backfill_done);

        checkpoint_full_page(&conn, 1, None).unwrap();
        checkpoint_history_page(&conn, 1, Some("history-2"), None).unwrap();
        let current = state(&conn, 1).unwrap();
        assert!(current.backfill_done);
        assert_eq!(current.history_id.as_deref(), Some("100"));
        assert_eq!(current.history_page_token.as_deref(), Some("history-2"));

        checkpoint_history_page(&conn, 1, None, Some("200")).unwrap();
        let current = state(&conn, 1).unwrap();
        assert_eq!(current.history_id.as_deref(), Some("200"));
        assert!(current.history_page_token.is_none());

        expire_history(&conn, 1).unwrap();
        // A History API request that was already in flight can still finish
        // after the reset. Its stale checkpoint must not prevent the next
        // cycle from recognizing that a new full generation is required.
        checkpoint_history_page(&conn, 1, None, Some("stale-history")).unwrap();
        let expired = state(&conn, 1).unwrap();
        assert_eq!(expired.generation, 0);
        assert!(!expired.backfill_done);
    }

    #[test]
    fn message_folder_memberships_replace_atomically() {
        let conn = testutil::conn();
        testutil::seed_account(&conn);
        let (_, message_id) = testutil::seed_message(&conn, "a@test.dev", "one", false);
        conn.execute(
            "INSERT INTO folders (id, account_id, path, role)
             VALUES (2, 1, 'Archive', 'archive')",
            [],
        )
        .unwrap();
        set_message_folders(&conn, message_id, &[1, 2]).unwrap();
        set_message_folders(&conn, message_id, &[2]).unwrap();
        let memberships: Vec<i64> = conn
            .prepare("SELECT folder_id FROM message_folders WHERE message_id = ?1")
            .unwrap()
            .query_map(params![message_id], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(memberships, vec![2]);
    }

    fn seed_gmail_system_labels(conn: &Connection) {
        testutil::seed_account(conn);
        conn.execute(
            "INSERT INTO folders (id, account_id, path, role) VALUES
             (2, 1, 'Archive', 'archive'),
             (3, 1, 'All Mail', 'all')",
            [],
        )
        .unwrap();
        for (provider_id, folder_id, kind) in [
            ("INBOX", Some(1i64), "system"),
            ("CATEGORY_PERSONAL", None, "system"),
        ] {
            conn.execute(
                "INSERT INTO gmail_labels (account_id, provider_id, name, kind, folder_id)
                 VALUES (1, ?1, ?1, ?2, ?3)",
                params![provider_id, kind, folder_id],
            )
            .unwrap();
        }
        // Archive and All Mail are local-only folders; they have no provider id.
    }

    #[test]
    fn label_delta_moves_inbox_to_archive_without_deleting() {
        let conn = testutil::conn();
        seed_gmail_system_labels(&conn);
        let (thread_id, message_id) = testutil::seed_message(&conn, "a@test.dev", "one", false);
        conn.execute(
            "UPDATE messages SET gm_msgid = 'gm-1' WHERE id = ?1",
            params![message_id],
        )
        .unwrap();
        set_message_folders(&conn, message_id, &[1, 3]).unwrap();

        let touched = apply_label_delta(&conn, 1, "gm-1", &[], &["INBOX".into()]).unwrap();
        assert_eq!(touched, Some(thread_id));

        let memberships: Vec<i64> = conn
            .prepare(
                "SELECT folder_id FROM message_folders WHERE message_id = ?1 ORDER BY folder_id",
            )
            .unwrap()
            .query_map(params![message_id], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(memberships, vec![2, 3]); // Archive + All Mail
        let folder_id: i64 = conn
            .query_row(
                "SELECT folder_id FROM messages WHERE id = ?1",
                params![message_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(folder_id, 2);
        let still_there: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id = ?1",
                params![message_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(still_there, 1);
        assert!(
            apply_label_delta(&conn, 1, "missing", &[], &["INBOX".into()])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn reconcile_drops_stale_inbox_membership_without_deleting_rows() {
        let conn = testutil::conn();
        seed_gmail_system_labels(&conn);
        let (_, keep_id) = testutil::seed_message(&conn, "a@test.dev", "keep", false);
        let (_, stale_id) = testutil::seed_message(&conn, "b@test.dev", "stale", false);
        conn.execute(
            "UPDATE messages SET gm_msgid = 'keep' WHERE id = ?1",
            params![keep_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE messages SET gm_msgid = 'stale' WHERE id = ?1",
            params![stale_id],
        )
        .unwrap();
        set_message_folders(&conn, keep_id, &[1, 3]).unwrap();
        set_message_folders(&conn, stale_id, &[1, 3]).unwrap();

        let mut server_inbox = HashSet::new();
        server_inbox.insert("keep".into());
        let touched = reconcile_inbox_membership(&conn, 1, &server_inbox).unwrap();
        assert!(!touched.is_empty());

        let keep_inbox: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_folders WHERE message_id = ?1 AND folder_id = 1",
                params![keep_id],
                |row| row.get(0),
            )
            .unwrap();
        let stale_inbox: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_folders WHERE message_id = ?1 AND folder_id = 1",
                params![stale_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(keep_inbox, 1);
        assert_eq!(stale_inbox, 0);
        let messages_left: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(messages_left, 2);
    }
}
