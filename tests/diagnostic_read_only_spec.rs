use std::{collections::BTreeMap, path::Path, process::Command};

use sqlx::{
    Connection, Executor,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};

fn command(data: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_simple-blog"));
    command.arg("--data-dir").arg(data);
    command.env("SIMPLE_BLOG_LOG_FORMAT", "json");
    command
}

async fn initialize(data: &Path) {
    let output = command(data).arg("init").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let options = SqliteConnectOptions::new().filename(data.join("simple-blog.sqlite3"));
    let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
    connection
        .execute("PRAGMA wal_checkpoint(TRUNCATE)")
        .await
        .unwrap();
    connection.close().await.unwrap();
}

fn snapshot(data: &Path) -> BTreeMap<String, String> {
    walkdir::WalkDir::new(data)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            (
                entry
                    .path()
                    .strip_prefix(data)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                blake3::hash(&std::fs::read(entry.path()).unwrap())
                    .to_hex()
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn doctor_reports_old_schema_without_migration_backup_or_byte_changes() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    initialize(&data).await;
    let options = SqliteConnectOptions::new().filename(data.join("simple-blog.sqlite3"));
    let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
    connection
        .execute("DROP TABLE media_variants")
        .await
        .unwrap();
    connection
        .execute("DELETE FROM _sqlx_migrations WHERE version = 2")
        .await
        .unwrap();
    connection.close().await.unwrap();
    let before = snapshot(&data);
    let output = command(&data).args(["doctor", "--json"]).output().unwrap();
    assert!(
        !output.status.success(),
        "doctor must report the missing migration"
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["diagnostics_schema"], 1);
    assert_eq!(report["healthy"], false);
    assert_eq!(
        snapshot(&data),
        before,
        "inspection must preserve archived evidence"
    );
}

#[tokio::test]
async fn doctor_keeps_inspecting_when_database_is_corrupt() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    initialize(&data).await;
    std::fs::write(
        data.join("simple-blog.sqlite3"),
        b"synthetic corrupt database",
    )
    .unwrap();
    let before = snapshot(&data);
    let output = command(&data).args(["doctor", "--json"]).output().unwrap();
    assert!(!output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("failure still returns a diagnostic report");
    assert_eq!(report["healthy"], false);
    for name in ["sqlite.connection", "filesystem.data", "release.active"] {
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["name"] == name),
            "missing independent check {name}"
        );
    }
    assert_eq!(snapshot(&data), before);
}

#[tokio::test]
async fn write_probes_are_explicit_and_report_scope_and_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    initialize(&data).await;
    let before = snapshot(&data);
    for (arguments, scope) in [
        (vec!["doctor", "--json"], "read_only"),
        (vec!["doctor", "--json", "--probe-writes"], "write_probes"),
    ] {
        let output = command(&data).args(arguments).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["inspection_scope"], scope);
        for check in report["checks"].as_array().unwrap() {
            assert!(check["code"].is_string());
            assert!(check["hint"].is_string());
        }
        assert_eq!(snapshot(&data), before);
    }
}

#[tokio::test]
async fn diagnostic_copy_reads_committed_wal_without_touching_source_sidecars() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("live.sqlite3");
    let writer = simple_blog::infrastructure::sqlite::SqliteRepository::connect(&source)
        .await
        .unwrap();
    sqlx::query("PRAGMA wal_autocheckpoint=0")
        .execute(writer.pool())
        .await
        .unwrap();
    sqlx::query("CREATE TABLE diagnostic_wal_fixture(value TEXT)")
        .execute(writer.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO diagnostic_wal_fixture VALUES ('committed only in WAL')")
        .execute(writer.pool())
        .await
        .unwrap();
    assert!(
        std::fs::metadata(temp.path().join("live.sqlite3-wal"))
            .unwrap()
            .len()
            > 0
    );
    let before = snapshot(temp.path());
    let reader =
        simple_blog::infrastructure::sqlite::SqliteRepository::connect_diagnostics(&source)
            .await
            .unwrap();
    let value: String = sqlx::query_scalar("SELECT value FROM diagnostic_wal_fixture")
        .fetch_one(reader.pool())
        .await
        .unwrap();
    assert_eq!(value, "committed only in WAL");
    reader.close().await;
    assert_eq!(snapshot(temp.path()), before);
    writer.close().await;
}
