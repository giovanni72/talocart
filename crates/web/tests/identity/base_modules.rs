use super::*;
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn base_inventory_has_no_private_module_and_its_urls_are_unavailable_in_both_languages() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity
            .session(&raw)
            .await
            .expect("session query")
            .expect("session");
        let state = test_app(pool.clone(), "Base only".into()).await;
        assert_eq!(
            state
                .module_inventory()
                .iter()
                .map(|module| module.id)
                .collect::<Vec<_>>(),
            ["base"]
        );
        let app = router(state);
        let cookie = format!("talocart_session={raw}");
        for (locale, empty, label) in [
            ("it", "Nessun modulo aggiuntivo installato", "Moduli"),
            ("en", "No additional modules installed", "Modules"),
        ] {
            identity
                .update_locale(&session, &session.csrf, locale)
                .await
                .expect("locale");
            let overview = html(request(&app, "GET", "/admin", &cookie, "", false).await).await;
            let main = overview
                .split("<main")
                .nth(1)
                .expect("overview main");
            assert!(!main.contains("href=\"/admin/modules\""), "No modules link in the overview body");
            assert!(!overview.contains("base-inventory-link"));
            let sidebar = overview.split("<main").next().expect("sidebar");
            let settings = sidebar
                .split("data-ui-icon=\"settings\"")
                .nth(1)
                .expect("settings group");
            assert!(settings.contains("id=\"sidebar-modules-link\" href=\"/admin/modules\"><svg"));
            assert!(settings.contains(&format!("<span>{label}</span></a>")));
            assert!(!settings.contains("id=\"sidebar-modules-link\" href=\"/admin/modules\" class=\"active\""));
            let page = request(&app, "GET", "/admin/modules", &cookie, "", false).await;
            assert_eq!(page.status(), StatusCode::OK);
            let page = html(page).await;
            assert!(page.contains(empty));
            assert!(page.contains(
                "id=\"sidebar-modules-link\" href=\"/admin/modules\" class=\"active\" aria-current=\"page\""
            ));
            assert!(page.contains("class=\"sidebar-group\" open>"));
            assert!(!page.contains("data-module=\"koala-import\""));
            assert!(!page.contains("href=\"/admin/catalog/imports\""));
            for (method, path) in [
                ("GET", "/admin/catalog/imports"),
                ("POST", "/admin/catalog/imports"),
                ("GET", "/admin/koala-import.css"),
            ] {
                assert_eq!(
                    request(&app, method, path, &cookie, "", false)
                        .await
                        .status(),
                    StatusCode::NOT_FOUND
                );
            }
        }
    })
    .await;
}
