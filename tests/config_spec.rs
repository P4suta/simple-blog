use std::{collections::BTreeMap, path::PathBuf};

use simple_blog::config::{Config, ConfigFile, ConfigSources, Overrides};

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
