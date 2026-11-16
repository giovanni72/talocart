use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn setup_uses_platform_brand_with_legacy_store_configuration() {
    isolated(|pool, _| async move {
        let app = router(test_app(pool, "Koala".into()).await);
        for language in ["en", "it"] {
            let preference = format!("talocart_admin_language={language}");
            let response = request(&app, "GET", "/setup", &preference, "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            let anon = cookie(&response);
            let page = html(response).await;
            assert!(page.contains("Talocart</title>"));
            assert!(page.contains(">Talocart</a>"));
            assert!(!page.contains("Koala"));
            let response = request(&app, "POST", "/setup", &format!("{anon}; {preference}"),
                &format!("csrf={}&name=Shop&email=owner%40example.test&password=correct+horse+battery&primary_locale=en&locale=it", csrf(&page)), false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let page = html(response).await;
            assert!(page.contains("Talocart</title>"));
            assert!(page.contains(">Talocart</a>"));
            assert!(!page.contains("Koala"));
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn setup_defaults_to_english_and_preserves_explicit_language() {
    isolated(|pool, _| async move {
        let app = router(test_app(pool, "Shop".into()).await);
        for (preference, expected) in [
            ("", "en"),
            ("talocart_admin_language=it", "it"),
            ("talocart_admin_language=en", "en"),
            ("talocart_admin_language=unknown", "en"),
            ("talocart_admin_language=it; talocart_admin_language=en", "en"),
        ] {
            let response = request(&app, "GET", "/setup", preference, "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], expected);
            let anon = cookie(&response);
            let page = html(response).await;
            assert!(page.contains(&format!("<html lang=\"{expected}\">")));
            let cookies = format!("{anon}; {preference}");
            let response = request(&app, "POST", "/setup", &cookies,
                &format!("csrf={}&name=Shop&email=owner%40example.test&password=correct+horse+battery&primary_locale=en&locale=it", csrf(&page)), false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], expected);
            assert!(html(response).await.contains("value=\"en\" selected"));
        }
        let response = request(&app, "GET", "/admin/login", "", "", false).await;
        assert_eq!(response.headers()[header::CONTENT_LANGUAGE], "it");
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn installation_language_choices_are_atomic_and_primary_urls_are_direct() {
    for (primary, active, demo) in [
        ("en", vec!["en"], false),
        ("it", vec!["it"], false),
        ("en", vec!["it", "en"], true),
        ("it", vec!["it", "en"], true),
    ] {
        isolated(move |pool, _| async move {
            let identity = test_identity(pool.clone()).await;
            assert!(!identity.installed().await.unwrap());
            let app = router(test_app_config(pool.clone(), "Shop".into(), talocart_web::AppStateConfig { public_origin: "https://shop.example.test".into(), ..Default::default() }).await);
            let response = request(&app,"GET","/setup","","",false).await;
            let anon = cookie(&response);
            let page = html(response).await;
            assert!(page.contains("name=\"primary_locale\""));
            assert!(page.contains("value=\"it\"") && page.contains("value=\"en\""));
            let token = csrf(&page);
            let mut body = format!("csrf={token}&name=Shop&email=owner%40example.com&password=correct+horse+battery&primary_locale={primary}&install_demo={demo}");
            for code in &active { body.push_str(&format!("&locale={code}")); }
            let response = request(&app,"POST","/setup",&anon,&body,false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(response.headers().get_all(header::SET_COOKIE).iter().any(|h| h.to_str().unwrap().contains(&format!("talocart_admin_language={primary}"))));
            let stored = talocart_platform::storefront::Storefront::new(pool.clone()).settings().await.unwrap();
            assert_eq!(stored.primary_locale, primary);
            let mut expected = active.iter().map(|code| (*code).to_owned()).collect::<Vec<_>>(); expected.sort();
            assert_eq!(stored.active_locales, expected);
            assert_eq!(stored.published_prefixes, active.iter().filter(|code| **code != primary).map(|code| (*code).to_owned()).collect::<Vec<_>>());
            assert_eq!(sqlx::query_scalar::<_,String>("SELECT locale FROM administrators").fetch_one(&pool).await.unwrap(), primary);
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_products").fetch_one(&pool).await.unwrap(), if demo {30} else {0});
            let defaults: Vec<(String, i32, bool)> = sqlx::query_as("SELECT c.name,t.basis_points,t.explicit_rule FROM resolve_tax('IT') t JOIN tax_classes c ON c.id=t.class_id WHERE c.name LIKE '% / %' ORDER BY t.basis_points").fetch_all(&pool).await.unwrap();
            assert_eq!(defaults.iter().map(|r| (r.1,r.2)).collect::<Vec<_>>(), vec![(0,true),(400,true),(500,true),(1000,true),(2200,true)]);
            assert!(defaults.iter().any(|r| r.0 == "Esente IVA / VAT exempt 0%"));
            assert_eq!(sqlx::query_as::<_,(String,i32)>("SELECT p.country_code,r.basis_points FROM pricing_settings p JOIN tax_classes c ON c.id=p.default_class_id JOIN tax_rates r ON r.id=c.rate_id").fetch_one(&pool).await.unwrap(), ("IT".into(),2200));
            for method in ["GET","HEAD"] {
                for route in ["/", "/catalog", "/brands", "/status"] {
                    let response = request(&app,method,route,"","",false).await;
                    assert_eq!(response.status(), StatusCode::OK);
                    assert_eq!(response.headers()[header::CONTENT_LANGUAGE], primary);
                    assert!(!response.headers().contains_key(header::LOCATION));
                    let alias = if route == "/" { format!("/{primary}") } else {format!("/{primary}{route}?filter=a%26b")};
                    let response = request(&app,method,&alias,"","",false).await;
                    assert_eq!(response.status(), StatusCode::NOT_FOUND, "Never published prefix: {alias}");
                    assert!(!response.headers().contains_key(header::LOCATION));
                }
            }
            let page = html(request(&app,"GET","/catalog","","",false).await).await;
            assert!(page.contains("href=\"/catalog\""));
            assert!(!page.contains(&format!("href=\"/{primary}/")));
            assert_eq!(page.contains("class=\"store-languages\""), active.len()>1);
            let sitemap = html(request(&app,"GET","/sitemap-catalog.xml","","",false).await).await;
            assert!(!sitemap.contains(&format!("https://shop.example.test/{primary}/")));
            for code in active.iter().filter(|code| **code != primary) {
                let response = request(&app,"GET",&format!("/{code}/catalog"),"","",false).await;
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()[header::CONTENT_LANGUAGE], *code);
            }
            // Explicit migration is repeatable and preserves the chosen configuration/history.
            database::migrate(&pool).await.unwrap();
            let after = talocart_platform::storefront::Storefront::new(pool.clone()).settings().await.unwrap();
            assert_eq!(after.primary_locale, stored.primary_locale);
            assert_eq!(after.active_locales, stored.active_locales);
            assert_eq!(after.published_prefixes, stored.published_prefixes);
        }).await;
    }
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn installation_languages_validate_csrf_duplicates_and_concurrent_winner() {
    isolated(|pool, _| async move {
        let identity = test_identity(pool.clone()).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let response = request(&app, "GET", "/setup", "", "", false).await;
        let anon = cookie(&response);
        let token = csrf(&html(response).await);
        let base = format!(
            "csrf={token}&name=Shop&email=owner%40example.com&password=correct+horse+battery"
        );
        let before = talocart_platform::storefront::Storefront::new(pool.clone())
            .settings()
            .await
            .unwrap();
        for choices in [
            "",
            "&primary_locale=en&locale=it",
            "&primary_locale=en",
            "&primary_locale=unknown&locale=unknown",
            "&primary_locale=en&locale=en&locale=en",
        ] {
            let response = request(
                &app,
                "POST",
                "/setup",
                &anon,
                &format!("{base}{choices}&install_demo=true"),
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let page = html(response).await;
            assert!(page.contains("aria-invalid=\"true\""));
            assert!(
                page.split("id=\"install-demo\"")
                    .nth(1)
                    .unwrap()
                    .split('>')
                    .next()
                    .unwrap()
                    .contains("checked")
            );
            if choices.contains("locale=it") {
                assert!(
                    page.split("name=\"locale\" value=\"it\"")
                        .nth(1)
                        .unwrap()
                        .split('>')
                        .next()
                        .unwrap()
                        .contains("checked")
                );
            }
            assert!(!identity.installed().await.unwrap());
        }
        for choices in [
            "&primary_locale=en&primary_locale=it&locale=en",
            "&primary_locale=en&locale=en&unexpected=true",
            "&primary_locale=en&locale=en&csrf=other",
            "&primary_locale=en&locale=en&token=obsolete",
        ] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/setup",
                    &anon,
                    &format!("{base}{choices}"),
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        let english = format!("{base}&primary_locale=en&locale=en");
        assert_eq!(
            request(
                &app,
                "POST",
                "/setup",
                &anon,
                &english.replace(&format!("csrf={token}"), "csrf=invalid"),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&app, "POST", "/setup", "", &english, false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let rejected = request(
            &app,
            "POST",
            "/setup",
            &anon,
            &english.replace("password=correct+horse+battery", "password=short"),
            false,
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let rejected = html(rejected).await;
        assert!(
            rejected
                .split("value=\"en\"")
                .nth(1)
                .unwrap()
                .split('>')
                .next()
                .unwrap()
                .contains("selected")
        );
        let after = talocart_platform::storefront::Storefront::new(pool.clone())
            .settings()
            .await
            .unwrap();
        assert_eq!(after.primary_locale, before.primary_locale);
        assert_eq!(after.active_locales, before.active_locales);
        assert!(after.published_prefixes.is_empty());
        let italian = format!("{base}&primary_locale=it&locale=it&locale=en");
        let (a, b) = tokio::join!(
            request(&app, "POST", "/setup", &anon, &english, false),
            request(&app, "POST", "/setup", &anon, &italian, false)
        );
        assert_eq!(
            [a.status(), b.status()]
                .iter()
                .filter(|s| **s == StatusCode::SEE_OTHER)
                .count(),
            1
        );
        let stored = talocart_platform::storefront::Storefront::new(pool.clone())
            .settings()
            .await
            .unwrap();
        assert_eq!(
            stored.primary_locale,
            if a.status() == StatusCode::SEE_OTHER {
                "en"
            } else {
                "it"
            }
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT locale FROM administrators")
                .fetch_one(&pool)
                .await
                .unwrap(),
            stored.primary_locale
        );
        assert_eq!(
            stored.active_locales.len(),
            if stored.primary_locale == "en" { 1 } else { 2 }
        );
        assert_eq!(
            stored.published_prefixes.len(),
            if stored.primary_locale == "en" { 0 } else { 1 }
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM administrators")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn setup_without_code_creates_owner_and_keeps_recovery_code() {
    isolated(|pool,_|async move {
        let identity=test_identity(pool.clone()).await;
        let app=router(test_app(pool.clone(),"Koala".into()).await);
        let response=request(&app,"GET","/setup","","",false).await;
        let anon=cookie(&response);let page=html(response).await;
        assert!(!page.contains("name=\"token\""), "Installer has no installation code");
        let setup_csrf=csrf(&page);
        let body=format!("csrf={setup_csrf}&name=Shop&email=owner%40example.com&password=correct+horse+battery&primary_locale=en&locale=en");
        assert_eq!(request(&app,"POST","/setup",&anon,&body,false).await.status(),StatusCode::SEE_OTHER);
        assert!(identity.installed().await.unwrap());
        let reset_page=html(request(&app,"GET","/admin/reset","","",false).await).await;
        assert!(reset_page.contains("name=\"token\""), "Recovery remains operator-authorized");
        let response=request(&app,"GET","/admin/login","","",false).await;
        let anon=cookie(&response);let csrf=csrf(&html(response).await);
        let repeat=body.replace(body.split('&').next().unwrap(),&format!("csrf={csrf}"));
        assert_eq!(request(&app,"POST","/setup",&anon,&repeat,false).await.status(),StatusCode::CONFLICT);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM administrators").fetch_one(&pool).await.unwrap(),1);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn installation_service_rolls_back_bootstrap_when_fiscal_initialization_fails() {
    isolated(|pool, _| async move {
        let identity = test_identity(pool.clone()).await;
        sqlx::query("CREATE FUNCTION reject_setup_tax() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic setup failure'; END $$")
            .execute(&pool).await.expect("synthetic failure function");
        sqlx::query("CREATE TRIGGER reject_setup_tax BEFORE INSERT ON tax_rates FOR EACH ROW EXECUTE FUNCTION reject_setup_tax()")
            .execute(&pool).await.expect("synthetic failure trigger");
        let app=router(test_app(pool.clone(), "Shop".into()).await);
        for (language, expected) in [("en", "Installation incomplete."), ("it", "Installazione non completata.")] {
            let preference=format!("talocart_admin_language={language}");
            let response=request(&app,"GET","/setup",&preference,"",false).await;
            let anonymous=cookie(&response);
            let page=html(response).await;
            let response=request(&app,"POST","/setup",&format!("{anonymous}; {preference}"),
                &format!("csrf={}&name=Atomic+shop&email=owner%40example.test&password=correct+horse+battery&primary_locale=it&locale=it&locale=en",csrf(&page)),false).await;
            assert_eq!(response.status(),StatusCode::SERVICE_UNAVAILABLE);
            assert!(html(response).await.contains(expected),"Fiscal setup errors keep their installation feedback in {language}");
        }
        let prepared = identity.prepare_bootstrap("owner@example.test", "correct horse battery".into(), "Atomic shop", "Owner").await.expect("prepared bootstrap");
        assert!(matches!(talocart_commerce::installation::install(&pool, prepared, None).await,Err(talocart_commerce::installation::Error::Demo(_))));
        assert!(!identity.installed().await.expect("installation rolled back"));
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM administrators").fetch_one(&pool).await.expect("no administrator"), 0);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM store_settings").fetch_one(&pool).await.expect("no shop"), 0);
        sqlx::query("DROP TRIGGER reject_setup_tax ON tax_rates").execute(&pool).await.expect("remove synthetic failure");
        let prepared = identity.prepare_bootstrap("owner@example.test", "correct horse battery".into(), "Atomic shop", "Owner").await.expect("retry preparation");
        talocart_commerce::installation::install(&pool, prepared, None).await.expect("retry installs cleanly");
    }).await;
}
