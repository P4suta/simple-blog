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
        "init", "build", "serve", "backup", "restore", "export", "migrate", "doctor", "owner",
    ] {
        assert!(help.contains(command), "missing {command}");
    }
}

#[test]
fn migrate_cli_moves_a_site_without_requiring_the_destination_to_be_initialized() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let archive = temp.path().join("site.simple-blog");
    assert!(
        binary()
            .args(["--data-dir", source.to_str().unwrap(), "init"])
            .output()
            .unwrap()
            .status
            .success()
    );

    let exported = binary()
        .args([
            "--data-dir",
            source.to_str().unwrap(),
            "migrate",
            "export",
            "--output",
            archive.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let export_report: serde_json::Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(export_report["archive"], archive.to_str().unwrap());
    assert_eq!(export_report["archive_id"].as_str().unwrap().len(), 64);

    let imported = binary()
        .args([
            "--data-dir",
            destination.to_str().unwrap(),
            "migrate",
            "import",
            archive.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let import_report: serde_json::Value = serde_json::from_slice(&imported.stdout).unwrap();
    assert_eq!(import_report["data_dir"], destination.to_str().unwrap());
    assert_eq!(import_report["release_id"].as_str().unwrap().len(), 64);
    assert!(destination.join("simple-blog.sqlite3").is_file());
    assert!(destination.join("releases/active").is_file());

    let doctor = binary()
        .args(["--data-dir", destination.to_str().unwrap(), "doctor"])
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stderr)
    );
}

#[test]
fn migrate_cli_preserves_an_existing_destination_origin_guard() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let archive = temp.path().join("site.simple-blog");
    for (data, origin) in [
        (&source, "https://source.example"),
        (&destination, "https://destination.example"),
    ] {
        let initialized = binary()
            .args([
                "--data-dir",
                data.to_str().unwrap(),
                "--public-url",
                origin,
                "init",
            ])
            .env_remove("SIMPLE_BLOG_PUBLIC_URL")
            .output()
            .unwrap();
        assert!(
            initialized.status.success(),
            "{}",
            String::from_utf8_lossy(&initialized.stderr)
        );
    }
    let exported = binary()
        .args([
            "--data-dir",
            source.to_str().unwrap(),
            "migrate",
            "export",
            "--output",
            archive.to_str().unwrap(),
        ])
        .env_remove("SIMPLE_BLOG_PUBLIC_URL")
        .output()
        .unwrap();
    assert!(exported.status.success());
    let destination_config_before = std::fs::read(destination.join("config.toml")).unwrap();

    let imported = binary()
        .args([
            "--data-dir",
            destination.to_str().unwrap(),
            "migrate",
            "import",
            archive.to_str().unwrap(),
            "--force",
        ])
        .env_remove("SIMPLE_BLOG_PUBLIC_URL")
        .output()
        .unwrap();

    assert!(!imported.status.success());
    assert!(String::from_utf8_lossy(&imported.stderr).contains("origin"));
    assert_eq!(
        std::fs::read(destination.join("config.toml")).unwrap(),
        destination_config_before
    );
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
    assert!(data.join("releases").is_dir());

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
        "filesystem.releases",
        "media.records",
        "media.orphans",
        "release.active",
        "release.history",
        "release.temporary_files",
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
fn build_activates_a_verified_release_and_can_materialize_it_without_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let output = temp.path().join("public");
    assert!(
        binary()
            .args(["--data-dir", data.to_str().unwrap(), "init"])
            .output()
            .unwrap()
            .status
            .success()
    );

    let built = binary()
        .args([
            "--data-dir",
            data.to_str().unwrap(),
            "build",
            "--output",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
    assert_eq!(result["public_revision"], 0);
    assert_eq!(result["materialized_to"], output.to_str().unwrap());
    assert_eq!(result["release_id"].as_str().unwrap().len(), 64);
    assert!(data.join("releases/active").is_file());
    assert!(output.join("index.html").is_file());
    assert!(output.join(".simple-blog-release.json").is_file());

    let refused = binary()
        .args([
            "--data-dir",
            data.to_str().unwrap(),
            "build",
            "--output",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("already exists"));
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
        .env("SIMPLE_BLOG_LOG_FORMAT", "json")
        .env("RUST_LOG", "simple_blog=debug")
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
    let restore_phases = String::from_utf8(restored.stderr)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|event| event["fields"]["event"].as_str().map(str::to_owned))
        .filter(|event| event.starts_with("backup.restore."))
        .collect::<Vec<_>>();
    assert_eq!(
        restore_phases,
        [
            "backup.restore.extracted",
            "backup.restore.manifest_verified",
            "backup.restore.database_verified",
            "backup.restore.completed",
        ]
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
#[test]
fn serve_prints_the_setup_link_while_no_owner_exists_and_the_admin_address() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("fresh");
    let mut child = binary()
        .args([
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--bind",
            "127.0.0.1:0",
            "serve",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut lines = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        match receiver.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(line) => {
                lines.push(line);
                if lines.iter().any(|line| line.starts_with("Admin: ")) {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    let output = lines.join("\n");
    assert!(
        output.contains("No owner passkey is registered yet"),
        "{output}"
    );
    assert!(
        output.contains("http://localhost:8080/admin/setup/?token="),
        "{output}"
    );
    assert!(
        output.contains("Admin: http://localhost:8080/admin/"),
        "{output}"
    );
    assert!(output.contains("Site:  http://localhost:8080/"), "{output}");
}
