use super::*;

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn public_route_context_preserves_invalid_paths_and_neutral_requests_without_identity_writes()
{
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let owner = identity.session(&raw).await.unwrap().unwrap();
        talocart_platform::storefront::Storefront::new(pool.clone())
            .save_languages(&owner, &owner.csrf, "en", &["en".into(), "it".into()])
            .await
            .unwrap();
        let customers = talocart_platform::identity::Customers::new(pool.clone(), identity);
        let (raw, _) = customers.anonymous().await.unwrap();
        let cookie = format!("talocart_customer={raw}");
        let counts = || async {
            sqlx::query_as::<_, (i64, i64, i64, i64)>(
                "SELECT (SELECT count(*) FROM customers), \
                 (SELECT count(*) FROM customer_sessions), \
                 (SELECT count(*) FROM guest_carts), \
                 (SELECT count(*) FROM auth_limits)",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        let before = counts().await;
        let app = router(test_app(pool.clone(), "Route context".into()).await);

        // These routes match, but their typed identifiers do not. Context setup
        // must preserve Path's 400 rather than turn the request into a 503.
        for (prefix, locale) in [("", "en"), ("/it", "it")] {
            for (method, path) in [
                ("GET", "/account/addresses/not-an-id"),
                ("POST", "/cart/items/not-an-id"),
            ] {
                let response =
                    request(&app, method, &format!("{prefix}{path}"), &cookie, "", false).await;
                assert_eq!(
                    response.status(),
                    StatusCode::BAD_REQUEST,
                    "{method} {prefix}{path}"
                );
                assert_eq!(response.headers()[header::CONTENT_LANGUAGE], locale);
                assert!(!response.headers().contains_key(header::SET_COOKIE));
            }
        }
        for (path, status, locale) in [
            ("/missing-context-route", StatusCode::NOT_FOUND, Some("en")),
            (
                "/it/missing-context-route",
                StatusCode::NOT_FOUND,
                Some("it"),
            ),
            ("/assets/ui.css", StatusCode::OK, None),
        ] {
            let response = request(&app, "GET", path, &cookie, "", false).await;
            assert_eq!(response.status(), status, "{path}");
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_LANGUAGE)
                    .map(|value| value.to_str().unwrap()),
                locale,
                "{path}"
            );
            assert!(!response.headers().contains_key(header::SET_COOKIE));
        }
        assert_eq!(counts().await, before);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn public_language_policy_primary_single_secondary_redirects_and_metadata() {
    isolated(|pool, _| async move {
        let (identity, session, _, _, _, _, _) = variant_fixture(&pool).await;
        let service = talocart_platform::storefront::Storefront::new(pool.clone());
        let app = router(
            test_app_config(
                pool.clone(),
                "Shop".into(),
                talocart_web::AppStateConfig {
                    public_origin: "https://shop.example.test".into(),
                    ..Default::default()
                },
            )
            .await,
        );
        let raw = authenticated(&identity).await;
        let admin = identity.session(&raw).await.unwrap().unwrap();
        identity
            .update_locale(&admin, &admin.csrf, "it")
            .await
            .unwrap();
        let cookie = format!("talocart_session={raw}; talocart_admin_language=it");
        // Exercise aliases after both prefixes have actually been enabled as secondaries.
        service
            .save_languages(&session, &session.csrf, "en", &["it".into(), "en".into()])
            .await
            .unwrap();
        service
            .save_languages(&session, &session.csrf, "it", &["it".into(), "en".into()])
            .await
            .unwrap();
        for (primary, active) in [
            ("it", vec!["it", "en"]),
            ("en", vec!["it", "en"]),
            ("en", vec!["en"]),
            ("it", vec!["it"]),
        ] {
            let active = active.into_iter().map(str::to_owned).collect::<Vec<_>>();
            service
                .save_languages(&session, &session.csrf, primary, &active)
                .await
                .unwrap();
            let stored = talocart_platform::storefront::Storefront::new(pool.clone())
                .settings()
                .await
                .unwrap();
            assert_eq!(stored.primary_locale, primary);
            let snapshots = demo_existing_snapshot(&pool).await;
            for base in [
                "/",
                "/catalog",
                "/catalog/categoria-prodotti",
                "/brands",
                "/brands/marchio-prodotti",
                "/search/Prodotto",
                "/products/model",
                "/status",
            ] {
                let response = request(&app, "GET", base, &cookie, "", false).await;
                assert_eq!(response.status(), StatusCode::OK, "{primary}: {base}");
                assert_eq!(response.headers()[header::CONTENT_LANGUAGE], primary);
                let page = html(response).await;
                assert!(page.contains(&format!("<html lang=\"{primary}\">")));
                assert_eq!(page.contains("class=\"store-languages\""), active.len() > 1);
                assert_eq!(page.matches("rel=\"alternate\"").count(), active.len() + 1);
                assert!(page.contains(&format!(
                    "hreflang=\"x-default\" href=\"https://shop.example.test{}\"",
                    if base == "/status" { "/" } else { base }
                )));
                for code in ["it", "en"] {
                    if !active.iter().any(|item| item == code) {
                        assert!(!page.contains(&format!("hreflang=\"{code}\"")));
                    }
                    let path = if base == "/" {
                        format!("/{code}")
                    } else {
                        format!("/{code}{base}")
                    };
                    let response = request(&app, "GET", &path, "", "", false).await;
                    if code != primary && active.iter().any(|item| item == code) {
                        assert_eq!(response.status(), StatusCode::OK, "{path}");
                        assert_eq!(response.headers()[header::CONTENT_LANGUAGE], code);
                        let page = html(response).await;
                        assert!(page.contains(&format!("action=\"/{code}/search\"")));
                        assert!(
                            page.contains(&format!("href=\"/{code}/products/model\""))
                                || base == "/products/model"
                                || base == "/brands"
                                || base == "/brands/marchio-prodotti"
                                || base == "/status"
                                || base == "/"
                        );
                    } else {
                        assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY, "{path}");
                        assert_eq!(response.headers()[header::LOCATION], base);
                    }
                }
            }
            let missing = request(&app, "GET", "/missing-language-page", "", "", false).await;
            assert_eq!(missing.status(), StatusCode::NOT_FOUND);
            assert_eq!(missing.headers()[header::CONTENT_LANGUAGE], primary);
            assert!(
                html(missing)
                    .await
                    .contains(&format!("<html lang=\"{primary}\">"))
            );
            let stale = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/products/model?photo=")
                        .header("hx-request", "true")
                        .header("x-talocart-store-policy", "outdated")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(stale.status(), StatusCode::OK);
            assert_eq!(stale.headers()["hx-redirect"], "/products/model?photo=");
            let prefix = format!("/{primary}");
            let state = "?value_1=1&value_2=2&photo=main&filter=a%26b%2F%25";
            for method in ["GET", "HEAD"] {
                let response = request(
                    &app,
                    method,
                    &format!("{prefix}/products/model{state}"),
                    "",
                    "",
                    false,
                )
                .await;
                assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
                assert_eq!(
                    response.headers()[header::LOCATION],
                    format!("/products/model{state}")
                );
                assert!(!response.headers().contains_key(header::SET_COOKIE));
            }
            let response = request(
                &app,
                "GET",
                &format!("{prefix}/products/model{state}"),
                "",
                "",
                true,
            )
            .await;
            assert_eq!(
                response.headers()["hx-redirect"],
                format!("/products/model{state}")
            );
            let redirect =
                request(&app, "GET", &format!("/{primary}/?q=a%2Fb"), "", "", false).await;
            assert_eq!(redirect.headers()[header::LOCATION], "/?q=a%2Fb");
            let submitted = request(
                &app,
                "GET",
                "/search?q=Prodotto&available=true",
                "",
                "",
                false,
            )
            .await;
            assert_eq!(
                submitted.headers()[header::LOCATION],
                "/search/Prodotto?available=true"
            );
            for path in [
                "/en/admin",
                "/it/admin",
                "/en/media/image/1/card",
                "/it/health/ready",
                "/unknown/catalog",
                "/unknown",
            ] {
                let response = request(&app, "GET", path, "", "", false).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
                assert!(!response.headers().contains_key(header::LOCATION));
            }
            assert_eq!(
                request(&app, "POST", &format!("/{primary}/catalog"), "", "", false)
                    .await
                    .status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
            let map =
                html(request(&app, "GET", "/sitemap-products.xml?page=1", "", "", false).await)
                    .await;
            assert_eq!(map.matches("<loc>").count(), active.len());
            assert!(map.contains("https://shop.example.test/products/model"));
            assert!(!map.contains(&format!(
                "https://shop.example.test/{primary}/products/model"
            )));
            let catalog =
                html(request(&app, "GET", "/sitemap-catalog.xml", "", "", false).await).await;
            assert_eq!(catalog.matches("<loc>").count(), active.len() * 2);
            assert_eq!(identity.account(&admin).await.unwrap().locale, "it");
            assert_eq!(demo_existing_snapshot(&pool).await, snapshots);
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn language_configuration_validation_security_theme_isolation_and_concurrent_saves() {
    use talocart_platform::{
        identity::{Permission, Role},
        storefront::{Storefront, StorefrontError},
    };
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let service = Storefront::new(pool.clone());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let route = "/admin/settings/storefront/languages";
        assert_eq!(
            request(
                &app,
                "POST",
                route,
                "",
                "csrf=x&primary_locale=en&locale=en",
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        let theme = service.settings().await.unwrap();
        for (csrf, primary, codes, status) in [
            ("wrong", "en", "locale=en", StatusCode::FORBIDDEN),
            (
                session.csrf.as_str(),
                "en",
                "",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                session.csrf.as_str(),
                "en",
                "locale=it",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                session.csrf.as_str(),
                "unknown",
                "locale=unknown",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                session.csrf.as_str(),
                "it",
                "locale=it&locale=it",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let response = request(
                &app,
                "POST",
                route,
                &cookie,
                &format!("csrf={csrf}&primary_locale={primary}&{codes}"),
                false,
            )
            .await;
            assert_eq!(response.status(), status);
            if status == StatusCode::UNPROCESSABLE_ENTITY {
                let page = html(response).await;
                assert!(
                    page.contains("aria-invalid=\"true\"")
                        && page.contains("/admin/settings/storefront/languages")
                );
            }
            assert_eq!(
                service.settings().await.unwrap().languages().unwrap(),
                theme.languages().unwrap()
            );
        }
        for extra in ["&primary_locale=it", "&csrf=wrong", "&administrator_id=2"] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    route,
                    &cookie,
                    &format!("csrf={}&primary_locale=en&locale=en{extra}", session.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        let response = request(
            &app,
            "POST",
            route,
            &cookie,
            &format!("csrf={}&primary_locale=en&locale=en", session.csrf),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let now = service.settings().await.unwrap();
        assert_eq!(now.primary_locale, "en");
        assert_eq!(now.active_locales, vec!["en"]);
        assert_eq!(
            (&now.palette, &now.heading, &now.seo_title),
            (&theme.palette, &theme.heading, &theme.seo_title)
        );
        let mut changed = now.clone();
        changed.palette = "clay".into();
        changed.primary_locale = "unknown".into();
        changed.active_locales = vec!["unknown".into()];
        service
            .save(&session, &session.csrf, changed)
            .await
            .unwrap();
        assert_eq!(
            service.settings().await.unwrap().languages().unwrap(),
            now.languages().unwrap()
        );
        let a = vec!["en".into()];
        let b = vec!["it".into(), "en".into()];
        let (first, second) = tokio::join!(
            service.save_languages(&session, &session.csrf, "en", &a),
            service.save_languages(&session, &session.csrf, "it", &b)
        );
        first.unwrap();
        second.unwrap();
        let final_policy = service.settings().await.unwrap().languages().unwrap();
        assert!(
            final_policy == talocart_platform::storefront::Languages::new("en", &a).unwrap()
                || final_policy == talocart_platform::storefront::Languages::new("it", &b).unwrap()
        );
        let role = identity
            .create_role(
                &session,
                &session.csrf,
                "Language reader",
                &[Permission::SettingsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &session,
                &session.csrf,
                "language-reader@example.test",
                "correct horse reader".into(),
                Role::Custom(role),
            )
            .await
            .unwrap();
        let (_, anonymous) = identity.anonymous().await.unwrap();
        let reader = identity
            .login(
                &anonymous,
                &anonymous.csrf,
                "language-reader@example.test",
                "correct horse reader".into(),
            )
            .await
            .unwrap();
        let reader_session = identity.session(&reader).await.unwrap().unwrap();
        let reader_cookie = format!("talocart_session={reader}");
        let page = html(
            request(
                &app,
                "GET",
                "/admin/settings/storefront",
                &reader_cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(page.contains("name=\"primary_locale\""));
        assert!(!page.contains("Salva lingue del negozio"));
        assert_eq!(
            request(
                &app,
                "POST",
                route,
                &reader_cookie,
                &format!("csrf={}&primary_locale=en&locale=en", reader_session.csrf),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        identity
            .update_locale(&session, &session.csrf, "en")
            .await
            .unwrap();
        service
            .save_languages(&session, &session.csrf, "it", &["it".into()])
            .await
            .unwrap();
        let administration = request(
            &app,
            "GET",
            "/admin/settings/storefront",
            &cookie,
            "",
            false,
        )
        .await;
        assert_eq!(administration.headers()[header::CONTENT_LANGUAGE], "en");
        let administration = html(administration).await;
        assert!(
            administration.contains("Store languages") && administration.contains("Save settings")
        );
        assert_eq!(identity.account(&session).await.unwrap().locale, "en");
        let codes = final_policy
            .active()
            .iter()
            .map(|code| code.code().to_owned())
            .collect::<Vec<_>>();
        service
            .save_languages(
                &session,
                &session.csrf,
                final_policy.primary().code(),
                &codes,
            )
            .await
            .unwrap();
        identity.logout(&session, &session.csrf).await.unwrap();
        assert!(matches!(
            service
                .save_languages(&session, &session.csrf, "en", &a)
                .await,
            Err(StorefrontError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert_eq!(
            service.settings().await.unwrap().languages().unwrap(),
            final_policy
        );
    })
    .await;
}
