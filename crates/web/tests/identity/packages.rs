use super::*;
#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn packages_are_owner_only_csrf_protected_and_bilingual_without_koala_schema() {
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let raw=authenticated(&identity).await;
        let session=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let folder=std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("talocart-package-http-{}",talocart_platform::identity::token().unwrap()));
        let manager=talocart_platform::packages::Manager::open(talocart_platform::packages::RuntimeConfig {root:folder.join("packages"),database_url:"postgres://unused@127.0.0.1:1/unused_test".into(),media_root:folder.join("media"),store_name:"Synthetic".into(),public_origin:"http://127.0.0.1:1".into(),secure_cookies:false,compiled:vec![],serve:true}).await.unwrap();
        let app=router(test_app_config(pool.clone(),"Synthetic".into(), talocart_web::AppStateConfig { packages: Some(manager.clone()), package_install: talocart_platform::config::PackageInstall::Panel, ..Default::default() }).await);
        for locale in ["en","it"] {
            sqlx::query("UPDATE administrators SET locale=$1 WHERE id=$2").bind(locale).bind(session.administrator_id).execute(&pool).await.unwrap();
            let response=request(&app,"GET","/admin/modules",&cookie,"",false).await;
            assert_eq!(response.status(),StatusCode::OK);assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");
            let body=html(response).await;assert!(body.contains(if locale=="it" {"Pacchetti installabili"} else {"Installable packages"}));assert!(body.contains("id=\"package-file\""));assert!(body.contains("enctype=\"multipart/form-data\""));assert!(body.contains("ui-control"));
            assert!(body.contains("data-package-contracts"));assert!(!body.contains("package-public-key"));
            assert!(body.contains(if locale=="it" {"Contratto database"} else {"Database contract"}));
            assert!(body.contains(talocart_platform::packages::sdk::VERSION));
            let response=request(&app,"POST","/admin/modules/publishers",&cookie,&format!("csrf={}&public_key=bad",session.csrf),false).await;
            assert_eq!(response.status(),StatusCode::NOT_FOUND);
        }
        for action in ["install","update","retry","uninstall","toggle","restart","critical"] {
            let body=match action {"critical"=>"csrf=wrong&id=fixture&critical=false","install"|"update"=>"csrf=wrong&preview=invalid","retry"|"uninstall"|"restart"=>"csrf=wrong&id=fixture",_=>"csrf=wrong&id=fixture&enabled=true"};
            assert_eq!(request(&app,"POST",&format!("/admin/modules/{action}"),&cookie,body,false).await.status(),StatusCode::FORBIDDEN);
            assert_eq!(request(&app,"POST",&format!("/admin/modules/{action}"),"",body,false).await.status(),StatusCode::SEE_OTHER);
            // No mutation through GET.
            assert_eq!(request(&app,"GET",&format!("/admin/modules/{action}"),&cookie,"",false).await.status(),StatusCode::METHOD_NOT_ALLOWED);
        }
        // Unknown IDs and previews are localized errors, never partial mutations.
        for locale in ["en","it"] {
            sqlx::query("UPDATE administrators SET locale=$1 WHERE id=$2").bind(locale).bind(session.administrator_id).execute(&pool).await.unwrap();
            for (action,body) in [("update","preview=invalid"),("retry","id=fixture"),("uninstall","id=fixture"),("restart","id=fixture"),("critical","id=fixture&critical=false")] {
                let response=request(&app,"POST",&format!("/admin/modules/{action}"),&cookie,&format!("csrf={}&password=correct+horse+battery&{body}",session.csrf),false).await;
                assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
                let body=html(response).await;assert!(body.contains("id=\"package-error\""));
                assert!(body.contains(if locale=="it" {"Pacchetti installabili"} else {"Installable packages"}));
            }
        }
        assert!(manager.publishers().unwrap().is_empty());
        let token="invalid";assert_eq!(request(&app,"POST","/admin/modules/install",&cookie,&format!("csrf={}&password=correct+horse+battery&preview={token}",session.csrf),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        let cli = router(test_app_config(pool.clone(), "Synthetic".into(), talocart_web::AppStateConfig { packages: Some(manager.clone()), ..Default::default() }).await);
        for locale in ["en", "it"] {
            sqlx::query("UPDATE administrators SET locale=$1 WHERE id=$2").bind(locale).bind(session.administrator_id).execute(&pool).await.unwrap();
            let body=html(request(&cli,"GET","/admin/modules",&cookie,"",false).await).await;
            assert!(body.contains("packages-cli-only"));
            assert!(!body.contains("id=\"package-file\""));
            for action in ["preview", "install", "update", "retry"] {
                if action == "preview" { continue; } // Multipart denial covered by native browser test.
                assert_eq!(request(&cli,"POST",&format!("/admin/modules/{action}"),&cookie,&format!("csrf={}&preview=invalid&id=fixture&password=correct+horse+battery",session.csrf),false).await.status(),StatusCode::FORBIDDEN);
            }
        }
        for action in ["install", "update", "retry"] {
            assert_eq!(request(&app,"POST",&format!("/admin/modules/{action}"),&cookie,&format!("csrf={}&preview=invalid&id=fixture&password=wrong",session.csrf),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        }
        drop(cli);
        // Role/session revocation must be rechecked even for a previously loaded Session.
        sqlx::query("UPDATE administrators SET role='viewer' WHERE id=$1").bind(session.administrator_id).execute(&pool).await.unwrap();
        assert!(identity.authorize_owner(&session).await.is_err());
        assert_eq!(request(&app,"POST","/admin/modules/install",&cookie,&format!("csrf={}&preview=invalid",session.csrf),false).await.status(),StatusCode::FORBIDDEN);
        for (action,body) in [("update","preview=invalid"),("retry","id=fixture"),("uninstall","id=fixture"),("restart","id=fixture"),("critical","id=fixture&critical=false")] {
            assert_eq!(request(&app,"POST",&format!("/admin/modules/{action}"),&cookie,&format!("csrf={}&password=correct+horse+battery&{body}",session.csrf),false).await.status(),StatusCode::FORBIDDEN);
        }
        let table:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_tables WHERE schemaname=current_schema() AND tablename='catalog_import_jobs')").fetch_one(&pool).await.unwrap();assert!(!table);
        drop(app);drop(manager);std::fs::remove_dir_all(folder).unwrap();
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn base_migrations_pending_is_read_only_and_exact() {
    let latest = database::MIGRATOR
        .iter()
        .last()
        .expect("Base migrations")
        .version;
    isolated_before(latest - 1, |pool, _| async move {
        assert!(database::pending(&pool).await.unwrap());
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(database::pending(&pool).await.unwrap());
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before, after, "Diagnostics never migrate");
        database::migrate(&pool).await.unwrap();
        assert!(!database::pending(&pool).await.unwrap());
        assert_eq!(
            database::applied_migrations(&pool, "_missing_ledger")
                .await
                .unwrap(),
            None
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn standalone_module_readiness_checks_the_host_checkpoint_instead_of_its_old_migrator() {
    isolated(|pool, _| async move {
        use sqlx::migrate::{Migration, MigrationType, Migrator};
        use std::borrow::Cow;
        let required = database::contract();
        let mut migrations = database::MIGRATOR.iter().cloned().collect::<Vec<_>>();
        let next = migrations.last().unwrap().version + 1;
        migrations.push(Migration::new(
            next,
            Cow::Borrowed("future host readiness"),
            MigrationType::Simple,
            sqlx::SqlStr::from_static("CREATE TABLE sdk_readiness_addition(id BIGINT PRIMARY KEY)"),
            false,
        ));
        let future = Migrator {
            migrations: Cow::Owned(migrations),
            ..Migrator::DEFAULT
        };
        future.run(&pool).await.unwrap();
        let state = test_app(pool.clone(), "Future host".into())
            .await
            .standalone_module(
                database::contract_for(&future),
                required,
                talocart_platform::version::ApplicationIdentity::current(),
            );
        state.check_database().await.unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=$2")
            .bind(vec![0_u8; 48])
            .bind(next)
            .execute(&pool)
            .await
            .unwrap();
        assert!(state.check_database().await.is_err());
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn package_reauthentication_checks_current_owner_password_session_and_budget() {
    use talocart_platform::identity::IdentityError;
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        assert!(matches!(
            identity
                .reauthenticate_owner(&session, "wrong", "correct horse battery".into())
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            identity
                .reauthenticate_owner(&session, &session.csrf, "wrong".into())
                .await,
            Err(IdentityError::CurrentPassword)
        ));
        identity
            .reauthenticate_owner(&session, &session.csrf, "correct horse battery".into())
            .await
            .unwrap();
        sqlx::query("UPDATE administrators SET role='viewer' WHERE id=$1")
            .bind(session.administrator_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            identity
                .reauthenticate_owner(&session, &session.csrf, "correct horse battery".into())
                .await,
            Err(IdentityError::Forbidden)
        ));
        sqlx::query("UPDATE administrators SET role='owner' WHERE id=$1")
            .bind(session.administrator_id)
            .execute(&pool)
            .await
            .unwrap();
        for _ in 0..8 {
            assert!(matches!(
                identity
                    .reauthenticate_owner(&session, &session.csrf, "wrong".into())
                    .await,
                Err(IdentityError::CurrentPassword)
            ));
        }
        assert!(matches!(
            identity
                .reauthenticate_owner(&session, &session.csrf, "correct horse battery".into())
                .await,
            Err(IdentityError::Limited)
        ));
        sqlx::query("DELETE FROM admin_sessions WHERE token_hash=$1")
            .bind(&session.token_hash)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            identity
                .reauthenticate_owner(&session, &session.csrf, "correct horse battery".into())
                .await,
            Err(IdentityError::Forbidden)
        ));
    })
    .await;
}
