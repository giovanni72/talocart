use super::*;

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn public_footer_is_shared_localized_and_only_lists_published_footer_pages() {
    isolated(|pool, _| async move {
        super::checkout_methods::fixture(&pool).await;
        sqlx::query("UPDATE store_settings SET name=$1 WHERE id=1")
            .bind("Shop <safe>")
            .execute(&pool)
            .await
            .expect("Store identity");
        let app = router(test_app(pool.clone(), "Shop <safe>".into()).await);
        for theme in ["default", "essential"] {
            sqlx::query("UPDATE storefront_settings SET theme_id=$1 WHERE id=1")
                .bind(theme)
                .execute(&pool)
                .await
                .expect("Select compiled theme");
            for (prefix, shop, purchases) in [
                ("", "Explore", "Your purchases"),
                ("/it", "Esplora", "I tuoi acquisti"),
            ] {
                for route in [
                    "/", "/catalog", "/brands", "/search/Item", "/products/method-item",
                    "/cart", "/account/login", "/account/register", "/account/reset", "/missing",
                ] {
                    let path = if route == "/" && !prefix.is_empty() {
                        prefix.to_owned()
                    } else {
                        format!("{prefix}{route}")
                    };
                    let response = request(&app, "GET", &path, "", "", false).await;
                    assert_eq!(response.status(), if route == "/missing" { StatusCode::NOT_FOUND } else { StatusCode::OK }, "{theme} {path}");
                    let document = html(response).await;
                    assert_eq!(document.matches("<footer class=\"site-footer\"").count(), 1, "{theme} {path}");
                    let footer = document.split("<footer class=\"site-footer\"").nth(1).expect("Public footer").split("</footer>").next().expect("Footer end");
                    assert!(footer.contains("data-store-name=\"Shop &#60;safe&#62;\""));
                    assert!(footer.contains(&format!("id=\"footer-shop-heading\">{shop}</h2>")));
                    assert!(footer.contains(&format!("id=\"footer-account-heading\">{purchases}</h2>")));
                    for target in ["/catalog", "/brands", "/account", "/cart"] {
                        assert!(footer.contains(&format!("href=\"{prefix}{target}\"")), "{theme} {path} {target}");
                    }
                    assert!(!footer.contains("footer-information-heading"));
                    assert!(!footer.contains("<safe>"));
                }
            }
        }
        sqlx::raw_sql("INSERT INTO information_pages(slug,name,description,published,header_menu,footer_menu) VALUES ('footer-page','Footer <safe>','Information',true,false,true),('header-only','Header only','Information',true,true,false),('draft-footer','Draft footer','Draft',false,false,true); INSERT INTO information_pages_translations(entity_id,locale,name,description) SELECT id,'it','Informazioni <safe>','Informazioni' FROM information_pages WHERE slug='footer-page';")
            .execute(&pool).await.expect("Published, header-only and draft pages");
        for theme in ["default", "essential"] {
            sqlx::query("UPDATE storefront_settings SET theme_id=$1 WHERE id=1").bind(theme).execute(&pool).await.expect("Select theme");
            for (prefix, label, heading) in [("", "Footer &#60;safe&#62;", "Information"), ("/it", "Informazioni &#60;safe&#62;", "Informazioni")] {
                let document = html(request(&app, "GET", &format!("{prefix}/pages/footer-page"), "", "", false).await).await;
                let footer = document.split("<footer class=\"site-footer\"").nth(1).expect("Footer").split("</footer>").next().expect("Footer end");
                assert!(footer.contains(&format!("id=\"footer-information-heading\">{heading}</h2>")));
                assert!(footer.contains(&format!("href=\"{prefix}/pages/footer-page\" aria-current=\"page\">{label}</a>")));
                assert!(!footer.contains("header-only") && !footer.contains("draft-footer"));
            }
        }
        let fragment = html(request(&app, "GET", "/cart?preview=1", "", "", true).await).await;
        assert!(!fragment.contains("<footer"), "HTMX fragments do not duplicate the document footer");
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn public_documents_use_theme_assets_and_admin_keeps_shared_styles() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let auth = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Theme test".into()).await);
        for prefix in ["", "/it"] {
            for route in [
                "/",
                "/catalog",
                "/brands",
                "/cart",
                "/account/login",
                "/account/register",
                "/account/reset",
                "/pages/missing",
                "/products/missing",
            ] {
                let path = if route == "/" {
                    if prefix.is_empty() {
                        "/".to_owned()
                    } else {
                        prefix.into()
                    }
                } else {
                    format!("{prefix}{route}")
                };
                let response = request(&app, "GET", &path, "", "", false).await;
                assert!(
                    matches!(response.status(), StatusCode::OK | StatusCode::NOT_FOUND),
                    "{path}"
                );
                let status = response.status();
                let h = html(response).await;
                if status == StatusCode::NOT_FOUND && !h.contains("<html") {
                    assert!(
                        !h.is_empty(),
                        "Preserve the existing plain-text error contract"
                    );
                    continue;
                }
                assert!(
                    h.contains("/assets/base.css") && h.contains("/assets/themes/default.css"),
                    "{path}"
                );
                assert!(!h.contains("/assets/admin.css"), "{path}");
            }
        }
        for route in [
            "/admin",
            "/admin/modules",
            "/admin/account",
            "/admin/settings",
            "/admin/login",
            "/setup",
        ] {
            let response = request(&app, "GET", route, &auth, "", false).await;
            if response.status().is_redirection() {
                continue;
            }
            let h = html(response).await;
            assert!(
                h.contains("/assets/base.css") && !h.contains("/assets/themes/default.css"),
                "{route}"
            );
        }
        sqlx::raw_sql("UPDATE storefront_settings SET theme_id='essential',primary_locale='en' WHERE id=1; DELETE FROM storefront_language_prefixes WHERE code='en';")
            .execute(&pool)
            .await
            .unwrap();
        // The unpublished primary-language prefix is still an HTML 404, and must
        // use the selected theme instead of rendering outside the request scope.
        let response = request(&app, "GET", "/en/catalog", "", "", false).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let h = html(response).await;
        assert!(
            h.contains("data-theme=\"essential\""),
            "Unpublished-prefix 404 uses selected theme"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn theme_stylesheets_support_get_head_and_conditional_revalidation() {
    isolated(|pool, _| async move {
        installed(&pool).await;
        let app = router(test_app(pool, "Theme test".into()).await);
        for path in ["/assets/themes/default.css", "/assets/themes/essential.css"] {
            let response = request(&app, "GET", path, "", "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "text/css; charset=utf-8"
            );
            assert!(
                response.headers()[header::CACHE_CONTROL]
                    .to_str()
                    .unwrap()
                    .contains("must-revalidate")
            );
            let etag = response.headers()[header::ETAG].clone();
            assert!(html(response).await.contains(".store-shell"));
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("HEAD")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::ETAG], etag);
            assert!(html(response).await.is_empty());
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::IF_NONE_MATCH, etag)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        }
    })
    .await;
}

use super::unified_settings::{form, set, snapshot};
use talocart_platform::{
    identity::{Permission, Role},
    storefront::Storefront,
};

async fn selected(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT theme_id FROM storefront_settings WHERE id=1")
        .fetch_one(pool)
        .await
        .unwrap()
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn themes_selection_is_atomic_validated_and_rejects_stale_or_unauthorized_forms() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let owner = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let old = form(&app, &cookie).await;
        assert_eq!(selected(&pool).await, "default");
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/save",
                &cookie,
                &set(&old, "theme_id", "essential"),
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(selected(&pool).await, "essential");
        let after = snapshot(&pool).await;
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &cookie, &old, false)
                .await
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(snapshot(&pool).await, after);
        for locale in ["en", "it"] {
            identity
                .update_locale(&owner, &owner.csrf, locale)
                .await
                .unwrap();
            let body = set(
                &set(&form(&app, &cookie).await, "theme_id", "missing"),
                "name",
                "Unsaved",
            );
            let response =
                request(&app, "POST", "/admin/settings/save", &cookie, &body, false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let h = html(response).await;
            assert!(h.contains("id=\"theme-id\" name=\"theme_id\" aria-invalid=\"true\""));
            assert!(h.contains("value=\"Unsaved\""));
            assert!(h.contains(if locale == "it" {
                "Scegli un tema incluso"
            } else {
                "Choose a theme included"
            }));
            let current = snapshot(&pool).await;
            let bad_csrf = set(&form(&app, &cookie).await, "csrf", "wrong");
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/admin/settings/save",
                    &cookie,
                    &bad_csrf,
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(snapshot(&pool).await, current);
        }
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Reader",
                &[Permission::AdminRead, Permission::SettingsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "reader@example.test",
                "correct horse battery".into(),
                Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let reader = format!(
            "talocart_session={}",
            login_as(&identity, "reader@example.test").await
        );
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &reader, &old, false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(selected(&pool).await, "essential");
        // Legacy presentation APIs cannot bypass the revision-checked theme selector.
        let mut settings = Storefront::new(pool.clone()).settings().await.unwrap();
        settings.theme_id = "default".into();
        Storefront::new(pool.clone())
            .save(&owner, &owner.csrf, settings)
            .await
            .unwrap();
        assert_eq!(selected(&pool).await, "essential");
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn themes_concurrent_settings_edits_have_one_winner() {
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let raw=authenticated(&identity).await;
        let cookie=format!("talocart_session={raw}");let app=router(test_app(pool.clone(),"Shop".into()).await);
        let original=form(&app,&cookie).await;
        let a=set(&set(&original,"theme_id","essential"),"name","WriterA");
        let b=set(&set(&original,"theme_id","default"),"name","WriterB");
        let(a,b)=tokio::join!(request(&app,"POST","/admin/settings/save",&cookie,&a,false),request(&app,"POST","/admin/settings/save",&cookie,&b,false));
        assert_eq!([a.status(),b.status()].into_iter().filter(|s|*s==StatusCode::SEE_OTHER).count(),1);
        assert_eq!([a.status(),b.status()].into_iter().filter(|s|*s==StatusCode::CONFLICT).count(),1);
        let (name,theme):(String,String)=sqlx::query_as("SELECT name,theme_id FROM store_settings CROSS JOIN storefront_settings WHERE store_settings.id=1 AND storefront_settings.id=1").fetch_one(&pool).await.unwrap();
        assert_eq!((name.as_str(),theme.as_str()),if a.status()==StatusCode::SEE_OTHER {("WriterA","essential")}else{("WriterB","default")});
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn themes_change_reloads_stale_htmx_documents_without_losing_the_url() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let auth = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let before = html(request(&app, "GET", "/catalog", "", "", false).await).await;
        assert!(before.contains("theme=default"));
        let policy = before
            .split("X-Talocart-Store-Policy\":\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let response = request(
            &app,
            "POST",
            "/admin/settings/save",
            &auth,
            &set(&form(&app, &auth).await, "theme_id", "essential"),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        for uri in [
            "/catalog?per_page=84",
            "/cart?preview=1",
            "/account/login",
            "/pages/missing",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("hx-request", "true")
                        .header("x-talocart-store-policy", policy)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()["hx-redirect"],
                if uri.starts_with("/cart?") {
                    "/cart"
                } else {
                    uri
                }
            );
        }
        let after = html(request(&app, "GET", "/catalog", "", "", false).await).await;
        assert!(
            after.contains("data-theme=\"essential\"")
                && after.contains("/assets/themes/essential.css")
                && after.contains("essential-masthead")
        );
        let admin = html(request(&app, "GET", "/admin", &auth, "", false).await).await;
        assert!(!admin.contains("/assets/themes/essential.css") && !admin.contains("data-theme="));
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn themes_missing_from_build_fail_diagnostics_and_reads_without_a_fallback() {
    isolated(|pool,_|async move {
        installed(&pool).await;
        sqlx::raw_sql("ALTER TABLE storefront_settings DROP CONSTRAINT storefront_settings_theme_id_check; UPDATE storefront_settings SET theme_id='unavailable';").execute(&pool).await.unwrap();
        assert!(matches!(database::check(&pool).await,Err(database::DatabaseError::Theme)));
        assert!(Storefront::new(pool.clone()).settings().await.is_err());
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        assert_eq!(request(&app,"GET","/catalog","","",false).await.status(),StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(selected(&pool).await,"unavailable");
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL, npm ci and Playwright Chromium"]
async fn themes_browser_native_selection_and_all_public_surfaces() {
    isolated(|pool,_|async move {
        super::checkout_methods::fixture(&pool).await;
        sqlx::raw_sql("INSERT INTO catalog_categories(name,slug) VALUES('Theme category','theme-category'); INSERT INTO catalog_categories(name,slug,parent_id) SELECT 'Theme child','theme-child',id FROM catalog_categories WHERE slug='theme-category'; INSERT INTO catalog_brands(name,slug) VALUES('Theme brand','theme-brand'); UPDATE catalog_products SET stock=100,category_id=(SELECT id FROM catalog_categories WHERE slug='theme-child'),brand_id=(SELECT id FROM catalog_brands WHERE slug='theme-brand'); INSERT INTO information_pages(slug,name,description,published,header_menu,footer_menu) VALUES('theme-info','Theme information','Preserved content',true,true,true);").execute(&pool).await.unwrap();
        sqlx::query("UPDATE email_configuration SET from_name='Shop',from_email='shop@example.test',admin_recipients=ARRAY['admin@example.test'],enabled=true").execute(&pool).await.expect("Synthetic order email configuration; no delivery worker");
        let identity=test_identity(pool.clone()).await;let admin=authenticated(&identity).await;
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let origin=format!("http://{}",listener.local_addr().unwrap());
        let state=test_app_config(pool.clone(),"Themes".into(), talocart_web::AppStateConfig { public_origin: origin.clone(), ..Default::default() }).await;
        let task=tokio::spawn(async move{axum::serve(listener,router(state)).await.unwrap();});
        let mut child=tokio::process::Command::new("node").arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/browser-themes.cjs"))
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true).spawn().unwrap();
        use tokio::io::AsyncWriteExt;child.stdin.take().unwrap().write_all(serde_json::json!({"origin":origin,"admin":admin}).to_string().as_bytes()).await.unwrap();
        let result=tokio::time::timeout(std::time::Duration::from_secs(300),child.wait_with_output()).await.expect("Theme browser timeout").unwrap();
        task.abort();let _=task.await;assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
        println!("{}",String::from_utf8_lossy(&result.stdout));
    }).await;
}

#[tokio::test]
#[ignore = "Requires npm ci and Playwright Chromium"]
async fn themes_card_sizes_match_actual_layout_for_density_and_breakpoints() {
    use askama::Template;
    #[derive(Template)]
    #[template(path = "essential/templates/card-sizes.html")]
    struct Sizes {
        compact: bool,
        nested: bool,
    }
    let samples: Vec<_> = [false, true].into_iter().flat_map(|compact| {
        [false, true].into_iter().map(move |nested| {
            let template = Sizes { compact, nested };
            serde_json::json!({"compact": compact, "nested": nested, "sizes": template.render().expect("Render sizes")})
        })
    }).collect();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = tokio::process::Command::new("node")
        .arg(root.join("scripts/browser-theme-card-sizes.cjs"))
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Launch browser regression");
    use tokio::io::AsyncWriteExt;
    child
        .stdin
        .take()
        .expect("Browser stdin")
        .write_all(
            serde_json::to_string(&samples)
                .expect("Serialize samples")
                .as_bytes(),
        )
        .await
        .expect("Write samples");
    let result = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
        .await
        .expect("Browser timeout")
        .expect("Browser output");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    println!("{}", String::from_utf8_lossy(&result.stdout));
}
