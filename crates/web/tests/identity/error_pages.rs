use super::*;

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn error_pages_use_frontend_and_backend_layouts_without_changing_fragments() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Error pages".into()).await);
        talocart_platform::storefront::Storefront::new(pool.clone())
            .save_languages(&session, &session.csrf, "it", &["it".into(), "en".into()])
            .await
            .unwrap();
        for theme in ["default", "essential"] {
            sqlx::query("UPDATE storefront_settings SET theme_id=$1")
                .bind(theme)
                .execute(&pool)
                .await
                .unwrap();
            for (locale, prefix) in [("it", ""), ("en", "/en")] {
                sqlx::query("UPDATE administrators SET locale=$1")
                    .bind(locale)
                    .execute(&pool)
                    .await
                    .unwrap();
                for (path, status) in [
                    ("/products/missing", StatusCode::NOT_FOUND),
                    ("/pages/missing", StatusCode::NOT_FOUND),
                    ("/missing", StatusCode::NOT_FOUND),
                    (
                        "/catalog?per_page=invalid",
                        StatusCode::UNPROCESSABLE_ENTITY,
                    ),
                ] {
                    let response =
                        request(&app, "GET", &format!("{prefix}{path}"), "", "", false).await;
                    assert_eq!(response.status(), status);
                    assert!(
                        response.headers()[header::CONTENT_TYPE]
                            .to_str()
                            .unwrap()
                            .starts_with("text/html")
                    );
                    let body = html(response).await;
                    assert!(body.contains(&format!("data-theme=\"{theme}\"")));
                    assert!(body.contains("class=\"site-header\""));
                    assert!(body.contains("class=\"site-footer\""));
                    assert!(body.contains(&format!("href=\"{prefix}/catalog\"")));
                    assert!(!body.contains("class=\"admin-shell\""));
                }
                for path in [
                    "/admin/catalog/products/9223372036854775807",
                    "/admin/unknown",
                ] {
                    let response = request(&app, "GET", path, &cookie, "", false).await;
                    assert_eq!(response.status(), StatusCode::NOT_FOUND);
                    let body = html(response).await;
                    assert!(body.contains("class=\"admin-shell\""));
                    assert!(body.contains("class=\"admin-sidebar\""));
                    assert!(body.contains("href=\"/admin\""));
                    assert!(!body.contains("class=\"site-footer\""));
                    assert!(body.contains(&format!("<html lang=\"{locale}\"")));
                }
                let response = request(
                    &app,
                    "POST",
                    "/admin/catalog/products/bulk",
                    &cookie,
                    "csrf=wrong&ids=1",
                    false,
                )
                .await;
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
                assert!(html(response).await.contains("class=\"admin-shell\""));
                for path in [
                    format!("{prefix}/products/missing"),
                    "/admin/catalog/products/9223372036854775807".into(),
                ] {
                    let response = request(&app, "GET", &path, &cookie, "", true).await;
                    assert_eq!(response.status(), StatusCode::NOT_FOUND);
                    assert!(!html(response).await.contains("<!doctype html>"));
                }
            }
        }
        let head = request(&app, "HEAD", "/products/missing", "", "", false).await;
        assert_eq!(head.status(), StatusCode::NOT_FOUND);
        assert!(
            axum::body::to_bytes(head.into_body(), 1024)
                .await
                .unwrap()
                .is_empty()
        );
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/products/missing")
                    .header(header::ACCEPT, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/plain")
        );
        for (path, status, content_type) in [
            ("/assets/missing.js", StatusCode::NOT_FOUND, "text/plain"),
            // Missing public images intentionally retain their SVG placeholder contract.
            (
                "/media/public/9223372036854775807/missing.png/original",
                StatusCode::OK,
                "image/svg+xml",
            ),
        ] {
            let response = request(&app, "GET", path, "", "", false).await;
            assert_eq!(response.status(), status);
            assert!(
                response.headers()[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with(content_type)
            );
            assert!(!html(response).await.contains("class=\"site-header\""));
        }
    })
    .await;
}
