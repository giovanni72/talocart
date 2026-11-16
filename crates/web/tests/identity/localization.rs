include!("localization_support.rs");

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn english_admin_pages_fragments_and_validation() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        identity.update_locale(&session, &session.csrf, "en").await.unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool, "Shop".into()).await);
        for path in ["/admin", "/admin/account", "/admin/settings", "/admin/settings/appearance", "/admin/settings/storefront", "/admin/settings/taxes", "/admin/settings/taxes/rates", "/admin/settings/taxes/classes", "/admin/users", "/admin/users/new", "/admin/roles", "/admin/roles/new", "/admin/catalog/categories", "/admin/catalog/categories/new", "/admin/catalog/brands", "/admin/catalog/brands/new", "/admin/catalog/products", "/admin/catalog/products/new", "/admin/catalog/options", "/admin/catalog/options/new", "/admin/media", "/admin/components"] {
            let response = fiscal_request(&app, "GET", path, &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::OK, "Route: {path}");
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], "en");
            let page = html(response).await;
            assert!(page.contains("<html lang=\"en\">"), "Language: {path}");
            assert!(page.contains("aria-label=\"Administration\""));
            assert!(!page.contains("Missing or invalid translation"));
            if path == "/admin/catalog/products" {
                assert!(page.contains(">Type<"));
                assert!(page.contains("Sort by Type in ascending order"));
            }
        }
        let preview = html(fiscal_request(&app, "GET", "/admin/components?label=Prodotti&status=active&preview=1", &cookie, "", false).await).await;
        assert!(preview.contains("value=\"Prodotti\""), "User-entered preview values are preserved");
        let cache = html(fiscal_request(&app, "GET", "/admin/media/cache/status", &cookie, "", true).await).await;
        assert!(cache.starts_with("Ready images:"));
        let partial = fiscal_request(&app, "GET", "/admin/status", &cookie, "", true).await;
        assert_eq!(partial.headers()[header::CONTENT_LANGUAGE], "en");
        let fragment = html(partial).await;
        assert!(!fragment.contains("<html"));
        assert!(fragment.contains("The foundations are ready"));
        let invalid = fiscal_request(&app, "POST", "/admin/account/password", &cookie, &format!("csrf={}&current_password=wrong&new_password=valid+new+password&confirmation=valid+new+password", session.csrf), false).await;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(html(invalid).await.contains("The current password is incorrect"));
        let forbidden = fiscal_request(&app, "POST", "/admin/catalog/categories/new", &cookie, "csrf=wrong&name=Test&slug=test&description=&sort_order=0&active=true", false).await;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        assert_eq!(forbidden.headers()[header::CONTENT_LANGUAGE], "en");
        let forbidden = html(forbidden).await;
        // Full-page failures use the contextual admin layout introduced by error pages.
        assert!(forbidden.contains("<html lang=\"en\">"));
        assert!(forbidden.contains("data-error-status=\"403\""));
        assert!(forbidden.contains("Operation not allowed"));
        let missing = fiscal_request(&app, "GET", "/admin/missing-page", &cookie, "", false).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert!(html(missing).await.contains("This page does not exist."));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn personal_language_security_persistence_and_http_scope() {
    isolated(|pool, options| async move {
        use talocart_platform::identity::{Permission, Role};
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let second = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        assert_eq!(session.locale, "it");
        let app = router(
            test_app_config(
                pool.clone(),
                "Shop".into(),
                talocart_web::AppStateConfig {
                    secure_cookies: true,
                    ..Default::default()
                },
            )
            .await,
        );
        let owner_cookie = format!("talocart_session={raw}; talocart_admin_language=it");
        let response = fiscal_request(
            &app,
            "POST",
            "/admin/account/language",
            &owner_cookie,
            &format!("csrf={}&locale=en", session.csrf),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/admin/account?saved=1"
        );
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(
            cookie.contains("talocart_admin_language=en")
                && cookie.contains("Secure")
                && cookie.contains("HttpOnly")
        );
        assert_eq!(
            identity.session(&second).await.unwrap().unwrap().locale,
            "en"
        );
        assert_eq!(identity.account(&session).await.unwrap().locale, "en");
        let response =
            fiscal_request(&app, "GET", "/admin/account", &owner_cookie, "", false).await;
        assert_eq!(response.headers()[header::CONTENT_LANGUAGE], "en");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let page = html(response).await;
        assert!(page.contains("<html lang=\"en\">") && page.contains("Administration language"));
        assert!(page.contains("value=\"en\" selected"));
        // Do not translate user data even when it matches an Italian UI label.
        identity
            .update_account(
                &session,
                &session.csrf,
                "Salva profilo <script>",
                "owner@example.com",
                String::new(),
            )
            .await
            .unwrap();
        let page =
            html(fiscal_request(&app, "GET", "/admin/account", &owner_cookie, "", false).await)
                .await;
        assert!(page.contains("Salva profilo &#60;script&#62;"));
        assert!(!page.contains("Salva profilo <script>"));
        for body in [
            "csrf=wrong&locale=it".to_owned(),
            format!("csrf={}&locale=unknown", session.csrf),
            format!("csrf={}&locale=../../en", session.csrf),
            format!("csrf={}&locale=en&administrator_id=2", session.csrf),
        ] {
            assert!(
                fiscal_request(
                    &app,
                    "POST",
                    "/admin/account/language",
                    &owner_cookie,
                    &body,
                    false
                )
                .await
                .status()
                .is_client_error()
            );
            assert_eq!(identity.account(&session).await.unwrap().locale, "en");
        }
        let invalid = fiscal_request(
            &app,
            "POST",
            "/admin/account/language",
            &owner_cookie,
            &format!("csrf={}&locale=unknown", session.csrf),
            false,
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(html(invalid).await.contains("Choose an available language"));
        assert_eq!(
            fiscal_request(
                &app,
                "GET",
                "/admin/account/language",
                &owner_cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(identity.account(&session).await.unwrap().locale, "en");
        for path in ["/", "/catalog", "/brands"] {
            let page =
                html(fiscal_request(&app, "GET", path, &owner_cookie, "", false).await).await;
            assert!(page.contains("<html lang=\"it\">"), "Public page: {path}");
        }
        let role = identity
            .create_role(&session, &session.csrf, "No backend areas", &[])
            .await
            .unwrap();
        let id = identity
            .create_administrator(
                &session,
                &session.csrf,
                "language@example.test",
                "correct horse battery".into(),
                Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let reader_raw = login_as(&identity, "language@example.test").await;
        let reader = identity.session(&reader_raw).await.unwrap().unwrap();
        assert!(!reader.allows(Permission::AdminRead));
        assert_eq!(reader.locale, "it");
        let reader_cookie = format!("talocart_session={reader_raw}; talocart_admin_language=en");
        // No backend permissions are required to edit one's own preference.
        let response = fiscal_request(
            &app,
            "POST",
            "/admin/account/language",
            &reader_cookie,
            &format!("csrf={}&locale=it", reader.csrf),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let (owner_page, reader_page) = tokio::join!(
            fiscal_request(&app, "GET", "/admin/account", &owner_cookie, "", false),
            fiscal_request(&app, "GET", "/admin/account", &reader_cookie, "", false),
        );
        assert_eq!(owner_page.headers()[header::CONTENT_LANGUAGE], "en");
        assert_eq!(reader_page.headers()[header::CONTENT_LANGUAGE], "it");
        assert!(html(reader_page).await.contains("<html lang=\"it\">"));
        assert!(html(owner_page).await.contains("<html lang=\"en\">"));
        // Reconstruct both pool/service/application: preference is stored, not process state.
        let reopened = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let new_identity = Identity::new(reopened.clone());
        assert_eq!(
            new_identity.session(&raw).await.unwrap().unwrap().locale,
            "en"
        );
        let restarted = router(test_app(reopened.clone(), "Shop".into()).await);
        assert!(
            html(
                fiscal_request(
                    &restarted,
                    "GET",
                    "/admin/account",
                    &owner_cookie,
                    "",
                    false
                )
                .await
            )
            .await
            .contains("<html lang=\"en\">")
        );
        reopened.close().await;
        sqlx::query("DELETE FROM admin_sessions WHERE administrator_id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            identity.update_locale(&reader, &reader.csrf, "en").await,
            Err(IdentityError::Forbidden)
        ));
        assert_eq!(
            fiscal_request(
                &app,
                "POST",
                "/admin/account/language",
                &reader_cookie,
                &format!("csrf={}&locale=en", reader.csrf),
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        let (a, b) = tokio::join!(
            identity.update_locale(&session, &session.csrf, "it"),
            identity.update_locale(&session, &session.csrf, "en")
        );
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(
            identity.session(&raw).await.unwrap().unwrap().locale,
            identity.session(&second).await.unwrap().unwrap().locale
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT locale FROM administrators WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "it"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn sign_in_language_csrf_redirect_and_personal_precedence() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let app = router(test_app(pool, "Shop".into()).await);
        let response = fiscal_request(&app, "GET", "/admin/login", "", "", false).await;
        let cookie = cookie_from_response(&response);
        let csrf = csrf(&html(response).await);
        for (body, expected) in [
            (
                "csrf=wrong&locale=en&return_to=/admin/login".to_owned(),
                StatusCode::FORBIDDEN,
            ),
            (
                format!("csrf={csrf}&locale=en&return_to=https://example.com"),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                format!("csrf={csrf}&locale=unknown&return_to=/admin/login"),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            assert_eq!(
                fiscal_request(&app, "POST", "/admin/language", &cookie, &body, false)
                    .await
                    .status(),
                expected
            );
        }
        let selected = fiscal_request(
            &app,
            "POST",
            "/admin/language",
            &cookie,
            &format!("csrf={csrf}&locale=en&return_to=/admin/login"),
            false,
        )
        .await;
        assert_eq!(selected.status(), StatusCode::SEE_OTHER);
        let cookies = format!("{cookie}; {}", cookie_from_response(&selected));
        for path in ["/admin/login", "/admin/reset"] {
            let page = html(fiscal_request(&app, "GET", path, &cookies, "", false).await).await;
            assert!(page.contains("<html lang=\"en\">"));
            assert!(page.contains("Sign-in language"));
        }
        let bad_login = fiscal_request(
            &app,
            "POST",
            "/admin/login",
            &cookies,
            &format!("csrf={csrf}&email=owner%40example.com&password=wrong"),
            false,
        )
        .await;
        assert_eq!(bad_login.status(), StatusCode::UNAUTHORIZED);
        assert!(
            html(bad_login)
                .await
                .contains("Invalid, expired or already used credentials or token")
        );
        let login = fiscal_request(
            &app,
            "POST",
            "/admin/login",
            &cookies,
            &format!("csrf={csrf}&email=owner%40example.com&password=correct+horse+battery"),
            false,
        )
        .await;
        assert_eq!(login.status(), StatusCode::SEE_OTHER);
        assert!(login.headers().get_all(header::SET_COOKIE).iter().any(|v| {
            v.to_str()
                .unwrap()
                .starts_with("talocart_admin_language=it")
        }));
        let authenticated = cookie_from_response(&login);
        assert!(
            html(
                fiscal_request(
                    &app,
                    "GET",
                    "/admin/account",
                    &format!("{authenticated}; talocart_admin_language=en"),
                    "",
                    false
                )
                .await
            )
            .await
            .contains("<html lang=\"it\">")
        );
        assert_eq!(
            fiscal_request(
                &app,
                "POST",
                "/admin/language",
                "",
                &format!("csrf={csrf}&locale=en&return_to=/admin/login"),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert!(
            identity
                .session(authenticated.split('=').nth(1).unwrap())
                .await
                .unwrap()
                .is_some()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn english_catalog_editors_preserve_names_variants_and_decimal_input() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        talocart_commerce::demo::DemoSeeder::new(pool.clone(), test_media(&pool).await)
            .install()
            .await
            .unwrap();
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        identity
            .update_locale(&session, &session.csrf, "en")
            .await
            .unwrap();
        let cookie = format!("talocart_session={raw}");
        let (product, variant): (i64, i64) =
            sqlx::query_as("SELECT product_id,id FROM catalog_variants ORDER BY id LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("WITH updated AS (UPDATE catalog_products SET name='Salva profilo <script>' WHERE id=$1 RETURNING id,name) UPDATE catalog_products_translations t SET name=u.name FROM updated u WHERE t.entity_id=u.id AND t.locale='it'")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        let option: i64 = sqlx::query_scalar("SELECT id FROM catalog_options ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let category: i64 =
            sqlx::query_scalar("SELECT id FROM catalog_categories ORDER BY id LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let brand: i64 = sqlx::query_scalar("SELECT id FROM catalog_brands ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let app = router(test_app(pool, "Shop".into()).await);
        for path in [
            format!("/admin/catalog/products/{product}"),
            format!("/admin/catalog/products/{product}/variants"),
            format!("/admin/catalog/products/{product}/variants/{variant}"),
            format!("/admin/catalog/options/{option}"),
            format!("/admin/catalog/categories/{category}"),
            format!("/admin/catalog/brands/{brand}"),
        ] {
            let response = fiscal_request(&app, "GET", &path, &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::OK, "Editor: {path}");
            let page = html(response).await;
            assert!(page.contains("<html lang=\"en\">"));
            assert!(!page.contains("Salva profilo <script>"));
            if path.contains("/products/") {
                assert!(page.contains("Salva profilo &#60;script&#62;"));
            }
        }
        let response = fiscal_request(
            &app,
            "GET",
            &format!("/admin/catalog/products/{product}/variants/{variant}"),
            &cookie,
            "",
            false,
        )
        .await;
        let page = html(response).await;
        assert!(page.contains("Variant price (€)"));
        assert!(page.contains("name=\"price\" value=\""));
        let list = html(
            fiscal_request(
                &app,
                "GET",
                &format!("/admin/catalog/products/{product}/variants"),
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(list.contains("Variants per page") && list.contains("VAT included"));
        assert!(!list.contains("IVA"));
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL or TALOCART_TEST_DATABASE_URL"]
async fn talocart_platform_name_preserves_custom_store_and_legacy_session() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let mut settings = identity.settings().await.unwrap().unwrap().values;
        settings.name = "Original custom shop".into();
        identity
            .save_settings(&session, &session.csrf, &settings)
            .await
            .unwrap();
        let app = router(test_app(pool.clone(), "Talocart".into()).await);
        let cookie = format!("talocart_session={raw}");
        for (locale, welcome) in [("it", "<h1>Panoramica</h1>"), ("en", "<h1>Overview</h1>")] {
            let response = fiscal_request(
                &app,
                "POST",
                "/admin/account/language",
                &cookie,
                &format!("csrf={}&locale={locale}", session.csrf),
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let response = fiscal_request(&app, "GET", "/admin", &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], locale);
            let page = html(response).await;
            assert!(page.contains(welcome));
            assert!(page.contains("Original custom shop"));
        }
    })
    .await;
}
