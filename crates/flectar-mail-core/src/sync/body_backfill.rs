//! Background download of message text for Gmail and JMAP.
//!
//! Those protocols used to fetch a body only when the message was opened, so a
//! mailbox full of cached headers still showed a loading wait on every open.
//! This loop stores the newest uncached messages first, a few accounts at a
//! time. IMAP already has its own body pool. Opening a message whose text is
//! cached reads SQLite and does not ask the server again.

use super::engine::SyncCtx;
use crate::db::repo;
use crate::models::{MailHistory, now_ms};
use std::collections::HashMap;

const PAGE: i64 = 8;
const IDLE: std::time::Duration = std::time::Duration::from_secs(20);
const BETWEEN_PAGES: std::time::Duration = std::time::Duration::from_millis(250);

pub(crate) fn spawn(ctx: SyncCtx) {
    tokio::spawn(async move {
        loop {
            let cached = match download_page(&ctx).await {
                Ok(cached) => cached,
                Err(error) => {
                    tracing::warn!(error = %error, "body backfill page failed");
                    0
                }
            };
            let pause = if cached == 0 { IDLE } else { BETWEEN_PAGES };
            tokio::time::sleep(pause).await;
        }
    });
}

async fn download_page(ctx: &SyncCtx) -> crate::error::Result<u64> {
    let now = now_ms();
    let page = ctx
        .db
        .read(move |conn| {
            repo::messages::newest_uncached_remote_bodies(
                conn,
                now,
                MailHistory::OneMonth.cutoff_ms_at(now).unwrap_or(0),
                MailHistory::ThreeMonths.cutoff_ms_at(now).unwrap_or(0),
                MailHistory::OneYear.cutoff_ms_at(now).unwrap_or(0),
                MailHistory::SixMonths.cutoff_ms_at(now).unwrap_or(0),
                PAGE,
            )
        })
        .await?;
    if page.is_empty() {
        return Ok(0);
    }

    let mut by_account: HashMap<i64, Vec<repo::messages::UncachedRemoteBody>> = HashMap::new();
    let mut order = Vec::new();
    for row in page {
        if !by_account.contains_key(&row.account_id) {
            order.push(row.account_id);
        }
        by_account.entry(row.account_id).or_default().push(row);
    }

    let mut cached = 0u64;
    // Two accounts at a time. A broken account records its own retry and does
    // not stop the other one, or the next page.
    for pair in order.chunks(2) {
        let mut jobs = tokio::task::JoinSet::new();
        for account_id in pair.iter().copied() {
            let Some(rows) = by_account.remove(&account_id) else {
                continue;
            };
            let ctx = ctx.clone();
            jobs.spawn(async move { download_account(&ctx, account_id, rows).await });
        }
        while let Some(joined) = jobs.join_next().await {
            match joined {
                Ok(Ok(count)) => cached += count,
                Ok(Err(error)) => tracing::warn!(error = %error, "body backfill account failed"),
                Err(error) => tracing::warn!(error = %error, "body backfill task failed"),
            }
        }
    }
    Ok(cached)
}

async fn download_account(
    ctx: &SyncCtx,
    account_id: i64,
    rows: Vec<repo::messages::UncachedRemoteBody>,
) -> crate::error::Result<u64> {
    let Some(config) = ctx
        .db
        .read(move |conn| repo::accounts::get_config(conn, account_id))
        .await?
    else {
        return Ok(0);
    };
    let gmail = rows.first().is_some_and(|row| row.gmail);
    let mut cached = 0u64;
    if gmail {
        for row in &rows {
            let message_id = row.message_id;
            let claimed = ctx
                .db
                .write(move |conn| repo::messages::begin_body_fetch(conn, message_id))
                .await
                .unwrap_or(false);
            if !claimed {
                continue;
            }
            match super::gmail::cache_message_body(ctx, &config, &row.remote_id).await {
                Ok(()) => cached += 1,
                Err(error) => {
                    tracing::warn!(
                        account_id,
                        message_id,
                        error = %error,
                        "Gmail body was not saved"
                    );
                    let _ = ctx
                        .db
                        .write(move |conn| repo::messages::cancel_body_fetch(conn, message_id))
                        .await;
                    record_failure(ctx, message_id, &error.to_string()).await;
                }
            }
        }
    } else {
        let ids = rows.iter().map(|row| row.message_id).collect::<Vec<_>>();
        for (message_id, result) in
            crate::jmap::sync::cache_message_bodies(ctx, &config, &ids).await
        {
            match result {
                Ok(()) => cached += 1,
                Err(error) => {
                    tracing::warn!(
                        account_id,
                        message_id,
                        error = %error,
                        "JMAP body was not saved"
                    );
                    record_failure(ctx, message_id, &error).await;
                }
            }
        }
    }
    if cached > 0 {
        let now = now_ms();
        let progress = ctx
            .db
            .read(move |conn| {
                repo::messages::body_cache_progress(
                    conn,
                    Some(&[account_id]),
                    MailHistory::OneMonth.cutoff_ms_at(now).unwrap_or(0),
                    MailHistory::ThreeMonths.cutoff_ms_at(now).unwrap_or(0),
                    MailHistory::OneYear.cutoff_ms_at(now).unwrap_or(0),
                    MailHistory::SixMonths.cutoff_ms_at(now).unwrap_or(0),
                )
            })
            .await
            .ok();
        if let Some(progress) = progress {
            ctx.bus.emit(crate::events::CoreEvent::SyncProgress(
                crate::events::SyncProgress {
                    account_id,
                    folder: String::new(),
                    phase: "bodies".into(),
                    done: u64::try_from(progress.done).unwrap_or(0),
                    total: u64::try_from(progress.done.saturating_add(progress.pending))
                        .unwrap_or(0),
                },
            ));
        }
    }
    Ok(cached)
}

async fn record_failure(ctx: &SyncCtx, message_id: i64, detail: &str) {
    let detail = detail.to_owned();
    let recorded = ctx
        .db
        .write(move |conn| {
            let now = now_ms();
            let attempts = repo::sync_failures::content_attempts(conn, message_id)?;
            let delay = match attempts {
                0..=4 => 60_000,
                5..=19 => 15 * 60_000,
                _ => 24 * 60 * 60_000,
            };
            repo::sync_failures::record_content_at(
                conn,
                message_id,
                Some(now + delay),
                &detail,
                now,
            )
        })
        .await;
    if let Err(error) = recorded {
        tracing::warn!(message_id, error = %error, "could not postpone a body retry");
    }
}
