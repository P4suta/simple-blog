use std::{collections::BTreeMap, path::PathBuf};

use simple_blog::config::{Config, ConfigError, ConfigFile, ConfigSources, Overrides};

#[test]
fn defaults_are_local_and_zero_configuration() {
    let config = Config::resolve(ConfigSources::default()).expect("default config");

    assert_eq!(config.data_dir, PathBuf::from("./data"));
    assert_eq!(config.bind.to_string(), "127.0.0.1:8080");
}

#[test]
fn precedence_is_cli_then_environment_then_toml_then_defaults() {
    let file = ConfigFile {
        data_dir: Some(PathBuf::from("from-file")),
        bind: Some("127.0.0.1:3000".into()),
        ..ConfigFile::default()
    };
    let env = BTreeMap::from([
        ("SIMPLE_BLOG_DATA_DIR".into(), "from-env".into()),
        ("SIMPLE_BLOG_BIND".into(), "127.0.0.1:4000".into()),
    ]);
    let cli = Overrides {
        data_dir: Some(PathBuf::from("from-cli")),
        bind: Some("127.0.0.1:5000".into()),
        ..Overrides::default()
    };

    let config = Config::resolve(ConfigSources {
        cli,
        env,
        file: Some(file),
    })
    .expect("resolved config");

    assert_eq!(config.data_dir, PathBuf::from("from-cli"));
    assert_eq!(config.bind.to_string(), "127.0.0.1:5000");
}

#[test]
fn public_url_must_be_an_http_origin_without_credentials_or_path() {
    for invalid in [
        "ftp://example.com",
        "https://user@example.com",
        "https://example.com/blog",
        "not a url",
    ] {
        let result = Config::resolve(ConfigSources {
            cli: Overrides {
                public_url: Some(invalid.into()),
                ..Overrides::default()
            },
            ..ConfigSources::default()
        });
        assert!(result.is_err(), "{invalid:?} must be rejected");
    }
}

#[test]
fn environment_safety_limits_and_proxy_lists_are_parsed_deterministically() {
    let config = Config::resolve(ConfigSources {
        env: BTreeMap::from([
            (
                "SIMPLE_BLOG_TRUSTED_PROXIES".into(),
                "127.0.0.1, ::1, ".into(),
            ),
            ("SIMPLE_BLOG_MAX_UPLOAD_BYTES".into(), "4096".into()),
        ]),
        ..ConfigSources::default()
    })
    .unwrap();

    assert_eq!(config.trusted_proxies.len(), 2);
    assert_eq!(config.max_upload_bytes, 4096);

    // A typo in the proxy list must not silently weaken rate limiting.
    let mistyped = Config::resolve(ConfigSources {
        env: BTreeMap::from([(
            "SIMPLE_BLOG_TRUSTED_PROXIES".into(),
            "127.0.0.1, invalid, ::1".into(),
        )]),
        ..ConfigSources::default()
    });
    assert!(matches!(mistyped, Err(ConfigError::TrustedProxy(item)) if item == "invalid"));

    let zero = Config::resolve(ConfigSources {
        cli: Overrides {
            max_upload_bytes: Some(0),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    });
    assert!(matches!(zero, Err(ConfigError::MaxUpload)));

    let bad_bind = Config::resolve(ConfigSources {
        cli: Overrides {
            bind: Some("not-a-socket".into()),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    });
    assert!(matches!(bad_bind, Err(ConfigError::Bind(_))));
}

#[test]
fn load_and_persist_report_corrupt_or_unusable_paths_without_partial_files() {
    let corrupt = tempfile::tempdir().unwrap();
    std::fs::write(corrupt.path().join("config.toml"), "bind = [").unwrap();
    let result = Config::load(Overrides {
        data_dir: Some(corrupt.path().to_path_buf()),
        ..Overrides::default()
    });
    assert!(matches!(result, Err(ConfigError::Parse { .. })));

    let unreadable = tempfile::tempdir().unwrap();
    std::fs::create_dir(unreadable.path().join("config.toml")).unwrap();
    let result = Config::load(Overrides {
        data_dir: Some(unreadable.path().to_path_buf()),
        ..Overrides::default()
    });
    assert!(matches!(result, Err(ConfigError::Read { .. })));

    let collision = tempfile::tempdir().unwrap();
    let file_instead_of_directory = collision.path().join("data");
    std::fs::write(&file_instead_of_directory, "occupied").unwrap();
    let config = Config::resolve(ConfigSources {
        cli: Overrides {
            data_dir: Some(file_instead_of_directory.clone()),
            ..Overrides::default()
        },
        ..ConfigSources::default()
    })
    .unwrap();
    assert!(matches!(config.persist(), Err(ConfigError::Write { .. })));
    assert_eq!(
        std::fs::read_to_string(file_instead_of_directory).unwrap(),
        "occupied"
    );
}

#[test]
fn backup_retention_defaults_to_fourteen_and_can_be_disabled() {
    let config = Config::resolve(ConfigSources::default()).unwrap();
    assert_eq!(config.backup_retention, 14);

    let from_env = Config::resolve(ConfigSources {
        env: BTreeMap::from([("SIMPLE_BLOG_BACKUP_RETENTION".into(), "3".into())]),
        ..ConfigSources::default()
    })
    .unwrap();
    assert_eq!(from_env.backup_retention, 3);

    let disabled = Config::resolve(ConfigSources {
        file: Some(ConfigFile {
            backup_retention: Some(0),
            ..ConfigFile::default()
        }),
        ..ConfigSources::default()
    })
    .unwrap();
    assert_eq!(
        disabled.backup_retention, 0,
        "zero switches the scheduler off"
    );

    let garbage = Config::resolve(ConfigSources {
        env: BTreeMap::from([("SIMPLE_BLOG_BACKUP_RETENTION".into(), "many".into())]),
        ..ConfigSources::default()
    });
    assert!(matches!(garbage, Err(ConfigError::BackupRetention(_))));
}
