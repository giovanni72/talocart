include!("ui_support.rs");

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn all_admin_forms_and_htmx_errors_use_shared_ui() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for path in [
            "/admin",
            "/admin/account",
            "/admin/settings",
            "/admin/settings/appearance",
            "/admin/settings/storefront",
            "/admin/settings/taxes",
            "/admin/settings/taxes/rates/new",
            "/admin/settings/taxes/classes/new",
            "/admin/catalog/products/new",
            "/admin/catalog/categories/new",
            "/admin/catalog/brands/new",
            "/admin/catalog/options/new",
            "/admin/users/new",
            "/admin/roles/new",
        ] {
            let response = request(&app, "GET", path, &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let body = html(response).await;
            assert!(body.contains("/assets/ui.css"), "{path}");
            assert!(body.contains("ui-button"), "{path}");
            assert!(
                !body.contains("data-ui-toast"),
                "{path}: opening an admin page must not turn persistent guidance into action feedback"
            );
            assert!(!body.contains("class=\"button"), "{path}");
            if body.contains("type=\"checkbox\"") {
                assert!(body.contains("ui-check"), "{path}");
            }
        }
        for partial in [false, true] {
            let response = request(
                &app,
                "POST",
                "/admin/catalog/categories/new",
                &cookie,
                &format!("csrf={}&name=&slug=bad+slug&sort_order=0", session.csrf),
                partial,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body = html(response).await;
            assert!(body.contains("ui-message ui-toast ui-message--error"));
            assert!(body.contains("field-error ui-field-error"));
            assert!(body.contains("aria-invalid=\"true\""));
            assert!(body.contains("role=\"alert\""));
            assert!(body.contains("value=\"bad slug\""));
            assert!(body.contains("ui-control"));
        }
        let body = html(request(&app, "GET", "/admin/login", "", "", false).await).await;
        assert!(body.contains("ui-control"));
        assert!(body.contains("ui-button"));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn components_require_overview_permission_and_preview_is_read_only() {
    use talocart_platform::identity::Role;
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for partial in [false, true] {
            assert_eq!(
                request(&app, "GET", "/admin/components", "", "", partial)
                    .await
                    .status(),
                if partial {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::SEE_OTHER
                }
            );
        }
        let role = identity
            .create_role(&session, &session.csrf, "Nessun accesso", &[])
            .await
            .unwrap();
        identity
            .create_administrator(
                &session,
                &session.csrf,
                "no-access@example.com",
                "correct horse battery".into(),
                Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let denied = format!(
            "talocart_session={}",
            login_as(&identity, "no-access@example.com").await
        );
        for partial in [false, true] {
            assert_eq!(
                request(&app, "GET", "/admin/components", &denied, "", partial)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        let before: String =
            sqlx::query_scalar("SELECT row_to_json(s)::text FROM store_settings s")
                .fetch_one(&pool)
                .await
                .unwrap();
        let response = request(&app, "GET", "/admin/components", &cookie, "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = html(response).await;
        for expected in [
            "/assets/ui.css",
            "Componenti",
            "href=\"/admin/components\" class=\"active\" aria-current=\"page\"",
            "id=\"example-label\"",
            "readonly",
            "disabled",
            "ui-button--danger",
            "ui-message--error",
            "ui-empty",
        ] {
            assert!(body.contains(expected), "Missing {expected}");
        }
        let response = request(
            &app,
            "GET",
            "/admin/components?preview=1&label=Caff%C3%A8&status=inactive",
            &cookie,
            "",
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = html(response).await;
        assert!(body.contains("Anteprima aggiornata."));
        assert!(body.contains("value=\"Caffè\""));
        assert!(body.contains("value=\"inactive\" selected"));
        for path in [
            "/admin/components?preview=1&label=%20%20&status=active",
            "/admin/components?preview=1&label=Nome&status=bad",
        ] {
            let response = request(&app, "GET", path, &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body = html(response).await;
            assert!(body.contains("aria-invalid=\"true\""));
            assert!(body.contains("role=\"alert\""));
            assert!(body.contains("-error\""));
        }
        let body = html(
            request(
                &app,
                "GET",
                "/admin/components?preview=1&label=%3Cscript%3Ebad%3C%2Fscript%3E&status=active",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(!body.contains("<script>bad</script>"));
        assert!(body.contains("&#60;script&#62;bad&#60;/script&#62;"));
        assert_eq!(
            request(&app, "POST", "/admin/components", &cookie, "", false)
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let after: String = sqlx::query_scalar("SELECT row_to_json(s)::text FROM store_settings s")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_products")
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
async fn pilot_lists_share_controls_messages_and_empty_states() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let cookie = format!("talocart_session={}", authenticated(&identity).await);
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for path in ["/admin/catalog/categories", "/admin/catalog/brands"] {
            let body = html(request(&app, "GET", path, &cookie, "", false).await).await;
            assert!(body.contains("ui-empty catalog-empty"));
            assert!(body.contains("class=\"ui-control\" id=\"catalog-search\""));
            assert!(body.contains("class=\"ui-control\" id=\"catalog-status\""));
            assert!(body.contains("ui-button--secondary"));
            let body = html(
                request(
                    &app,
                    "GET",
                    &format!("{path}?q=assente&status=active&saved=1"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(body.contains("Nessun risultato."));
            assert!(body.contains("ui-message ui-toast ui-message--success"));
            assert!(body.contains("data-ui-toast"));
            assert!(body.contains("role=\"status\""));
            assert!(body.contains("value=\"assente\""));
            assert!(body.contains("value=\"active\" selected"));
            assert_eq!(
                request(&app, "GET", path, "", "", false).await.status(),
                StatusCode::SEE_OTHER
            );
            assert_eq!(
                request(
                    &app,
                    "GET",
                    &format!("{path}?status=unknown"),
                    &cookie,
                    "",
                    false
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        let body = html(request(&app, "GET", "/catalog", "", "", false).await).await;
        assert!(body.contains("ui-empty store-empty"));
        assert!(body.contains("Il catalogo si prepara."));
        assert!(body.contains("ui-control\" id=\"store-search\""));
        assert!(body.contains("ui-control\" id=\"catalog-page-size\""));
        assert!(body.contains("pagination-button ui-button is-disabled"));
        let body = html(request(&app, "GET", "/search/assente", "", "", false).await).await;
        assert!(body.contains("Nessun prodotto trovato."));
        assert!(body.contains("ui-empty store-empty"));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn lucide_examples_and_store_controls_keep_bilingual_names_and_native_links() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let primary: String = sqlx::query_scalar("SELECT primary_locale FROM storefront_settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        for (locale, title, fields, search, close) in [
            (
                "it",
                "Icone Lucide",
                "Vai ai campi di esempio",
                "Cerca",
                "Chiudi il carrello",
            ),
            (
                "en",
                "Lucide icons",
                "Go to example fields",
                "Search",
                "Close cart",
            ),
        ] {
            identity
                .update_locale(&session, &session.csrf, locale)
                .await
                .unwrap();
            let response = request(&app, "GET", "/admin/components", &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], locale);
            let body = html(response).await;
            assert!(body.contains(title));
            assert!(body.contains(&format!("aria-label=\"{fields}\"")));
            assert!(body.contains("ui-button--mini"));
            assert!(body.contains(if locale == "it" {
                "Icona mini"
            } else {
                "Mini icon"
            }));
            for name in [
                "shopping-cart",
                "search",
                "x",
                "pencil",
                "trash",
                "plus",
                "check",
                "triangle-alert",
            ] {
                assert!(body.contains(&format!("data-ui-icon=\"{name}\"")));
            }
            let prefix = if locale == primary {
                String::new()
            } else {
                format!("/{locale}")
            };
            let response = request(&app, "GET", &format!("{prefix}/catalog"), "", "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], locale);
            let body = html(response).await;
            assert_eq!(body.matches("data-ui-icon=\"shopping-cart\"").count(), 3);
            assert_eq!(body.matches("data-ui-icon=\"search\"").count(), 1);
            assert_eq!(body.matches("data-ui-icon=\"x\"").count(), 2);
            assert!(
                body.contains(
                    "id=\"minicart-error\" class=\"ui-message ui-toast ui-message--error"
                )
            );
            assert!(body.contains(&format!("<span>{search}</span>")));
            assert!(body.contains(&format!("aria-label=\"{close}\"")));
            assert!(body.contains(&format!("title=\"{close}\"")));
            assert!(body.contains("ui-button--mini ui-button--icon"));
            assert!(!body.contains(&format!("<span>{close}</span>")));
            assert!(body.contains(&format!("href=\"{prefix}/cart\"")));
            assert!(body.contains("aria-labelledby=\"minicart-title\""));
            assert!(!body.contains("lucide.dev"));
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn list_edit_icons_preserve_localized_names_permissions_and_escaping() {
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let owner_raw = authenticated(&identity).await;
        let owner = identity.session(&owner_raw).await.unwrap().unwrap();
        let name = "Synthetic <b>entity</b>";
        let rate: i64 = sqlx::query_scalar("INSERT INTO tax_rates(name,basis_points) VALUES($1,2000) RETURNING id").bind(name).fetch_one(&pool).await.unwrap();
        let class: i64 = sqlx::query_scalar("INSERT INTO tax_classes(name,rate_id) VALUES($1,$2) RETURNING id").bind(name).bind(rate).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_categories(name,slug) VALUES($1,'icon-category')").bind(name).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_brands(name,slug) VALUES($1,'icon-brand')").bind(name).execute(&pool).await.unwrap();
        let product: i64 = sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,tax_class_id) VALUES($1,'ICON-P','icon-product',100,$2) RETURNING id").bind(name).bind(class).fetch_one(&pool).await.unwrap();
        let option: i64 = sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Icon size') RETURNING id").fetch_one(&pool).await.unwrap();
        let value: i64 = sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Large') RETURNING id").bind(option).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)").bind(product).bind(option).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],'ICON-V',100,1)").bind(product).bind(value).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let zone: i64 = sqlx::query_scalar("INSERT INTO tax_zones(name) VALUES($1) RETURNING id").bind(name).fetch_one(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO tax_zone_countries(country_code,zone_id) VALUES('IT',$1)").bind(zone).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let role = identity.create_role(&owner, &owner.csrf, "Icon reader", &[Permission::AdminRead, Permission::ProductsRead, Permission::CategoriesRead, Permission::BrandsRead, Permission::TaxesRead]).await.unwrap();
        identity.create_administrator(&owner, &owner.csrf, "icon-reader@example.test", "correct horse battery".into(), Role::parse(&role).unwrap()).await.unwrap();
        let viewer_raw = login_as(&identity, "icon-reader@example.test").await;
        let viewer = identity.session(&viewer_raw).await.unwrap().unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let paths = ["/admin/catalog/products".to_string(),format!("/admin/catalog/products/{product}/variants"),"/admin/catalog/categories".into(),"/admin/catalog/brands".into(),"/admin/settings/taxes/rates".into(),"/admin/settings/taxes/classes".into(),"/admin/settings/taxes/zones".into()];
        for (locale, edit, view) in [("it", "Modifica", "Consulta"),("en", "Edit", "View")] {
            for (session, raw, writable) in [(&owner, &owner_raw, true),(&viewer, &viewer_raw, false)] {
                identity.update_locale(session, &session.csrf, locale).await.unwrap();
                for path in &paths {
                    let response = request(&app, "GET", path, &format!("talocart_session={raw}"), "", false).await;
                    assert_eq!(response.status(), StatusCode::OK, "{path}");
                    assert_eq!(response.headers()[header::CONTENT_LANGUAGE], locale);
                    let body = html(response).await;
                    assert!(!body.contains("<b>entity</b>"), "Resource names stay escaped");
                    let actions: Vec<_> = body.split("<a ").filter_map(|tail| tail.split_once("</a>")).map(|(action, _)| action).filter(|action| action.split('>').next().unwrap().contains("catalog-edit")).collect();
                    assert_eq!(actions.len(), 1, "{path}");
                    let action = actions[0];
                    let (attributes, content) = action.split_once('>').unwrap();
                    assert!(attributes.contains(&format!("aria-label=\"{} ",if writable {edit} else {view})));
                    assert!(attributes.contains(&format!("title=\"{} ",if writable {edit} else {view})));
                    assert_eq!(attributes.contains("data-ui-edit"),writable);
                    assert_eq!(attributes.contains("ui-button--icon"),writable);
                    if writable {
                        assert!(content.contains("data-ui-icon=\"pencil\""));
                        assert!(!content.contains("<span>"));
                    } else {
                        assert!(content.contains("data-ui-icon=\"eye\""));
                        assert!(content.contains(&format!("<span>{view}</span>")));
                        assert!(!body.contains("href=\"/admin/users\""));
                    }
                }
                let options = html(request(&app, "GET", "/admin/catalog/options", &format!("talocart_session={raw}"), "", false).await).await;
                let action = options.split("<a ").filter_map(|tail| tail.split_once("</a>")).map(|(action, _)| action).find(|action| action.split('>').next().unwrap().contains("catalog-edit")).expect("Option action exists");
                let (_, content) = action.split_once('>').unwrap();
                assert!(content.contains(&format!("data-ui-icon=\"{}\"", if writable {"pencil"} else {"eye"})));
                assert!(content.contains(&format!("<span>{}</span>", if writable { if locale == "it" {"Gestisci"} else {"Manage"} } else {view})));
                if writable {
                    assert!(options.contains(if locale == "it" {"<span>Nuova opzione</span>"} else {"<span>New option</span>"}));
                }
            }
        }
    }).await;
}
