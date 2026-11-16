use super::*;
use talocart_commerce::countries::{CheckoutCountries, CountryError};

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn checkout_countries_default_block_validation_atomic_save_and_persistence() {
    isolated(|pool, options| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let countries = CheckoutCountries::new(pool.clone(), identity);
        assert!(countries.selected(&s).await.unwrap().is_empty());
        assert!(countries.destinations().await.unwrap().is_empty());
        assert!(matches!(
            countries.check_destination("IT").await,
            Err(CountryError::NotEnabled)
        ));
        assert!(matches!(
            countries.check_destination("ZZ").await,
            Err(CountryError::Invalid)
        ));
        countries
            .save(
                &s,
                &s.csrf,
                &["IT".into(), "FR".into()],
                countries.revision().await.unwrap(),
            )
            .await
            .unwrap();
        assert!(countries.check_destination("IT").await.is_ok());
        assert!(matches!(
            countries.check_destination("US").await,
            Err(CountryError::NotEnabled)
        ));
        for values in [
            vec!["IT".into(), "ZZ".into()],
            vec!["IT".into(), "IT".into()],
            vec!["it".into()],
            vec!["US".into(); 250],
        ] {
            assert!(matches!(
                countries
                    .save(&s, &s.csrf, &values, countries.revision().await.unwrap())
                    .await,
                Err(CountryError::Invalid)
            ));
            assert_eq!(countries.selected(&s).await.unwrap(), ["FR", "IT"]);
        }
        assert!(
            sqlx::query("INSERT INTO checkout_countries(country_code) VALUES('ZZ')")
                .execute(&pool)
                .await
                .is_err()
        );
        let restarted_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let restarted = CheckoutCountries::new(
            restarted_pool.clone(),
            Identity::new(restarted_pool.clone()),
        );
        assert_eq!(restarted.selected(&s).await.unwrap(), ["FR", "IT"]);
        restarted_pool.close().await;
        let all: Vec<_> = talocart_platform::geography::COUNTRIES
            .iter()
            .map(|c| (*c).into())
            .collect();
        countries
            .save(&s, &s.csrf, &all, countries.revision().await.unwrap())
            .await
            .unwrap();
        assert_eq!(countries.selected(&s).await.unwrap().len(), 249);
        countries
            .save(&s, &s.csrf, &[], countries.revision().await.unwrap())
            .await
            .unwrap();
        assert!(matches!(
            countries.check_destination("IT").await,
            Err(CountryError::NotEnabled)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tax_zones")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn checkout_countries_concurrent_replacement_and_order_guard() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let countries = CheckoutCountries::new(pool.clone(), identity);
        let one = vec!["IT".into(), "FR".into()];
        let two = vec!["US".into(), "GB".into()];
        let (a, b) = tokio::join!(
            countries.save(&s, &s.csrf, &one, countries.revision().await.unwrap()),
            countries.save(&s, &s.csrf, &two, countries.revision().await.unwrap())
        );
        assert_ne!(a.is_ok(), b.is_ok());
        assert!(matches!(a, Err(CountryError::Stale)) || matches!(b, Err(CountryError::Stale)));
        let stored = countries.selected(&s).await.unwrap();
        assert!(
            stored == ["FR", "IT"] || stored == ["GB", "US"],
            "never combine partial country sets"
        );
        countries
            .save(
                &s,
                &s.csrf,
                &["IT".into()],
                countries.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let mut order = pool.begin().await.unwrap();
        CheckoutCountries::require_destination(&mut order, "IT")
            .await
            .unwrap();
        let writer = countries.clone();
        let session = s.clone();
        let mut update = tokio::spawn(async move {
            writer
                .save(
                    &session,
                    &session.csrf,
                    &[],
                    writer.revision().await.unwrap(),
                )
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(150), &mut update)
                .await
                .is_err(),
            "country removal waits for an order transaction holding its destination"
        );
        order.commit().await.unwrap();
        update.await.unwrap().unwrap();
        assert!(matches!(
            countries.check_destination("IT").await,
            Err(CountryError::NotEnabled)
        ));
    })
    .await;
}
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn checkout_countries_http_bilingual_csrf_reader_and_no_catalog_restriction() {
    isolated(|pool, _| async move {
        use talocart_platform::identity::{Permission, Role};
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let countries = CheckoutCountries::new(pool.clone(), identity.clone());
        for (locale, title) in [("it", "Paesi di acquisto"), ("en", "Purchase countries")] {
            identity.update_locale(&s, &s.csrf, locale).await.unwrap();
            let response =
                fiscal_request(&app, "GET", "/admin/settings/countries", &cookie, "", true).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], locale);
            assert!(html(response).await.contains(title));
            let response = fiscal_request(
                &app,
                "POST",
                "/admin/settings/countries",
                &cookie,
                &format!("csrf={}&country=IT&country=FR", s.csrf),
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let response = fiscal_request(
                &app,
                "POST",
                "/admin/settings/countries",
                &cookie,
                &format!("csrf={}&country=IT&country=ZZ", s.csrf),
                true,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let html = html(response).await;
            assert!(html.contains("ZZ"));
            assert!(html.contains("value=\"IT\" selected"));
            assert!(html.contains("aria-invalid=\"true\""));
            assert_eq!(countries.selected(&s).await.unwrap(), ["FR", "IT"]);
        }
        assert_eq!(
            fiscal_request(
                &app,
                "POST",
                "/admin/settings/countries",
                &cookie,
                "csrf=wrong&country=US",
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let role = identity
            .create_role(&s, &s.csrf, "Country reader", &[Permission::SettingsRead])
            .await
            .unwrap();
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "countries-reader@example.test",
                "correct horse reader".into(),
                Role::Custom(role),
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anon,
                &anon.csrf,
                "countries-reader@example.test",
                "correct horse reader".into(),
            )
            .await
            .unwrap();
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let reader_cookie = format!("talocart_session={raw}");
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/settings/taxes/coverage",
                &reader_cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let body = html(
            fiscal_request(
                &app,
                "GET",
                "/admin/settings/countries",
                &reader_cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(body.contains("id=\"checkout-countries\""));
        assert!(!body.contains("Salva paesi di acquisto"));
        assert_eq!(
            fiscal_request(
                &app,
                "POST",
                "/admin/settings/countries",
                &reader_cookie,
                &format!("csrf={}&country=US", reader.csrf),
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fiscal_request(&app, "GET", "/admin/settings/countries", "", "", false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        countries
            .save(&s, &s.csrf, &[], countries.revision().await.unwrap())
            .await
            .unwrap();
        assert_eq!(
            fiscal_request(&app, "GET", "/catalog", "", "", false)
                .await
                .status(),
            StatusCode::OK
        );
    })
    .await;
}
