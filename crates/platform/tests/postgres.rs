use std::{
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use talocart_platform::database::{self, DatabaseError, MIGRATOR};

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL e permesso CREATE SCHEMA nel database dedicato"]
async fn optional_category_clean_schema_enforces_tax_and_visibility_contract() {
    let url = talocart_platform::config::environment("TALOCART_TEST_DATABASE_URL")
        .and_then(|value| {
            value.ok_or(talocart_platform::config::ConfigError::Invalid(
                "test environment required",
            ))
        })
        .expect("URL test dedicata richiesta");
    let owner = PgPool::connect(&url).await.unwrap();
    for original_version in [1] {
        let schema = format!(
            "talocart_optional_category_{}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            original_version
        );
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&owner)
            .await
            .unwrap();
        let options = PgConnectOptions::from_str(&url)
            .unwrap()
            .options([("search_path", schema.as_str())]);
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let test_pool = pool.clone();
        let result=tokio::spawn(async move {
            database::migrate(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_categories(name,slug) VALUES('Preserved','preserved')").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO tax_rates(name,basis_points) VALUES('Preserved',2200)").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO tax_classes(name,rate_id) VALUES('Preserved',1)").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,category_id,tax_class_id) VALUES('Published','PUBLISHED','published',1299,8,'published',1,1),('Draft','DRAFT','draft',1400,9,'draft',NULL,1)").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_options(name) VALUES('Size')").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_option_values(option_id,name) VALUES(1,'M')").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES(1,1,1)").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES(1,ARRAY[1]::bigint[],'VARIANT',1599,7)").execute(&test_pool).await.unwrap();
            let snapshot="SELECT jsonb_build_array((SELECT jsonb_agg(p ORDER BY id) FROM catalog_products p),(SELECT jsonb_agg(v ORDER BY id) FROM catalog_variants v),(SELECT jsonb_agg(o ORDER BY id) FROM catalog_options o),(SELECT jsonb_agg(v ORDER BY id) FROM catalog_option_values v),(SELECT jsonb_agg(o ORDER BY product_id,position) FROM catalog_product_options o),(SELECT jsonb_agg(c ORDER BY id) FROM catalog_categories c),(SELECT jsonb_agg(c ORDER BY id) FROM tax_classes c),(SELECT jsonb_agg(r ORDER BY id) FROM tax_rates r))::text";
            let before:String=sqlx::query_scalar(snapshot).fetch_one(&test_pool).await.unwrap();
            database::migrate(&test_pool).await.unwrap();database::migrate(&test_pool).await.unwrap();
            database::check(&test_pool).await.unwrap();
            assert_eq!(before,sqlx::query_scalar::<_,String>(snapshot).fetch_one(&test_pool).await.unwrap());
            sqlx::query("UPDATE catalog_products SET category_id=NULL,status='published'").execute(&test_pool).await.unwrap();
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_visible_products WHERE category_id IS NULL AND category_name IS NULL AND category_slug IS NULL").fetch_one(&test_pool).await.unwrap(),2);
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT price_cents FROM catalog_visible_products WHERE id=1").fetch_one(&test_pool).await.unwrap(),1599);
            assert!(sqlx::query("UPDATE catalog_products SET tax_class_id=NULL WHERE id=2").execute(&test_pool).await.is_err());
            sqlx::query("UPDATE catalog_categories SET active=false WHERE id=1").execute(&test_pool).await.unwrap();
            assert!(sqlx::query("UPDATE catalog_products SET category_id=1 WHERE id=2").execute(&test_pool).await.is_err());
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_visible_products").fetch_one(&test_pool).await.unwrap(),2);
        }).await;
        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&owner)
            .await
            .unwrap();
        result.expect("Categoria facoltativa con dati conservati");
    }
    owner.close().await;
}

/// Explicitly opt in with a test URL. Uses a unique schema and never resets a database.
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL e permesso CREATE SCHEMA nel database dedicato"]
async fn migration_lifecycle_and_readiness() {
    let url = talocart_platform::config::environment("TALOCART_TEST_DATABASE_URL")
        .and_then(|value| {
            value.ok_or(talocart_platform::config::ConfigError::Invalid(
                "test environment required",
            ))
        })
        .expect("TALOCART_TEST_DATABASE_URL richiesta");
    let schema = format!(
        "talocart_test_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let owner = PgPool::connect(&url)
        .await
        .expect("Connessione PostgreSQL di test");
    // Schema identifier contains only a fixed prefix, PID and numeric timestamp.
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&owner)
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let test_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();

    let media = talocart_platform::media::MediaStore::new(std::env::temp_dir().join(&schema));
    let result = exercise(&test_pool, &media).await;
    if media.root().exists() {
        tokio::fs::remove_dir_all(media.root()).await.unwrap();
    }
    test_pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&owner)
        .await
        .unwrap();
    owner.close().await;
    result.expect("Ciclo delle migrazioni e readiness");
}

async fn exercise(
    pool: &PgPool,
    media: &talocart_platform::media::MediaStore,
) -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        database::check(pool).await,
        Err(DatabaseError::Schema)
    ));
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('_sqlx_migrations') IS NULL")
            .fetch_one(pool)
            .await?
    );
    database::migrate(pool).await?;
    database::migrate(pool).await?;
    database::check(pool).await?;
    media.check(pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await?,
        MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM administrators")
            .fetch_one(pool)
            .await?,
        0
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT installed FROM identity_state WHERE id=1")
            .fetch_one(pool)
            .await?
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM operation_events")
            .fetch_one(pool)
            .await?,
        0
    );
    let codes: Vec<String> = sqlx::query_scalar("SELECT code FROM payment_methods ORDER BY id")
        .fetch_all(pool)
        .await?;
    assert_eq!(
        codes,
        ["bank_transfer", "cash_on_delivery", "online_checkout"]
    );
    sqlx::query("UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=1")
        .execute(pool)
        .await?;
    assert!(matches!(
        database::check(pool).await,
        Err(DatabaseError::Schema)
    ));
    sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=1")
        .bind(MIGRATOR.iter().next().unwrap().checksum.as_ref())
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(9999,'future',true,decode('00','hex'),0)").execute(pool).await?;
    assert!(matches!(
        database::check(pool).await,
        Err(DatabaseError::Schema)
    ));
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version=9999")
        .execute(pool)
        .await?;
    database::check(pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL e permesso CREATE SCHEMA nel database dedicato"]
async fn storefront_defaults_and_repeat_migrate_keep_current_preferences() {
    let url = talocart_platform::config::environment("TALOCART_TEST_DATABASE_URL")
        .and_then(|value| {
            value.ok_or(talocart_platform::config::ConfigError::Invalid(
                "test environment required",
            ))
        })
        .expect("URL test dedicata richiesta");
    let owner = PgPool::connect(&url).await.unwrap();
    for (case, installed, palette) in [
        ("clean", false, None),
        ("pending", false, Some("forest")),
        ("forest", true, Some("forest")),
        ("custom", true, Some("clay")),
    ] {
        let schema = format!(
            "talocart_defaults_{}_{}_{case}",
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
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let test_pool = pool.clone();
        let result = tokio::spawn(async move {
            let before = if let Some(palette) = palette {
                database::migrate(&test_pool).await.unwrap();
                sqlx::query("UPDATE identity_state SET installed=$1 WHERE id=1").bind(installed).execute(&test_pool).await.unwrap();
                sqlx::query("UPDATE storefront_settings SET palette=$1,typography='editorial',density='comfortable',heading='Kept heading',seo_title='Kept SEO' WHERE id=1").bind(palette).execute(&test_pool).await.unwrap();
                Some(sqlx::query_scalar::<_, String>("SELECT to_jsonb(s)::text FROM storefront_settings s WHERE id=1").fetch_one(&test_pool).await.unwrap())
            } else { None };
            database::migrate(&test_pool).await.unwrap();
            database::migrate(&test_pool).await.unwrap();
            database::check(&test_pool).await.unwrap();
            let settings = talocart_platform::storefront::Storefront::new(test_pool.clone()).settings().await.unwrap();
            assert_eq!(settings.theme_id, "default", "Existing and clean stores keep the original theme");
            let safe_defaults: (bool, bool) = sqlx::query_as("SELECT maintenance_enabled,indexing_enabled FROM storefront_settings WHERE id=1").fetch_one(&test_pool).await.unwrap();
            assert_eq!(safe_defaults, (false, false), "New operational settings use their explicit safe defaults");
            if installed {
                let after: String = sqlx::query_scalar("SELECT to_jsonb(s)::text FROM storefront_settings s WHERE id=1").fetch_one(&test_pool).await.unwrap();
                assert_eq!(Some(after), before, "All saved preferences survive migration");
            } else {
                assert_eq!((settings.palette.as_str(), settings.typography.as_str(), settings.density.as_str()), (palette.unwrap_or("ocean"), if palette.is_some() {"editorial"} else {"modern"}, if palette.is_some() {"comfortable"} else {"compact"}));
                if palette.is_some() { assert_eq!(settings.heading, "Kept heading"); assert_eq!(settings.seo_title, "Kept SEO"); }
            }
            assert_eq!(settings.languages().unwrap(), talocart_platform::storefront::Languages::default());
            // Exercise SQL defaults separately from the initialized singleton row.
            let mut transaction = test_pool.begin().await.unwrap();
            sqlx::query("DELETE FROM storefront_settings WHERE id=1").execute(&mut *transaction).await.unwrap();
            let defaults: (String, String, String, String) = sqlx::query_as("INSERT INTO storefront_settings(id) VALUES(1) RETURNING palette,typography,density,theme_id").fetch_one(&mut *transaction).await.unwrap();
            assert_eq!(defaults, ("ocean".into(), "modern".into(), "compact".into(), "default".into()));
            transaction.rollback().await.unwrap();
        }).await;
        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&owner)
            .await
            .unwrap();
        result.expect("Default e preferenze durante installazione/aggiornamento");
    }
    owner.close().await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL e permesso CREATE SCHEMA nel database dedicato"]
async fn unlimited_catalog_clean_schema_and_repeat_migrate() {
    let url = talocart_platform::config::environment("TALOCART_TEST_DATABASE_URL")
        .and_then(|value| {
            value.ok_or(talocart_platform::config::ConfigError::Invalid(
                "test environment required",
            ))
        })
        .expect("URL test dedicata richiesta");
    let owner = PgPool::connect(&url).await.unwrap();
    for upgrade in [false] {
        let schema = format!(
            "talocart_unlimited_{}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            upgrade
        );
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&owner)
            .await
            .unwrap();
        let options = PgConnectOptions::from_str(&url)
            .unwrap()
            .options([("search_path", schema.as_str())]);
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let test_pool = pool.clone();
        let result=tokio::spawn(async move {
            database::migrate(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_options(name) SELECT 'Option '||n FROM generate_series(1,100)n").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_option_values(option_id,name) SELECT 1,'Color '||n FROM generate_series(1,100)n UNION ALL SELECT 2,'Size '||n FROM generate_series(1,5)n").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents,stock) VALUES('Preserved','PRESERVED','preserved',1299,8)").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES(1,1,1),(1,2,2)").execute(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) SELECT 1,ARRAY[c.id,s.id],'PRESERVED-'||row_number() OVER(),1299,8 FROM catalog_option_values c CROSS JOIN catalog_option_values s WHERE c.option_id=1 AND s.option_id=2").execute(&test_pool).await.unwrap();
            // Repeating migrate must leave current catalog data unchanged.
            let snapshot="SELECT json_build_array((SELECT json_agg(to_jsonb(o) ORDER BY id) FROM catalog_options o),(SELECT json_agg(to_jsonb(v) ORDER BY id) FROM catalog_option_values v),(SELECT json_agg(to_jsonb(p) ORDER BY id) FROM catalog_products p),(SELECT json_agg(a ORDER BY product_id,position) FROM catalog_product_options a),(SELECT json_agg(to_jsonb(v) ORDER BY id) FROM catalog_variants v),(SELECT json_agg(r ORDER BY variant_id,value_id) FROM catalog_variant_values r))::text";
            let before:String=sqlx::query_scalar(snapshot).fetch_one(&test_pool).await.unwrap();
            database::migrate(&test_pool).await.unwrap();
            database::migrate(&test_pool).await.unwrap();
            database::check(&test_pool).await.unwrap();
            assert_eq!(before,sqlx::query_scalar::<_,String>(snapshot).fetch_one(&test_pool).await.unwrap(),"All records and references survive migration and retry");
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT (SELECT count(*) FROM catalog_options WHERE sort_order<>0)+(SELECT count(*) FROM catalog_option_values WHERE sort_order<>0)").fetch_one(&test_pool).await.unwrap(),0,"New ordering defaults are checked separately from preserved fields");
            let option:i64=sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Beyond 100') RETURNING id").fetch_one(&test_pool).await.unwrap();
            let value:i64=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES(1,'Beyond 100') RETURNING id").fetch_one(&test_pool).await.unwrap();
            sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES(1,ARRAY[$1,101],'BEYOND-500',1599,9)").bind(value).execute(&test_pool).await.unwrap();
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_variants").fetch_one(&test_pool).await.unwrap(),501);
            assert!(sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES(1,4,$1)").bind(option).execute(&test_pool).await.is_err());
            assert!(sqlx::query("UPDATE catalog_option_values SET option_id=$1 WHERE id=$2").bind(option).bind(value).execute(&test_pool).await.is_err(),"Owner remains immutable");
            assert!(sqlx::query("DELETE FROM catalog_option_values WHERE id=$1").bind(value).execute(&test_pool).await.is_err(),"Used values remain protected");
        }).await;
        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&owner)
            .await
            .unwrap();
        result.expect("Installazione e aggiornamento senza tetti catalogo");
    }
    owner.close().await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn base_schema_contains_no_koala_objects_and_currency_works() {
    let url = talocart_platform::config::environment("TALOCART_TEST_DATABASE_URL")
        .unwrap()
        .expect("Dedicated test URL");
    let schema = format!(
        "talocart_base_boundary_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let owner = PgPool::connect(&url).await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&owner)
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .unwrap();
    let test = pool.clone();
    let result=tokio::spawn(async move {
        database::migrate(&test).await.unwrap();database::migrate(&test).await.unwrap();database::check(&test).await.unwrap();
        let names:Vec<String>=sqlx::query_scalar("SELECT tablename FROM pg_tables WHERE schemaname=current_schema() AND (tablename LIKE 'catalog_import_%' OR tablename LIKE '%koala%')").fetch_all(&test).await.unwrap();
        assert!(names.is_empty(),"Unexpected Koala tables: {names:?}");
        sqlx::query("INSERT INTO store_settings(id,name) VALUES(1,'Base fixture')").execute(&test).await.unwrap();
        sqlx::query("UPDATE store_settings SET currency='USD' WHERE id=1").execute(&test).await.expect("Currency guard works without Koala tables");
    }).await;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&owner)
        .await
        .unwrap();
    owner.close().await;
    result.unwrap();
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn payment_provider_upgrade_preserves_legacy_configuration_and_allows_distinct_methods() {
    let url = talocart_platform::config::environment("TALOCART_TEST_DATABASE_URL")
        .unwrap()
        .unwrap();
    let owner = PgPool::connect(&url).await.unwrap();
    let schema = format!("talocart_payment_upgrade_{}", std::process::id());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&owner)
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    let test_pool = pool.clone();
    let result = tokio::spawn(async move {
        let previous = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(MIGRATOR.iter().filter(|m|m.version<4).cloned().collect()),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        previous.run(&test_pool).await.unwrap();
        sqlx::query("UPDATE payment_methods SET name_it='La mia carta',name_en='My card',active=true,instructions_en='Preserve this',sort_order=42 WHERE id=3").execute(&test_pool).await.unwrap();
        sqlx::query("INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents) VALUES('ORD-LEGACY',987654,0,$1,$2,'EUR',100,0,0,100,0,100)").bind("a".repeat(64)).bind("status_id = 1\n").execute(&test_pool).await.unwrap();
        sqlx::query("INSERT INTO payment_attempts(provider_id,order_id,request_key,amount_cents,currency,title,return_url,locale,state,action) SELECT 'first',id,$1,100,'EUR','Legacy','http://127.0.0.1/','en','review','none' FROM orders WHERE reference='ORD-LEGACY'").bind("b".repeat(64)).execute(&test_pool).await.unwrap();
        sqlx::query("INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents) VALUES('ORD-OLDER',987655,0,$1,'status_id = 1','EUR',100,0,0,100,0,100)").bind("c".repeat(64)).execute(&test_pool).await.unwrap();
        sqlx::query("INSERT INTO payment_attempts(provider_id,order_id,request_key,amount_cents,currency,title,return_url,locale,state,action) SELECT 'retired',id,$1,100,'EUR','Older','http://127.0.0.1/','en','paid','none' FROM orders WHERE reference='ORD-OLDER'").bind("d".repeat(64)).execute(&test_pool).await.unwrap();
        let history: String = sqlx::query_scalar("SELECT jsonb_build_array((SELECT jsonb_agg(o) FROM orders o),(SELECT jsonb_agg(a) FROM payment_attempts a))::text").fetch_one(&test_pool).await.unwrap();
        let before: String = sqlx::query_scalar("SELECT to_jsonb(p)::text FROM payment_methods p WHERE id=3").fetch_one(&test_pool).await.unwrap();
        database::migrate(&test_pool).await.unwrap();
        let after: String = sqlx::query_scalar("SELECT (to_jsonb(p)-'provider_id')::text FROM payment_methods p WHERE id=3").fetch_one(&test_pool).await.unwrap();
        assert_eq!(before,after);
        assert_eq!(sqlx::query_scalar::<_,Option<String>>("SELECT provider_id FROM payment_methods WHERE id=3").fetch_one(&test_pool).await.unwrap(),None);
        assert_eq!(history,sqlx::query_scalar::<_,String>("SELECT jsonb_build_array((SELECT jsonb_agg(o) FROM orders o),(SELECT jsonb_agg(a) FROM payment_attempts a))::text").fetch_one(&test_pool).await.unwrap());
        sqlx::query("UPDATE payment_methods SET provider_id='first' WHERE id=3").execute(&test_pool).await.unwrap();
        sqlx::query("INSERT INTO payment_methods(code,provider_id,name_it,name_en,initial_order_status_id) VALUES('online_checkout','second','Second','Second',1)").execute(&test_pool).await.unwrap();
        assert!(sqlx::query("UPDATE payment_methods SET provider_id='second' WHERE id=3").execute(&test_pool).await.is_err());
        assert!(sqlx::query("INSERT INTO payment_methods(code,provider_id,name_it,name_en,initial_order_status_id) VALUES('online_checkout','second','Duplicate','Duplicate',1)").execute(&test_pool).await.is_err());
        database::migrate(&test_pool).await.unwrap();
        database::check(&test_pool).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM payment_methods").fetch_one(&test_pool).await.unwrap(),4);
    }).await;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&owner)
        .await
        .unwrap();
    result.unwrap();
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn module_install_upgrade_preserves_settings_and_bootstrap_defaults() {
    use talocart_platform::config::PackageInstall;
    let url = talocart_platform::config::environment("TALOCART_TEST_DATABASE_URL")
        .unwrap()
        .unwrap();
    let owner = PgPool::connect(&url).await.unwrap();
    let schema = format!("talocart_module_mode_{}", std::process::id());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&owner)
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    let test_pool = pool.clone();
    let result = tokio::spawn(async move {
        let old = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                MIGRATOR
                    .iter()
                    .filter(|m| m.version <= 4)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        old.run(&test_pool).await.unwrap();
        sqlx::query(
            "INSERT INTO store_settings(id,name,legal_name) VALUES(1,'Preserved','Legal name')",
        )
        .execute(&test_pool)
        .await
        .unwrap();
        sqlx::query("UPDATE storefront_settings SET maintenance_enabled=true WHERE id=1")
            .execute(&test_pool)
            .await
            .unwrap();
        let before: String = sqlx::query_scalar("SELECT to_jsonb(s)::text FROM store_settings s")
            .fetch_one(&test_pool)
            .await
            .unwrap();
        database::migrate(&test_pool).await.unwrap();
        assert_eq!(
            before,
            sqlx::query_scalar::<_, String>(
                "SELECT (to_jsonb(s)-'module_install_mode')::text FROM store_settings s"
            )
            .fetch_one(&test_pool)
            .await
            .unwrap()
        );
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT maintenance_enabled FROM storefront_settings")
                .fetch_one(&test_pool)
                .await
                .unwrap()
        );
        for fallback in [PackageInstall::Cli, PackageInstall::Panel] {
            assert_eq!(
                talocart_platform::settings::module_install(&test_pool, fallback)
                    .await
                    .unwrap(),
                fallback
            );
        }
        sqlx::query("UPDATE store_settings SET module_install_mode='panel'")
            .execute(&test_pool)
            .await
            .unwrap();
        assert_eq!(
            talocart_platform::settings::module_install(&test_pool, PackageInstall::Cli)
                .await
                .unwrap(),
            PackageInstall::Panel
        );
        assert!(
            sqlx::query("UPDATE store_settings SET module_install_mode='invalid'")
                .execute(&test_pool)
                .await
                .is_err()
        );
        database::migrate(&test_pool).await.unwrap();
        database::check(&test_pool).await.unwrap();
    })
    .await;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&owner)
        .await
        .unwrap();
    result.unwrap();
}
