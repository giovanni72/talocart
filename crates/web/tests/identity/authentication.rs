use super::*;
use talocart_platform::identity::IdentityError;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bootstrap_without_code_csrf_and_concurrent_single_owner() {
    isolated(|pool,_|async move {
        let service=test_identity(pool.clone()).await;
        let app=router(test_app(pool.clone(),"Koala".into()).await);
        assert!(!service.installed().await.unwrap());
        assert_eq!(request(&app,"GET","/admin","","",false).await.status(),StatusCode::SEE_OTHER);
        let response=request(&app,"GET","/setup","","",false).await;
        let setup_cookie=cookie(&response);let setup_csrf=csrf(&html(response).await);
        let body=format!("csrf={setup_csrf}&email=owner%40example.com&password=correct+horse+battery&name=Shop&primary_locale=it&locale=it&locale=en");
        assert_eq!(request(&app,"POST","/setup",&setup_cookie,&body.replace(&format!("csrf={setup_csrf}"),"csrf=wrong"),false).await.status(),StatusCode::FORBIDDEN);
        assert_eq!(request(&app,"POST","/setup","",&body,false).await.status(),StatusCode::FORBIDDEN);
        assert!(!service.installed().await.unwrap());
        let (a,b)=tokio::join!(service.bootstrap("owner@example.com","correct horse battery".into(),"Shop",""),service.bootstrap("owner@example.com","correct horse battery".into(),"Shop",""));
        assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        let loser=if a.is_err() {a} else {b};assert!(matches!(loser,Err(IdentityError::Installed)));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM administrators").fetch_one(&pool).await.unwrap(),1);
        assert!(matches!(service.bootstrap("another@example.com","correct horse battery".into(),"Another","" ).await,Err(IdentityError::Installed)));
        assert!(service.session(setup_cookie.split('=').nth(1).unwrap()).await.unwrap().is_none());
        assert_eq!(request(&app,"GET","/setup","","",false).await.status(),StatusCode::SEE_OTHER);
        assert_eq!(request(&app,"POST","/setup",&setup_cookie,&body,false).await.status(),StatusCode::FORBIDDEN);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn login_settings_and_persistence_http_contract() {
    isolated(|pool, options| async move {
        let service = installed(&pool).await;
        let app = router(test_app(pool.clone(), "Koala".into()).await);
        let response = request(&app, "GET", "/admin/login", "", "", false).await;
        let anonymous_cookie = cookie(&response);
        let login_csrf = csrf(&html(response).await);
        let invalid = request(
            &app,
            "POST",
            "/admin/login",
            &anonymous_cookie,
            &format!("csrf={login_csrf}&email=owner%40example.com&password=wrong"),
            false,
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
        let error_html = html(invalid).await;
        assert!(error_html.contains("role=\"alert\""));
        assert!(!error_html.contains("value=\"wrong\""));
        let response = request(
            &app,
            "POST",
            "/admin/login",
            &anonymous_cookie,
            &format!("csrf={login_csrf}&email=owner%40example.com&password=correct+horse+battery"),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let authenticated = cookie(&response);
        assert_ne!(anonymous_cookie, authenticated);
        let raw = authenticated.split('=').nth(1).unwrap();
        assert!(
            service
                .session(anonymous_cookie.split('=').nth(1).unwrap())
                .await
                .unwrap()
                .is_none()
        );
        let session = service.session(raw).await.unwrap().unwrap();
        assert!(session.require_owner().is_ok());
        let saved: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM admin_sessions WHERE token_hash=$1)")
                .bind(raw)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!saved);
        for (path, partial) in [
            ("/status", false),
            ("/status", true),
            ("/admin/status", false),
            ("/admin/status", true),
        ] {
            let response = request(&app, "GET", path, &authenticated, "", partial).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(response.headers().contains_key("x-request-id"));
            assert!(
                response.headers()[header::VARY]
                    .to_str()
                    .unwrap()
                    .contains("HX-Request")
            );
            let body = html(response).await;
            assert_eq!(body.contains("<!doctype html>"), !partial);
            assert_eq!(body.contains("id=\"check-status\""), !partial);
            assert!(body.contains("Le fondamenta sono pronte"));
        }
        let response = request(&app, "GET", "/admin", &authenticated, "", false).await;
        let admin_html = html(response).await;
        assert!(admin_html.contains("<h1>Panoramica</h1>"));
        assert_eq!(admin_html.matches("class=\"dashboard-metric\"").count(), 9);
        assert!(admin_html.contains("href=\"/admin/settings/email\""));
        assert!(admin_html.contains("href=\"/admin/operations\""));
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings",
                &authenticated,
                "csrf=wrong&name=Changed",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings",
                &authenticated,
                &format!(
                    "csrf={}&name=%3Cscript%3Ealert%281%29%3C%2Fscript%3E",
                    session.csrf
                ),
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        let store = html(request(&app, "GET", "/", "", "", false).await).await;
        assert!(!store.contains("<script>alert(1)</script>"));
        assert!(store.contains("&#60;script&#62;"));
        // Reconnecting and rebuilding application state must preserve sessions and settings.
        let restarted_pool = PgPoolOptions::new().connect_with(options).await.unwrap();
        let restarted = test_identity(restarted_pool.clone()).await;
        assert!(restarted.session(raw).await.unwrap().is_some());
        assert_eq!(
            restarted.settings().await.unwrap().unwrap().values.name,
            "<script>alert(1)</script>"
        );
        let restarted_app = router(test_app(restarted_pool.clone(), "Wrong fallback".into()).await);
        assert_eq!(
            request(&restarted_app, "GET", "/admin", &authenticated, "", false)
                .await
                .status(),
            StatusCode::OK
        );
        restarted_pool.close().await;
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn role_changes_and_deactivation_apply_to_existing_sessions() {
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let app = router(test_app(pool.clone(), "Koala".into()).await);
        let raw = authenticated(&service).await;
        let authenticated = format!("talocart_session={raw}");
        let session = service.session(&raw).await.unwrap().unwrap();
        // Role changes and account deactivation take effect without waiting for session expiry.
        sqlx::query("UPDATE administrators SET role='viewer'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings",
                &authenticated,
                &format!("csrf={}&name=Denied", session.csrf),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let viewer = html(request(&app, "GET", "/admin", &authenticated, "", false).await).await;
        assert!(viewer.contains("data-metric=\"dash-mail\""));
        assert!(!viewer.contains("href=\"/admin/operations\""));
        assert!(!viewer.contains("Salva impostazioni"));
        sqlx::query("UPDATE administrators SET active=false")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            request(&app, "GET", "/admin", &authenticated, "", true)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        sqlx::query("UPDATE administrators SET role='owner',active=true")
            .execute(&pool)
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn recovery_tokens_expire_replace_and_revoke_sessions() {
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let app = router(test_app(pool.clone(), "Koala".into()).await);
        let raw = authenticated(&service).await;
        let authenticated = format!("talocart_session={raw}");
        let session = service.session(&raw).await.unwrap().unwrap();
        // Recovery tokens expire, are replaced, and are single-use; all active sessions are revoked.
        let expired = service.issue_reset("owner@example.com").await.unwrap();
        sqlx::query("UPDATE password_resets SET expires_at=now()-interval '1 second'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            service
                .reset(
                    &session,
                    &session.csrf,
                    &expired,
                    "new correct horse".into()
                )
                .await
                .is_err()
        );
        let replaced = service.issue_reset("owner@example.com").await.unwrap();
        let reset = service.issue_reset("owner@example.com").await.unwrap();
        assert!(
            service
                .reset(
                    &session,
                    &session.csrf,
                    &replaced,
                    "new correct horse".into()
                )
                .await
                .is_err()
        );
        assert!(
            service
                .reset(&session, "wrong", &reset, "new correct horse".into())
                .await
                .is_err()
        );
        let response = request(
            &app,
            "POST",
            "/admin/reset",
            &authenticated,
            &format!(
                "csrf={}&token={reset}&password=new+correct+horse",
                session.csrf
            ),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(
            response.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
        assert!(service.session(&raw).await.unwrap().is_none());
        let (_, anon) = service.anonymous().await.unwrap();
        assert!(
            service
                .reset(&anon, &anon.csrf, &reset, "other correct horse".into())
                .await
                .is_err()
        );
        assert!(matches!(
            service
                .login(
                    &anon,
                    &anon.csrf,
                    "owner@example.com",
                    "correct horse battery".into()
                )
                .await,
            Err(IdentityError::Unauthorized)
        ));
        let new_raw = service
            .login(
                &anon,
                &anon.csrf,
                "owner@example.com",
                "new correct horse".into(),
            )
            .await
            .unwrap();
        assert!(service.session(&new_raw).await.unwrap().is_some());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn logout_requires_post_and_csrf_and_revokes_session() {
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let app = router(test_app(pool.clone(), "Koala".into()).await);
        let new_raw = authenticated(&service).await;
        let new_session = service.session(&new_raw).await.unwrap().unwrap();
        assert!(service.logout(&new_session, "wrong").await.is_err());
        assert!(service.session(&new_raw).await.unwrap().is_some());
        let get_logout = request(
            &app,
            "GET",
            "/admin/logout",
            &format!("talocart_session={new_raw}"),
            "",
            false,
        )
        .await;
        assert_eq!(get_logout.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert!(service.session(&new_raw).await.unwrap().is_some());
        let logged_out = request(
            &app,
            "POST",
            "/admin/logout",
            &format!("talocart_session={new_raw}"),
            &format!("csrf={}", new_session.csrf),
            false,
        )
        .await;
        assert_eq!(logged_out.status(), StatusCode::SEE_OTHER);
        assert!(service.session(&new_raw).await.unwrap().is_none());
        assert_eq!(
            request(&app, "GET", "/admin/logout", "", "", false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn failed_login_limits_persist_and_expire() {
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        // Account limits persist and count concurrent submissions atomically.
        let (_, anon) = service.anonymous().await.unwrap();
        for _ in 0..5 {
            assert!(matches!(
                service
                    .login(&anon, &anon.csrf, "owner@example.com", "wrong".into())
                    .await,
                Err(IdentityError::Unauthorized)
            ));
        }
        let restarted = test_identity(pool.clone()).await;
        assert!(matches!(
            restarted
                .login(
                    &anon,
                    &anon.csrf,
                    "owner@example.com",
                    "correct horse battery".into()
                )
                .await,
            Err(IdentityError::Limited)
        ));
        sqlx::query("UPDATE auth_limits SET expires_at=now()-interval '1 second'")
            .execute(&pool)
            .await
            .unwrap();
        let _raw = service
            .login(
                &anon,
                &anon.csrf,
                "owner@example.com",
                "correct horse battery".into(),
            )
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn expired_session_is_rejected() {
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        sqlx::query(
            "UPDATE admin_sessions SET expires_at=now()-interval '1 second' WHERE token_hash=$1",
        )
        .bind(digest(&raw))
        .execute(&pool)
        .await
        .unwrap();
        assert!(service.session(&raw).await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn successful_logins_do_not_lock_the_account() {
    isolated(|pool, _| async move {
        let service = test_identity(pool).await;
        service
            .bootstrap(
                "owner@example.com",
                "correct horse battery".into(),
                "Shop",
                "",
            )
            .await
            .unwrap();
        for _ in 0..7 {
            let (_, session) = service.anonymous().await.unwrap();
            let raw = service
                .login(
                    &session,
                    &session.csrf,
                    "owner@example.com",
                    "correct horse battery".into(),
                )
                .await
                .expect("Successful login must not consume failure budget");
            let authenticated = service.session(&raw).await.unwrap().unwrap();
            service
                .logout(&authenticated, &authenticated.csrf)
                .await
                .unwrap();
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn settings_read_failure_is_not_a_missing_configuration() {
    isolated(|pool, _| async move {
        let app = router(test_app(pool.clone(), "Fallback".into()).await);
        assert_eq!(
            request(&app, "GET", "/", "", "", false).await.status(),
            StatusCode::OK
        );
        sqlx::query("ALTER TABLE store_settings RENAME TO unavailable_settings")
            .execute(&pool)
            .await
            .unwrap();
        let response = request(&app, "GET", "/", "", "", false).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = html(response).await;
        assert!(!body.contains("unavailable_settings"));
        assert!(!body.contains("Fallback"));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn setup_validates_and_persists_display_name_before_first_login() {
    isolated(|pool, options| async move {
        let service = test_identity(pool.clone()).await;
        let app = router(test_app(pool.clone(), "Koala".into()).await);
        let response = request(&app, "GET", "/setup", "", "", false).await;
        let anon = cookie(&response);
        let body = html(response).await;
        let token = csrf(&body);
        assert!(body.contains("label for=\"display-name\">Display name"));
        assert!(body.contains("name=\"display_name\""));
        for invalid in ["a".repeat(81), "Invalid%00Name".into()] {
            let response = request(&app, "POST", "/setup", &anon, &format!("csrf={token}&email=owner%40example.com&password=correct+horse+battery&name=Shop&primary_locale=it&locale=it&locale=en&display_name={invalid}"), false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert!(html(response).await.contains("role=\"alert\""));
            assert!(!service.installed().await.unwrap());
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM administrators")
                .fetch_one(&pool).await.unwrap();
            assert_eq!(count, 0);
            assert!(service.settings().await.unwrap().is_none());
        }
        // Exactly 80 Unicode characters, trimmed, with HTML significant characters.
        let name = format!("{}<>&", "é".repeat(77));
        let encoded = format!("++{}%3C%3E%26++", "%C3%A9".repeat(77));
        let response = request(&app, "POST", "/setup", &anon, &format!("csrf={token}&email=owner%40example.com&password=correct+horse+battery&name=Shop&primary_locale=it&locale=it&locale=en&display_name={encoded}"), false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/admin/login");
        let persisted: String = sqlx::query_scalar("SELECT display_name FROM administrators WHERE role='owner'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(persisted, name);
        let reopened = PgPoolOptions::new().connect_with(options).await.unwrap();
        let service = test_identity(reopened.clone()).await;
        let raw = authenticated(&service).await;
        let session = service.session(&raw).await.unwrap().unwrap();
        assert_eq!(session.display_name, name);
        assert_eq!(service.account(&session).await.unwrap().display_name, name);
        let app = router(test_app(reopened.clone(), "Koala".into()).await);
        let profile = html(request(&app, "GET", "/admin/account", &format!("talocart_session={raw}"), "", false).await).await;
        assert!(profile.contains(&format!("{}&#60;&#62;&#38;", "é".repeat(77))));
        assert!(!profile.contains(&name));
        reopened.close().await;
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn complete_http_setup_login_settings_logout() {
    isolated(|pool, _| async move {
        let service = test_identity(pool.clone()).await;
        let app = router(test_app_config(pool, "Koala".into(), talocart_web::AppStateConfig { secure_cookies: true, ..Default::default() }).await);
        let response = request(&app, "GET", "/setup", "", "", false).await;
        let anon = cookie(&response);
        let token = csrf(&html(response).await);
        let response = request(&app, "POST", "/setup", &anon, &format!("csrf={token}&email=owner%40example.com&password=correct+horse+battery&name=Shop&primary_locale=it&locale=it&locale=en"), false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let response = request(&app, "GET", "/admin/login", "", "", false).await;
        assert_cookie_lifetime(&response, talocart_platform::identity::ANONYMOUS_SESSION_SECONDS);
        let anon = cookie(&response);
        let token = csrf(&html(response).await);
        let response = request(&app, "POST", "/admin/login", &anon, &format!("csrf={token}&email=owner%40example.com&password=correct+horse+battery"), false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_cookie_lifetime(&response, talocart_platform::identity::AUTHENTICATED_SESSION_SECONDS);
        let auth = cookie(&response);
        let session = service.session(auth.split('=').nth(1).unwrap()).await.unwrap().unwrap();
        assert_eq!(session.display_name, "");
        assert!(html(request(&app, "GET", "/admin/account", &auth, "", false).await).await.contains("owner@example.com"));
        let token = csrf(&html(request(&app,"GET","/admin",&auth,"",false).await).await);
        let response = request(&app,"POST","/admin/settings",&auth,&format!("csrf={token}&name=Updated"),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(service.settings().await.unwrap().unwrap().values.name,"Updated");
        let response = request(&app,"POST","/admin/logout",&auth,&format!("csrf={token}"),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(request(&app,"GET","/admin",&auth,"",true).await.status(),StatusCode::UNAUTHORIZED);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn concurrent_failures_are_counted_and_global_limit_remains() {
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let (_, a) = service.anonymous().await.unwrap();
        let (_, b) = service.anonymous().await.unwrap();
        let (first, second) = tokio::join!(
            service.login(&a, &a.csrf, "owner@example.com", "wrong".into()),
            service.login(&b, &b.csrf, "owner@example.com", "wrong".into())
        );
        assert!(matches!(first, Err(IdentityError::Unauthorized)));
        assert!(matches!(second, Err(IdentityError::Unauthorized)));
        let bucket = format!("login-failed:{}", digest("owner@example.com"));
        let attempts: i32 = sqlx::query_scalar("SELECT attempts FROM auth_limits WHERE bucket=$1")
            .bind(&bucket)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(attempts, 2);
        let raw = authenticated(&service).await;
        assert!(service.session(&raw).await.unwrap().is_some());
        let attempts: i32 = sqlx::query_scalar("SELECT attempts FROM auth_limits WHERE bucket=$1")
            .bind(&bucket)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            attempts, 2,
            "Success neither consumes nor erases the failure budget"
        );
        sqlx::query("UPDATE auth_limits SET attempts=100 WHERE bucket='login-global'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            service
                .login(
                    &a,
                    &a.csrf,
                    "owner@example.com",
                    "correct horse battery".into()
                )
                .await,
            Err(IdentityError::Limited)
        ));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn database_diagnostics_keep_sqlstate_without_query_details() {
    isolated(|pool, _| async move {
        let error = sqlx::query("SELECT secret_value FROM private_missing_table").execute(&pool).await.unwrap_err();
        let error = IdentityError::from(error);
        assert!(matches!(&error,IdentityError::Technical(talocart_platform::technical::TechnicalError::Database(cause)) if cause.category()=="database" && cause.sqlstate()==Some("42P01")));
        let diagnostic=format!("{error:?} {error}");
        assert!(!diagnostic.contains("private_missing_table"));
        assert!(!diagnostic.contains("secret_value"));
        // Exercise the real PostgreSQL SQLSTATE through the nested order/cart cause.
        for (statement, retryable) in [
            ("DO $$ BEGIN RAISE EXCEPTION USING ERRCODE='40001', MESSAGE='synthetic retry'; END $$", true),
            ("DO $$ BEGIN RAISE EXCEPTION USING ERRCODE='40P01', MESSAGE='synthetic retry'; END $$", true),
            ("DO $$ BEGIN RAISE EXCEPTION USING ERRCODE='23505', MESSAGE='synthetic constraint'; END $$", false),
        ] {
            let database=sqlx::query(statement).execute(&pool).await.unwrap_err();
            let error=talocart_commerce::orders::OrderError::Cart(talocart_commerce::cart::CartError::from(database));
            assert_eq!(error.technical().unwrap().retryable(),retryable);
        }

    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn disabled_accounts_cannot_consume_recovery_tokens() {
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let reset = service.issue_reset("owner@example.com").await.unwrap();
        let (_, anon) = service.anonymous().await.unwrap();
        sqlx::query("UPDATE administrators SET active=false")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            service
                .reset(&anon, &anon.csrf, &reset, "new correct horse".into())
                .await,
            Err(IdentityError::Unauthorized)
        ));
    })
    .await;
}
