use super::*;

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn regional_settings_http_bilingual_validation_conflicts_and_permissions() {
    isolated(|pool, options| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let owner = identity.session(&raw).await.unwrap().unwrap();
        let auth = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let page = html(request(&app, "GET", "/admin/settings", &auth, "", false).await).await;
            assert!(page.contains("id=\"platform-time-zone\""));
            assert!(page.contains("id=\"platform-date-format\""));
            assert!(page.contains(if locale == "it" { "Data e ora" } else { "Date and time" }));
        }
        let post = |zone: &str, style: &str, revision: i64| {
            format!("csrf={}&time_zone={zone}&date_format={style}&revision={revision}", owner.csrf)
        };
        let invalid = post("Invalid%2FZone", "dmy", 0);
        let response = request(&app, "POST", "/admin/settings/date-time", &auth, &invalid, true).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let page = html(response).await;
        assert!(page.contains("aria-invalid=\"true\""));
        assert!(page.contains("Invalid/Zone"));
        assert!(page.contains("id=\"regional-error\""));
        assert_eq!(request(&app, "POST", "/admin/settings/date-time", &auth, &post("UTC", "invalid", 0), false).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(request(&app, "POST", "/admin/settings/date-time", &auth, "csrf=wrong&time_zone=UTC&date_format=ymd&revision=0", false).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(request(&app, "POST", "/admin/settings/date-time", "", &post("UTC", "ymd", 0), true).await.status(), StatusCode::UNAUTHORIZED);
        let response = request(&app, "POST", "/admin/settings/date-time", &auth, &post("UTC", "ymd", 0), false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/admin/settings?regional_saved=1#date-time");
        let saved: (String, String, i64) = sqlx::query_as("SELECT time_zone,date_format,revision FROM platform_regional_settings WHERE singleton")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(saved, ("UTC".into(), "ymd".into(), 1));
        let stale = request(&app, "POST", "/admin/settings/date-time", &auth, &post("Europe%2FLondon", "mdy", 0), false).await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert!(html(stale).await.contains("Europe/London"));
        assert_eq!(saved, sqlx::query_as::<_, (String, String, i64)>("SELECT time_zone,date_format,revision FROM platform_regional_settings WHERE singleton").fetch_one(&pool).await.unwrap());
        identity.create_administrator(&owner, &owner.csrf, "regional-reader@example.test", "correct horse battery".into(), talocart_platform::identity::Role::Viewer).await.unwrap();
        let reader_raw = login_as(&identity, "regional-reader@example.test").await;
        let reader = identity.session(&reader_raw).await.unwrap().unwrap();
        let reader_auth = format!("talocart_session={reader_raw}");
        let page = html(request(&app, "GET", "/admin/settings", &reader_auth, "", false).await).await;
        assert!(page.contains("UTC"));
        assert!(!page.contains("name=\"time_zone\""));
        assert_eq!(request(&app, "POST", "/admin/settings/date-time", &reader_auth, &format!("csrf={}&time_zone=Europe%2FRome&date_format=dmy&revision=1", reader.csrf), false).await.status(), StatusCode::FORBIDDEN);
        let reopened = PgPoolOptions::new().connect_with(options).await.unwrap();
        assert_eq!(saved, sqlx::query_as::<_, (String, String, i64)>("SELECT time_zone,date_format,revision FROM platform_regional_settings WHERE singleton").fetch_one(&reopened).await.unwrap());
        reopened.close().await;
        assert!(sqlx::query("UPDATE platform_regional_settings SET time_zone='Invalid/Zone'").execute(&pool).await.is_err());
        assert!(sqlx::query("UPDATE platform_regional_settings SET date_format='invalid'").execute(&pool).await.is_err());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn regional_settings_clock_dst_formats_and_numbering_follow_the_selected_zone() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let settings = talocart_platform::regional::settings(&pool, &identity, &session).await.unwrap();
        assert_eq!(settings.time_zone, "Europe/Rome");
        assert_eq!(settings.date_format, "dmy");
        let zones = talocart_platform::regional::time_zones(&pool, &identity, &session).await.unwrap();
        for expected in ["UTC", "Europe/Rome", "America/New_York", "Asia/Kolkata"] {
            assert!(zones.iter().any(|zone| zone == expected));
        }
        assert!(zones.len() < 1000);
        for (instant, expected) in [
            ("2026-01-15T12:00:00Z", "15/01/2026 13:00:00"),
            ("2026-07-15T12:00:00Z", "15/07/2026 14:00:00"),
            ("2026-03-29T00:59:59Z", "29/03/2026 01:59:59"),
            ("2026-03-29T01:00:00Z", "29/03/2026 03:00:00"),
        ] {
            assert_eq!(sqlx::query_scalar::<_,String>("SELECT platform_datetime($1::text::timestamptz,time_zone,date_format) FROM platform_regional_settings WHERE singleton")
                .bind(instant).fetch_one(&pool).await.unwrap(), expected);
        }
        let reference = "SELECT next_order_reference('2026-12-31T23:30:00Z'::timestamptz)";
        assert_eq!(sqlx::query_scalar::<_,String>(reference).fetch_one(&pool).await.unwrap(), "ORD-2027-1");
        talocart_platform::regional::save(&pool, &session, &session.csrf, "UTC", "ymd", 0).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>(reference).fetch_one(&pool).await.unwrap(), "ORD-2026-1");
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT platform_datetime('2026-12-31T23:30:00Z',time_zone,date_format) FROM platform_regional_settings WHERE singleton")
            .fetch_one(&pool).await.unwrap(), "2026-12-31 23:30:00");
        talocart_platform::regional::save(&pool, &session, &session.csrf, "America/New_York", "mdy", 1).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>(reference).fetch_one(&pool).await.unwrap(), "ORD-2026-2");
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT platform_datetime('2027-01-01T00:30:00Z',time_zone,date_format) FROM platform_regional_settings WHERE singleton")
            .fetch_one(&pool).await.unwrap(), "12/31/2026 19:30:00");
        for (zone, style, revision) in [("+02", "dmy", 2), ("Europe/Rome'", "dmy", 2), ("UTC", "ymd", -1)] {
            assert!(matches!(talocart_platform::regional::save(&pool, &session, &session.csrf, zone, style, revision).await, Err(talocart_platform::regional::RegionalError::Invalid)));
        }
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn regional_settings_concurrent_saves_are_atomic_and_revoked_sessions_cannot_write() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let (a, b) = tokio::join!(
            talocart_platform::regional::save(&pool, &session, &session.csrf, "UTC", "ymd", 0),
            talocart_platform::regional::save(
                &pool,
                &session,
                &session.csrf,
                "America/New_York",
                "mdy",
                0
            )
        );
        assert_eq!([&a, &b].into_iter().filter(|r| r.is_ok()).count(), 1);
        assert!(
            [&a, &b]
                .into_iter()
                .any(|r| matches!(r, Err(talocart_platform::regional::RegionalError::Conflict)))
        );
        let saved = talocart_platform::regional::settings(&pool, &identity, &session)
            .await
            .unwrap();
        assert!(matches!(
            (saved.time_zone.as_str(), saved.date_format.as_str()),
            ("UTC", "ymd") | ("America/New_York", "mdy")
        ));
        assert_eq!(saved.revision, 1);
        identity.logout(&session, &session.csrf).await.unwrap();
        assert!(matches!(
            talocart_platform::regional::save(
                &pool,
                &session,
                &session.csrf,
                "Europe/Rome",
                "dmy",
                1
            )
            .await,
            Err(talocart_platform::regional::RegionalError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
    })
    .await;
}
