use crate::error::{CoreError, Result};
use crate::models::Label;
use rusqlite::{Connection, OptionalExtension, Row, params};

const DEFAULT_AUTO_LABELS: [(&str, &str, &str, i64); 4] = [
    ("Marketing", "#e0708a", "FlectarMailAutoMarketing", 1000),
    ("News", "#5b9dd9", "FlectarMailAutoNews", 1001),
    ("Social", "#7bc47f", "FlectarMailAutoSocial", 1002),
    ("Pitch", "#c9a04e", "FlectarMailAutoPitch", 1003),
];

fn from_row(row: &Row) -> rusqlite::Result<Label> {
    Ok(Label {
        id: row.get("id")?,
        name: row.get("name")?,
        color: row.get("color")?,
        keyword: row.get("keyword")?,
        position: row.get("position")?,
        owner_account_id: row.get("owner_account_id")?,
        is_auto: row.get::<_, i64>("is_auto")? != 0,
    })
}

/// Derive a valid IMAP keyword atom from a label name. IMAP flag-keywords are
/// atoms, so strip the characters an atom may not contain (RFC 3501) and fall
/// back to a stable placeholder when nothing usable remains.
pub fn keyword_for(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '(' | ')' | '{' | ' ' | '%' | '*' | '"' | '\\' | ']' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    out.truncate(out.trim_end_matches('_').len());
    let start = out.len() - out.trim_start_matches('_').len();
    if start > 0 {
        out.drain(..start);
    }
    if out.capacity() > out.len().saturating_mul(2).max(64) {
        out.shrink_to_fit();
    }
    if out.is_empty() {
        "Label".to_string()
    } else {
        out
    }
}

pub fn list(conn: &Connection) -> Result<Vec<Label>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, color, keyword, position, owner_account_id, is_auto
         FROM labels
         ORDER BY CASE scope WHEN 'automatic' THEN 0 WHEN 'global' THEN 1 ELSE 2 END,
                  owner_account_id, position, name",
    )?;
    let rows = stmt
        .query_map([], from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Label>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, color, keyword, position, owner_account_id, is_auto
         FROM labels WHERE id = ?1",
    )?;
    Ok(stmt.query_row(params![id], from_row).optional()?)
}

pub fn save(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    color: &str,
    position: i64,
    owner_account_id: Option<i64>,
) -> Result<Label> {
    let owner_account_id = match id {
        Some(id) => {
            get(conn, id)?
                .ok_or_else(|| CoreError::NotFound(format!("label {id}")))?
                .owner_account_id
        }
        None => owner_account_id,
    };
    let duplicate: bool = match owner_account_id {
        Some(account_id) => conn.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM labels
                WHERE name = ?1 COLLATE NOCASE
                  AND scope = 'account' AND owner_account_id = ?2
                  AND (?3 IS NULL OR id != ?3)
             )",
            params![name, account_id, id],
            |row| row.get(0),
        )?,
        None => conn.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM labels
                WHERE name = ?1 COLLATE NOCASE AND scope = 'global'
                  AND (?2 IS NULL OR id != ?2)
             )",
            params![name, id],
            |row| row.get(0),
        )?,
    };
    if duplicate {
        return Err(CoreError::Other(format!(
            "a label named '{name}' already exists in this scope"
        )));
    }
    let id = match id {
        // Rename/recolor: keep the existing keyword so the server mapping is
        // stable even when the display name changes.
        Some(id) => {
            conn.execute(
                "UPDATE labels SET name=?2, color=?3, position=?4 WHERE id=?1",
                params![id, name, color, position],
            )?;
            id
        }
        None => {
            conn.execute(
                "INSERT INTO labels (
                   name, color, keyword, position, scope, owner_account_id, origin
                 ) VALUES (?1,?2,?3,?4,?5,?6,'user')",
                params![
                    name,
                    color,
                    keyword_for(name),
                    position,
                    if owner_account_id.is_some() {
                        "account"
                    } else {
                        "global"
                    },
                    owner_account_id,
                ],
            )?;
            conn.last_insert_rowid()
        }
    };
    let mut stmt = conn.prepare(
        "SELECT id, name, color, keyword, position, owner_account_id, is_auto
         FROM labels WHERE id = ?1",
    )?;
    Ok(stmt.query_row(params![id], from_row)?)
}

/// Rename/recolor one label and, when requested, keep every slash-delimited
/// descendant attached to it. Gmail exposes labels as flat names even though
/// its standard UI treats `Parent/Child` as a tree, so parent renames have to
/// update the complete subtree explicitly when performed through the API.
///
/// Descendants are renamed deepest-first. This avoids transient collisions for
/// valid moves such as renaming `A` to `A/B`, where the existing `A/B` child
/// must move out of the way before the root takes that name.
pub fn save_with_descendants(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    color: &str,
    position: i64,
    owner_account_id: Option<i64>,
    cascade_descendants: bool,
) -> Result<(Label, Vec<Label>)> {
    let Some(id) = id else {
        return Ok((
            save(conn, None, name, color, position, owner_account_id)?,
            Vec::new(),
        ));
    };
    let previous = get(conn, id)?.ok_or_else(|| CoreError::NotFound(format!("label {id}")))?;
    if !cascade_descendants || previous.name == name {
        return Ok((
            save(conn, Some(id), name, color, position, owner_account_id)?,
            Vec::new(),
        ));
    }

    let prefix = format!("{}/", previous.name);
    let replacement_prefix = format!("{name}/");
    let all_labels = list(conn)?;
    let mut descendants = all_labels
        .iter()
        .filter(|label| {
            label.owner_account_id == previous.owner_account_id
                && label.id != previous.id
                && label.name.starts_with(&prefix)
        })
        .collect::<Vec<_>>();
    let subtree_ids = std::iter::once(previous.id)
        .chain(descendants.iter().map(|label| label.id))
        .collect::<std::collections::HashSet<_>>();
    let target_names =
        std::iter::once(std::borrow::Cow::Borrowed(name)).chain(descendants.iter().map(|label| {
            std::borrow::Cow::Owned(format!(
                "{replacement_prefix}{}",
                label.name.strip_prefix(&prefix).unwrap_or(&label.name)
            ))
        }));

    for target in target_names {
        if all_labels.iter().any(|label| {
            label.owner_account_id == previous.owner_account_id
                && !subtree_ids.contains(&label.id)
                && label.name.eq_ignore_ascii_case(&target)
        }) {
            return Err(CoreError::Other(format!(
                "a label named '{target}' already exists in this scope"
            )));
        }
    }

    descendants.sort_by_key(|label| std::cmp::Reverse(label.name.len()));
    let mut renamed_descendants = Vec::with_capacity(descendants.len());
    for descendant in descendants {
        let suffix = descendant
            .name
            .strip_prefix(&prefix)
            .unwrap_or(&descendant.name);
        renamed_descendants.push(save(
            conn,
            Some(descendant.id),
            &format!("{replacement_prefix}{suffix}"),
            &descendant.color,
            descendant.position,
            descendant.owner_account_id,
        )?);
    }
    let root = save(conn, Some(id), name, color, position, owner_account_id)?;
    Ok((root, renamed_descendants))
}

/// Return one slash-delimited label subtree deepest-first. Matching is scoped
/// to the root's owner so identical paths in other accounts stay independent.
pub fn subtree(conn: &Connection, root: &Label) -> Result<Vec<Label>> {
    let prefix = format!("{}/", root.name);
    let mut labels = list(conn)?
        .into_iter()
        .filter(|label| {
            label.owner_account_id == root.owner_account_id
                && (label.id == root.id || label.name.starts_with(&prefix))
        })
        .collect::<Vec<_>>();
    labels.sort_by_key(|label| std::cmp::Reverse(label.name.len()));
    Ok(labels)
}

pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    let key = format!("label:{id}");
    // Any thread routed into this label (an auto category) must fall back to
    // Important/Other, so clear its routed tab before the row goes. Chip
    // memberships in message_labels cascade via the foreign key.
    conn.execute(
        "UPDATE threads SET routed_tab = NULL WHERE routed_tab = ?1",
        params![key],
    )?;
    // A rule that routed matches into this category would dangle (and fail the
    // message_labels foreign key on the next route); revert those rules to their
    // own tab.
    conn.execute(
        "UPDATE split_rules SET target = NULL WHERE target = ?1",
        params![key],
    )?;
    conn.execute("DELETE FROM labels WHERE id = ?1", params![id])?;
    Ok(())
}

/// Recreate any missing built-in auto categories. Existing categories and
/// their user-defined order are left untouched. A conflicting user label with
/// the same display name is also left untouched.
pub fn restore_auto_defaults(conn: &Connection) -> Result<i64> {
    let mut restored = 0;
    for (name, color, keyword, position) in DEFAULT_AUTO_LABELS {
        let exists: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM labels WHERE keyword = ?1 AND is_auto = 1)",
            params![keyword],
            |row| row.get::<_, i64>(0).map(|value| value != 0),
        )?;
        if exists {
            continue;
        }
        restored += conn.execute(
            "INSERT OR IGNORE INTO labels (
               name, color, keyword, position, is_auto, scope, origin
             ) VALUES (?1, ?2, ?3, ?4, 1, 'automatic', 'system')",
            params![name, color, keyword, position],
        )? as i64;
    }
    Ok(restored)
}

/// Label ids applied to any message in a thread.
pub fn for_thread(conn: &Connection, thread_id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT ml.label_id FROM message_labels ml
         JOIN messages m ON m.id = ml.message_id
         WHERE m.thread_id = ?1 ORDER BY ml.label_id",
    )?;
    let rows = stmt
        .query_map(params![thread_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The account's tag for `keyword`, creating it when the account has none.
///
/// The new row keeps `keyword` exactly, so a tab tag stays one keyword across
/// accounts instead of a name-derived atom that would not match `$label:`.
pub fn ensure_keyword_label(
    conn: &Connection,
    account_id: i64,
    name: &str,
    color: &str,
    keyword: &str,
) -> Result<Label> {
    if let Some(existing) = label_by_keyword(conn, account_id, keyword)? {
        return Ok(existing);
    }
    let position: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM labels WHERE owner_account_id = ?1",
        params![account_id],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO labels (
           name, color, keyword, position, is_auto, scope, owner_account_id, origin
         ) VALUES (?1, ?2, ?3, ?4, 0, 'account', ?5, 'user')",
        params![name, color, keyword, position, account_id],
    )?;
    let id = conn.last_insert_rowid();
    get(conn, id)?.ok_or_else(|| CoreError::NotFound(format!("label {id}")))
}

pub fn label_by_keyword(
    conn: &Connection,
    account_id: i64,
    keyword: &str,
) -> Result<Option<Label>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, color, keyword, position, owner_account_id, is_auto
         FROM labels WHERE owner_account_id = ?1 AND keyword = ?2",
    )?;
    Ok(stmt
        .query_row(params![account_id, keyword], from_row)
        .optional()?)
}

/// Give every account copy of `keyword` the same name and color.
pub fn restyle_keyword(
    conn: &Connection,
    account_ids: &[i64],
    keyword: &str,
    name: &str,
    color: &str,
) -> Result<Vec<i64>> {
    let mut updated = Vec::new();
    for account_id in account_ids {
        let Some(label) = label_by_keyword(conn, *account_id, keyword)? else {
            continue;
        };
        if label.name == name && label.color == color {
            updated.push(label.id);
            continue;
        }
        conn.execute(
            "UPDATE labels SET name = ?2, color = ?3 WHERE id = ?1",
            params![label.id, name, color],
        )?;
        updated.push(label.id);
    }
    Ok(updated)
}

pub fn add_to_message(conn: &Connection, message_id: i64, label_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO message_labels (message_id, label_id) VALUES (?1, ?2)",
        params![message_id, label_id],
    )?;
    Ok(())
}

pub fn remove_from_message(conn: &Connection, message_id: i64, label_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM message_labels WHERE message_id = ?1 AND label_id = ?2",
        params![message_id, label_id],
    )?;
    Ok(())
}

/// Keyword prefix that server-side tooling (Mailroom, Bulwark webmail tags) uses
/// for tags: `$label:needs-action`.
const SERVER_TAG_PREFIX: &str = "$label:";

/// Display name for a server tag id: `needs-action` -> `Needs Action`,
/// `one-time` -> `One Time`.
pub fn server_tag_name(id: &str) -> String {
    id.split(['-', '_', '/'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Create an account-scoped provider label for every `$label:<id>` keyword the
/// server reports that this account has no label for yet, so tags applied by
/// a server filter or another client show up as tags here too.
fn adopt_server_tags(conn: &Connection, account_id: i64, keywords: &[String]) -> Result<()> {
    for keyword in keywords.iter().filter(|k| k.starts_with(SERVER_TAG_PREFIX)) {
        let id = &keyword[SERVER_TAG_PREFIX.len()..];
        if id.is_empty() {
            continue;
        }
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM labels WHERE owner_account_id = ?1 AND keyword = ?2)",
            params![account_id, keyword],
            |r| r.get(0),
        )?;
        if exists {
            continue;
        }
        let mut name = server_tag_name(id);
        let taken: bool = conn.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM labels
                WHERE owner_account_id = ?1 AND name = ?2 COLLATE NOCASE
             )",
            params![account_id, name],
            |r| r.get(0),
        )?;
        if taken {
            name = keyword.clone();
        }
        conn.execute(
            "INSERT INTO labels (name, color, keyword, position, is_auto, scope, owner_account_id, origin)
             VALUES (?1, '#6b7280', ?2, 500, 0, 'account', ?3, 'provider')",
            params![name, keyword, account_id],
        )?;
    }
    Ok(())
}

/// Reconcile a message's label memberships against the IMAP keywords the server
/// reports. `$label:<id>` keywords are adopted as account tags first. Other
/// unknown server keywords are left alone so foreign keywords never masquerade
/// as labels. Returns true if anything changed.
pub fn reconcile_keywords(conn: &Connection, message_id: i64, keywords: &[String]) -> Result<bool> {
    let account_id: i64 = conn.query_row(
        "SELECT account_id FROM messages WHERE id = ?1",
        params![message_id],
        |row| row.get(0),
    )?;
    adopt_server_tags(conn, account_id, keywords)?;
    let labels = list(conn)?;
    if labels.is_empty() {
        return Ok(false);
    }
    let mut changed = false;
    // Auto labels are local-only (classified at sync, never pushed to IMAP):
    // reconciling them against server keywords would strip their memberships
    // on every flag pass, so they are excluded here.
    for label in labels
        .into_iter()
        .filter(|label| label.owner_account_id == Some(account_id))
    {
        let present = keywords.iter().any(|k| k == &label.keyword);
        let has: bool = conn.query_row(
            "SELECT COUNT(*) FROM message_labels WHERE message_id = ?1 AND label_id = ?2",
            params![message_id, label.id],
            |r| r.get::<_, i64>(0),
        )? > 0;
        if present && !has {
            add_to_message(conn, message_id, label.id)?;
            changed = true;
        } else if !present && has {
            remove_from_message(conn, message_id, label.id)?;
            changed = true;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil;

    #[test]
    fn short_keywords_do_not_retain_discarded_padding() {
        let name = format!("{}é{}", "_".repeat(100_000), "_".repeat(100_000));
        let keyword = keyword_for(&name);
        assert_eq!(keyword, "é");
        assert_eq!(keyword.capacity(), "é".len());
    }

    #[test]
    fn keyword_for_strips_forbidden_atom_chars() {
        assert_eq!(keyword_for("Follow up"), "Follow_up");
        // "{" is an atom-special and is stripped; "}" is a legal atom char.
        assert_eq!(keyword_for("(a) {b} \"c\""), "a___b}__c");
        assert_eq!(keyword_for("   "), "Label");
        assert_eq!(keyword_for("Déjà"), "Déjà"); // non-ASCII survives
    }

    #[test]
    fn migration_seeds_four_auto_labels() {
        let c = testutil::conn();
        let auto: Vec<Label> = list(&c)
            .unwrap()
            .into_iter()
            .filter(|l| l.is_auto)
            .collect();
        let keywords: Vec<&str> = auto.iter().map(|l| l.keyword.as_str()).collect();
        assert_eq!(auto.len(), 4);
        for k in [
            "FlectarMailAutoMarketing",
            "FlectarMailAutoNews",
            "FlectarMailAutoSocial",
            "FlectarMailAutoPitch",
        ] {
            assert!(keywords.contains(&k), "missing seed {k}");
        }
    }

    #[test]
    fn save_keeps_keyword_on_rename() {
        let c = testutil::conn();
        let l = save(&c, None, "Follow up", "#fff", 0, None).unwrap();
        assert_eq!(l.keyword, "Follow_up");
        let renamed = save(&c, Some(l.id), "Chase later", "#000", 1, None).unwrap();
        assert_eq!(renamed.keyword, "Follow_up");
        assert_eq!(renamed.name, "Chase later");
    }

    #[test]
    fn gmail_style_parent_rename_keeps_the_complete_label_tree() {
        let c = testutil::conn();
        testutil::seed_account(&c);
        let root = save(&c, None, "Projects", "#111111", 1, Some(1)).unwrap();
        let child = save(&c, None, "Projects/Launch", "#222222", 2, Some(1)).unwrap();
        let leaf = save(&c, None, "Projects/Launch/Design", "#333333", 3, Some(1)).unwrap();
        save(&c, None, "Other", "#444444", 4, Some(1)).unwrap();

        let (renamed, descendants) =
            save_with_descendants(&c, Some(root.id), "Work", "#555555", 5, None, true).unwrap();

        assert_eq!(renamed.name, "Work");
        assert_eq!(get(&c, child.id).unwrap().unwrap().name, "Work/Launch");
        assert_eq!(
            get(&c, leaf.id).unwrap().unwrap().name,
            "Work/Launch/Design"
        );
        assert_eq!(
            descendants.iter().map(|label| label.id).collect::<Vec<_>>(),
            [leaf.id, child.id]
        );
        assert_eq!(
            subtree(&c, &renamed)
                .unwrap()
                .iter()
                .map(|label| label.id)
                .collect::<Vec<_>>(),
            [leaf.id, child.id, root.id]
        );
    }

    #[test]
    fn subtree_rename_rejects_collisions_before_changing_any_name() {
        let c = testutil::conn();
        testutil::seed_account(&c);
        let root = save(&c, None, "Projects", "#111111", 1, Some(1)).unwrap();
        let child = save(&c, None, "Projects/Launch", "#222222", 2, Some(1)).unwrap();
        save(&c, None, "Work/Launch", "#333333", 3, Some(1)).unwrap();

        let error =
            save_with_descendants(&c, Some(root.id), "Work", "#444444", 4, None, true).unwrap_err();

        assert!(error.to_string().contains("already exists"));
        assert_eq!(get(&c, root.id).unwrap().unwrap().name, "Projects");
        assert_eq!(get(&c, child.id).unwrap().unwrap().name, "Projects/Launch");
    }

    #[test]
    fn delete_removes_auto_rows_and_clears_routed_tab() {
        use crate::db::repo::threads;
        let c = testutil::conn();
        // Match production (db::open sets this): chip memberships cascade on delete.
        c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        testutil::seed_account(&c);
        let auto_id: i64 = c
            .query_row("SELECT id FROM labels WHERE is_auto = 1 LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        // A thread routed into that auto category.
        let (thread, _msg) = testutil::seed_message(&c, "x@shop.example", "sale", true);
        crate::route::apply_tab(&c, thread, Some(&format!("label:{auto_id}"))).unwrap();

        delete(&c, auto_id).unwrap();
        assert!(
            get(&c, auto_id).unwrap().is_none(),
            "auto label must delete"
        );
        // The orphaned thread falls back to Important/Other (routed_tab cleared).
        let routed: Option<String> = c
            .query_row(
                "SELECT routed_tab FROM threads WHERE id = ?1",
                params![thread],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(routed, None);
        assert!(
            threads::get_summary(&c, thread)
                .unwrap()
                .unwrap()
                .labels
                .is_empty()
        );

        let manual = save(&c, None, "Temp", "#fff", 0, None).unwrap();
        delete(&c, manual.id).unwrap();
        assert!(get(&c, manual.id).unwrap().is_none());
    }

    #[test]
    fn restore_auto_defaults_recreates_only_missing_categories() {
        let c = testutil::conn();
        let social_id = c
            .query_row(
                "SELECT id FROM labels WHERE keyword = 'FlectarMailAutoSocial'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        delete(&c, social_id).unwrap();

        assert_eq!(restore_auto_defaults(&c).unwrap(), 1);
        assert_eq!(restore_auto_defaults(&c).unwrap(), 0);
        let auto = list(&c)
            .unwrap()
            .into_iter()
            .filter(|label| label.is_auto)
            .collect::<Vec<_>>();
        assert_eq!(auto.len(), 4);
        assert!(
            auto.iter()
                .any(|label| label.keyword == "FlectarMailAutoSocial")
        );
    }

    #[test]
    fn delete_reverts_rules_that_targeted_the_category() {
        use crate::db::repo::splits;
        let c = testutil::conn();
        let auto_id = c
            .query_row(
                "SELECT id FROM labels WHERE keyword = 'FlectarMailAutoMarketing'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        // A rule that routes matches into that category.
        let rule = splits::save(
            &c,
            None,
            "promos",
            0,
            &crate::models::SplitRuleQuery {
                senders: Some(vec!["shop.example".into()]),
                ..Default::default()
            },
            Some(&format!("label:{auto_id}")),
        )
        .unwrap();

        delete(&c, auto_id).unwrap();

        // The rule survives but reverts to its own tab (no dangling target).
        let got = splits::get(&c, rule.id).unwrap().unwrap();
        assert_eq!(got.target, None);
    }

    /// Regression: server reconcile must never strip local-only auto labels.
    #[test]
    fn reconcile_skips_auto_labels() {
        let c = testutil::conn();
        testutil::seed_account(&c);
        let (_t, msg) = testutil::seed_message(&c, "a@b.c", "hello", false);

        let manual = save(&c, None, "Work", "#fff", 0, Some(1)).unwrap();
        let auto_id: i64 = c
            .query_row(
                "SELECT id FROM labels WHERE keyword = 'FlectarMailAutoNews'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        add_to_message(&c, msg, manual.id).unwrap();
        add_to_message(&c, msg, auto_id).unwrap();

        // Server reports no keywords at all: manual membership is removed,
        // auto membership survives.
        let changed = reconcile_keywords(&c, msg, &[]).unwrap();
        assert!(changed);
        let remaining = for_thread(&c, 1).unwrap();
        assert!(
            remaining.contains(&auto_id),
            "auto label was stripped by reconcile"
        );
        assert!(!remaining.contains(&manual.id));

        // Server later reports the manual keyword: membership comes back.
        reconcile_keywords(&c, msg, &["Work".to_string()]).unwrap();
        assert!(for_thread(&c, 1).unwrap().contains(&manual.id));
    }

    #[test]
    fn reconcile_adopts_server_tags() {
        let c = testutil::conn();
        testutil::seed_account(&c);
        let (_t, msg) = testutil::seed_message(&c, "a@b.c", "hello", false);
        let kws = vec![
            "$label:needs-action".to_string(),
            "$label:invoice".to_string(),
            "$label:unpaid".to_string(),
            "$label:".to_string(),
            "$seen".to_string(),
        ];
        assert!(reconcile_keywords(&c, msg, &kws).unwrap());
        let (id, name, scope, origin, is_auto, owner): (i64, String, String, String, i64, i64) = c
            .query_row(
                "SELECT id, name, scope, origin, is_auto, owner_account_id
                 FROM labels WHERE keyword = '$label:needs-action'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(name, "Needs Action");
        assert_eq!(scope, "account");
        assert_eq!(origin, "provider");
        assert_eq!(is_auto, 0);
        assert_eq!(owner, 1);
        let applied = for_thread(&c, 1).unwrap();
        assert!(applied.contains(&id));
        for (keyword, display) in [("$label:invoice", "Invoice"), ("$label:unpaid", "Unpaid")] {
            let (label_id, label_name): (i64, String) = c
                .query_row(
                    "SELECT id, name FROM labels WHERE keyword = ?1",
                    params![keyword],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(label_name, display);
            assert!(applied.contains(&label_id));
        }
        let blank: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM labels WHERE keyword = '$label:'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(blank, 0);
        // Idempotent: a second pass adds no duplicate label.
        reconcile_keywords(&c, msg, &kws).unwrap();
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM labels WHERE keyword = '$label:needs-action'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(server_tag_name("one-time"), "One Time");
        assert_eq!(server_tag_name("invoice"), "Invoice");
        assert_eq!(server_tag_name("unpaid"), "Unpaid");
        // Server clears the keyword: membership drops, the tag itself stays.
        assert!(reconcile_keywords(&c, msg, &["$seen".to_string()]).unwrap());
        assert!(!for_thread(&c, 1).unwrap().contains(&id));
        let still: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM labels WHERE keyword = '$label:needs-action'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still, 1);
    }

    #[test]
    fn reconcile_server_tag_uses_keyword_when_display_name_is_taken() {
        let c = testutil::conn();
        testutil::seed_account(&c);
        let (_t, msg) = testutil::seed_message(&c, "a@b.c", "hello", false);
        save(&c, None, "needs action", "#fff", 0, Some(1)).unwrap();
        assert!(reconcile_keywords(&c, msg, &["$label:needs-action".to_string()]).unwrap());
        let name: String = c
            .query_row(
                "SELECT name FROM labels WHERE keyword = '$label:needs-action'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "$label:needs-action");
        let id: i64 = c
            .query_row(
                "SELECT id FROM labels WHERE keyword = '$label:needs-action'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(for_thread(&c, 1).unwrap().contains(&id));
    }
}
