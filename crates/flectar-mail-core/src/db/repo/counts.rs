//! Exact unread counts for split tabs and sidebar rows: a handful of scalar
//! COUNT queries sharing the predicate shapes of `threads::list`. Kept out of
//! threads.rs so the two evolve independently.

use crate::error::Result;
use crate::models::{Label, MailGrouping, MailboxBadgeCounts, SplitRule, UnreadCounts, roles};
use rusqlite::Connection;
use std::collections::HashMap;

const INBOX_BADGE_SQL: &str = "SELECT t.account_id, COUNT(*)
         FROM threads t
         LEFT JOIN snoozes s ON s.thread_id = t.id
         WHERE t.unread_count > 0
           AND s.thread_id IS NULL
           AND (EXISTS (
                SELECT 1 FROM messages m
                WHERE m.thread_id = t.id
                  AND m.folder_id IN (SELECT id FROM folders WHERE role = 'inbox')
           ) OR EXISTS (
                SELECT 1 FROM messages m
                JOIN accounts ma ON ma.id = m.account_id AND ma.provider = 'gmail'
                JOIN message_folders mf ON mf.message_id = m.id
                JOIN folders f ON f.id = mf.folder_id
                WHERE m.thread_id = t.id AND f.role = 'inbox'
           ))
         GROUP BY t.account_id";

/// Unread messages in an inbox outside snoozed conversations, for lists that
/// show each message on its own. Mirrors the message-level inbox filter of
/// `threads::list_messages`.
const INBOX_MESSAGE_BADGE_SQL: &str = "SELECT m.account_id, COUNT(*)
         FROM messages m
         JOIN accounts a ON a.id = m.account_id
         LEFT JOIN snoozes s ON s.thread_id = m.thread_id
         WHERE m.is_read = 0
           AND m.is_draft = 0
           AND m.thread_id IS NOT NULL
           AND s.thread_id IS NULL
           AND (m.folder_id IN (SELECT id FROM folders WHERE role = 'inbox')
                OR (a.provider = 'gmail' AND EXISTS (
                     SELECT 1 FROM message_folders mf
                     JOIN folders f ON f.id = mf.folder_id
                     WHERE mf.message_id = m.id AND f.role = 'inbox'
                )))
         GROUP BY m.account_id";

/// Query-plan details for the inbox badge path used by `mailbox_badge_counts`.
pub fn inbox_badge_query_plan(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {INBOX_BADGE_SQL}"))?;
    Ok(stmt
        .query_map([], |row| row.get(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Composable COUNT(*) query over threads (+ snoozes join, like SUMMARY_SELECT).
struct Q {
    clauses: Vec<String>,
    bind: Vec<Box<dyn rusqlite::types::ToSql>>,
}

impl Q {
    /// Base: threads with unread messages, optionally account-scoped.
    fn unread(account_id: Option<i64>) -> Self {
        let mut q = Q {
            clauses: vec!["t.unread_count > 0".into()],
            bind: Vec::new(),
        };
        if let Some(acc) = account_id {
            q.bind.push(Box::new(acc));
            q.clauses.push(format!("t.account_id = ?{}", q.bind.len()));
        }
        q
    }

    /// Same base without the unread filter (drafts badge counts all drafts).
    fn any(account_id: Option<i64>) -> Self {
        let mut q = Q {
            clauses: Vec::new(),
            bind: Vec::new(),
        };
        if let Some(acc) = account_id {
            q.bind.push(Box::new(acc));
            q.clauses.push(format!("t.account_id = ?{}", q.bind.len()));
        }
        q
    }

    fn clause(mut self, c: impl Into<String>) -> Self {
        self.clauses.push(c.into());
        self
    }

    fn role_exists(&mut self, role: &str) -> String {
        self.bind.push(Box::new(role.to_string()));
        format!(
            "(EXISTS (SELECT 1 FROM messages m JOIN folders f ON f.id = m.folder_id
                      WHERE m.thread_id = t.id AND f.role = ?{0})
              OR EXISTS (
                    SELECT 1 FROM messages m
                    JOIN accounts ma ON ma.id = m.account_id AND ma.provider = 'gmail'
                    JOIN message_folders mf ON mf.message_id = m.id
                    JOIN folders f ON f.id = mf.folder_id
                    WHERE m.thread_id = t.id AND f.role = ?{0}
              ))",
            self.bind.len(),
        )
    }

    fn inbox(mut self) -> Self {
        let c = self.role_exists(roles::INBOX);
        self.clauses.push(c);
        self.clauses.push("s.thread_id IS NULL".into());
        self
    }

    /// Important (`automated=false`) / Other (`automated=true`) default buckets:
    /// forced by a routing rule, or unrouted mail split by the newest incoming
    /// message. Mirrors the bucket clauses in `threads::list`.
    fn bucket(mut self, automated: bool) -> Self {
        let (want, forced) = if automated {
            (1, "other")
        } else {
            (0, "important")
        };
        self.clauses.push(format!(
            "(t.routed_tab = '{forced}'
              OR ((t.routed_tab IS NULL OR t.routed_tab = 'pending')
                  AND (SELECT m.is_automated FROM messages m
                       WHERE m.thread_id = t.id AND m.is_draft = 0 AND m.is_outgoing = 0
                       ORDER BY m.date DESC, m.id DESC LIMIT 1) = {want}))"
        ));
        self
    }

    /// A custom split tab: threads routed to `split:<id>`.
    fn routed_split(mut self, id: i64) -> Self {
        self.bind.push(Box::new(format!("split:{id}")));
        self.clauses
            .push(format!("t.routed_tab = ?{}", self.bind.len()));
        self
    }

    /// An auto-category tab: threads routed to `label:<id>`.
    fn routed_label(mut self, id: i64) -> Self {
        self.bind.push(Box::new(format!("label:{id}")));
        self.clauses
            .push(format!("t.routed_tab = ?{}", self.bind.len()));
        self
    }

    /// A manual (user) label: a cross-cutting membership filter, not a routed tab.
    fn label(mut self, label_id: i64) -> Self {
        self.bind.push(Box::new(label_id));
        self.clauses.push(format!(
            "EXISTS (SELECT 1 FROM message_labels ml JOIN messages m ON m.id = ml.message_id
                     WHERE m.thread_id = t.id AND ml.label_id = ?{})",
            self.bind.len()
        ));
        self
    }

    fn run(self, conn: &Connection) -> Result<i64> {
        let where_sql = if self.clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", self.clauses.join(" AND "))
        };
        let sql = format!(
            "SELECT COUNT(*) FROM threads t LEFT JOIN snoozes s ON s.thread_id = t.id {where_sql}"
        );
        let params: Vec<&dyn rusqlite::types::ToSql> =
            self.bind.iter().map(|b| b.as_ref()).collect();
        Ok(conn.query_row(&sql, params.as_slice(), |r| r.get(0))?)
    }
}

pub fn unread_counts(
    conn: &Connection,
    account_id: Option<i64>,
    splits: &[SplitRule],
    labels: &[Label],
) -> Result<UnreadCounts> {
    let inbox = Q::unread(account_id).inbox().run(conn)?;
    let important = Q::unread(account_id).inbox().bucket(false).run(conn)?;
    let other = Q::unread(account_id).inbox().bucket(true).run(conn)?;

    let mut splits_map = HashMap::new();
    for sp in splits {
        let n = Q::unread(account_id)
            .inbox()
            .routed_split(sp.id)
            .run(conn)?;
        splits_map.insert(sp.id.to_string(), n);
    }

    // Auto-category tabs read the single resolved tab; manual labels stay a
    // membership filter.
    let mut labels_map = HashMap::new();
    for l in labels {
        let q = Q::unread(account_id).inbox();
        let n = if l.is_auto {
            q.routed_label(l.id).run(conn)?
        } else {
            q.label(l.id).run(conn)?
        };
        labels_map.insert(l.id.to_string(), n);
    }

    let mut views = HashMap::new();
    views.insert(
        "starred".to_string(),
        Q::unread(account_id)
            .clause("t.starred_count > 0")
            .run(conn)?,
    );
    views.insert(
        "snoozed".to_string(),
        Q::unread(account_id)
            .clause("s.thread_id IS NOT NULL")
            .run(conn)?,
    );
    // Drafts badge = number of threads with a draft, unread or not.
    views.insert(
        "drafts".to_string(),
        Q::any(account_id)
            .clause(
                "(EXISTS (SELECT 1 FROM messages m JOIN folders f ON f.id = m.folder_id
                          WHERE m.thread_id = t.id AND m.is_draft = 1 AND f.role = 'drafts')
                  OR EXISTS (
                        SELECT 1 FROM messages m
                        JOIN accounts ma ON ma.id = m.account_id AND ma.provider = 'gmail'
                        JOIN message_folders mf ON mf.message_id = m.id
                        JOIN folders f ON f.id = mf.folder_id
                        WHERE m.thread_id = t.id AND m.is_draft = 1 AND f.role = 'drafts'
                  ))",
            )
            .run(conn)?,
    );

    Ok(UnreadCounts {
        inbox,
        important,
        other,
        splits: splits_map,
        labels: labels_map,
        views,
    })
}

/// Return the native sidebar's badges for every account in one grouped scan.
/// This deliberately avoids calling `unread_counts` once per account: the
/// Slint sidebar does not render split/label badges, and repeating those
/// scalar queries would make a live refresh proportional to account count.
/// Badges count what the mail list shows: conversations, or messages.
pub fn mailbox_badge_counts(
    conn: &Connection,
    grouping: MailGrouping,
) -> Result<Vec<MailboxBadgeCounts>> {
    mailbox_badge_counts_filtered(
        conn,
        grouping,
        &crate::recipient_match::MailScope::default(),
    )
}

/// `mailbox_badge_counts`, with inbox and starred totals limited to mail
/// matching `scope`. An empty scope is the unfiltered count.
pub fn mailbox_badge_counts_filtered(
    conn: &Connection,
    grouping: MailGrouping,
    scope: &crate::recipient_match::MailScope,
) -> Result<Vec<MailboxBadgeCounts>> {
    crate::recipient_match::install_recipient_matcher(conn, &scope.recipient_patterns)?;
    let messages = grouping == MailGrouping::Messages;
    let mut counts = conn
        .prepare("SELECT id FROM accounts ORDER BY sort_order, id")?
        .query_map([], |row| {
            Ok(MailboxBadgeCounts {
                account_id: row.get(0)?,
                inbox: 0,
                starred: 0,
                drafts: 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let positions = counts
        .iter()
        .enumerate()
        .map(|(index, counts)| (counts.account_id, index))
        .collect::<HashMap<_, _>>();

    // The inbox predicate mirrors `Q::inbox`, but grouping by account returns
    // every native sidebar section without repeating the scan per account.
    let (inbox_sql, inbox_binds) = scoped_badge_sql(
        if messages {
            INBOX_MESSAGE_BADGE_SQL
        } else {
            INBOX_BADGE_SQL
        },
        scope,
        messages,
    );
    let mut inbox = conn.prepare(&inbox_sql)?;
    let inbox_params: Vec<&dyn rusqlite::types::ToSql> =
        inbox_binds.iter().map(|value| value as _).collect();
    for row in inbox.query_map(inbox_params.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (account_id, value) = row?;
        if let Some(index) = positions.get(&account_id) {
            counts[*index].inbox = value;
        }
    }

    // Starred lists every starred item, including mail the reader has already
    // opened. The badge uses that same predicate.
    let starred_base = if messages {
        "SELECT m.account_id, COUNT(*) FROM messages m
         WHERE m.is_draft = 0 AND m.is_starred = 1 AND m.thread_id IS NOT NULL
         GROUP BY m.account_id"
    } else {
        "SELECT t.account_id, COUNT(*) FROM threads t
         WHERE t.starred_count > 0
         GROUP BY t.account_id"
    };
    let (starred_sql, starred_binds) = scoped_badge_sql(starred_base, scope, messages);
    let mut starred = conn.prepare(&starred_sql)?;
    let starred_params: Vec<&dyn rusqlite::types::ToSql> =
        starred_binds.iter().map(|value| value as _).collect();
    for row in starred.query_map(starred_params.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (account_id, value) = row?;
        if let Some(index) = positions.get(&account_id) {
            counts[*index].starred = value;
        }
    }

    // Draft folders are normally tiny. Starting from those folders uses the
    // existing folder/message indexes and avoids inspecting every inbox row.
    let counted = if messages { "message_id" } else { "thread_id" };
    let mut drafts = conn.prepare(&format!(
        "SELECT account_id, COUNT(DISTINCT {counted})
         FROM (
             SELECT m.account_id, m.thread_id, m.id AS message_id
             FROM folders f
             JOIN messages m ON m.folder_id = f.id
             WHERE f.role = 'drafts' AND m.is_draft = 1 AND m.thread_id IS NOT NULL
             UNION ALL
             SELECT m.account_id, m.thread_id, m.id AS message_id
             FROM folders f
             JOIN message_folders mf ON mf.folder_id = f.id
             JOIN messages m ON m.id = mf.message_id
             JOIN accounts a ON a.id = m.account_id AND a.provider = 'gmail'
             WHERE f.role = 'drafts' AND m.is_draft = 1 AND m.thread_id IS NOT NULL
         )
         GROUP BY account_id"
    ))?;
    for row in drafts.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))? {
        let (account_id, value) = row?;
        if let Some(index) = positions.get(&account_id) {
            counts[*index].drafts = value;
        }
    }

    Ok(counts)
}

/// Insert a business-tab scope before the badge query's GROUP BY.
fn scoped_badge_sql(
    sql: &str,
    scope: &crate::recipient_match::MailScope,
    messages: bool,
) -> (String, Vec<String>) {
    let clause = if messages {
        crate::recipient_match::message_scope_clause(scope, 1)
    } else {
        crate::recipient_match::thread_scope_clause(scope, "t.id", "t.account_id", 1)
    };
    match clause {
        Some((clause, likes)) => (
            sql.replacen("GROUP BY", &format!("AND {clause}\n         GROUP BY"), 1),
            likes,
        ),
        None => (sql.to_owned(), Vec::new()),
    }
}

/// Unread threads in each folder. A thread counts once per folder that holds
/// one of its messages. `account_ids` limits the scan to one company tab.
pub fn folder_unread_counts_for(
    conn: &Connection,
    account_ids: Option<&[i64]>,
    scope: Option<&crate::recipient_match::MailScope>,
) -> Result<HashMap<i64, i64>> {
    let (mut filter, mut binds) = match account_ids {
        None => (String::new(), Vec::new()),
        Some([]) => return Ok(HashMap::new()),
        Some(ids) => {
            let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            let clause = super::search::account_set_clause("m.account_id", ids, &mut binds);
            (format!(" AND {clause}"), binds)
        }
    };
    if let Some(scope) = scope.filter(|scope| scope.splits_shared_mailbox())
        && let Some((sql, scope_binds)) =
            crate::recipient_match::message_scope_clause(scope, binds.len() + 1)
    {
        filter.push_str(&format!(" AND ({sql})"));
        for value in scope_binds {
            binds.push(Box::new(value));
        }
    }
    let mut stmt = conn.prepare(&format!(
        "SELECT folder_id, COUNT(DISTINCT thread_id) FROM (
             SELECT m.folder_id AS folder_id, m.thread_id AS thread_id
             FROM messages m
             JOIN threads t ON t.id = m.thread_id
             WHERE t.unread_count > 0 AND m.folder_id IS NOT NULL{filter}
             UNION
             SELECT mf.folder_id, m.thread_id
             FROM message_folders mf
             JOIN messages m ON m.id = mf.message_id
             JOIN threads t ON t.id = m.thread_id
             JOIN accounts a ON a.id = m.account_id AND a.provider = 'gmail'
             WHERE t.unread_count > 0{filter}
         )
         GROUP BY folder_id"
    ))?;
    let params: Vec<&dyn rusqlite::types::ToSql> =
        binds.iter().map(|value| value.as_ref()).collect();
    let rows = stmt
        .query_map(params.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().collect())
}

/// Unread threads in each folder, across every account.
pub fn folder_unread_counts(conn: &Connection) -> Result<HashMap<i64, i64>> {
    folder_unread_counts_for(conn, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SplitRuleQuery;
    use rusqlite::params;

    fn test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, email, provider, auth_kind, username,
             incoming_host, incoming_port, smtp_host, smtp_port, created_at, mail_protocol, settings_json)
             VALUES (1,'t@x.com','generic','password','t','h',993,'h',587,0,'imap','{\"connection\":{\"incomingSecurity\":\"tls\",\"smtpSecurity\":\"tls\"}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO folders (id, account_id, path, role) VALUES (1,1,'INBOX','inbox')",
            [],
        )
        .unwrap();
        conn
    }

    fn seed_thread(conn: &Connection, id: i64, unread: i64, automated: bool) {
        conn.execute(
            "INSERT INTO threads (id, account_id, subject_norm, unread_count, last_message_at)
             VALUES (?1, 1, 'subj', ?2, 1000)",
            params![id, unread],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (thread_id, account_id, folder_id, uid, message_id, subject,
             from_addr, date, is_read, is_automated, is_draft, is_outgoing)
             VALUES (?1, 1, 1, ?1, 'mid-' || ?1, 'subj', 'a@b.c', 1000, ?2, ?3, 0, 0)",
            params![id, (unread == 0) as i64, automated as i64],
        )
        .unwrap();
    }

    fn append_incoming(conn: &Connection, thread_id: i64, date: i64, automated: bool) {
        conn.execute(
            "INSERT INTO messages (thread_id, account_id, folder_id, subject,
             from_addr, date, is_read, is_automated, is_draft, is_outgoing)
             VALUES (?1, 1, 1, 'follow-up', 'reply@b.c', ?2, 0, ?3, 0, 0)",
            params![thread_id, date, automated as i64],
        )
        .unwrap();
    }

    #[test]
    fn partitions_important_and_other() {
        let conn = test_db();
        seed_thread(&conn, 1, 1, false); // unread human
        seed_thread(&conn, 2, 1, true); // unread automated
        seed_thread(&conn, 3, 0, true); // read automated

        let c = unread_counts(&conn, None, &[], &[]).unwrap();
        assert_eq!(c.inbox, 2);
        assert_eq!(c.important, 1);
        assert_eq!(c.other, 1);
        assert_eq!(c.views["starred"], 0);

        // account filter that matches nothing
        let none = unread_counts(&conn, Some(99), &[], &[]).unwrap();
        assert_eq!(none.inbox, 0);
    }

    #[test]
    fn mixed_threads_are_counted_by_the_newest_incoming_message() {
        let conn = test_db();
        seed_thread(&conn, 1, 1, false);
        append_incoming(&conn, 1, 2000, true);
        seed_thread(&conn, 2, 1, true);
        append_incoming(&conn, 2, 2000, false);

        let c = unread_counts(&conn, None, &[], &[]).unwrap();
        assert_eq!(c.inbox, 2);
        assert_eq!(c.important, 1, "latest human reply belongs in Important");
        assert_eq!(c.other, 1, "latest automated reply belongs in Other");
    }

    #[test]
    fn native_sidebar_badges_follow_read_and_draft_mutations() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO accounts (id, email, provider, auth_kind, username,
             incoming_host, incoming_port, smtp_host, smtp_port, created_at, sort_order, mail_protocol, settings_json)
             VALUES (2,'two@x.com','generic','password','two','h',993,'h',587,0,1,'imap','{\"connection\":{\"incomingSecurity\":\"tls\",\"smtpSecurity\":\"tls\"}}')",
            [],
        )
        .unwrap();
        seed_thread(&conn, 1, 1, false);
        conn.execute("UPDATE threads SET starred_count = 1 WHERE id = 1", [])
            .unwrap();
        conn.execute(
            "INSERT INTO folders (id, account_id, path, role)
             VALUES (2,1,'Drafts','drafts')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, account_id, subject_norm, unread_count, last_message_at)
             VALUES (2,1,'draft',0,2000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (thread_id, account_id, folder_id, subject, from_addr,
             date, is_read, is_draft, is_outgoing)
             VALUES (2,1,2,'draft','t@x.com',2000,1,1,1)",
            [],
        )
        .unwrap();

        let badges = mailbox_badge_counts(&conn, MailGrouping::Conversations).unwrap();
        assert_eq!(
            badges,
            vec![
                MailboxBadgeCounts {
                    account_id: 1,
                    inbox: 1,
                    starred: 1,
                    drafts: 1,
                },
                MailboxBadgeCounts {
                    account_id: 2,
                    inbox: 0,
                    starred: 0,
                    drafts: 0,
                },
            ]
        );

        conn.execute("UPDATE threads SET unread_count = 0 WHERE id = 1", [])
            .unwrap();
        let badges = mailbox_badge_counts(&conn, MailGrouping::Conversations).unwrap();
        assert_eq!(badges[0].inbox, 0);
        assert_eq!(
            badges[0].starred, 1,
            "reading a starred thread keeps it starred"
        );
        assert_eq!(badges[0].drafts, 1);
    }

    #[test]
    fn starred_badges_count_read_mail_in_both_groupings() {
        let conn = test_db();
        seed_thread(&conn, 1, 0, false);
        conn.execute(
            "UPDATE threads SET starred_count = 1, unread_count = 0 WHERE id = 1",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE messages SET is_starred = 1, is_read = 1 WHERE thread_id = 1",
            [],
        )
        .unwrap();
        seed_thread(&conn, 2, 1, false);

        let conversations = mailbox_badge_counts(&conn, MailGrouping::Conversations).unwrap();
        let messages = mailbox_badge_counts(&conn, MailGrouping::Messages).unwrap();
        assert_eq!(conversations[0].inbox, 1);
        assert_eq!(conversations[0].starred, 1);
        assert_eq!(messages[0].inbox, 1);
        assert_eq!(messages[0].starred, 1);
    }

    #[test]
    fn native_sidebar_badges_count_gmail_links_once_and_exclude_snoozed_inbox() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO accounts (id, email, provider, auth_kind, username,
             incoming_host, incoming_port, smtp_host, smtp_port, created_at, sort_order, mail_protocol, settings_json)
             VALUES (2,'gmail@x.com','gmail','oauth2','gmail','h',993,'h',587,0,1,'gmail','{\"connection\":{\"incomingSecurity\":\"tls\",\"smtpSecurity\":\"tls\"}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO folders (id, account_id, path, role) VALUES
             (2,2,'All Mail','all'), (3,2,'INBOX','inbox'), (4,2,'Drafts','drafts')",
            [],
        )
        .unwrap();

        seed_thread(&conn, 1, 1, false);
        seed_thread(&conn, 2, 1, false);
        conn.execute("UPDATE threads SET starred_count = 1 WHERE id = 1", [])
            .unwrap();
        conn.execute(
            "INSERT INTO snoozes (thread_id, account_id, wake_at) VALUES (1,1,9999)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO threads (id, account_id, subject_norm, unread_count, starred_count,
             last_message_at) VALUES (3,2,'gmail',2,1,1000)",
            [],
        )
        .unwrap();
        for date in [1000, 1001] {
            conn.execute(
                "INSERT INTO messages (thread_id, account_id, folder_id, subject, date)
                 VALUES (3,2,2,'gmail',?1)",
                [date],
            )
            .unwrap();
            let message_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO message_folders (message_id, folder_id) VALUES (?1,3)",
                [message_id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO threads (id, account_id, subject_norm, unread_count, last_message_at)
             VALUES (4,2,'draft',0,1002)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (thread_id, account_id, folder_id, subject, date, is_draft)
             VALUES (4,2,4,'draft',1002,1)",
            [],
        )
        .unwrap();

        assert_eq!(
            mailbox_badge_counts(&conn, MailGrouping::Conversations).unwrap(),
            vec![
                MailboxBadgeCounts {
                    account_id: 1,
                    inbox: 1,
                    starred: 1,
                    drafts: 0,
                },
                MailboxBadgeCounts {
                    account_id: 2,
                    inbox: 1,
                    starred: 1,
                    drafts: 1,
                },
            ]
        );
    }

    #[test]
    fn split_and_label_maps() {
        let conn = test_db();
        seed_thread(&conn, 1, 1, false);
        conn.execute(
            "INSERT INTO labels (id, name, color, keyword, position) VALUES (5,'L','#fff','KwL',0)",
            [],
        )
        .unwrap();
        let msg_id: i64 = conn
            .query_row("SELECT id FROM messages WHERE thread_id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "INSERT INTO message_labels (message_id, label_id) VALUES (?1, 5)",
            params![msg_id],
        )
        .unwrap();
        // The custom-split count now reads the resolved tab, so route thread 1
        // into split:7 (the resolver does this at sync time).
        conn.execute("UPDATE threads SET routed_tab = 'split:7' WHERE id = 1", [])
            .unwrap();

        let split = SplitRule {
            id: 7,
            name: "s".into(),
            position: 0,
            query: SplitRuleQuery {
                senders: Some(vec!["a@b.c".into()]),
                ..Default::default()
            },
            target: None,
        };
        let label = Label {
            id: 5,
            name: "L".into(),
            color: "#fff".into(),
            keyword: "KwL".into(),
            position: 0,
            owner_account_id: None,
            is_auto: false,
        };

        let c = unread_counts(&conn, Some(1), &[split], &[label]).unwrap();
        assert_eq!(c.splits["7"], 1);
        assert_eq!(c.labels["5"], 1);
    }

    #[test]
    fn message_badges_count_unread_messages() {
        let conn = crate::db::testutil::conn();
        crate::db::testutil::seed_account(&conn);
        let (thread_id, _) =
            crate::db::testutil::seed_message(&conn, "a@b.dev", "two unread", false);
        let (own_thread, reply) =
            crate::db::testutil::seed_message(&conn, "a@b.dev", "re: two unread", false);
        conn.execute(
            "UPDATE messages SET thread_id = ?1, is_starred = 1, uid = 10000 + id WHERE id = ?2",
            params![thread_id, reply],
        )
        .unwrap();
        conn.execute("DELETE FROM threads WHERE id = ?1", [own_thread])
            .unwrap();
        crate::db::repo::threads::recompute(&conn, thread_id).unwrap();

        let conversations = mailbox_badge_counts(&conn, MailGrouping::Conversations).unwrap();
        assert_eq!((conversations[0].inbox, conversations[0].starred), (1, 1));
        let messages = mailbox_badge_counts(&conn, MailGrouping::Messages).unwrap();
        assert_eq!((messages[0].inbox, messages[0].starred), (2, 1));
    }

    #[test]
    fn folder_unread_counts_can_limit_the_scan_to_one_account_set() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO accounts (id, email, provider, auth_kind, username,
             incoming_host, incoming_port, smtp_host, smtp_port, created_at, mail_protocol, settings_json)
             VALUES (2,'other@x.com','generic','password','o','h',993,'h',587,0,'imap','{\"connection\":{\"incomingSecurity\":\"tls\",\"smtpSecurity\":\"tls\"}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO folders (id, account_id, path) VALUES (2, 1, 'Clients'), (3, 2, 'Clients')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, account_id, subject_norm, unread_count, last_message_at)
             VALUES (1, 1, 'a', 1, 1000), (2, 2, 'b', 1, 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (thread_id, account_id, folder_id, uid, message_id, subject,
             from_addr, date, is_read, is_automated, is_draft, is_outgoing)
             VALUES (1, 1, 2, 1, 'mid-1', 'a', 'a@b.c', 1000, 0, 0, 0, 0),
                    (2, 2, 3, 2, 'mid-2', 'b', 'b@b.c', 1000, 0, 0, 0, 0)",
            [],
        )
        .unwrap();

        let all = folder_unread_counts_for(&conn, None, None).unwrap();
        assert_eq!(all.get(&2), Some(&1));
        assert_eq!(all.get(&3), Some(&1));
        let company = folder_unread_counts_for(&conn, Some(&[2]), None).unwrap();
        assert_eq!(company.get(&3), Some(&1));
        assert!(!company.contains_key(&2));
        assert!(
            folder_unread_counts_for(&conn, Some(&[]), None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn badge_scans_can_be_limited_to_the_open_tab() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO accounts (id, email, provider, auth_kind, username,
             incoming_host, incoming_port, smtp_host, smtp_port, created_at, sort_order, mail_protocol, settings_json)
             VALUES (2,'two@x.com','generic','password','two','h',993,'h',587,0,1,'imap','{\"connection\":{\"incomingSecurity\":\"tls\",\"smtpSecurity\":\"tls\"}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO folders (id, account_id, path, role) VALUES (2,2,'INBOX','inbox')",
            [],
        )
        .unwrap();
        seed_thread(&conn, 1, 1, false);
        conn.execute(
            "INSERT INTO threads (id, account_id, subject_norm, unread_count, last_message_at)
             VALUES (2, 2, 'other', 1, 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (thread_id, account_id, folder_id, uid, message_id, subject,
             from_addr, date, is_read, is_automated, is_draft, is_outgoing)
             VALUES (2, 2, 2, 2, 'mid-2', 'other', 'a@b.c', 1000, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        let scope = crate::recipient_match::MailScope {
            base_account_ids: Some(vec![1]),
            ..Default::default()
        };
        let badges =
            mailbox_badge_counts_filtered(&conn, MailGrouping::Conversations, &scope).unwrap();
        let inbox = badges
            .iter()
            .map(|counts| (counts.account_id, counts.inbox))
            .collect::<Vec<_>>();
        assert_eq!(inbox, vec![(1, 1), (2, 0)]);
    }
}
