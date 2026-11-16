use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "talocart-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("isolated CLI configuration directory");
        Self(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_talocart"));
        command.current_dir(&self.0);
        for prefix in ["TALOCART", "KOALA"] {
            for key in [
                "DATABASE_URL",
                "STORE_NAME",
                "LISTEN",
                "PUBLIC_ORIGIN",
                "MEDIA_ROOT",
                "LOG",
                "MODULE_KOALA_IMPORT",
                "IMPORT_CONFIG",
                "IMPORT_FTP_CONFIG",
                "SMTP_ENABLED",
                "SMTP_HOST",
                "SMTP_PORT",
                "SMTP_SECURITY",
                "SMTP_USERNAME",
                "SMTP_PASSWORD",
                "SMTP_TIMEOUT_SECONDS",
            ] {
                command.env_remove(format!("{prefix}_{key}"));
            }
        }
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove isolated CLI configuration");
    }
}

#[test]
fn only_talocart_is_discovered_and_explicit_paths_are_preserved() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("koala.toml"), "[store]\nname = ''").expect("legacy configuration");
    let legacy = fixture
        .command()
        .arg("doctor")
        .output()
        .expect("legacy CLI");
    assert!(!legacy.status.success());
    assert!(
        legacy.stdout.is_empty(),
        "diagnostics must leave machine-readable stdout clean"
    );
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("PostgreSQL URL required"));
    fs::write(fixture.0.join("talocart.toml"), "invalid TOML!").expect("invalid new configuration");
    let new = fixture.command().arg("doctor").output().expect("new CLI");
    assert!(!new.status.success());
    assert!(String::from_utf8_lossy(&new.stderr).contains("Invalid TOML configuration"));
    let explicit = fixture
        .command()
        .args(["--config", "koala.toml", "doctor"])
        .output()
        .expect("explicit legacy CLI");
    assert!(String::from_utf8_lossy(&explicit.stderr).contains("store.name"));
    let missing = fixture
        .command()
        .args(["--config", "missing.toml", "doctor"])
        .output()
        .expect("missing explicit CLI");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Unable to read"));
}

#[test]
fn invalid_talocart_log_setting_is_rejected() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("TALOCART_LOG", "[")
        .env("KOALA_LOG", "warn")
        .arg("doctor")
        .output()
        .expect("logging CLI");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Invalid TALOCART_LOG"));
}

#[test]
fn legacy_environment_settings_are_ignored() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("KOALA_LOG", "[")
        .env("KOALA_DATABASE_URL", "postgres://unused@localhost/unused")
        .arg("doctor")
        .output()
        .expect("legacy variables ignored");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("PostgreSQL URL required"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Invalid TALOCART_LOG"));
}

#[test]

fn base_binary_has_no_import_command_and_rejects_optional_settings() {
    let fixture = Fixture::new();
    let help = fixture.command().arg("--help").output().expect("base help");
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("import-doctor"));
    fs::write(
        fixture.0.join("talocart.toml"),
        "[database]\nurl='postgres://fixture@127.0.0.1:1/unused'\n[modules]\nkoala_import=true",
    )
    .expect("base configuration");
    let output = fixture
        .command()
        .arg("doctor")
        .output()
        .expect("base doctor");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Invalid TOML configuration"));
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("Configurazione del database esterno")
    );
}

#[test]
fn cli_failure_json_preserves_unique_message_and_error_fields() {
    use serde::{
        Deserialize, Deserializer,
        de::{MapAccess, Visitor},
    };
    use std::collections::BTreeMap;
    struct Fields(BTreeMap<String, serde_json::Value>);
    impl<'de> Deserialize<'de> for Fields {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct UniqueFields;
            impl<'de> Visitor<'de> for UniqueFields {
                type Value = Fields;
                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("JSON fields with unique keys")
                }
                fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Fields, M::Error> {
                    let mut fields = BTreeMap::new();
                    while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
                        if fields.insert(key.clone(), value).is_some() {
                            return Err(serde::de::Error::custom(format!(
                                "duplicate JSON key: {key}"
                            )));
                        }
                    }
                    Ok(Fields(fields))
                }
            }
            deserializer.deserialize_map(UniqueFields)
        }
    }
    #[derive(Deserialize)]
    struct Event {
        level: String,
        fields: Fields,
    }
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["--config", "missing.toml", "doctor"])
        .output()
        .expect("isolated CLI failure");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
    let events: Vec<Event> = stderr
        .lines()
        .map(|line| {
            serde_json::from_str(line).expect("JSON diagnostics must preserve unique fields")
        })
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].level, "ERROR");
    assert_eq!(events[0].fields.0["message"], "Operation failed");
    assert_eq!(
        events[0].fields.0["error"],
        "Unable to read the requested configuration file"
    );
    assert!(!stderr.contains(&fixture.0.to_string_lossy().to_string()));
}
