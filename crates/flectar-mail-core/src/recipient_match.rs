//! Wildcard matching for business-tab recipient and sender filters.
//!
//! A pattern is an address with `*` meaning any sequence and `?` meaning one
//! character, compared without regard to case. `tim@*` matches
//! `tim@example.com` and not `tmentink@gmail.com`. `ohnage?@*` matches
//! `ohnage1@x.com` and not `ohnage@x.com`. A recipient filter matches To, Cc,
//! Bcc, persisted receiver delivery headers, and (for legacy business rules)
//! the connected account login as a fallback. That login is not Delivered-To.
//! Exact address inboxes omit that fallback. Literal addresses are an exact set. The SQL clause
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
        self.matches_delivery_headers(to_json, cc_json, bcc_json, "[]", mailbox)
    }

    /// Legacy rule match: real recipients plus the connected account login.
    pub fn matches_delivery_headers(
        &self,
        to_json: &str,
        cc_json: &str,
        bcc_json: &str,
        delivery_json: &str,
        mailbox: &str,
    ) -> bool {
        if self.is_empty() {
            return false;
        }
        self.matches_email(mailbox)
            || json_has_match(to_json, self)
            || json_has_match(cc_json, self)
            || json_has_match(bcc_json, self)
            || json_has_match(delivery_json, self)
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

/// An exact delivery-address inbox. Source accounts constrain storage, not
/// evidence of the address. None searches all sources; Some([]) matches none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddressInboxScope {
    pub address: String,
    pub account_ids: Option<Vec<i64>>,
}

struct AddressInboxMatcher {
    sources: HashMap<String, Option<HashSet<i64>>>,
}

impl AddressInboxMatcher {
    fn compile(inboxes: Vec<AddressInboxScope>) -> Self {
        let mut sources: HashMap<String, Option<HashSet<i64>>> = HashMap::new();
        for inbox in inboxes {
            let address = inbox.address.trim().to_ascii_lowercase();
            if address.is_empty() || address.len() > 320 {
                continue;
            }
            let allowed = inbox
                .account_ids
                .map(|ids| ids.into_iter().filter(|id| *id > 0).collect::<HashSet<_>>());
            if let Some(existing) = sources.get_mut(&address) {
                if allowed.is_none() {
                    *existing = None;
                } else if let (Some(existing), Some(allowed)) =
                    (existing.as_mut(), allowed.as_ref())
                {
                    existing.extend(allowed);
                }
            } else {
                sources.insert(address, allowed);
            }
        }
        Self { sources }
    }

    fn matches(&self, json: &str, account_id: Option<i64>) -> bool {
        serde_json::from_str::<Vec<crate::models::Address>>(json).is_ok_and(|addresses| {
            addresses.iter().any(|recipient| {
                self.sources
                    .get(&recipient.email.trim().to_ascii_lowercase())
                    .is_some_and(|allowed| {
                        allowed
                            .as_ref()
                            .is_none_or(|ids| account_id.is_some_and(|id| ids.contains(&id)))
                    })
            })
        })
    }
}

/// Receiving-address classification for an automatic business group. Exact
/// registry overrides are resolved before business domains, per address.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddressGroupScope {
    pub profile_ids: Vec<String>,
    pub rules: Vec<crate::models::AddressRule>,
}

struct AddressGroupMatcher {
    groups: Vec<AddressGroupScope>,
}

impl AddressGroupMatcher {
    fn matches(&self, json: &str) -> bool {
        serde_json::from_str::<Vec<crate::models::Address>>(json).is_ok_and(|addresses| {
            addresses.iter().any(|recipient| {
                self.groups.iter().any(|group| {
                    crate::brand_registry::classify_receiving_address(
                        &group.rules,
                        &recipient.email,
                    )
                    .is_some_and(|profile| group.profile_ids.contains(&profile))
                })
            })
        })
    }
}

fn install_address_inbox_matcher(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "flectar_address_inbox_match",
        6,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            let matcher = ctx.get_or_create_aux(
                5,
                |value| -> std::result::Result<
                    AddressInboxMatcher,
                    Box<dyn std::error::Error + Send + Sync>,
                > {
                    Ok(AddressInboxMatcher::compile(serde_json::from_str(
                        value.as_str()?,
                    )?))
                },
            )?;
            let account_id = ctx.get::<Option<i64>>(4)?;
            for index in 0..4 {
                if matcher.matches(
                    &ctx.get::<Option<String>>(index)?.unwrap_or_default(),
                    account_id,
                ) {
                    return Ok(true);
                }
            }
            Ok(false)
        },
    )?;
    conn.create_scalar_function(
        "flectar_address_group_match",
        5,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            let matcher = ctx.get_or_create_aux(
                4,
                |value| -> std::result::Result<
                    AddressGroupMatcher,
                    Box<dyn std::error::Error + Send + Sync>,
                > {
                    Ok(AddressGroupMatcher {
                        groups: serde_json::from_str(value.as_str()?)?,
                    })
                },
            )?;
            for index in 0..4 {
                if matcher.matches(&ctx.get::<Option<String>>(index)?.unwrap_or_default()) {
                    return Ok(true);
                }
            }
            Ok(false)
        },
    )
}

struct MatcherSlot {
    matcher: RwLock<Arc<RecipientMatcher>>,
}

fn matcher_slots() -> &'static Mutex<HashMap<usize, Arc<MatcherSlot>>> {
    static SLOTS: OnceLock<Mutex<HashMap<usize, Arc<MatcherSlot>>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register `flectar_recipient_match(to_json, cc_json, bcc_json, delivery_json, mailbox)` on
/// `conn`. The function reads a slot updated in place, so a cached statement
/// keeps calling the patterns installed for the latest query on that
/// connection. An empty pattern list leaves the clause unused and is a no-op.
pub fn install_recipient_matcher(
    conn: &rusqlite::Connection,
    patterns: &[String],
) -> rusqlite::Result<()> {
    install_address_inbox_matcher(conn)?;
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
        5,
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
            let delivery_json = ctx.get::<Option<String>>(3)?.unwrap_or_default();
            let mailbox = ctx.get::<Option<String>>(4)?.unwrap_or_default();
            Ok(matcher.matches_delivery_headers(
                &to_json,
                &cc_json,
                &bcc_json,
                &delivery_json,
                &mailbox,
            ))
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
             AND flectar_recipient_match(rm.to_json, rm.cc_json, rm.bcc_json, rm.delivery_recipients_json, ra.email))"
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
        "flectar_recipient_match(m.to_json, m.cc_json, m.bcc_json, m.delivery_recipients_json, \
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
    /// Account restriction for the legacy arm only, before alias arms are ORed.
    pub base_account_ids: Option<Vec<i64>>,
    pub address_inboxes: Vec<AddressInboxScope>,
    pub address_groups: Vec<AddressGroupScope>,
    /// Shared-mailbox messages whose To, Cc, Bcc, delivery headers, or From
    /// name one of these addresses. The account list is the shared storage.
    pub evidenced_mail: Vec<EvidencedMail>,
    /// Shared-mailbox messages that are not Tim-only. A message that names
    /// only an omit address is left out; one that names no owned address, or
    /// also names a keep address, stays.
    pub remainder_mail: Option<RemainderMail>,
}

/// Addresses that identify mail inside specific stored accounts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvidencedMail {
    pub account_ids: Vec<i64>,
    pub addresses: Vec<String>,
}

/// What remains of a shared mailbox after evidenced addresses are removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemainderMail {
    pub account_ids: Vec<i64>,
    pub omit_addresses: Vec<String>,
    pub keep_addresses: Vec<String>,
}

impl MailScope {
    /// True when this scope separates messages inside a shared mailbox.
    pub fn splits_shared_mailbox(&self) -> bool {
        self.evidenced_mail
            .iter()
            .any(|mail| !mail.account_ids.is_empty() && !mail.addresses.is_empty())
            || self
                .remainder_mail
                .as_ref()
                .is_some_and(|mail| !mail.account_ids.is_empty() && !mail.omit_addresses.is_empty())
    }

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
    let legacy = if !scope.match_any {
        thread_exists_clause(&scope.recipient_patterns, thread_column, first_bind)
    } else {
        any_clause(
            scope,
            first_bind,
            account_column,
            &format!(
                "EXISTS (SELECT 1 FROM messages sm WHERE sm.thread_id = {thread_column} AND ("
            ),
            "sm",
            true,
            thread_column,
        )
    };
    compose_address_inboxes(
        scope,
        legacy,
        account_column,
        Some(thread_column),
        first_bind,
    )
}

/// Same filter for a search row already aliased as `m`.
pub fn message_scope_clause(scope: &MailScope, first_bind: usize) -> Option<(String, Vec<String>)> {
    let legacy = if !scope.match_any {
        message_clause(&scope.recipient_patterns, first_bind)
    } else {
        any_clause(scope, first_bind, "m.account_id", "", "m", false, "")
    };
    compose_address_inboxes(scope, legacy, "m.account_id", None, first_bind)
}

fn compose_address_inboxes(
    scope: &MailScope,
    legacy: Option<(String, Vec<String>)>,
    account_column: &str,
    thread_column: Option<&str>,
    first_bind: usize,
) -> Option<(String, Vec<String>)> {
    let (mut clause, mut binds) = legacy
        .map(|(sql, binds)| (Some(sql), binds))
        .unwrap_or_default();
    if let Some(ids) = &scope.base_account_ids {
        let restriction = if ids.is_empty() {
            "0".to_owned()
        } else {
            let placeholders = ids
                .iter()
                .map(|id| {
                    let index = first_bind + binds.len();
                    binds.push(id.to_string());
                    format!("?{index}")
                })
                .collect::<Vec<_>>();
            format!("{account_column} IN ({})", placeholders.join(", "))
        };
        clause = Some(clause.map_or_else(
            || restriction.clone(),
            |old| format!("({restriction} AND {old})"),
        ));
    }
    if !scope.address_inboxes.is_empty() {
        let index = first_bind + binds.len();
        binds.push(
            serde_json::to_string(&scope.address_inboxes).expect("address inbox scopes serialize"),
        );
        let alias = if thread_column.is_some() { "am" } else { "m" };
        let predicate = format!(
            "flectar_address_inbox_match({alias}.to_json, {alias}.cc_json, {alias}.bcc_json, {alias}.delivery_recipients_json, {alias}.account_id, ?{index})"
        );
        let address_clause = thread_column.map_or_else(|| predicate.clone(), |thread| {
            format!("EXISTS (SELECT 1 FROM messages am WHERE am.thread_id = {thread} AND {predicate})")
        });
        clause = Some(clause.map_or_else(
            || address_clause.clone(),
            |old| format!("({old} OR {address_clause})"),
        ));
    }
    if !scope.address_groups.is_empty() {
        let index = first_bind + binds.len();
        binds.push(
            serde_json::to_string(&scope.address_groups).expect("address group scopes serialize"),
        );
        let alias = if thread_column.is_some() { "gm" } else { "m" };
        let predicate = format!(
            "flectar_address_group_match({alias}.to_json, {alias}.cc_json, {alias}.bcc_json, {alias}.delivery_recipients_json, ?{index})"
        );
        let group_clause = thread_column.map_or_else(|| predicate.clone(), |thread| {
            format!("EXISTS (SELECT 1 FROM messages gm WHERE gm.thread_id = {thread} AND {predicate})")
        });
        clause = Some(clause.map_or_else(
            || group_clause.clone(),
            |old| format!("({old} OR {group_clause})"),
        ));
    }
    if let Some(sql) = shared_mailbox_clause(scope, thread_column, &mut binds, first_bind) {
        clause = Some(clause.map_or_else(|| sql.clone(), |old| format!("({old} OR {sql})")));
    }
    clause.map(|sql| (sql, binds))
}

fn shared_mailbox_clause(
    scope: &MailScope,
    thread_column: Option<&str>,
    binds: &mut Vec<String>,
    first_bind: usize,
) -> Option<String> {
    let alias = if thread_column.is_some() { "sm" } else { "m" };
    let mut parts = Vec::new();
    for mail in &scope.evidenced_mail {
        if let Some(predicate) =
            evidenced_predicate(alias, &mail.account_ids, &mail.addresses, binds, first_bind)
        {
            parts.push(predicate);
        }
    }
    if let Some(mail) = &scope.remainder_mail
        && let Some(predicate) = remainder_predicate(alias, mail, binds, first_bind)
    {
        parts.push(predicate);
    }
    if parts.is_empty() {
        return None;
    }
    let predicate = format!("({})", parts.join(" OR "));
    Some(thread_column.map_or_else(
        || predicate.clone(),
        |thread| {
            format!(
                "EXISTS (SELECT 1 FROM messages sm WHERE sm.thread_id = {thread} AND {predicate})"
            )
        },
    ))
}

fn evidenced_predicate(
    alias: &str,
    account_ids: &[i64],
    addresses: &[String],
    binds: &mut Vec<String>,
    first_bind: usize,
) -> Option<String> {
    let addresses = normalized_addresses(addresses);
    if account_ids.is_empty() || addresses.is_empty() {
        return None;
    }
    let accounts = account_in(alias, account_ids, binds, first_bind);
    let header = address_header_match(alias, account_ids, &addresses, binds, first_bind);
    let from = from_in(alias, &addresses, binds, first_bind);
    Some(format!("({accounts} AND ({header} OR {from}))"))
}

fn remainder_predicate(
    alias: &str,
    mail: &RemainderMail,
    binds: &mut Vec<String>,
    first_bind: usize,
) -> Option<String> {
    let omit = normalized_addresses(&mail.omit_addresses);
    if mail.account_ids.is_empty() || omit.is_empty() {
        return None;
    }
    let accounts = account_in(alias, &mail.account_ids, binds, first_bind);
    let omitted = format!(
        "({} OR {})",
        address_header_match(alias, &mail.account_ids, &omit, binds, first_bind),
        from_in(alias, &omit, binds, first_bind)
    );
    let keep = normalized_addresses(&mail.keep_addresses);
    let kept = if keep.is_empty() {
        "0".to_owned()
    } else {
        format!(
            "({} OR {})",
            address_header_match(alias, &mail.account_ids, &keep, binds, first_bind),
            from_in(alias, &keep, binds, first_bind)
        )
    };
    Some(format!("({accounts} AND (NOT {omitted} OR {kept}))"))
}

fn account_in(
    alias: &str,
    account_ids: &[i64],
    binds: &mut Vec<String>,
    first_bind: usize,
) -> String {
    let mut placeholders = Vec::new();
    for id in account_ids {
        let index = first_bind + binds.len();
        binds.push(id.to_string());
        placeholders.push(format!("?{index}"));
    }
    format!("{alias}.account_id IN ({})", placeholders.join(", "))
}

fn address_header_match(
    alias: &str,
    account_ids: &[i64],
    addresses: &[String],
    binds: &mut Vec<String>,
    first_bind: usize,
) -> String {
    let inboxes = addresses
        .iter()
        .map(|address| AddressInboxScope {
            address: address.clone(),
            account_ids: Some(account_ids.to_vec()),
        })
        .collect::<Vec<_>>();
    let index = first_bind + binds.len();
    binds.push(serde_json::to_string(&inboxes).expect("shared address scopes serialize"));
    format!(
        "flectar_address_inbox_match({alias}.to_json, {alias}.cc_json, {alias}.bcc_json, {alias}.delivery_recipients_json, {alias}.account_id, ?{index})"
    )
}

fn from_in(
    alias: &str,
    addresses: &[String],
    binds: &mut Vec<String>,
    first_bind: usize,
) -> String {
    // A giant IN (...) list blocks the UI thread while SQLite prepares the
    // statement. json_each keeps one bind regardless of catalog size.
    if addresses.len() > 64 {
        let index = first_bind + binds.len();
        binds.push(serde_json::to_string(addresses).expect("from addresses serialize"));
        return format!(
            "EXISTS (SELECT 1 FROM json_each(?{index}) WHERE value = LOWER(COALESCE({alias}.from_addr, '')))"
        );
    }
    let mut placeholders = Vec::new();
    for address in addresses {
        let index = first_bind + binds.len();
        binds.push(address.clone());
        placeholders.push(format!("?{index}"));
    }
    format!(
        "LOWER(COALESCE({alias}.from_addr, '')) IN ({})",
        placeholders.join(", ")
    )
}

fn normalized_addresses(addresses: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for address in addresses {
        let address = address.trim().to_ascii_lowercase();
        if address.is_empty()
            || address.len() > 320
            || out.iter().any(|kept: &String| kept == &address)
        {
            continue;
        }
        out.push(address);
    }
    out
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
            sql.contains("flectar_recipient_match(rm.to_json, rm.cc_json, rm.bcc_json, rm.delivery_recipients_json, ra.email)")
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
                ..MailScope::default()
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

    #[test]
    fn delivery_recipient_is_real_evidence_and_mailbox_fallback_stays_legacy_only() {
        let conn = crate::db::testutil::conn();
        crate::db::testutil::seed_account(&conn);
        let (_, message_id) =
            crate::db::testutil::seed_message(&conn, "sender@example.test", "Delivery", false);
        conn.execute("UPDATE messages SET to_json='[]',cc_json='[]',bcc_json='[]',delivery_recipients_json=?1 WHERE id=?2", rusqlite::params![r#"[{"name":null,"email":"alias@example.test"}]"#, message_id]).unwrap();
        let scope = MailScope {
            address_inboxes: vec![AddressInboxScope {
                address: "ALIAS@example.test".into(),
                account_ids: Some(vec![1]),
            }],
            ..MailScope::default()
        };
        install_recipient_matcher(&conn, &[]).unwrap();
        let (clause, bind) = message_scope_clause(&scope, 1).unwrap();
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM messages m WHERE {clause}"),
                rusqlite::params_from_iter(bind.iter()),
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        conn.execute(
            "UPDATE messages SET delivery_recipients_json='[]' WHERE id=?1",
            [message_id],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM messages m WHERE {clause}"),
                rusqlite::params_from_iter(bind.iter()),
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        // Even an exact account login is not proof of that delivery recipient.
        let login: String = conn
            .query_row("SELECT email FROM accounts WHERE id=1", [], |r| r.get(0))
            .unwrap();
        let primary_scope = MailScope {
            address_inboxes: vec![AddressInboxScope {
                address: login,
                account_ids: None,
            }],
            ..MailScope::default()
        };
        let (clause, bind) = message_scope_clause(&primary_scope, 1).unwrap();
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM messages m WHERE {clause}"),
                rusqlite::params_from_iter(bind.iter()),
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn alias_source_arms_do_not_cross_product_and_empty_sources_match_nothing() {
        let conn = crate::db::testutil::conn();
        crate::db::testutil::seed_account(&conn);
        let (thread, message) = crate::db::testutil::seed_message(
            &conn,
            "sender@example.test",
            "Source constrained",
            false,
        );
        conn.execute(
            "UPDATE messages SET to_json=?1, cc_json='[]', bcc_json='[]' WHERE id=?2",
            rusqlite::params![r#"[{"name":null,"email":"one@example.test"}]"#, message],
        )
        .unwrap();
        install_recipient_matcher(&conn, &[]).unwrap();
        let mut scope = MailScope {
            address_inboxes: vec![
                AddressInboxScope {
                    address: "one@example.test".into(),
                    account_ids: Some(vec![2]),
                },
                AddressInboxScope {
                    address: "two@example.test".into(),
                    account_ids: Some(vec![1]),
                },
            ],
            ..MailScope::default()
        };
        let matching = |scope: &MailScope| {
            let (clause, bind) = thread_scope_clause(scope, "t.id", "t.account_id", 1).unwrap();
            conn.query_row(
                &format!("SELECT COUNT(*) FROM threads t WHERE {clause}"),
                rusqlite::params_from_iter(bind.iter()),
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        };
        assert_eq!(matching(&scope), 0);
        scope.address_inboxes[0].account_ids = Some(vec![1]);
        assert_eq!(matching(&scope), 1);
        scope.address_inboxes[0].account_ids = Some(vec![]);
        assert_eq!(matching(&scope), 0);
        scope.address_inboxes[0].account_ids = None;
        assert_eq!(matching(&scope), 1);
        assert!(thread > 0);
        // A legacy account restriction may not eliminate an independent alias arm.
        scope.base_account_ids = Some(vec![]);
        scope.recipient_patterns = vec!["not-this-recipient@example.test".into()];
        assert_eq!(matching(&scope), 1);
    }

    #[test]
    fn shared_mailbox_keeps_unmarked_mail_and_splits_named_addresses() {
        let conn = crate::db::testutil::conn();
        crate::db::testutil::seed_account(&conn);
        let insert = |to: &str, from: &str| {
            let (_, id) = crate::db::testutil::seed_message(&conn, from, "Shared", false);
            conn.execute(
                "UPDATE messages SET to_json=?1, cc_json='[]', bcc_json='[]', delivery_recipients_json='[]' WHERE id=?2",
                rusqlite::params![to, id],
            )
            .unwrap();
            id
        };
        let tim_to = insert(
            r#"[{"email":"tim@mentinkmemorial.org"}]"#,
            "outside@example.test",
        );
        let contact_to = insert(
            r#"[{"email":"contact@mentinkmemorial.org"}]"#,
            "outside@example.test",
        );
        let unmarked = insert("[]", "outside@example.test");
        let tim_from = insert(
            r#"[{"email":"reader@example.test"}]"#,
            "tim@mentinkmemorial.org",
        );
        let both = insert(
            r#"[{"email":"tim@mentinkmemorial.org"},{"email":"contact@mentinkmemorial.org"}]"#,
            "outside@example.test",
        );
        install_recipient_matcher(&conn, &[]).unwrap();
        let tim = MailScope {
            match_any: true,
            or_account_ids: vec![2],
            evidenced_mail: vec![EvidencedMail {
                account_ids: vec![1],
                addresses: vec!["tim@mentinkmemorial.org".into()],
            }],
            ..MailScope::default()
        };
        let media = MailScope {
            match_any: true,
            or_account_ids: vec![4],
            remainder_mail: Some(RemainderMail {
                account_ids: vec![1],
                omit_addresses: vec!["tim@mentinkmemorial.org".into()],
                keep_addresses: vec!["contact@mentinkmemorial.org".into()],
            }),
            ..MailScope::default()
        };
        let ids = |scope: &MailScope| {
            let (clause, bind) = message_scope_clause(scope, 1).unwrap();
            let mut statement = conn
                .prepare(&format!(
                    "SELECT id FROM messages m WHERE {clause} ORDER BY id"
                ))
                .unwrap();
            let rows = statement
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap();
            rows.collect::<Result<Vec<_>, _>>().unwrap()
        };
        assert_eq!(ids(&tim), vec![tim_to, tim_from, both]);
        assert_eq!(ids(&media), vec![contact_to, unmarked, both]);
    }

    #[test]
    fn legacy_recipient_match_includes_delivery_headers_and_preserves_to_cc_bcc() {
        let matcher = RecipientMatcher::compile(&["hidden@example.test".into()]);
        let address = r#"[{"email":"HIDDEN@example.test","name":null}]"#;
        assert!(matcher.matches_delivery_headers(
            "[]",
            "[]",
            "[]",
            address,
            "mailbox@example.test"
        ));
        for (to, cc, bcc) in [
            (address, "[]", "[]"),
            ("[]", address, "[]"),
            ("[]", "[]", address),
        ] {
            assert!(matcher.matches_delivery_headers(to, cc, bcc, "[]", "mailbox@example.test"));
        }
        assert!(!matcher.matches_delivery_headers("[]", "[]", "[]", "[]", "mailbox@example.test"));
    }

    #[test]
    fn receiving_group_uses_exact_overrides_unknown_domains_and_each_actual_recipient() {
        let conn = crate::db::testutil::conn();
        crate::db::testutil::seed_account(&conn);
        let (_, message) =
            crate::db::testutil::seed_message(&conn, "sender@example.test", "Groups", false);
        let rules = vec![
            crate::models::AddressRule {
                profile_id: "business".into(),
                addresses: vec![],
                domains: vec!["business.test".into()],
            },
            crate::models::AddressRule {
                profile_id: "other".into(),
                addresses: vec!["foreign@business.test".into()],
                domains: vec!["other.test".into()],
            },
        ];
        install_recipient_matcher(&conn, &[]).unwrap();
        let count = |profile: &str| {
            let scope = MailScope {
                address_groups: vec![AddressGroupScope {
                    profile_ids: vec![profile.into()],
                    rules: rules.clone(),
                }],
                ..MailScope::default()
            };
            let (clause, binds) = message_scope_clause(&scope, 1).unwrap();
            conn.query_row(
                &format!("SELECT COUNT(*) FROM messages m WHERE {clause}"),
                rusqlite::params_from_iter(binds.iter()),
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
        };
        let recipients = |to: &str, cc: &str| {
            conn.execute("UPDATE messages SET to_json=?1,cc_json=?2,bcc_json='[]',delivery_recipients_json='[]' WHERE id=?3", rusqlite::params![to, cc, message]).unwrap();
        };
        recipients(r#"[{"email":"foreign@business.test"}]"#, "[]");
        assert_eq!(count("business"), 0);
        assert_eq!(count("other"), 1);
        recipients(r#"[{"email":"unknown@business.test"}]"#, "[]");
        assert_eq!(count("business"), 1);
        assert_eq!(count("other"), 0);
        recipients(r#"[{"email":"unrelated@elsewhere.test"}]"#, "[]");
        assert_eq!(count("business"), 0);
        assert_eq!(count("other"), 0);
        recipients(
            r#"[{"email":"foreign@business.test"}]"#,
            r#"[{"email":"unknown@business.test"}]"#,
        );
        assert_eq!(count("business"), 1);
        assert_eq!(count("other"), 1);
    }
}
