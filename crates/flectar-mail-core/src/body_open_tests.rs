//! Opening a message after load reads only local storage.
use crate::accounts::credentials::DevelopmentFileCredentialStore;
use crate::sync::engine::AccountHandle;
use crate::{Core, config::Paths, db::testutil};
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn core_with_cached_message(html: &'static str) -> (tempfile::TempDir, Core, i64) {
    let temp = tempfile::tempdir().unwrap();
    let credentials = Arc::new(DevelopmentFileCredentialStore::new(
        temp.path().join("credentials.json"),
    ));
    let core = Core::start_mail_ui_with_credentials(Paths::for_tests(temp.path()), credentials)
        .await
        .unwrap();
    let id = core
        .db
        .write(move |conn| {
            testutil::seed_account(conn);
            let (_, id) = testutil::seed_message(conn, "notifications@github.com", "Run failed", true);
            conn.execute(
                "UPDATE messages SET body_state = 'cached' WHERE id = ?1",
                [id],
            )?;
            conn.execute(
                "INSERT INTO message_bodies (message_id, text_body, html_body) VALUES (?1, 'Run failed', ?2)",
                rusqlite::params![id, html],
            )?;
            Ok(id)
        })
        .await
        .unwrap();
    (temp, core, id)
}

#[tokio::test]
async fn opening_a_cached_message_makes_no_network_request() {
    let (_temp, core, id) =
        core_with_cached_message("<table><tr><td><p>Run failed</p></td></tr></table>").await;
    let (handle, mut sync_rx, mut fetch_rx) = AccountHandle::recording(1);
    core.handles.write().await.insert(1, handle);

    let started = Instant::now();
    let detail = core.get_body(id).await.unwrap();
    let elapsed = started.elapsed();
    assert!(detail.html_body.unwrap().contains("Run failed"));
    assert!(
        elapsed < Duration::from_millis(100),
        "cached open took {elapsed:?}"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        sync_rx.try_recv().is_err(),
        "cached open reached the sync actor"
    );
    assert!(
        fetch_rx.try_recv().is_err(),
        "cached open requested a download"
    );
}

#[tokio::test]
async fn missing_inline_images_never_delay_opening_a_cached_message() {
    let (_temp, core, id) =
        core_with_cached_message("<p>Logo <img src=\"cid:logo@test\"></p>").await;
    core.db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO attachments (message_id, part_id, filename, mime_type, size, content_id, is_inline)
                 VALUES (?1, '2', 'logo.png', 'image/png', 64, 'logo@test', 1)",
                [id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    // The download is captured and never answered, as on a slow server.
    let (handle, _sync_rx, mut fetch_rx) = AccountHandle::recording(1);
    core.handles.write().await.insert(1, handle);

    let detail = tokio::time::timeout(Duration::from_millis(500), core.get_body(id))
        .await
        .expect("opening waited on the inline image download")
        .unwrap();
    assert!(detail.html_body.unwrap().contains("cid:logo@test"));
    // The image still downloads once, in the background.
    let request = tokio::time::timeout(Duration::from_secs(2), fetch_rx.recv())
        .await
        .expect("missing inline image was not requested");
    assert!(request.is_some());
}
