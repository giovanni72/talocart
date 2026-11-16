//! Module/base database compatibility: real PostgreSQL, synthetic isolated schema.
use sqlx::{
    PgPool,
    migrate::{Migration, MigrationType, Migrator},
    postgres::PgConnectOptions,
};
use std::{
    borrow::Cow,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};
use talocart_platform::database::{self, DatabaseError, MIGRATOR};

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn module_checks_the_entire_host_ledger_and_its_historical_prefix_without_writes() {
    let url = std::env::var("TALOCART_TEST_DATABASE_URL").expect("Dedicated test URL");
    let parsed = url::Url::parse(&url).unwrap();
    assert!(
        matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            && parsed.path().ends_with("_test")
    );
    let owner = PgPool::connect(&url).await.unwrap();
    let schema = format!(
        "talocart_sdk_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&owner)
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPool::connect_with(options).await.unwrap();
    let scoped = pool.clone();
    let result = tokio::spawn(async move {
        database::migrate(&scoped).await.unwrap();
        let previous = Migrator {
            migrations: Cow::Owned(
                MIGRATOR
                    .iter()
                    .take(MIGRATOR.iter().count())
                    .cloned()
                    .collect(),
            ),
            ..Migrator::DEFAULT
        };
        let required = database::contract_for(&previous);
        let original = database::applied_migrations(&scoped, "_sqlx_migrations")
            .await
            .unwrap()
            .unwrap();
        database::check_module(&scoped, &database::contract(), &required)
            .await
            .unwrap();
        assert_eq!(
            original,
            database::applied_migrations(&scoped, "_sqlx_migrations")
                .await
                .unwrap()
                .unwrap(),
            "Compatibility checks must be read-only"
        );
        let next = MIGRATOR.iter().last().unwrap().version + 1;
        let mut migrations = MIGRATOR.iter().cloned().collect::<Vec<_>>();
        migrations.push(Migration::new(
            next,
            Cow::Borrowed("future compatible addition"),
            MigrationType::Simple,
            sqlx::SqlStr::from_static(
                "CREATE TABLE sdk_compatible_addition(id BIGINT PRIMARY KEY)",
            ),
            false,
        ));
        let future = Migrator {
            migrations: Cow::Owned(migrations),
            ..Migrator::DEFAULT
        };
        future.run(&scoped).await.unwrap();
        let host = database::contract_for(&future);
        database::check_module(&scoped, &host, &required)
            .await
            .unwrap();
        assert!(
            matches!(database::check(&scoped).await, Err(DatabaseError::Schema)),
            "A base executable still requires its exact own migration set"
        );
        let mut wrong = host.clone();
        wrong.sha256 = "00".repeat(32);
        assert!(
            database::check_module(&scoped, &wrong, &required)
                .await
                .is_err()
        );
        wrong = host.clone();
        wrong.version += 1;
        assert!(
            database::check_module(&scoped, &wrong, &required)
                .await
                .is_err()
        );
        let mut wrong_required = required.clone();
        wrong_required.sha256 = "00".repeat(32);
        assert!(
            database::check_module(&scoped, &host, &wrong_required)
                .await
                .is_err()
        );
        assert!(
            database::check_module(&scoped, &database::contract(), &required)
                .await
                .is_err(),
            "Unexpected migrations cannot be accepted just because they extend a prefix"
        );
        sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=$2")
            .bind(vec![0_u8; 48])
            .bind(original[0].0)
            .execute(&scoped)
            .await
            .unwrap();
        assert!(
            database::check_module(&scoped, &host, &required)
                .await
                .is_err()
        );
        sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=$2")
            .bind(&original[0].1)
            .bind(original[0].0)
            .execute(&scoped)
            .await
            .unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET success=false WHERE version=$1")
            .bind(next)
            .execute(&scoped)
            .await
            .unwrap();
        assert!(
            database::check_module(&scoped, &host, &required)
                .await
                .is_err(),
            "Dirty migrations remain fatal"
        );
        sqlx::query("UPDATE _sqlx_migrations SET success=true WHERE version=$1")
            .bind(next)
            .execute(&scoped)
            .await
            .unwrap();
        database::check_module(&scoped, &host, &required)
            .await
            .unwrap();
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version=$1")
            .bind(next)
            .execute(&scoped)
            .await
            .unwrap();
        assert!(
            database::check_module(&scoped, &host, &required)
                .await
                .is_err(),
            "A missing host migration remains fatal"
        );
    })
    .await;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&owner)
        .await
        .unwrap();
    owner.close().await;
    result.expect("Database compatibility assertions");
}
