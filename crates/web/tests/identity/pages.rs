use super::*;
use talocart_commerce::pages::{CONTENT, Input, PageError, Pages};
use talocart_platform::{
    content::{Draft, Language, Translation},
    identity::{Permission, Role, Session},
    locale::Locale,
    storefront::Storefront,
};
fn lang(code: &str) -> Language {
    Language::new(Locale::parse(code).unwrap(), Locale::ENGLISH)
}
fn input(slug: &str) -> Input {
    Input {
        slug: slug.into(),
        name: "About our shop".into(),
        description: "First paragraph.\n\nSecond <script>alert(1)</script> paragraph.".into(),
        seo_title: "About & shop".into(),
        seo_description: "Shop information".into(),
        published: true,
        header_menu: true,
        footer_menu: true,
        sort_order: "0".into(),
    }
}
fn draft() -> Draft {
    Draft {
        primary: "en".into(),
        rows: vec![Translation {
            locale: "it".into(),
            name: "Il nostro negozio".into(),
            description: "Primo paragrafo.\n\nSecondo paragrafo.".into(),
            seo_title: "Il negozio & noi".into(),
            seo_description: "Informazioni sul negozio".into(),
        }],
        active: Some(vec!["en".into(), "it".into()]),
        submitted: None,
    }
}
async fn fixture(pool: &PgPool) -> (Identity, Session, Pages) {
    let identity = installed(pool).await;
    let s = taxonomy_session(&identity).await;
    Storefront::new(pool.clone())
        .save_languages(&s, &s.csrf, "en", &["en".into(), "it".into()])
        .await
        .unwrap();
    let pages = Pages::new(pool.clone(), identity.clone());
    (identity, s, pages)
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_bilingual_publication_navigation_seo_and_escaping() {
    isolated(|pool, _| async move {
        let (_, s, pages) = fixture(&pool).await;
        let id = pages
            .save(&s, &s.csrf, 0, 0, &input("about"), Some(&draft()))
            .await
            .unwrap();
        let mut hidden = input("private-draft");
        hidden.published = false;
        pages.save(&s, &s.csrf, 0, 0, &hidden, None).await.unwrap();
        assert!(matches!(
            pages.public("private-draft", lang("en")).await,
            Err(PageError::NotFound)
        ));
        assert_eq!(
            pages.navigation(lang("it")).await.unwrap()[0].name,
            "Il nostro negozio"
        );
        assert_eq!(pages.sitemap().await.unwrap(), vec!["about"]);
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
        for (prefix, code, title, seo) in [
            ("", "en", "About our shop", "About &#38; shop"),
            ("/it", "it", "Il nostro negozio", "Il negozio &#38; noi"),
        ] {
            let r = request(&app, "GET", &format!("{prefix}/pages/about"), "", "", false).await;
            assert_eq!(r.status(), StatusCode::OK);
            assert_eq!(r.headers()[header::CONTENT_LANGUAGE], code);
            let h = html(r).await;
            assert!(h.contains(&format!("<h1>{title}</h1>")));
            assert!(
                h.contains(seo),
                "SEO: expected {seo}; title={}",
                h.split("</title>").next().unwrap_or_default()
            );
            assert!(h.contains(&format!(
                "rel=\"canonical\" href=\"https://shop.example.test{prefix}/pages/about\""
            )));
            assert!(h.contains("hreflang=\"en\""));
            assert!(h.contains("hreflang=\"it\""));
            assert!(h.contains("aria-current=\"page\""));
            assert!(!h.contains("<script>alert(1)</script>"));
            if code == "en" {
                assert!(h.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
            }
            let home_path = if prefix.is_empty() { "/" } else { prefix };
            let home = html(request(&app, "GET", home_path, "", "", false).await).await;
            assert!(home.contains(title));
            assert!(!home.contains("private-draft"));
        }
        let map = html(request(&app, "GET", "/sitemap-catalog.xml", "", "", false).await).await;
        assert!(map.contains("/pages/about"));
        assert!(map.contains("/it/pages/about"));
        assert!(!map.contains("private-draft"));
        let mut i = pages.get(&s, id, lang("en")).await.unwrap().input();
        i.published = false;
        pages.save(&s, &s.csrf, id, 0, &i, None).await.unwrap();
        assert!(pages.navigation(lang("en")).await.unwrap().is_empty());
        assert_eq!(
            request(&app, "GET", "/it/pages/about", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_slug_history_redirects_without_loops_or_draft_leaks() {
    isolated(|pool, _| async move {
        let (_, s, pages) = fixture(&pool).await;
        let mut i = input("original");
        let id = pages.save(&s, &s.csrf, 0, 0, &i, None).await.unwrap();
        for (revision, slug) in [(0, "second"), (1, "third"), (2, "original")] {
            i.slug = slug.into();
            pages
                .save(&s, &s.csrf, id, revision, &i, None)
                .await
                .unwrap();
        }
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for prefix in ["", "/it"] {
            for slug in ["second", "third"] {
                let r = request(
                    &app,
                    "GET",
                    &format!("{prefix}/pages/{slug}"),
                    "",
                    "",
                    false,
                )
                .await;
                assert_eq!(r.status(), StatusCode::MOVED_PERMANENTLY);
                assert_eq!(
                    r.headers()[header::LOCATION],
                    format!("{prefix}/pages/original")
                );
            }
        }
        assert!(matches!(
            pages.save(&s, &s.csrf, 0, 0, &input("second"), None).await,
            Err(PageError::Duplicate)
        ));
        i.published = false;
        pages.save(&s, &s.csrf, id, 3, &i, None).await.unwrap();
        assert_eq!(
            request(&app, "GET", "/it/pages/second", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        pages.delete(&s, &s.csrf, id, 4).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM information_page_slugs")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM information_pages_translations")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert!(matches!(
            pages.delete(&s, &s.csrf, id, 4).await,
            Err(PageError::Stale)
        ));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_concurrent_revision_slug_claim_and_delete_are_atomic() {
    isolated(|pool, _| async move {
        let (_, s, pages) = fixture(&pool).await;
        let a = input("unique");
        let (r1, r2) = tokio::join!(
            pages.save(&s, &s.csrf, 0, 0, &a, None),
            pages.save(&s, &s.csrf, 0, 0, &a, None)
        );
        assert_eq!(usize::from(r1.is_ok()) + usize::from(r2.is_ok()), 1);
        assert!(matches!(
            (&r1, &r2),
            (Err(PageError::Duplicate), _) | (_, Err(PageError::Duplicate))
        ));
        let id = r1.or(r2).unwrap();
        let mut b = a.clone();
        b.slug = "updated".into();
        let (r1, r2) = tokio::join!(
            pages.save(&s, &s.csrf, id, 0, &a, None),
            pages.save(&s, &s.csrf, id, 0, &b, None)
        );
        assert_eq!(usize::from(r1.is_ok()) + usize::from(r2.is_ok()), 1);
        assert!(matches!(
            (&r1, &r2),
            (Err(PageError::Stale), _) | (_, Err(PageError::Stale))
        ));
        assert!(matches!(
            pages.delete(&s, &s.csrf, id, 0).await,
            Err(PageError::Stale)
        ));
        assert_eq!(pages.get(&s, id, lang("en")).await.unwrap().revision, 1);
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_permissions_csrf_revocation_and_native_forms() {
    isolated(|pool,_|async move{
        let(identity,s,pages)=fixture(&pool).await;
        let role=identity.create_role(&s,&s.csrf,"Page reader",&[Permission::PagesRead]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"reader@example.test","correct horse battery".into(),Role::parse(&role).unwrap()).await.unwrap();
        let(_,anon)=identity.anonymous().await.unwrap();let raw=identity.login(&anon,&anon.csrf,"reader@example.test","correct horse battery".into()).await.unwrap();
        let reader=identity.session(&raw).await.unwrap().unwrap();let c=format!("talocart_session={raw}");
        let id=pages.save(&s,&s.csrf,0,0,&input("about"),Some(&draft())).await.unwrap();
        assert!(matches!(pages.save(&s,"wrong",id,0,&input("bad"),None).await,Err(PageError::Access(talocart_platform::technical::AccessError::Forbidden))));
        assert!(matches!(pages.save(&reader,&reader.csrf,id,0,&input("bad"),None).await,Err(PageError::Access(talocart_platform::technical::AccessError::Forbidden))));
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        assert_ne!(request(&app,"GET","/admin/pages","","",false).await.status(),StatusCode::OK);
        assert_eq!(request(&app,"GET","/admin/pages",&c,"",false).await.status(),StatusCode::OK);
        assert_eq!(request(&app,"GET","/admin/pages/new",&c,"",false).await.status(),StatusCode::FORBIDDEN);
        let h=html(request(&app,"GET",&format!("/admin/pages/{id}"),&c,"",false).await).await;assert!(h.contains("readonly"));assert!(!h.contains("value=\"save\""));
        let raw=authenticated(&identity).await;let owner=identity.session(&raw).await.unwrap().unwrap();let c=format!("talocart_session={raw}");
        let f=format!("csrf={}&revision=0&slug=invalid%2Fslug&name=Preserved+draft&description=Body&sort_order=0&content_primary_locale=en&content_active_locales=en%2Cit&content_it_name=Bozza+conservata",owner.csrf);
        for partial in [false,true] {
            let r=request(&app,"POST",&format!("/admin/pages/{id}"),&c,&f,partial).await;assert_eq!(r.status(),StatusCode::UNPROCESSABLE_ENTITY);
            let h=html(r).await;assert!(h.contains("Preserved draft"));assert!(h.contains("Bozza conservata"));assert!(h.contains("aria-invalid=\"true\""));
        }
        let f=f.replace("invalid%2Fslug","saved");
        assert_eq!(request(&app,"POST",&format!("/admin/pages/{id}"),&c,&f,false).await.status(),StatusCode::SEE_OTHER);
        let r=request(&app,"POST",&format!("/admin/pages/{id}"),&c,&f,false).await;assert_eq!(r.status(),StatusCode::CONFLICT);assert!(html(r).await.contains("Preserved draft"));
        pages.delete(&s,&s.csrf,id,1).await.unwrap();
        let r=request(&app,"POST",&format!("/admin/pages/{id}"),&c,&f,false).await;
        assert_eq!(r.status(),StatusCode::CONFLICT,"a deleted page must preserve the submitted draft");
        assert!(html(r).await.contains("Preserved draft"));
        identity.update_role(&s,&s.csrf,&role,"Page reader",&[]).await.unwrap();
        assert!(matches!(pages.get(&reader,id,lang("en")).await,Err(PageError::Access(talocart_platform::technical::AccessError::Forbidden))));
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_language_snapshot_fallback_and_primary_change_preserve_translations() {
    isolated(|pool, _| async move {
        let (_, s, pages) = fixture(&pool).await;
        let mut d = draft();
        d.rows[0].description.clear();
        let id = pages
            .save(&s, &s.csrf, 0, 0, &input("about"), Some(&d))
            .await
            .unwrap();
        assert_eq!(
            pages.public("about", lang("it")).await.unwrap().description,
            input("about").description
        );
        let rows = CONTENT
            .load(&pool, &Identity::new(pool.clone()), &s, id)
            .await
            .unwrap();
        assert!(
            rows.iter()
                .find(|r| r.locale == "it")
                .unwrap()
                .description
                .is_empty()
        );
        Storefront::new(pool.clone())
            .save_languages(&s, &s.csrf, "it", &["en".into(), "it".into()])
            .await
            .unwrap();
        assert!(matches!(
            pages
                .save(&s, &s.csrf, id, 0, &input("about"), Some(&d))
                .await,
            Err(PageError::Content(
                talocart_platform::content::ContentError::LanguageChanged
            ))
        ));
        let current = pages
            .public("about", Language::new(Locale::ITALIAN, Locale::ITALIAN))
            .await
            .unwrap();
        assert_eq!(current.name, "Il nostro negozio");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        assert_eq!(
            request(&app, "GET", "/pages/about", "", "", false)
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "GET", "/en/pages/about", "", "", false)
                .await
                .status(),
            StatusCode::OK
        );
        Storefront::new(pool.clone())
            .save_languages(&s, &s.csrf, "it", &["it".into()])
            .await
            .unwrap();
        let h = html(request(&app, "GET", "/sitemap-catalog.xml", "", "", false).await).await;
        assert!(!h.contains("/en/pages/"));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_pagination_validation_menu_limits_and_order() {
    isolated(|pool,_|async move{
        let(_,s,pages)=fixture(&pool).await;
        for n in 0..23 { let mut i=input(&format!("page-{n}"));i.published=false;i.sort_order=n.to_string();pages.save(&s,&s.csrf,0,0,&i,None).await.unwrap(); }
        assert_eq!(pages.list(&s,1,lang("en")).await.unwrap().entries.len(),20);
        assert_eq!(pages.list(&s,2,lang("en")).await.unwrap().entries.len(),3);
        assert!(pages.list(&s,0,lang("en")).await.is_err());
        for n in 0..20 {let mut i=input(&format!("link-{n}"));i.sort_order=(20-n).to_string();pages.save(&s,&s.csrf,0,0,&i,None).await.unwrap();}
        assert!(matches!(pages.save(&s,&s.csrf,0,0,&input("too-many"),None).await,Err(PageError::Limit)));
        assert_eq!(pages.navigation(lang("en")).await.unwrap()[0].slug,"link-19");
        let mut i=input("empty");i.description.clear();assert!(matches!(pages.save(&s,&s.csrf,0,0,&i,None).await,Err(PageError::Invalid(_))));
        let mut d=draft();d.rows[0].name="bad\0name".into();assert!(matches!(pages.save(&s,&s.csrf,0,0,&input("bad-text"),Some(&d)).await,Err(PageError::Content(_))));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM information_pages WHERE slug IN ('too-many','empty','bad-text')").fetch_one(&pool).await.unwrap(),0);
        sqlx::query("INSERT INTO information_pages(slug,name) SELECT 'capacity-'||n,'Capacity' FROM generate_series(1,157) n").execute(&pool).await.unwrap();
        let mut i=input("over-capacity"); i.published=false;
        assert!(matches!(pages.save(&s,&s.csrf,0,0,&i,None).await,Err(PageError::Limit)));
        assert_eq!(pages.list(&s,999,lang("en")).await.unwrap().page,10);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL, npm ci and Playwright Chromium"]
async fn pages_browser_bilingual_desktop_mobile_and_no_js() {
    for chosen_theme in ["default", "essential"] {
        isolated(move |pool, _| async move {
            let (identity, _, _) = fixture(&pool).await;
            let admin = authenticated(&identity).await;
            sqlx::query("UPDATE storefront_settings SET theme_id=$1 WHERE id=1")
                .bind(chosen_theme)
                .execute(&pool)
                .await
                .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let state = test_app_config(
                pool.clone(),
                "Shop pages".into(),
                talocart_web::AppStateConfig {
                    public_origin: origin.clone(),
                    ..Default::default()
                },
            )
            .await;
            let task = tokio::spawn(async move {
                axum::serve(
                    listener,
                    router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .await
                .unwrap()
            });
            let mut child = tokio::process::Command::new("node")
                .arg(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../scripts/browser-pages.cjs"),
                )
                .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            use tokio::io::AsyncWriteExt;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(
                    serde_json::json!({"origin":origin,"admin":admin})
                        .to_string()
                        .as_bytes(),
                )
                .await
                .unwrap();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(300),
                child.wait_with_output(),
            )
            .await
            .expect("Browser timeout")
            .unwrap();
            task.abort();
            let _ = task.await;
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            println!("{}", String::from_utf8_lossy(&result.stdout));
        })
        .await;
    }
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_delete_conflicts_return_current_list_without_empty_editor() {
    isolated(|pool, _| async move {
        let (identity, s, pages) = fixture(&pool).await;
        let id = pages
            .save(&s, &s.csrf, 0, 0, &input("delete-conflict"), Some(&draft()))
            .await
            .unwrap();
        pages
            .save(
                &s,
                &s.csrf,
                id,
                0,
                &input("delete-conflict"),
                Some(&draft()),
            )
            .await
            .unwrap();
        let raw = authenticated(&identity).await;
        let owner = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let form = format!("csrf={}&revision=0&action=delete", owner.csrf);
        assert_eq!(
            request(
                &app,
                "POST",
                &format!("/admin/pages/{id}"),
                &cookie,
                "csrf=invalid&revision=1&action=delete",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        for partial in [false, true] {
            let r = request(
                &app,
                "POST",
                &format!("/admin/pages/{id}"),
                &cookie,
                &form,
                partial,
            )
            .await;
            assert_eq!(r.status(), StatusCode::CONFLICT);
            let body = html(r).await;
            assert!(
                !body.contains("id=\"page-name\""),
                "delete conflict must not fabricate an empty editor"
            );
            assert!(body.contains("About our shop"));
            assert!(body.contains(&format!("href=\"/admin/pages/{id}\"")));
        }
        assert_eq!(pages.get(&s, id, lang("en")).await.unwrap().revision, 1);
        pages.delete(&s, &s.csrf, id, 1).await.unwrap();
        let r = request(
            &app,
            "POST",
            &format!("/admin/pages/{id}"),
            &cookie,
            &form,
            false,
        )
        .await;
        assert_eq!(r.status(), StatusCode::CONFLICT);
        let body = html(r).await;
        assert!(!body.contains("id=\"page-name\""));
        assert!(!body.contains(&format!("href=\"/admin/pages/{id}\"")));
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_list_projection_preserves_localized_summaries() {
    isolated(|pool, _| async move {
        let (_, s, pages) = fixture(&pool).await;
        let mut values = input("summary");
        values.description = "x".repeat(20_000);
        values.sort_order = "-7".into();
        values.footer_menu = false;
        let id = pages
            .save(&s, &s.csrf, 0, 0, &values, Some(&draft()))
            .await
            .unwrap();
        for (locale, title) in [("en", "About our shop"), ("it", "Il nostro negozio")] {
            let result = pages.list(&s, 1, lang(locale)).await.unwrap();
            assert_eq!(result.total, 1);
            let row = &result.entries[0];
            assert_eq!(row.id, id);
            assert_eq!(row.slug, "summary");
            assert_eq!(row.name, title);
            assert_eq!(row.sort_order, -7);
            assert!(row.published && row.header_menu && !row.footer_menu);
        }
        assert_eq!(
            pages
                .get(&s, id, lang("en"))
                .await
                .unwrap()
                .description
                .len(),
            20_000
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_rich_html_sanitized_persisted_and_empty_publication_rejected() {
    isolated(|pool, _| async move {
        let (_, s, pages) = fixture(&pool).await;
        let mut values = input("rich-page");
        values.description = talocart_platform::rich_text::submitted("<h2>Delivery</h2><p>Safe <strong>text</strong> &amp; details</p><script>evil()</script><a href='javascript:evil()'>link</a>", "html");
        let mut translations = draft();
        translations.rows[0].description = talocart_platform::rich_text::submitted("<p>Consegna <em>rapida</em></p>", "html");
        let id = pages.save(&s, &s.csrf, 0, 0, &values, Some(&translations)).await.unwrap();
        let stored = pages.get(&s, id, lang("en")).await.unwrap();
        assert!(!stored.description.contains("evil"));
        assert!(stored.description.contains("<strong>text</strong>"));
        let app = router(test_app(pool.clone(), "Rich text".into()).await);
        for (prefix, expected) in [("", "<strong>text</strong>"), ("/it", "<em>rapida</em>")] {
            let body = html(request(&app, "GET", &format!("{prefix}/pages/rich-page"), "", "", false).await).await;
            assert!(body.contains(expected));
            assert!(!body.contains(talocart_platform::rich_text::PREFIX));
            assert!(!body.contains("javascript:"));
        }
        values.description = talocart_platform::rich_text::submitted("<p><br></p><script>x</script>", "html");
        assert!(matches!(pages.save(&s, &s.csrf, id, 0, &values, None).await, Err(PageError::Invalid(fields)) if fields.contains(&"description")));
        assert_eq!(pages.get(&s, id, lang("en")).await.unwrap().description, stored.description);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn pages_rich_form_accepts_encoded_long_text_and_bounds_requests() {
    isolated(|pool, _| async move {
        let (identity, session, pages) = fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let owner = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Rich pages".into()).await);
        let description = format!("<p>{}</p>", "è".repeat(19_900));
        let encoded = format!("%3Cp%3E{}%3C%2Fp%3E", "%C3%A8".repeat(19_900));
        let form = format!("csrf={}&revision=0&slug=long-rich&name=Long&sort_order=0&description_format=html&description={encoded}", owner.csrf);
        assert!(form.len() > 16 * 1024);
        assert_eq!(request(&app, "POST", "/admin/pages/new", &cookie, &form, false).await.status(), StatusCode::SEE_OTHER);
        let id: i64 = sqlx::query_scalar("SELECT id FROM information_pages WHERE slug='long-rich'").fetch_one(&pool).await.unwrap();
        let path = format!("/admin/pages/{id}");
        assert_eq!(request(&app, "POST", &path, &cookie, &form, false).await.status(), StatusCode::SEE_OTHER);
        assert_eq!(talocart_platform::rich_text::source(&pages.get(&session, id, lang("en")).await.unwrap().description), description);
        let excessive = format!("padding={}", "x".repeat(512 * 1024));
        for path in ["/admin/pages/new", &path] {
            assert_eq!(request(&app, "POST", path, &cookie, &excessive, false).await.status(), StatusCode::PAYLOAD_TOO_LARGE);
        }
    }).await;
}
