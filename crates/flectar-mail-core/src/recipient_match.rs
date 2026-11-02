//! Wildcard matching for business-tab recipient and sender filters.
//!
//! A pattern is an address with `*` meaning any sequence and `?` meaning one
//! character, compared without regard to case. `tim@*` matches
//! `tim@example.com` and not `tmentink@gmail.com`. `ohnage?@*` matches
//! `ohnage1@x.com` and not `ohnage@x.com`. The SQL clause treats a matching
//! mailbox address as Delivered-To (that is the address the message was
//! delivered into) and also matches `To` and `Cc`. Sender display names match
//! only together with one of the scope's own domains.

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

/// Bound values for one pattern: the mailbox address, then the same address
/// as it appears in a `{"email":"..."}` JSON list (To or Cc).
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

/// `EXISTS` predicate on `thread_column` (for example `t.id`). Placeholders
/// start at `first_bind` (1-based). `None` when there is nothing to filter.
pub fn thread_exists_clause(
    patterns: &[String],
    thread_column: &str,
    first_bind: usize,
) -> Option<(String, Vec<String>)> {
    let likes = pattern_likes(patterns);
    if likes.is_empty() {
        return None;
    }
    let mut binds = Vec::with_capacity(likes.len() * 3);
    let mut groups = Vec::with_capacity(likes.len());
    let mut index = first_bind;
    for like in &likes {
        let address = index;
        let header_to = index + 1;
        let header_cc = index + 2;
        index += 3;
        groups.push(format!(
            "(LOWER(ra.email) LIKE ?{address} ESCAPE '\\' \
             OR LOWER(rm.to_json) LIKE ?{header_to} ESCAPE '\\' \
             OR LOWER(rm.cc_json) LIKE ?{header_cc} ESCAPE '\\')"
        ));
        binds.push(like.address.clone());
        binds.push(like.header.clone());
        binds.push(like.header.clone());
    }
    let sql = format!(
        "EXISTS (SELECT 1 FROM messages rm \
         JOIN accounts ra ON ra.id = rm.account_id \
         WHERE rm.thread_id = {thread_column} AND ({}))",
        groups.join(" OR ")
    );
    Some((sql, binds))
}

/// Same filter for a search row already aliased as `m`.
pub fn message_clause(patterns: &[String], first_bind: usize) -> Option<(String, Vec<String>)> {
    let likes = pattern_likes(patterns);
    if likes.is_empty() {
        return None;
    }
    let mut binds = Vec::with_capacity(likes.len() * 3);
    let mut groups = Vec::with_capacity(likes.len());
    let mut index = first_bind;
    for like in &likes {
        let address = index;
        let header_to = index + 1;
        let header_cc = index + 2;
        index += 3;
        groups.push(format!(
            "(LOWER((SELECT email FROM accounts ra WHERE ra.id = m.account_id)) LIKE ?{address} ESCAPE '\\' \
             OR LOWER(m.to_json) LIKE ?{header_to} ESCAPE '\\' \
             OR LOWER(m.cc_json) LIKE ?{header_cc} ESCAPE '\\')"
        ));
        binds.push(like.address.clone());
        binds.push(like.header.clone());
        binds.push(like.header.clone());
    }
    Some((format!("({})", groups.join(" OR ")), binds))
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
    fn clause_numbers_placeholders_from_the_caller() {
        let (sql, binds) = thread_exists_clause(&["tim@*".into()], "t.id", 4).unwrap();
        assert!(sql.contains("?4"));
        assert!(sql.contains("?6"));
        assert!(sql.contains("rm.thread_id = t.id"));
        assert_eq!(binds.len(), 3);
        assert!(thread_exists_clause(&[], "t.id", 1).is_none());
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
