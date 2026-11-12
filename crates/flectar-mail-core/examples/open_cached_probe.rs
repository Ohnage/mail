//! Time opening cached messages the way the reading pane does (thread
//! outline, then the selected body) on a copy of a real profile. Credentials
//! are unavailable, so no account can reach a server.
//!
//! Usage: open_cached_probe <profile dir> [--in-place] [--writer-busy-ms=N]

use flectar_mail_core::{Core, accounts::credentials::UnavailableCredentialStore, config::Paths};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn stats(label: &str, mut ms: Vec<f64>) {
    if ms.is_empty() {
        println!("{label}: no samples");
        return;
    }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    println!(
        "{label}: n={} median={:.1}ms p95={:.1}ms max={:.1}ms over100ms={}",
        ms.len(),
        pick(0.5),
        pick(0.95),
        ms[ms.len() - 1],
        ms.iter().filter(|v| **v > 100.0).count()
    );
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = std::env::args()
        .nth(1)
        .ok_or("pass a profile data directory")?;
    let in_place = std::env::args().any(|arg| arg == "--in-place");
    let writer_busy_ms = std::env::args()
        .find_map(|arg| arg.strip_prefix("--writer-busy-ms=").map(str::to_owned))
        .map(|value| value.parse::<u64>())
        .transpose()?;
    let temp = tempfile::tempdir()?;
    let data_dir = if in_place {
        Path::new(&source).to_path_buf()
    } else {
        std::fs::copy(
            Path::new(&source).join("flectar-mail.db"),
            temp.path().join("flectar-mail.db"),
        )?;
        temp.path().to_path_buf()
    };
    let started = Instant::now();
    let core = Core::start_mail_ui_with_credentials(
        Paths::new(data_dir, temp.path().join("cache")),
        Arc::new(UnavailableCredentialStore),
    )
    .await?;
    println!(
        "core_start_ms={:.0}",
        started.elapsed().as_secs_f64() * 1000.0
    );

    let (recent, inline): (Vec<(i64, i64)>, Vec<(i64, i64)>) = core
        .db
        .read(|conn| {
            let collect = |sql: &str| -> rusqlite::Result<Vec<(i64, i64)>> {
                let mut stmt = conn.prepare(sql)?;
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect()
            };
            Ok((
                collect(
                    "SELECT id, thread_id FROM messages
                     WHERE body_state = 'cached' AND thread_id IS NOT NULL AND is_draft = 0
                     ORDER BY date DESC LIMIT 200",
                )?,
                collect(
                    "SELECT DISTINCT m.id, m.thread_id FROM messages m
                     JOIN attachments a ON a.message_id = m.id
                     WHERE m.body_state = 'cached' AND m.thread_id IS NOT NULL
                       AND a.content_id IS NOT NULL AND a.file_path IS NULL
                       AND (a.part_id IS NOT NULL OR a.imap_section IS NOT NULL)
                     ORDER BY m.date DESC LIMIT 50",
                )?,
            ))
        })
        .await?;

    // Sync commits on the one writer connection. Model a steady stream of
    // batches so an open that waits for a write shows that wait.
    let busy = writer_busy_ms.map(|busy_ms| {
        let core = core.clone();
        tokio::spawn(async move {
            loop {
                let _ = core
                    .db
                    .write(move |_| {
                        std::thread::sleep(Duration::from_millis(busy_ms));
                        Ok(())
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    });

    for (label, rows) in [("recent_cached", &recent), ("inline_cid_missing", &inline)] {
        let mut samples = Vec::with_capacity(rows.len());
        let mut errors = 0;
        let mut timeouts = 0;
        for (id, thread_id) in rows.iter().copied() {
            let started = Instant::now();
            let opened = tokio::time::timeout(Duration::from_secs(10), async {
                core.get_thread_outline(thread_id).await?;
                core.get_body(id).await
            })
            .await;
            match opened {
                Ok(Ok(_)) => samples.push(started.elapsed().as_secs_f64() * 1000.0),
                Ok(Err(_)) => errors += 1,
                Err(_) => {
                    timeouts += 1;
                    samples.push(10_000.0);
                }
            }
        }
        stats(label, samples);
        println!("{label}_errors={errors} {label}_timeouts_10s={timeouts}");
    }
    if let Some(busy) = busy {
        busy.abort();
    }
    Ok(())
}
