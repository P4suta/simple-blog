use std::process::Command;

use sqlx::{
    Connection, Executor,
    sqlite::{SqliteConnectOptions, SqliteConnection},
};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_simple-blog"))
}

#[test]
fn help_exposes_the_v01_operational_surface() {
    let output = binary().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in [
        "init", "serve", "backup", "restore", "export", "doctor", "owner",
    ] {
        assert!(help.contains(command), "missing {command}");
    }
}

#[test]
fn init_to_doctor_and_backup_works_with_only_the_release_binary_contract() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let init = binary()
        .args(["--data-dir", data.to_str().unwrap(), "init"])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let stdout = String::from_utf8(init.stdout).unwrap();
    assert!(stdout.contains("/admin/setup/?token="));
    assert!(data.join("simple-blog.sqlite3").is_file());
    assert!(data.join("config.toml").is_file());
    assert!(data.join("media").is_dir());
    assert!(data.join("backups").is_dir());

    let doctor = binary()
        .args(["--data-dir", data.to_str().unwrap(), "doctor"])
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    assert!(
        String::from_utf8(doctor.stdout)
            .unwrap()
            .contains("healthy")
    );

    let doctor_json = binary()
        .args(["--data-dir", data.to_str().unwrap(), "doctor", "--json"])
        .output()
        .unwrap();
    assert!(doctor_json.status.success());
    let report: serde_json::Value = serde_json::from_slice(&doctor_json.stdout).unwrap();
    assert_eq!(report["diagnostics_schema"], 1);
    assert_eq!(report["application_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["healthy"], true);
    let checks = report["checks"].as_array().unwrap();
    for expected in [
        "sqlite.quick_check",
        "sqlite.foreign_keys",
        "sqlite.runtime_pragmas",
        "sqlite.migrations",
        "filesystem.data",
        "filesystem.media",
        "filesystem.backups",
        "media.records",
        "media.orphans",
    ] {
        assert!(checks.iter().any(|check| check["name"] == expected));
    }

    let backup = binary()
        .args(["--data-dir", data.to_str().unwrap(), "backup"])
        .output()
        .unwrap();
    assert!(
        backup.status.success(),
        "{}",
        String::from_utf8_lossy(&backup.stderr)
    );
    let archive = String::from_utf8(backup.stdout).unwrap();
    let archive = std::path::PathBuf::from(archive.trim());
    assert!(archive.is_file());
}

#[test]
fn restore_command_requires_force_before_replacing_an_installation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    for data in [&source, &destination] {
        let output = binary()
            .args(["--data-dir", data.to_str().unwrap(), "init"])
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let backup = binary()
        .args(["--data-dir", source.to_str().unwrap(), "backup"])
        .output()
        .unwrap();
    let archive = String::from_utf8(backup.stdout).unwrap();

    let refused = binary()
        .args([
            "--data-dir",
            destination.to_str().unwrap(),
            "restore",
            archive.trim(),
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());

    let restored = binary()
        .args([
            "--data-dir",
            destination.to_str().unwrap(),
            "restore",
            archive.trim(),
            "--force",
        ])
        .output()
        .unwrap();
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
}

#[test]
fn json_traces_are_machine_readable_and_do_not_duplicate_setup_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let output = binary()
        .env("SIMPLE_BLOG_LOG_FORMAT", "json")
        .env("RUST_LOG", "simple_blog=info")
        .args(["--data-dir", data.to_str().unwrap(), "init"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let token = stdout.split("token=").nth(1).unwrap().trim();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let events = stderr
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();

    assert!(!events.is_empty());
    assert!(
        events
            .iter()
            .any(|event| event["fields"]["message"] == "command started")
    );
    let started = events
        .iter()
        .find(|event| event["fields"]["event"] == "cli.command.started")
        .expect("typed command event");
    assert_eq!(started["fields"]["diagnostics_schema"], 1);
    assert_eq!(started["fields"]["version"], env!("CARGO_PKG_VERSION"));
    for field in [
        "timestamp",
        "level",
        "target",
        "filename",
        "line_number",
        "threadName",
        "threadId",
    ] {
        assert!(
            !started[field].is_null(),
            "missing JSON trace field {field}"
        );
    }
    assert!(!stderr.contains(token));
}

#[test]
fn invalid_diagnostic_configuration_fails_before_touching_the_data_directory() {
    for (variable, value) in [
        ("RUST_LOG", "simple_blog=[invalid"),
        ("SIMPLE_BLOG_LOG_FORMAT", "xml"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let output = binary()
            .env(variable, value)
            .args(["--data-dir", data.to_str().unwrap(), "init"])
            .output()
            .unwrap();

        assert!(!output.status.success(), "{variable}={value}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("could not initialize diagnostics"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!data.exists());
    }
}

#[tokio::test]
async fn every_operational_database_open_applies_pending_migrations_after_a_safety_backup() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let init = binary()
        .args(["--data-dir", data.to_str().unwrap(), "init"])
        .output()
        .unwrap();
    assert!(init.status.success());

    let options = SqliteConnectOptions::new()
        .filename(data.join("simple-blog.sqlite3"))
        .create_if_missing(false);
    let mut database = SqliteConnection::connect_with(&options).await.unwrap();
    database.execute("DROP TABLE media_variants").await.unwrap();
    database
        .execute("DELETE FROM _sqlx_migrations WHERE version = 2")
        .await
        .unwrap();
    database.close().await.unwrap();

    for _ in 0..2 {
        let doctor = binary()
            .env("SIMPLE_BLOG_LOG_FORMAT", "json")
            .args(["--data-dir", data.to_str().unwrap(), "doctor"])
            .output()
            .unwrap();
        assert!(
            doctor.status.success(),
            "{}",
            String::from_utf8_lossy(&doctor.stderr)
        );
    }
    let safety_backups = std::fs::read_dir(data.join("backups"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("simple-blog-pre-migration-"))
        .collect::<Vec<_>>();
    assert_eq!(safety_backups.len(), 1);
}
