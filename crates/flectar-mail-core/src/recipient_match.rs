//! Wildcard matching for business-tab recipient and sender filters.
//!
//! A pattern is an address with `*` meaning any sequence and `?` meaning one
//! character, compared without regard to case. `tim@*` matches
//! `tim@example.com` and not `tmentink@gmail.com`. `ohnage?@*` matches
//! `ohnage1@x.com` and not `ohnage@x.com`. A recipient filter matches To, Cc,
//! Bcc, and the mailbox address (the address the message was delivered into,
//! used as Delivered-To). Literal addresses are an exact set. The SQL clause
//! is one scalar function, so a few hundred patterns do not build an `OR`
//! tree SQLite will reject. Sender display names match only together with
//! one of the scope's own domains.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// `true` when `email` matches `pattern`. Both sides are trimmed. `*` is the
/// only wildcard; an empty pattern matches nothing.
pub fn email_matches(pattern: &str, email: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    let email = email.trim().to_ascii_lowercase();
    if pattern.is_empty() || email.is_empty() || pattern.len() > 200 {
        return false;
    }
    glob(&pattern, &email)
}

fn glob(pattern: &str, text: &str) -> bool {
    // Addresses are ASCII. `*` consumes any run; `?` consumes one byte.
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut pattern_index = 0;
    let mut text_index = 0;
    let mut star_pattern = None;
    let mut star_text = 0;
    while text_index < text.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == text[text_index])
        {
            pattern_index += 1;
            text_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_pattern = Some(pattern_index);
            star_text = text_index;
            pattern_index += 1;
        } else if let Some(star) = star_pattern {
            pattern_index = star + 1;
            star_text += 1;
            text_index = star_text;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

/// Precompiled recipient patterns. Exact addresses are a set lookup; `*` and
/// `?` patterns are globs.
#[derive(Debug, Clone)]
pub struct RecipientMatcher {
    exact: HashSet<String>,
    globs: Vec<String>,
}

impl RecipientMatcher {
    pub fn compile(patterns: &[String]) -> Self {
        let mut exact = HashSet::new();
        let mut globs = Vec::new();
        for pattern in patterns {
            let pattern = pattern.trim().to_ascii_lowercase();
            if pattern.is_empty() || pattern.len() > 200 {
                continue;
            }
            if pattern.contains('*') || pattern.contains('?') {
                if !globs.iter().any(|kept: &String| kept == &pattern) {
                    globs.push(pattern);
                }
            } else {
                exact.insert(pattern);
            }
        }
        Self { exact, globs }
    }

    pub fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.globs.is_empty()
    }

    /// `true` when `email` matches any compiled pattern.
    pub fn matches_email(&self, email: &str) -> bool {
        let email = email.trim().to_ascii_lowercase();
        if email.is_empty() {
            return false;
        }
        self.exact.contains(&email) || self.globs.iter().any(|pattern| glob(pattern, &email))
    }

    /// `true` when the mailbox or any To/Cc/Bcc address matches.
    pub fn matches_headers(
        &self,
        to_json: &str,
        cc_json: &str,
        bcc_json: &str,
        mailbox: &str,
    ) -> bool {
        if self.is_empty() {
            return false;
        }
        self.matches_email(mailbox)
            || json_has_match(to_json, self)
            || json_has_match(cc_json, self)
            || json_has_match(bcc_json, self)
    }
}

fn json_has_match(json: &str, matcher: &RecipientMatcher) -> bool {
    let bytes = json.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        index += 1;
        let start = index;
        let end = skip_json_string(bytes, &mut index);
        if &json[start..end] != "email" {
            continue;
        }
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b':' {
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'"' {
            continue;
        }
        index += 1;
        let value_start = index;
        let value_end = skip_json_string(bytes, &mut index);
        if matcher.matches_email(&json[value_start..value_end]) {
            return true;
        }
    }
    false
}

/// `index` points at the first byte of a JSON string. Returns the end of the
/// decoded span and leaves `index` just past the closing quote.
fn skip_json_string(bytes: &[u8], index: &mut usize) -> usize {
    let start = *index;
    let mut escaped = false;
    while *index < bytes.len() {
        let byte = bytes[*index];
        if escaped {
            escaped = false;
            *index += 1;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            *index += 1;
            continue;
        }
        if byte == b'"' {
            let end = *index;
            *index += 1;
            return end;
        }
        *index += 1;
    }
    start
}

struct MatcherSlot {
    matcher: RwLock<Arc<RecipientMatcher>>,
}

fn matcher_slots() -> &'static Mutex<HashMap<usize, Arc<MatcherSlot>>> {
    static SLOTS: OnceLock<Mutex<HashMap<usize, Arc<MatcherSlot>>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register `flectar_recipient_match(to_json, cc_json, bcc_json, mailbox)` on
/// `conn`. The function reads a slot updated in place, so a cached statement
/// keeps calling the patterns installed for the latest query on that
/// connection. An empty pattern list leaves the clause unused and is a no-op.
pub fn install_recipient_matcher(
    conn: &rusqlite::Connection,
    patterns: &[String],
) -> rusqlite::Result<()> {
    let compiled = Arc::new(RecipientMatcher::compile(patterns));
    if compiled.is_empty() {
        return Ok(());
    }
    // Identity only: the pointer is not dereferenced. A reopened database has
    // a new handle, so it gets its own function instead of a stale slot.
    let key = unsafe { conn.handle() as usize };
    let mut slots = matcher_slots()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(slot) = slots.get(&key) {
        *slot
            .matcher
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = compiled;
        return Ok(());
    }
    let slot = Arc::new(MatcherSlot {
        matcher: RwLock::new(compiled),
    });
    slots.insert(key, Arc::clone(&slot));
    drop(slots);
    conn.create_scalar_function(
        "flectar_recipient_match",
        4,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        move |ctx| {
            let matcher = slot
                .matcher
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let to_json = ctx.get::<Option<String>>(0)?.unwrap_or_default();
            let cc_json = ctx.get::<Option<String>>(1)?.unwrap_or_default();
            let bcc_json = ctx.get::<Option<String>>(2)?.unwrap_or_default();
            let mailbox = ctx.get::<Option<String>>(3)?.unwrap_or_default();
            Ok(matcher.matches_headers(&to_json, &cc_json, &bcc_json, &mailbox))
        },
    )
}

/// Bound values for one pattern: the mailbox address, then the same address
/// as it appears in a `{"email":"..."}` JSON list (To, Cc, or Bcc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternLikes {
    pub address: String,
    pub header: String,
}

/// LIKE patterns for the non-empty filters. `*` becomes `%`; `%`, `_` and
/// `\` in the literal text are escaped for `ESCAPE '\'`.
pub fn pattern_likes(patterns: &[String]) -> Vec<PatternLikes> {
    patterns
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty() && pattern.len() <= 200)
        .map(|pattern| {
            let body = like_body(pattern);
            let header = if pattern.ends_with('*') {
                format!("%\"email\":\"{body}")
            } else {
                format!("%\"email\":\"{body}\"")
            };
            PatternLikes {
                address: body,
                header,
            }
        })
        .collect()
}

fn like_body(pattern: &str) -> String {
    let mut out = String::new();
    for ch in pattern.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            '*' => out.push('%'),
            '?' => out.push('_'),
            other => out.push(other.to_ascii_lowercase()),
        }
    }
    out
}

/// `EXISTS` predicate on `thread_column` (for example `t.id`). The clause
/// calls `flectar_recipient_match` and binds nothing; install that function
/// on the connection before preparing. `None` when there is nothing to filter.
/// `first_bind` is unused and kept so callers can compose later predicates.
pub fn thread_exists_clause(
    patterns: &[String],
    thread_column: &str,
    first_bind: usize,
) -> Option<(String, Vec<String>)> {
    let _ = first_bind;
    if RecipientMatcher::compile(patterns).is_empty() {
        return None;
    }
    Some((
        format!(
            "EXISTS (SELECT 1 FROM messages rm \
             JOIN accounts ra ON ra.id = rm.account_id \
             WHERE rm.thread_id = {thread_column} \
             AND flectar_recipient_match(rm.to_json, rm.cc_json, rm.bcc_json, ra.email))"
        ),
        Vec::new(),
    ))
}

/// Same filter for a search row already aliased as `m`. Binds nothing.
pub fn message_clause(patterns: &[String], first_bind: usize) -> Option<(String, Vec<String>)> {
    let _ = first_bind;
    if RecipientMatcher::compile(patterns).is_empty() {
        return None;
    }
    Some((
        "flectar_recipient_match(m.to_json, m.cc_json, m.bcc_json, \
         (SELECT email FROM accounts ra WHERE ra.id = m.account_id))"
            .to_owned(),
        Vec::new(),
    ))
}

/// How a business tab filters mail on top of the account set the caller applies.
///
/// `match_any` ORs the profile accounts (`or_account_ids`) with recipient
/// patterns and sender rules. Otherwise only `recipient_patterns` apply, and
/// the caller ANDs its own account filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailScope {
    pub recipient_patterns: Vec<String>,
    pub sender_addresses: Vec<String>,
    pub sender_names: Vec<String>,
    pub sender_domains: Vec<String>,
    pub or_account_ids: Vec<i64>,
    pub match_any: bool,
}

impl MailScope {
    pub fn recipients(patterns: Vec<String>) -> Self {
        Self {
            recipient_patterns: patterns,
            ..Self::default()
        }
    }
}

/// Thread predicate for `scope`. `None` means the scope adds nothing.
/// `match_any` with no arms matches nothing (`0`) rather than every thread.
pub fn thread_scope_clause(
    scope: &MailScope,
    thread_column: &str,
    account_column: &str,
    first_bind: usize,
) -> Option<(String, Vec<String>)> {
    if !scope.match_any {
        return thread_exists_clause(&scope.recipient_patterns, thread_column, first_bind);
    }
    any_clause(
        scope,
        first_bind,
        account_column,
        &format!("EXISTS (SELECT 1 FROM messages sm WHERE sm.thread_id = {thread_column} AND ("),
        "sm",
        true,
        thread_column,
    )
}

/// Same filter for a search row already aliased as `m`.
pub fn message_scope_clause(scope: &MailScope, first_bind: usize) -> Option<(String, Vec<String>)> {
    if !scope.match_any {
        return message_clause(&scope.recipient_patterns, first_bind);
    }
    any_clause(scope, first_bind, "m.account_id", "", "m", false, "")
}

fn any_clause(
    scope: &MailScope,
    first_bind: usize,
    account_column: &str,
    sender_exists_prefix: &str,
    sender_alias: &str,
    recipients_are_exists: bool,
    thread_column: &str,
) -> Option<(String, Vec<String>)> {
    let mut parts = Vec::new();
    let mut binds = Vec::new();
    let mut index = first_bind;
    if !scope.or_account_ids.is_empty() {
        let mut placeholders = Vec::with_capacity(scope.or_account_ids.len());
        for id in &scope.or_account_ids {
            placeholders.push(format!("?{index}"));
            binds.push(id.to_string());
            index += 1;
        }
        parts.push(format!("{account_column} IN ({})", placeholders.join(", ")));
    }
    if recipients_are_exists {
        if let Some((sql, likes)) =
            thread_exists_clause(&scope.recipient_patterns, thread_column, index)
        {
            index += likes.len();
            parts.push(sql);
            binds.extend(likes);
        }
    } else if let Some((sql, likes)) = message_clause(&scope.recipient_patterns, index) {
        index += likes.len();
        parts.push(sql);
        binds.extend(likes);
    }
    if let Some((sql, likes)) = sender_predicate(scope, sender_alias, index) {
        let sql = if sender_exists_prefix.is_empty() {
            format!("({sql})")
        } else {
            format!("{sender_exists_prefix}{sql}))")
        };
        parts.push(sql);
        binds.extend(likes);
    }
    if parts.is_empty() {
        return Some(("0".to_owned(), Vec::new()));
    }
    Some((format!("({})", parts.join(" OR ")), binds))
}

fn sender_predicate(
    scope: &MailScope,
    alias: &str,
    first_bind: usize,
) -> Option<(String, Vec<String>)> {
    let mut parts = Vec::new();
    let mut binds = Vec::new();
    let mut index = first_bind;
    for address in &scope.sender_addresses {
        let address = address.trim();
        if address.is_empty() || address.len() > 200 {
            continue;
        }
        let lowered = address.to_ascii_lowercase();
        if lowered.contains('*') || lowered.contains('?') {
            parts.push(format!(
                "LOWER({alias}.from_addr) LIKE ?{index} ESCAPE '\\'"
            ));
            binds.push(like_body(&lowered));
        } else {
            parts.push(format!("LOWER({alias}.from_addr) = ?{index}"));
            binds.push(lowered);
        }
        index += 1;
    }
    let names = scope
        .sender_names
        .iter()
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty() && name.len() <= 80)
        .collect::<Vec<_>>();
    let domains = scope
        .sender_domains
        .iter()
        .map(|domain| domain.trim().trim_start_matches('@').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty() && domain.len() <= 253 && !domain.contains(' '))
        .collect::<Vec<_>>();
    if !names.is_empty() && !domains.is_empty() {
        let mut name_placeholders = Vec::with_capacity(names.len());
        for name in &names {
            name_placeholders.push(format!("?{index}"));
            binds.push(name.clone());
            index += 1;
        }
        let mut domain_parts = Vec::with_capacity(domains.len());
        for domain in &domains {
            domain_parts.push(format!(
                "LOWER({alias}.from_addr) LIKE ?{index} ESCAPE '\\'"
            ));
            binds.push(format!("%@{}", like_literal(domain)));
            index += 1;
        }
        parts.push(format!(
            "(LOWER(TRIM({alias}.from_name)) IN ({}) AND ({}))",
            name_placeholders.join(", "),
            domain_parts.join(" OR ")
        ));
    }
    if parts.is_empty() {
        return None;
    }
    Some((parts.join(" OR "), binds))
}

fn like_literal(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_match_the_whole_address() {
        assert!(email_matches("tim@*", "Tim@Example.com"));
        assert!(email_matches("tim.mentink@*", "tim.mentink@corp.test"));
        assert!(email_matches("ohnage@*", "ohnage@x.test"));
        assert!(email_matches("tmentink@gmail.com", "Tmentink@Gmail.com"));
        assert!(!email_matches("tim@*", "tmentink@gmail.com"));
        assert!(!email_matches("tim@*", "nottim@example.com"));
        assert!(!email_matches("*@mentinkizm.com", "a@mentinkizm.com.evil"));
        assert!(email_matches("*@mentinkizm.com", "a@mentinkizm.com"));
        assert!(!email_matches("", "tim@example.com"));
        assert!(!email_matches("tim@*", ""));
        assert!(email_matches("ohnage?@*", "ohnage1@ug4l.com"));
        assert!(email_matches("ohnage1?@*", "ohnage10@ug4l.com"));
        assert!(!email_matches("ohnage?@*", "ohnage@ug4l.com"));
        assert!(!email_matches("ohnage?@*", "ohnage10@ug4l.com"));
    }

    #[test]
    fn exact_header_like_stops_at_the_address() {
        let likes = pattern_likes(&["tmentink@gmail.com".into(), "tim@*".into()]);
        assert_eq!(likes[0].address, "tmentink@gmail.com");
        assert_eq!(likes[0].header, "%\"email\":\"tmentink@gmail.com\"");
        assert_eq!(likes[1].address, "tim@%");
        assert_eq!(likes[1].header, "%\"email\":\"tim@%");
    }

    #[test]
    fn clause_is_one_function_call_with_no_binds() {
        let (sql, binds) = thread_exists_clause(&["tim@*".into()], "t.id", 4).unwrap();
        assert!(
            sql.contains("flectar_recipient_match(rm.to_json, rm.cc_json, rm.bcc_json, ra.email)")
        );
        assert!(sql.contains("rm.thread_id = t.id"));
        assert!(!sql.contains(" OR "));
        assert!(binds.is_empty());
        assert!(thread_exists_clause(&[], "t.id", 1).is_none());
    }

    #[test]
    fn exact_addresses_skip_globs_and_headers_are_case_insensitive() {
        let matcher = RecipientMatcher::compile(&[
            "tim.mentink@mentinkizm.com".into(),
            "tim@*".into(),
            "ohnage?@*".into(),
        ]);
        assert!(matcher.matches_email("Tim.Mentink@Mentinkizm.com"));
        assert!(matcher.matches_email("tim@elsewhere.test"));
        assert!(matcher.matches_email("ohnage1@ug4l.com"));
        assert!(!matcher.matches_email("ohnage10@ug4l.com"));
        assert!(!matcher.matches_email("other@mentinkizm.com"));
        let to_json = r#"[{"name":null,"email":"Tim.Mentink@Mentinkizm.com"}]"#;
        assert!(matcher.matches_headers(to_json, "[]", "[]", "hub@example.org"));
        assert!(matcher.matches_headers("[]", "[]", to_json, ""));
        assert!(matcher.matches_headers("[]", "[]", "[]", "tim@delivered.test"));
        assert!(!matcher.matches_headers(
            r#"[{"name":"Tim","email":"other@mentinkizm.com"}]"#,
            "[]",
            "[]",
            "hub@example.org"
        ));
    }

    #[test]
    fn hundreds_of_patterns_match_under_an_untrusted_schema() {
        let conn = crate::db::testutil::conn();
        conn.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA,
            false,
        )
        .unwrap();
        crate::db::testutil::seed_account(&conn);
        let (thread_id, message_id) =
            crate::db::testutil::seed_message(&conn, "someone@example.com", "Hello", false);
        conn.execute(
            "UPDATE messages SET to_json = ?1, cc_json = '[]', bcc_json = '[]' WHERE id = ?2",
            rusqlite::params![
                r#"[{"name":null,"email":"tim.mentink@mentinkizm.com"}]"#,
                message_id
            ],
        )
        .unwrap();
        let mut patterns: Vec<String> = (0..866)
            .map(|index| format!("other{index}@example.com"))
            .collect();
        patterns.push("tim.mentink@mentinkizm.com".into());
        install_recipient_matcher(&conn, &patterns).unwrap();
        let (sql, binds) = thread_exists_clause(&patterns, "t.id", 1).unwrap();
        assert!(binds.is_empty());
        let found: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM threads t WHERE {sql}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, 1);
        let only: i64 = conn
            .query_row("SELECT id FROM threads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(thread_id, only);

        let mut missed = patterns.clone();
        missed.pop();
        install_recipient_matcher(&conn, &missed).unwrap();
        let mut statement = conn
            .prepare_cached(&format!("SELECT COUNT(*) FROM threads t WHERE {sql}"))
            .unwrap();
        let found: i64 = statement.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(found, 0);

        install_recipient_matcher(&conn, &["tim.mentink@*".into()]).unwrap();
        let found: i64 = statement.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(found, 1);
    }

    #[test]
    fn question_mark_is_one_like_character() {
        let likes = pattern_likes(&["ohnage?@*".into()]);
        assert_eq!(likes[0].address, "ohnage_@%");
    }

    #[test]
    fn any_scope_ors_accounts_recipients_and_guarded_sender_names() {
        let (sql, binds) = thread_scope_clause(
            &MailScope {
                recipient_patterns: vec!["rook@helplearnhow.com".into()],
                sender_addresses: vec!["hi@p47.io".into()],
                sender_names: vec!["Mailroom".into(), "47 by Rook".into()],
                sender_domains: vec!["p47.io".into()],
                or_account_ids: vec![7, 8],
                match_any: true,
            },
            "t.id",
            "t.account_id",
            1,
        )
        .unwrap();
        assert!(sql.starts_with('('));
        assert!(sql.contains("t.account_id IN (?1, ?2)"));
        assert!(sql.contains("sm.from_addr"));
        assert!(sql.contains("sm.from_name"));
        assert!(sql.contains(" OR "));
        assert_eq!(binds[0], "7");
        assert_eq!(binds[1], "8");
        assert!(binds.iter().any(|bind| bind == "hi@p47.io"));
        assert!(binds.iter().any(|bind| bind == "mailroom"));
        assert!(binds.iter().any(|bind| bind == "%@p47.io"));
        assert!(
            thread_scope_clause(
                &MailScope {
                    match_any: true,
                    ..MailScope::default()
                },
                "t.id",
                "t.account_id",
                1,
            )
            .unwrap()
            .0 == "0"
        );
    }
}
