//! How the plugin ends without leaving a file half written: the driver's `shutdown`, and this
//! plugin's program stopped as SpelaionDB stops it. In a test program of its own: `shutdown`
//! waits for every file this process has open, so no other test may hold one meanwhile.

use std::path::PathBuf;
use std::time::Duration;

use futures_util::TryStreamExt;
use spelaion_driver_api::{ConnectParams, Connection, Driver, ExecEvent, ExecOptions, Value};
use spelaion_driver_duckdb::DuckdbDriver;

async fn run(conn: &dyn Connection, sql: &str) -> Vec<ExecEvent> {
    conn.execute(sql, &ExecOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .try_collect()
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

#[tokio::test]
async fn shutdown_returns_once_every_file_is_closed_with_what_it_held() {
    // What the plugin's program does before it ends: the SDK calls `shutdown` once every
    // connection is gone. Each file closes on the thread of its last connection, writing what
    // DuckDB holds into it; ended before that, the program could leave the file corrupt.
    let path = std::env::temp_dir().join(format!(
        "spelaion-duck-shutdown-{}.duckdb",
        std::process::id()
    ));
    let wal = PathBuf::from(format!("{}.wal", path.display()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&wal);
    let params = ConnectParams {
        database: path.display().to_string(),
        ..Default::default()
    };
    let driver = DuckdbDriver {
        create_if_missing: true,
    };
    for round in 0..5 {
        // Two connections share the file, as a tab and the explorer do.
        let writer = driver.connect(&params).await.expect("opening DuckDB");
        let reader = driver.connect(&params).await.expect("opening DuckDB");
        run(writer.as_ref(), "create table if not exists t (n bigint)").await;
        run(
            writer.as_ref(),
            &format!(
                "insert into t select * from range({round} * 200000, {} * 200000)",
                round + 1
            ),
        )
        .await;
        run(reader.as_ref(), "select count(*) from t").await;
        drop(writer);
        drop(reader);
        tokio::time::timeout(Duration::from_secs(60), driver.shutdown())
            .await
            .expect("shutdown returns");
        // Closed: what DuckDB held is in the file, and no log of it is left to replay.
        assert!(!wal.exists(), "round {round}: the file was not closed");
        let conn = driver.connect(&params).await.expect("opening DuckDB");
        let counted = run(conn.as_ref(), "select count(*) from t").await;
        let Some(ExecEvent::Batch(batch)) = counted
            .iter()
            .find(|event| matches!(event, ExecEvent::Batch(b) if b.num_rows() > 0))
        else {
            panic!("no count: {counted:?}");
        };
        let ExecEvent::Columns(schema) = &counted[0] else {
            panic!("no columns: {counted:?}");
        };
        let count = Value::from_array(batch.column(0).as_ref(), 0, schema.columns[0].logical)
            .expect("a value");
        assert_eq!(count.to_string(), ((round + 1) * 200_000).to_string());
        drop(conn);
        driver.shutdown().await;
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&wal);
}

/// Bounded: a plugin that stops answering fails the test in a minute instead of hanging it.
async fn within<T>(
    what: impl std::future::Future<Output = T>,
) -> Result<T, tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(60), what).await
}

/// The one value of a `select count(*)`.
async fn count(conn: &dyn Connection) -> String {
    let counted = run(conn, "select count(*) from t").await;
    let ExecEvent::Columns(schema) = &counted[0] else {
        panic!("no columns: {counted:?}");
    };
    let Some(ExecEvent::Batch(batch)) = counted
        .iter()
        .find(|event| matches!(event, ExecEvent::Batch(b) if b.num_rows() > 0))
    else {
        panic!("no count: {counted:?}");
    };
    Value::from_array(batch.column(0).as_ref(), 0, schema.columns[0].logical)
        .expect("a value")
        .to_string()
}

#[tokio::test]
async fn the_plugin_stopped_after_it_wrote_leaves_its_file_closed_and_whole() {
    // This program, as SpelaionDB runs it: stopping it closes its input, so it closes its
    // files before it ends, also with its sessions still open. Nothing is left for DuckDB to
    // replay, and every reopen, by a new process, finds every row.
    let path = std::env::temp_dir().join(format!(
        "spelaion-duck-stopped-{}.duckdb",
        std::process::id()
    ));
    let wal = PathBuf::from(format!("{}.wal", path.display()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&wal);
    let params = ConnectParams {
        database: path.display().to_string(),
        ..Default::default()
    };
    let plugin = spelaion_plugin_host::Plugin::new(env!("CARGO_BIN_EXE_spelaiondb-plugin-duckdb"));
    let driver = plugin.driver("duckdb");
    driver
        .create_database(&params.database)
        .await
        .expect("creating the file");
    for round in 0..20 {
        let writer = within(driver.connect(&params))
            .await
            .expect("in time")
            .expect("opening DuckDB");
        let reader = within(driver.connect(&params))
            .await
            .expect("in time")
            .expect("opening DuckDB");
        run(writer.as_ref(), "create table if not exists t (n bigint)").await;
        run(
            writer.as_ref(),
            &format!(
                "insert into t select * from range({round} * 50000, {} * 50000)",
                round + 1
            ),
        )
        .await;
        assert_eq!(
            count(reader.as_ref()).await,
            ((round + 1) * 50_000).to_string()
        );
        if round % 2 == 0 {
            // Closed first, as a tab that is closed before quitting.
            drop(writer);
            drop(reader);
            within(plugin.stop()).await.expect("stopped in time");
        } else {
            // Still open, as the sessions of an app that quits.
            within(plugin.stop()).await.expect("stopped in time");
            drop(writer);
            drop(reader);
        }
        assert!(
            !wal.exists(),
            "round {round}: the plugin ended before closing the file"
        );
        // The next connection starts the program again.
        let conn = within(driver.connect(&params))
            .await
            .expect("in time")
            .expect("reopening DuckDB");
        assert_eq!(
            count(conn.as_ref()).await,
            ((round + 1) * 50_000).to_string()
        );
        drop(conn);
    }
    within(plugin.stop()).await.expect("stopped in time");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&wal);
}
