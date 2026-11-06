//! Background download of message text for Gmail and JMAP.
//!
//! Headers are stored by each account's sync first. This loop then fills
//! bodies for rows already on disk, newest first. JMAP and Gmail each take
//! their own newest page, so a large Gmail backlog cannot shrink a JMAP
//! `Email/get`. JMAP asks for a few hundred messages in one call
//! (`bodyValues`). Gmail still fetches one message at a time, a few at once.
//! Opening a cached message reads SQLite and does not ask the server again.
//! The progress banner is the UI's own two-second poll of the saved counts;
//! this loop reports a batch, not each message.

use super::engine::SyncCtx;
use crate::db::repo;
use crate::models::{MailHistory, now_ms};
use std::collections::{HashMap, HashSet};

/// Newest uncached rows pulled in one pass. One JMAP account turns this into
/// a handful of `Email/get` calls.
const PAGE: i64 = 500;
/// Accounts whose bodies are fetched at the same time. Each JMAP account uses
/// one connection and writes one transaction per batch.
const ACCOUNTS_AT_ONCE: usize = 4;
/// Gmail has no multi-get for message bodies. Keep a page small and a few
/// requests in flight so it cannot stall the JMAP accounts behind it.
const GMAIL_PAGE: usize = 32;
const GMAIL_CONCURRENCY: usize = 4;
const IDLE: std::time::Duration = std::time::Duration::from_secs(20);

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
            if cached == 0 {
                tokio::time::sleep(IDLE).await;
            } else {
                tokio::task::yield_now().await;
            }
        }
    });
}

async fn download_page(ctx: &SyncCtx) -> crate::error::Result<u64> {
    let now = now_ms();
    let (jmap, gmail) = ctx
        .db
        .read(move |conn| {
            let one_month = MailHistory::OneMonth.cutoff_ms_at(now).unwrap_or(0);
            let three_months = MailHistory::ThreeMonths.cutoff_ms_at(now).unwrap_or(0);
            let one_year = MailHistory::OneYear.cutoff_ms_at(now).unwrap_or(0);
            let six_months = MailHistory::SixMonths.cutoff_ms_at(now).unwrap_or(0);
            // Separate queues. One shared newest-N page let ~100k uncached
            // Gmail rows crowd JMAP down to the one or two JMAP ids that
            // happened to be in that page.
            let jmap = repo::messages::newest_uncached_remote_bodies(
                conn,
                now,
                one_month,
                three_months,
                one_year,
                six_months,
                PAGE,
                repo::messages::RemoteBodySource::Jmap,
            )?;
            let gmail = repo::messages::newest_uncached_remote_bodies(
                conn,
                now,
                one_month,
                three_months,
                one_year,
                six_months,
                i64::try_from(GMAIL_PAGE).unwrap_or(32),
                repo::messages::RemoteBodySource::Gmail,
            )?;
            Ok((jmap, gmail))
        })
        .await?;
    if jmap.is_empty() && gmail.is_empty() {
        return Ok(0);
    }

    let mut jobs = tokio::task::JoinSet::new();
    if !jmap.is_empty() {
        let ctx = ctx.clone();
        jobs.spawn(async move { download_grouped(&ctx, jmap).await });
    }
    if !gmail.is_empty() {
        let ctx = ctx.clone();
        jobs.spawn(async move { download_grouped(&ctx, gmail).await });
    }
    let mut cached = 0u64;
    while let Some(joined) = jobs.join_next().await {
        match joined {
            Ok(Ok(count)) => cached += count,
            Ok(Err(error)) => tracing::warn!(error = %error, "body backfill page failed"),
            Err(error) => tracing::warn!(error = %error, "body backfill task failed"),
        }
    }
    Ok(cached)
}

async fn download_grouped(
    ctx: &SyncCtx,
    page: Vec<repo::messages::UncachedRemoteBody>,
) -> crate::error::Result<u64> {
    let mut by_account: HashMap<i64, Vec<repo::messages::UncachedRemoteBody>> = HashMap::new();
    let mut order = Vec::new();
    for row in page {
        if !by_account.contains_key(&row.account_id) {
            order.push(row.account_id);
        }
        by_account.entry(row.account_id).or_default().push(row);
    }

    let mut cached = 0u64;
    for group in order.chunks(ACCOUNTS_AT_ONCE) {
        let mut jobs = tokio::task::JoinSet::new();
        for account_id in group.iter().copied() {
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
    let cached = if gmail {
        download_gmail(ctx, account_id, &config, rows).await?
    } else {
        download_jmap(ctx, account_id, &config, rows).await?
    };
    if cached > 0 {
        emit_progress(ctx, account_id).await;
    }
    Ok(cached)
}

async fn download_jmap(
    ctx: &SyncCtx,
    account_id: i64,
    config: &crate::models::AccountConfig,
    rows: Vec<repo::messages::UncachedRemoteBody>,
) -> crate::error::Result<u64> {
    let ids = rows.iter().map(|row| row.message_id).collect::<Vec<_>>();
    let mut cached = 0u64;
    let mut failed = 0usize;
    let mut sample = String::new();
    for (message_id, result) in crate::jmap::sync::cache_message_bodies(ctx, config, &ids).await {
        match result {
            Ok(()) => cached += 1,
            Err(error) => {
                failed += 1;
                if sample.is_empty() {
                    sample = error.clone();
                }
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
    if failed > 0 {
        crate::sync::engine::log_sync_problem(
            &ctx.paths.data_dir,
            "jmap body",
            account_id,
            failed,
            &sample,
        );
    }
    Ok(cached)
}

async fn download_gmail(
    ctx: &SyncCtx,
    account_id: i64,
    config: &crate::models::AccountConfig,
    rows: Vec<repo::messages::UncachedRemoteBody>,
) -> crate::error::Result<u64> {
    let ids = rows.iter().map(|row| row.message_id).collect::<Vec<_>>();
    let claimed = ctx
        .db
        .write(move |conn| repo::messages::begin_body_fetches(conn, &ids))
        .await
        .unwrap_or_default()
        .into_iter()
        .collect::<HashSet<_>>();
    let pending = rows
        .into_iter()
        .filter(|row| claimed.contains(&row.message_id))
        .collect::<Vec<_>>();
    let started = std::time::Instant::now();
    let mut cached = 0u64;
    let mut failed = 0usize;
    let mut sample = String::new();
    for group in pending.chunks(GMAIL_CONCURRENCY) {
        let mut jobs = tokio::task::JoinSet::new();
        for row in group {
            let ctx = ctx.clone();
            let config = config.clone();
            let message_id = row.message_id;
            let remote_id = row.remote_id.clone();
            jobs.spawn(async move {
                let saved = super::gmail::cache_message_body(&ctx, &config, &remote_id).await;
                (message_id, saved)
            });
        }
        while let Some(joined) = jobs.join_next().await {
            let Ok((message_id, saved)) = joined else {
                continue;
            };
            match saved {
                Ok(()) => cached += 1,
                Err(error) => {
                    failed += 1;
                    let detail = error.to_string();
                    if sample.is_empty() {
                        sample.clone_from(&detail);
                    }
                    tracing::warn!(
                        account_id,
                        message_id,
                        error = %detail,
                        "Gmail body was not saved"
                    );
                    let _ = ctx
                        .db
                        .write(move |conn| repo::messages::cancel_body_fetch(conn, message_id))
                        .await;
                    // The message row stays. A 404 postpones this id; it does
                    // not delete mail or mark the body cached.
                    record_failure(ctx, message_id, &detail).await;
                }
            }
        }
    }
    crate::sync::engine::log_sync_batch(
        &ctx.paths.data_dir,
        "gmail body",
        account_id,
        usize::try_from(cached).unwrap_or(0),
        started.elapsed(),
    );
    if failed > 0 {
        crate::sync::engine::log_sync_problem(
            &ctx.paths.data_dir,
            "gmail body",
            account_id,
            failed,
            &sample,
        );
    }
    Ok(cached)
}

async fn emit_progress(ctx: &SyncCtx, account_id: i64) {
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
                total: u64::try_from(progress.done.saturating_add(progress.pending)).unwrap_or(0),
            },
        ));
    }
}

async fn record_failure(ctx: &SyncCtx, message_id: i64, detail: &str) {
    let detail = detail.to_owned();
    let recorded = ctx
        .db
        .write(move |conn| {
            let now = now_ms();
            let attempts = repo::sync_failures::content_attempts(conn, message_id)?;
            let delay = body_retry_delay_ms(attempts, &detail);
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

/// How long to wait before the background downloader tries this message again.
/// A confirmed provider 404 waits longer than a network error so missing ids
/// do not sit at the head of the newest page every minute. The row is kept,
/// and opening the message can still fetch it immediately.
pub(crate) fn body_retry_delay_ms(attempts: i64, detail: &str) -> i64 {
    if repo::sync_failures::is_provider_missing(detail) {
        if attempts <= 0 {
            15 * 60_000
        } else {
            24 * 60 * 60_000
        }
    } else {
        match attempts {
            0..=4 => 60_000,
            5..=19 => 15 * 60_000,
            _ => 24 * 60 * 60_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::body_retry_delay_ms;

    #[test]
    fn a_gmail_404_leaves_the_queue_without_becoming_permanent() {
        let missing = "not found: Gmail resource: Requested entity was not found.";
        assert_eq!(body_retry_delay_ms(0, missing), 15 * 60_000);
        assert_eq!(body_retry_delay_ms(1, missing), 24 * 60 * 60_000);
        assert_eq!(body_retry_delay_ms(11, missing), 24 * 60 * 60_000);
        assert_eq!(body_retry_delay_ms(0, "network error: timed out"), 60_000);
        assert_eq!(
            body_retry_delay_ms(0, "the server no longer has this message"),
            15 * 60_000
        );
    }
}
