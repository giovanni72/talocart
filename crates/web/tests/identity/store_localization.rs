use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn store_localization_routes_filters_seo_and_admin_are_isolated() {
    isolated(|pool, _| async move {
        let (identity, session, products, _, product, _, _) = variant_fixture(&pool).await;
        let entry = products.entry(&session, product).await.unwrap();
        sqlx::query("UPDATE catalog_products_translations SET name='Disponibile <script>',description='Descrizione originale',seo_title='SEO originale' WHERE entity_id=$1 AND locale='it'").bind(product).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock) SELECT 'Synthetic '||n,'LANG-'||n,'language-'||n,'published',$1,$2,$3,1234,10 FROM generate_series(1,30) n").bind(entry.category_id).bind(entry.brand_id).bind(entry.tax_class_id).execute(&pool).await.unwrap();
        let raw = authenticated(&identity).await;
        let admin = identity.session(&raw).await.unwrap().unwrap();
        identity.update_locale(&admin, &admin.csrf, "en").await.unwrap();
        let cookie = format!("talocart_session={raw}; talocart_admin_language=en");
        let app = router(test_app_config(pool.clone(), "Shop".into(), talocart_web::AppStateConfig { public_origin: "https://shop.example.test".into(), ..Default::default() }).await);
        let before = demo_existing_snapshot(&pool).await;
        for route in ["", "/catalog", "/catalog/categoria-prodotti", "/brands", "/brands/marchio-prodotti", "/search/Synthetic", "/products/model", "/status"] {
            for (prefix, code) in [("", "it"), ("/en", "en")] {
                let path = if route.is_empty() && prefix.is_empty() { "/".into() } else { format!("{prefix}{route}") };
                let response = request(&app, "GET", &path, &cookie, "", false).await;
                assert_eq!(response.status(), StatusCode::OK, "{path}");
                assert_eq!(response.headers()[header::CONTENT_LANGUAGE], code);
                if route == "/products/model" {
                    let cookies:Vec<_>=response.headers().get_all(header::SET_COOKIE).iter().collect();
                    assert_eq!(cookies.len(),1);
                    assert!(cookies[0].to_str().unwrap().starts_with("talocart_cart="));
                } else {
                    assert!(!response.headers().contains_key(header::SET_COOKIE));
                }
                let html = html(response).await;
                assert!(html.contains(&format!("<html lang=\"{code}\">")), "{path}");
                assert!(html.contains("hreflang=\"it\"") && html.contains("hreflang=\"en\""));
                assert!(html.contains(&format!("action=\"{prefix}/search\"")), "{path}");
                if route != "/status" { assert!(html.contains(&format!("rel=\"canonical\" href=\"https://shop.example.test{path}\"")), "{path}"); }
                if route == "/products/model" { assert!(html.contains("Disponibile &#60;script&#62;") && html.contains("Descrizione originale") && html.contains("SEO originale")); }
            }
        }
        for route in ["/en/catalog?brand=marchio-prodotti&available=true&page=2&per_page=21", "/en/catalog/categoria-prodotti?brand=marchio-prodotti&available=true&page=2&per_page=21", "/en/brands/marchio-prodotti?category=categoria-prodotti&available=true&page=2&per_page=21", "/en/search/Synthetic?category=categoria-prodotti&brand=marchio-prodotti&available=true&page=2&per_page=21"] {
            let page = html(request(&app, "GET", route, "", "", false).await).await;
            assert!(page.contains("31 products") || page.contains("30 products"));
            assert!(page.contains("Products per page") && page.contains("Apply filters"));
            assert!(page.contains(&format!("href=\"{}\" lang=\"it\"", route[3..].replace('&', "&#38;"))), "{route}: equivalent Italian state");
            assert!(page.contains("href=\"/en/products/language-") && page.contains("rel=\"prev\" href=\"/en/"));
            assert!(page.contains("per_page=21") && page.contains("available=true"));
        }
        let page = html(request(&app,"GET","/en/catalog?brand=missing","","",false).await).await;
        assert!(page.contains("0 products") && page.contains("No products found."));
        let page = html(request(&app,"GET","/en/search/%3Cscript%3E%26","","",false).await).await;
        assert!(page.contains("Results for") && !page.contains("<script>&") && page.contains("&#60;script&#62;&#38;"));
        for (path, expected) in [("/en/search?q=Synthetic&brand=marchio-prodotti&available=true&per_page=42", "/en/search/Synthetic?brand=marchio-prodotti&available=true&per_page=42"),("/en/search?q=&category=categoria-prodotti", "/en/catalog/categoria-prodotti"),("/en/search?q=", "/en/catalog"),("/search?q=Synthetic", "/search/Synthetic")] {
            let response = request(&app,"GET",path,"","",false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(response.headers()[header::LOCATION],expected);
        }
        for path in ["/en/catalog?category=categoria-prodotti", "/en/search?q=bad&per_page=999", "/en/products/model?value_1=bad"] { assert_eq!(request(&app,"GET",path,"","",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY); }
        for path in ["/en/products/missing", "/en/catalog/missing", "/en/admin", "/unknown/catalog", "/en/unknown"] { assert_eq!(request(&app,"GET",path,"","",false).await.status(),StatusCode::NOT_FOUND); }
        let missing=html(request(&app,"GET","/en/unknown","","",false).await).await;
        assert!(missing.contains("<html lang=\"en\">") && missing.contains("href=\"/en\""));
        let redirected=request(&app,"GET","/en/","","",false).await;
        assert_eq!(redirected.status(),StatusCode::MOVED_PERMANENTLY);
        assert_eq!(redirected.headers()[header::LOCATION],"/en");
        let fragment=request(&app,"GET","/en/status","","",true).await;
        assert_eq!(fragment.headers()[header::CONTENT_LANGUAGE],"en");
        let fragment=html(fragment).await;assert!(fragment.contains("The foundations are ready") && !fragment.contains("<html"));
        let admin_page=request(&app,"GET","/admin/account",&cookie,"",false).await;
        assert_eq!(admin_page.headers()[header::CONTENT_LANGUAGE],"en");
        assert_eq!(identity.account(&admin).await.unwrap().locale,"en");
        assert_eq!(demo_existing_snapshot(&pool).await,before);
        for path in ["/sitemap-catalog.xml","/sitemap-products.xml?page=1"] { let sitemap=html(request(&app,"GET",path,"","",false).await).await;assert!(sitemap.contains("https://shop.example.test/en") && sitemap.contains("https://shop.example.test/")); }
        let english=app.clone().oneshot(Request::builder().uri("/en/catalog").header("accept-language","it").header("host","evil.example").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(english.headers()[header::CONTENT_LANGUAGE],"en");assert!(!html(english).await.contains("evil.example"));
        let (it,en)=tokio::join!(request(&app,"GET","/catalog","","",false),request(&app,"GET","/en/catalog","","",false));
        assert!(html(it).await.contains("31 prodotti"));assert!(html(en).await.contains("31 products"));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn store_localization_variants_gallery_htmx_and_money_preserve_contracts() {
    isolated(|pool, _| async move {
        let (_, session, products, variants, product, options, values) =
            variant_fixture(&pool).await;
        variants
            .configure(&session, &session.csrf, product, &options)
            .await
            .unwrap();
        variants
            .save(
                &session,
                &session.csrf,
                product,
                None,
                &variant_values("LANG-RED-S", &[values[0], values[2]], "12.34", "10"),
            )
            .await
            .unwrap();
        let png = include_bytes!("../fixtures/logo.png").to_vec();
        products
            .image(
                &session,
                &session.csrf,
                product,
                Some(png.clone()),
                "main.png",
            )
            .await
            .unwrap();
        products
            .add_gallery_image(&session, &session.csrf, product, png, "extra.png")
            .await
            .unwrap();
        let gallery = products.gallery(&session, product).await.unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let incomplete =
            html(request(&app, "GET", "/en/products/model", "", "", false).await).await;
        assert!(incomplete.contains("Minimum purchase quantity: 1 piece."));
        assert!(
            incomplete.contains("Choose a value for each option.")
                && incomplete.contains("Choose Colore")
        );
        assert!(incomplete.contains("action=\"/en/products/model\" hx-get=\"/en/products/model\""));
        let path = format!(
            "/en/products/model?value_1={}&value_2={}&photo={}",
            values[0], values[2], gallery[0].id
        );
        for partial in [false, true] {
            let response = request(&app, "GET", &path, "", "", partial).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], "en");
            let page = html(response).await;
            assert!(
                page.contains("12.34 €")
                    && page.contains("VAT included")
                    && page.contains("LANG-RED-S")
                    && page.contains("Additional image 1")
            );
            assert!(
                page.contains(&gallery[0].image_url("detail"))
                    && page.contains("href=\"/en/products/model?value_1=")
            );
            assert!(page.contains(&format!(
                "href=\"{}\" lang=\"it\"",
                path[3..].replace('&', "&#38;")
            )));
            assert!(
                page.contains("rel=\"canonical\" href=\"http://127.0.0.1:3000/en/products/model\"")
            );
        }
        let italian = html(request(&app, "GET", &path[3..], "", "", false).await).await;
        assert!(italian.contains("12,34 €") && italian.contains("IVA inclusa"));
        assert_eq!(
            request(
                &app,
                "GET",
                "/en/products/model?photo=invalid",
                "",
                "",
                true
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(&app, "GET", "/en/catalog?available=true", "", "", false)
                .await
                .status(),
            StatusCode::OK
        );
        sqlx::query("UPDATE catalog_products SET status='draft' WHERE id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            request(&app, "GET", "/en/products/model", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(&app, "GET", "/products/model", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    })
    .await;
}
