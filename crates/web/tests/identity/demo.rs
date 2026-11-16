use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn demo_preview_concurrent_install_preserves_existing_data_and_user_edits() {
    use talocart_commerce::{
        demo::{DemoSeeder, Outcome, SeedError},
        products::Products,
    };
    isolated(|pool,_|async move{
        let identity=installed(&pool).await;
        sqlx::query("UPDATE pricing_settings SET price_mode='exclusive'").execute(&pool).await.unwrap();
        let raw=authenticated(&identity).await;let session=identity.session(&raw).await.unwrap().unwrap();
        let media=test_media(&pool).await;let products=Products::new(pool.clone(),identity.clone(),media.clone());
        let mut original=product_values("ORIGINAL","original");original.status="draft".into();original.category_id.clear();original.tax_class_id.clear();original.price_mode="exclusive".into();
        let id=products.save(&session,&session.csrf,None,&original).await.unwrap();
        products.image(&session,&session.csrf,id,Some(include_bytes!("../fixtures/logo.png").to_vec()),"demo-mug.png").await.unwrap();
        let old=products.entry(&session,id).await.unwrap();let old_key=old.image_key.unwrap();let old_bytes=media.read(&old_key).await.unwrap();
        let untouched:String=sqlx::query_scalar("SELECT json_build_array((SELECT row_to_json(p) FROM catalog_products p WHERE model='ORIGINAL'),(SELECT row_to_json(s) FROM store_settings s),(SELECT row_to_json(i) FROM identity_state i),(SELECT row_to_json(s) FROM pricing_settings s))::text").fetch_one(&pool).await.unwrap();
        let sequence:(i64,bool)=sqlx::query_as("SELECT last_value,is_called FROM catalog_products_id_seq").fetch_one(&pool).await.unwrap();
        let seeder=DemoSeeder::new(pool.clone(),media.clone());let preview=seeder.preview().await.unwrap();assert_eq!(preview.outcome,Outcome::Preview);assert_eq!(preview.price_mode,"exclusive");
        assert_eq!(sequence,sqlx::query_as::<_,(i64,bool)>("SELECT last_value,is_called FROM catalog_products_id_seq").fetch_one(&pool).await.unwrap());assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_demo_packages").fetch_one(&pool).await.unwrap(),0);
        let(a,b)=tokio::join!(seeder.install(),seeder.install());let(a,b)=(a.unwrap(),b.unwrap());assert!(matches!((&a.outcome,&b.outcome),(Outcome::Installed,Outcome::AlreadyInstalled)|(Outcome::AlreadyInstalled,Outcome::Installed)));
        assert_eq!(untouched,sqlx::query_scalar::<_,String>("SELECT json_build_array((SELECT row_to_json(p) FROM catalog_products p WHERE model='ORIGINAL'),(SELECT row_to_json(s) FROM store_settings s),(SELECT row_to_json(i) FROM identity_state i),(SELECT row_to_json(s) FROM pricing_settings s))::text").fetch_one(&pool).await.unwrap());assert_eq!(old_bytes,media.read(&old_key).await.unwrap());
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_products").fetch_one(&pool).await.unwrap(),31);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_variants").fetch_one(&pool).await.unwrap(),4);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_visible_products WHERE model <> 'ORIGINAL'").fetch_one(&pool).await.unwrap(),27);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM (SELECT model AS code FROM catalog_products UNION ALL SELECT sku AS code FROM catalog_variants) s WHERE code ILIKE '%demo%'").fetch_one(&pool).await.unwrap(),0);
        let demo_mug:String=sqlx::query_scalar("SELECT image_key FROM catalog_products WHERE model='MUG'").fetch_one(&pool).await.unwrap();assert_eq!(demo_mug,"demo-mug-2.png");assert_ne!(demo_mug,old_key);
        let app=router(test_app(pool.clone(),"Shop".into()).await);let mug=html(request(&app,"GET","/products/tazza-atelier","","",false).await).await;assert!(mug.contains("18,18 €"),"Demo obeys the existing exclusive price mode, 14.90 + 22%");
        for slug in ["prodotto-bozza","categoria-disattivata","varianti-da-completare"]{assert_eq!(request(&app,"GET",&format!("/products/{slug}"),"","",false).await.status(),StatusCode::NOT_FOUND);}
        let images:i64=sqlx::query_scalar("SELECT count(*) FROM media_objects").fetch_one(&pool).await.unwrap();
        sqlx::query("UPDATE catalog_products SET name='Demo edited',price_cents=777 WHERE model='MUG'").execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM catalog_products WHERE model='DRAFT'").execute(&pool).await.unwrap();
        assert_eq!(seeder.install().await.unwrap().outcome,Outcome::AlreadyInstalled);assert_eq!(seeder.preview().await.unwrap().outcome,Outcome::AlreadyInstalled);
        assert_eq!(sqlx::query_as::<_,(String,i64)>("SELECT name,price_cents FROM catalog_products WHERE model='MUG'").fetch_one(&pool).await.unwrap(),("Demo edited".into(),777));assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_objects").fetch_one(&pool).await.unwrap(),images);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_products WHERE model='DRAFT'").fetch_one(&pool).await.unwrap(),0);
        assert_eq!(sqlx::query_scalar::<_,i32>("SELECT version FROM catalog_demo_packages WHERE package='catalog-demo'").fetch_one(&pool).await.unwrap(),talocart_commerce::demo::VERSION,"The installed receipt is unchanged");
        for version in [1, 2, 3, 4, talocart_commerce::demo::VERSION + 1] {
            sqlx::query("UPDATE catalog_demo_packages SET version=$1 WHERE package='catalog-demo'").bind(version).execute(&pool).await.unwrap();
            let before = demo_existing_snapshot(&pool).await;
            assert!(matches!(seeder.preview().await,Err(SeedError::Dataset)));
            assert!(matches!(seeder.install().await,Err(SeedError::Dataset)));
            assert_eq!(demo_existing_snapshot(&pool).await, before);
            assert_eq!(sqlx::query_scalar::<_,i32>("SELECT version FROM catalog_demo_packages").fetch_one(&pool).await.unwrap(),version);
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_objects").fetch_one(&pool).await.unwrap(),images);
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn demo_rejects_uninstalled_conflicts_and_capacity_without_partial_rows_or_files() {
    use talocart_commerce::demo::{DemoSeeder, SeedError};
    isolated(|pool, _| async move {
        let media = test_media(&pool).await;
        let seeder = DemoSeeder::new(pool.clone(), media.clone());
        assert!(matches!(
            seeder.preview().await,
            Err(SeedError::NotInstalled)
        ));
        assert!(matches!(
            seeder.install().await,
            Err(SeedError::NotInstalled)
        ));
        assert!(!media.root().exists());
        installed(&pool).await;
        sqlx::query(
            "INSERT INTO catalog_brands(name,slug) VALUES('Talocart Atelier','original-brand')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO catalog_brands_translations(entity_id,locale,name) SELECT id,'it',name FROM catalog_brands WHERE slug='original-brand'").execute(&pool).await.unwrap();
        assert!(matches!(seeder.preview().await, Err(SeedError::Collision)));
        assert!(matches!(seeder.install().await, Err(SeedError::Collision)));
        assert!(!media.root().exists());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_products")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("DELETE FROM catalog_brands")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO catalog_categories(name,slug) SELECT 'Existing '||n,'existing-'||n FROM generate_series(1,999)n",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(matches!(seeder.install().await, Err(SeedError::Capacity)));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_products")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("DELETE FROM catalog_categories")
            .execute(&pool)
            .await
            .unwrap();
        tokio::fs::write(media.root(), b"storage blocked")
            .await
            .unwrap();
        assert!(matches!(seeder.install().await, Err(SeedError::Media(_))));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_demo_packages")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_products")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        tokio::fs::remove_file(media.root()).await.unwrap();
        assert!(seeder.install().await.is_ok());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn setup_demo_checkbox_defaults_off_preserves_selection_and_commits_one_owner_with_catalog() {
    isolated(|pool,_|async move{
        let identity=test_identity(pool.clone()).await;let app=router(test_app(pool.clone(),"Koala".into()).await);let response=request(&app,"GET","/setup","","",false).await;let cookie=cookie(&response);let body=html(response).await;let csrf=csrf(&body);let input=body.split("id=\"install-demo\"").nth(1).unwrap().split('>').next().unwrap();assert!(!input.contains("checked"));assert!(!input.contains("disabled"));
        let invalid=format!("csrf={csrf}&name=Demo&primary_locale=it&locale=it&locale=en&email=owner%40example.test&password=short&install_demo=true");let response=request(&app,"POST","/setup",&cookie,&invalid,false).await;assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);assert!(html(response).await.split("id=\"install-demo\"").nth(1).unwrap().split('>').next().unwrap().contains("checked"));
        let body=format!("csrf={csrf}&name=Demo&primary_locale=it&locale=it&locale=en&email=owner%40example.test&password=long-development-password&install_demo=true");
        let wrong=body.replace(&format!("csrf={csrf}"),"csrf=wrong");assert_eq!(request(&app,"POST","/setup",&cookie,&wrong,false).await.status(),StatusCode::FORBIDDEN);assert!(!identity.installed().await.unwrap());
        let(a,b)=tokio::join!(request(&app,"POST","/setup",&cookie,&body,false),request(&app,"POST","/setup",&cookie,&body,false));let statuses=[a.status(),b.status()];assert_eq!(statuses.iter().filter(|s|**s==StatusCode::SEE_OTHER).count(),1);assert!(statuses.iter().all(|s|matches!(*s,StatusCode::SEE_OTHER|StatusCode::CONFLICT|StatusCode::FORBIDDEN)));
        assert!(identity.installed().await.unwrap());assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM administrators").fetch_one(&pool).await.unwrap(),1);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_products").fetch_one(&pool).await.unwrap(),30);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_demo_packages").fetch_one(&pool).await.unwrap(),1);
        let body=html(request(&app,"GET","/catalog","","",false).await).await;assert_eq!(body.matches("class=\"store-product\"").count(),21);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn setup_demo_failure_rolls_back_owner_settings_languages_and_all_catalog_then_plain_setup_succeeds()
 {
    isolated(|pool,_|async move{
        let identity=test_identity(pool.clone()).await;let app=router(test_app(pool.clone(),"Koala".into()).await);let response=request(&app,"GET","/setup","","",false).await;let cookie=cookie(&response);let csrf=csrf(&html(response).await);
        sqlx::query("INSERT INTO catalog_brands(name,slug) VALUES('Talocart Atelier','existing')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_brands_translations(entity_id,locale,name) SELECT id,'en',name FROM catalog_brands WHERE slug='existing'").execute(&pool).await.unwrap();
        let body=format!("csrf={csrf}&name=Demo&primary_locale=en&locale=en&email=owner%40example.test&password=long-development-password&install_demo=true");
        let response=request(&app,"POST","/setup",&cookie,&body,false).await;assert_eq!(response.status(),StatusCode::SERVICE_UNAVAILABLE);let html=html(response).await;assert!(html.contains("Installation incomplete. No data was saved"));assert!(html.split("id=\"install-demo\"").nth(1).unwrap().split('>').next().unwrap().contains("checked"));assert!(!identity.installed().await.unwrap());
        let settings=talocart_platform::storefront::Storefront::new(pool.clone()).settings().await.unwrap(); assert_eq!(settings.primary_locale,"it"); assert_eq!(settings.active_locales.len(),2); assert!(settings.published_prefixes.is_empty());
        for table in ["administrators","store_settings","catalog_products","catalog_demo_packages","media_objects"]{assert_eq!(sqlx::query_scalar::<_,i64>(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}"))).fetch_one(&pool).await.unwrap(),0);}
        assert_eq!(request(&app,"POST","/setup",&cookie,&body.replace("&install_demo=true",""),false).await.status(),StatusCode::SEE_OTHER);assert!(identity.installed().await.unwrap()); let settings=talocart_platform::storefront::Storefront::new(pool.clone()).settings().await.unwrap();assert_eq!(settings.primary_locale,"en");assert_eq!(settings.active_locales,vec!["en"]);assert!(settings.published_prefixes.is_empty());assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_products").fetch_one(&pool).await.unwrap(),0);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn demo_install_allows_more_than_one_hundred_existing_options() {
    isolated(|pool, _| async move {
        installed(&pool).await;
        sqlx::query(
            "INSERT INTO catalog_options(name) SELECT 'Existing '||n FROM generate_series(1,105)n",
        )
        .execute(&pool)
        .await
        .unwrap();
        let media = test_media(&pool).await;
        let seeder = talocart_commerce::demo::DemoSeeder::new(pool.clone(), media);
        assert!(seeder.preview().await.is_ok());
        assert!(seeder.install().await.is_ok());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM catalog_options WHERE name LIKE 'Existing %'"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            105
        );
        assert!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_options")
                .fetch_one(&pool)
                .await
                .unwrap()
                > 105
        );
        assert!(seeder.install().await.is_ok());
    })
    .await;
}
