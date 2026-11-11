//! Guards local tab-switch budgets against the owner's live shape:
//! ~108k messages, ~14k folders, 300+ accounts, one Gmail account holding
//! ~106k messages with All Mail duplicates, and ~1.7k messages across JMAP.
//!
//! Does not touch the owner's real DB. Builds a synthetic SQLite mailbox.

use flectar_mail_core::db::{Db, repo};
use flectar_mail_core::models::{MailGrouping, View};
use std::time::Instant;

const GMAIL_ACCOUNT: i64 = 1;
const COMPANY_ACCOUNTS: i64 = 300;
const TOTAL_ACCOUNTS: i64 = 1 + COMPANY_ACCOUNTS; // Gmail + JMAP hubs
const FOLDERS_PER_HUB: i64 = 46;
const GMAIL_MESSAGES: i64 = 106_000;
const GMAIL_INBOX: i64 = 1_400;
const JMAP_MESSAGES: i64 = 1_700;
const PAGE_LIMIT: i64 = 25;
const FIRST_PAINT_MS: u128 = 200;
const FULL_LIST_MS: u128 = 500;

fn settings_json() -> &'static str {
    r#"{"connection":{"incomingSecurity":"tls","smtpSecurity":"tls"}}"#
}

async fn seed_live_shape(db: &Db) {
    db.write(|conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO accounts (id,email,provider,auth_kind,username,incoming_host,incoming_port,smtp_host,smtp_port,created_at,mail_protocol,settings_json)
             VALUES (?1,'tmentink@gmail.com','gmail','oauth2','tmentink@gmail.com','imap.gmail.com',993,'smtp.gmail.com',587,0,'gmail',?2)",
            rusqlite::params![GMAIL_ACCOUNT, settings_json()],
        )?;
        tx.execute(
            "INSERT INTO folders (id,account_id,path,role) VALUES (1,1,'INBOX','inbox'), (2,1,'[Gmail]/All Mail','all')",
            [],
        )?;
        let mut next_folder = 3i64;
        for account in 2..=TOTAL_ACCOUNTS {
            tx.execute(
                "INSERT INTO accounts (id,email,provider,auth_kind,username,incoming_host,incoming_port,smtp_host,smtp_port,created_at,mail_protocol,settings_json)
                 VALUES (?1,?2,'generic','password',?2,'mail.example.test',993,'mail.example.test',587,0,'jmap',?3)",
                rusqlite::params![
                    account,
                    format!("hub{account}@pixelatedcities.test"),
                    settings_json()
                ],
            )?;
            tx.execute(
                "INSERT INTO folders (id,account_id,path,role,jmap_id) VALUES (?1,?2,'INBOX','inbox',?3)",
                rusqlite::params![next_folder, account, format!("inbox-{account}")],
            )?;
            next_folder += 1;
            for topic in 0..(FOLDERS_PER_HUB - 1) {
                tx.execute(
                    "INSERT INTO folders (id,account_id,path,role,jmap_id)
                     VALUES (?1,?2,?3,NULL,?4)",
                    rusqlite::params![
                        next_folder,
                        account,
                        format!("Topics/Topic-{topic}"),
                        format!("topic-{account}-{topic}")
                    ],
                )?;
                next_folder += 1;
            }
        }

        let now = flectar_mail_core::models::now_ms();
        {
            let mut threads = tx.prepare(
                "INSERT INTO threads (id,account_id,subject_norm,last_message_at,message_count,unread_count,snippet)
                 VALUES (?1,?2,'subject',?3,1,?4,'snippet')",
            )?;
            let mut messages = tx.prepare(
                "INSERT INTO messages (id,account_id,thread_id,folder_id,uid,message_id,subject,from_addr,date,is_read)
                 VALUES (?1,?2,?1,?3,?1,?4,'subject','sender@example.test',?5,?6)",
            )?;
            let mut links = tx.prepare(
                "INSERT INTO message_folders (message_id, folder_id) VALUES (?1, ?2)",
            )?;
            for id in 1..=GMAIL_MESSAGES {
                let in_inbox = id <= GMAIL_INBOX;
                let folder_id = if in_inbox { 1 } else { 2 };
                let unread = i64::from(in_inbox && id % 5 == 0);
                let date = now - (GMAIL_MESSAGES - id) * 1_000;
                threads.execute(rusqlite::params![id, GMAIL_ACCOUNT, date, unread])?;
                messages.execute(rusqlite::params![
                    id,
                    GMAIL_ACCOUNT,
                    folder_id,
                    format!("gmail-{id}"),
                    date,
                    1 - unread
                ])?;
                // Every INBOX message is also linked in All Mail.
                if in_inbox {
                    links.execute(rusqlite::params![id, 2i64])?;
                }
            }
            let mut id = GMAIL_MESSAGES;
            let mut remaining = JMAP_MESSAGES;
            let mut account = 2i64;
            while remaining > 0 {
                id += 1;
                let date = now - remaining * 2_000;
                let inbox_id: i64 = tx.query_row(
                    "SELECT id FROM folders WHERE account_id = ?1 AND role = 'inbox'",
                    [account],
                    |row| row.get(0),
                )?;
                threads.execute(rusqlite::params![id, account, date, 0i64])?;
                messages.execute(rusqlite::params![
                    id,
                    account,
                    inbox_id,
                    format!("jmap-{id}"),
                    date,
                    1i64
                ])?;
                remaining -= 1;
                account = if account >= TOTAL_ACCOUNTS {
                    2
                } else {
                    account + 1
                };
            }
        }
        tx.commit()?;
        Ok(())
    })
    .await
    .expect("seed live-shaped fixture");
}

/// Real tab switches hit a warm SQLite page cache; cold-open timings after a
/// 100k-row seed are not the product path. Touch the hot indexes once.
async fn warm_local_reads(db: &Db, account_ids: &[i64]) {
    let ids = account_ids.to_vec();
    let _ = db
        .read({
            let ids = ids.clone();
            move |conn| {
                let _ = repo::folders::list_info_for_accounts(conn, &ids)?;
                let _ = repo::threads::list(
                    conn,
                    &repo::threads::ListArgs {
                        view: View::Inbox,
                        tab: None,
                        account_id: if ids.len() == 1 { Some(ids[0]) } else { None },
                        account_ids: if ids.len() == 1 {
                            None
                        } else {
                            Some(ids.clone())
                        },
                        folder_id: None,
                        folder_ids: None,
                        label_ids: None,
                        cursor: None,
                        limit: PAGE_LIMIT,
                    },
                )?;
                let _ = repo::counts::folder_unread_counts_for(conn, Some(&ids), None)?;
                Ok(())
            }
        })
        .await;
}

#[tokio::test]
async fn company_tab_switch_stays_within_local_budgets_at_live_scale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open(&dir.path().join("budget.db")).expect("open db");
    seed_live_shape(&db).await;

    let company_ids: Vec<i64> = (2..=31).collect(); // one company tab (~30 hub accounts)
    warm_local_reads(&db, &company_ids).await;
    let folder_started = Instant::now();
    let folders = db
        .read({
            let company_ids = company_ids.clone();
            move |conn| repo::folders::list_info_for_accounts(conn, &company_ids)
        })
        .await
        .expect("scoped folders");
    let folder_ms = folder_started.elapsed().as_millis();
    assert!(
        folders.len() < 2_000,
        "company tab must not load the full 14k-folder catalog; got {}",
        folders.len()
    );
    assert!(
        folders
            .iter()
            .all(|folder| folder.account_id != GMAIL_ACCOUNT),
        "company tab must never touch the Gmail corpus"
    );

    let list_started = Instant::now();
    let page = db
        .read({
            let company_ids = company_ids.clone();
            move |conn| {
                repo::threads::list(
                    conn,
                    &repo::threads::ListArgs {
                        view: View::Inbox,
                        tab: None,
                        account_id: None,
                        account_ids: Some(company_ids),
                        folder_id: None,
                        folder_ids: None,
                        label_ids: None,
                        cursor: None,
                        limit: PAGE_LIMIT,
                    },
                )
            }
        })
        .await
        .expect("company inbox page");
    let list_ms = list_started.elapsed().as_millis();

    let badge_started = Instant::now();
    let badges = db
        .read({
            let company_ids = company_ids.clone();
            move |conn| {
                repo::counts::mailbox_badge_counts_filtered(
                    conn,
                    MailGrouping::Conversations,
                    &flectar_mail_core::recipient_match::MailScope {
                        base_account_ids: Some(company_ids),
                        ..Default::default()
                    },
                )
            }
        })
        .await
        .expect("company badges");
    let badge_ms = badge_started.elapsed().as_millis();

    let nonempty_started = Instant::now();
    let with_mail = db
        .read({
            let company_ids = company_ids.clone();
            move |conn| repo::folders::folder_ids_with_mail(conn, &company_ids)
        })
        .await
        .expect("folders with mail");
    let nonempty_ms = nonempty_started.elapsed().as_millis();

    let first_paint = folder_ms + list_ms;
    let full_usable = first_paint + badge_ms + nonempty_ms;
    eprintln!(
        "company tab: folders={} ({} ms), list={} rows ({} ms), badges={} ms, nonempty={} ({} ms); first_paint={} ms full={} ms",
        folders.len(),
        folder_ms,
        page.threads.len(),
        list_ms,
        badge_ms,
        with_mail.len(),
        nonempty_ms,
        first_paint,
        full_usable
    );
    assert!(
        first_paint < FIRST_PAINT_MS,
        "company first paint {first_paint} ms exceeds {FIRST_PAINT_MS} ms budget"
    );
    assert!(
        full_usable < FULL_LIST_MS,
        "company full usable {full_usable} ms exceeds {FULL_LIST_MS} ms budget"
    );
    assert!(
        badges
            .iter()
            .filter(|row| company_ids.contains(&row.account_id))
            .all(|row| row.inbox >= 0),
        "scoped badges should return"
    );
}

#[tokio::test]
async fn tm_tab_with_gmail_stays_within_local_budgets_and_skips_all_mail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open(&dir.path().join("budget-tm.db")).expect("open db");
    seed_live_shape(&db).await;

    let tm_ids = vec![GMAIL_ACCOUNT];
    warm_local_reads(&db, &tm_ids).await;
    let folder_started = Instant::now();
    let folders = db
        .read(move |conn| repo::folders::list_info_for_accounts(conn, &tm_ids))
        .await
        .expect("gmail folders");
    let folder_ms = folder_started.elapsed().as_millis();
    assert!(
        folders
            .iter()
            .any(|folder| folder.role.as_deref() == Some("all")),
        "Gmail All Mail exists in the store"
    );

    let list_started = Instant::now();
    let page = db
        .read(|conn| {
            repo::threads::list(
                conn,
                &repo::threads::ListArgs {
                    view: View::Inbox,
                    tab: None,
                    account_id: Some(GMAIL_ACCOUNT),
                    account_ids: None,
                    folder_id: None,
                    folder_ids: None,
                    label_ids: None,
                    cursor: None,
                    limit: PAGE_LIMIT,
                },
            )
        })
        .await
        .expect("gmail inbox page");
    let list_ms = list_started.elapsed().as_millis();
    assert_eq!(page.threads.len(), PAGE_LIMIT as usize);

    let unreads_started = Instant::now();
    let unreads = db
        .read(|conn| repo::counts::folder_unread_counts_for(conn, Some(&[GMAIL_ACCOUNT]), None))
        .await
        .expect("folder unreads");
    let unreads_ms = unreads_started.elapsed().as_millis();
    let all_mail_id = folders
        .iter()
        .find(|folder| folder.role.as_deref() == Some("all"))
        .map(|folder| folder.id)
        .expect("all mail id");
    assert!(
        !unreads.contains_key(&all_mail_id),
        "All Mail must not be counted alongside INBOX"
    );

    let first_paint = folder_ms + list_ms;
    let full_usable = first_paint + unreads_ms;
    eprintln!(
        "TM/Gmail tab: folders={} ({} ms), list={} rows ({} ms), unreads={} ms; first_paint={} ms full={} ms",
        folders.len(),
        folder_ms,
        page.threads.len(),
        list_ms,
        unreads_ms,
        first_paint,
        full_usable
    );
    assert!(
        first_paint < FIRST_PAINT_MS,
        "TM first paint {first_paint} ms exceeds {FIRST_PAINT_MS} ms budget"
    );
    assert!(
        full_usable < FULL_LIST_MS,
        "TM full usable {full_usable} ms exceeds {FULL_LIST_MS} ms budget"
    );
}

#[test]
fn tiered_sync_backs_quiet_accounts_off() {
    use flectar_mail_core::sync::engine::tiered_sync_interval;
    use std::time::Duration;
    let base = Duration::from_secs(5 * 60);
    assert_eq!(tiered_sync_interval(base, Duration::from_secs(60)), base);
    assert_eq!(
        tiered_sync_interval(base, Duration::from_secs(2 * 24 * 60 * 60)),
        base * 4
    );
    assert_eq!(
        tiered_sync_interval(base, Duration::from_secs(10 * 24 * 60 * 60)),
        base * 48
    );
    assert_eq!(
        tiered_sync_interval(
            Duration::from_secs(15 * 60),
            Duration::from_secs(10 * 24 * 60 * 60)
        ),
        Duration::from_secs(6 * 60 * 60)
    );
}
