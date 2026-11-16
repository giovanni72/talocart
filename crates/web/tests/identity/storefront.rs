use super::*;

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn indexing_is_explicit_localized_and_keeps_private_errors_filters_and_fragments_excluded() {
    use talocart_platform::storefront::Storefront;
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        identity.update_branding_named(&session, &session.csrf,
            talocart_platform::identity::BrandingKind::Logo,
            Some(include_bytes!("../fixtures/avatar.png").to_vec()), "indexing-logo.png")
            .await.unwrap();
        let logo = identity.branding_metadata(talocart_platform::identity::BrandingKind::Logo)
            .await.unwrap().unwrap();
        let image_url = format!("/media/public/{}/{}/original", logo.id, logo.key);
        let (category, brand, class) = product_references(&pool, &session).await;
        let products = talocart_commerce::products::Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let mut values = product_values("Indexing", "indexing-product");
        values.status = "published".into();
        values.category_id = category.to_string();
        values.brand_id = brand.to_string();
        values.tax_class_id = class.to_string();
        products.save(&session, &session.csrf, None, &values).await.unwrap();
        sqlx::query("INSERT INTO information_pages(name,slug,description,published) VALUES('Info','indexing-info','Information',true)")
            .execute(&pool).await.unwrap();
        let storefront = Storefront::new(pool.clone());
        assert!(!storefront.settings().await.unwrap().indexing_enabled);
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for enabled in [false, true, false] {
            let mut settings = storefront.settings().await.unwrap();
            settings.indexing_enabled = enabled;
            storefront.save(&session, &session.csrf, settings).await.unwrap();
            assert_eq!(Storefront::new(pool.clone()).settings().await.unwrap().indexing_enabled, enabled);
            for prefix in ["", "/en"] {
                for route in ["/", "/catalog", "/catalog/categoria-prodotti", "/brands", "/brands/marchio-prodotti", "/products/indexing-product", "/pages/indexing-info"] {
                    let path = if route == "/" { if prefix.is_empty() { "/".into() } else { prefix.to_string() } } else { format!("{prefix}{route}") };
                    for method in ["GET", "HEAD"] {
                        let response = request(&app, method, &path, "", "", false).await;
                        assert_eq!(response.status(), StatusCode::OK, "{method} {path}");
                        assert_eq!(response.headers()["x-robots-tag"], if enabled { "index, follow" } else { "noindex, nofollow" }, "{method} {path}");
                    }
                }
                for route in ["/account", "/cart", "/checkout", "/search/test", "/catalog?available=true", "/catalog?page=2", "/products/indexing-product?photo=1", "/products/missing", "/unknown"] {
                    let response = request(&app, "GET", &format!("{prefix}{route}"), "", "", false).await;
                    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow", "{prefix}{route}");
                }
                let fragment = request(&app, "GET", &format!("{prefix}/catalog"), "", "", true).await;
                assert_eq!(fragment.headers()["x-robots-tag"], "noindex, nofollow");
            }
            for route in ["/admin/login", "/setup", "/health/live", "/status", "/media/image/999999/card", "/assets/ui.css", "/favicon.svg"] {
                let response = request(&app, "GET", route, "", "", false).await;
                assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow", "{route}");
            }
            for route in ["/sitemap.xml", "/sitemap-catalog.xml", "/sitemap-products.xml?page=1"] {
                let response = request(&app, "GET", route, "", "", false).await;
                assert_eq!(response.status(), StatusCode::OK, "{route}");
                assert_eq!(response.headers()["x-robots-tag"], if enabled { "index, follow" } else { "noindex, nofollow" }, "{route}");
            }
            let image = image_request(&app, &image_url, "", "GET", "").await;
            assert_eq!(image.status(), StatusCode::OK);
            assert_eq!(image.headers()["x-robots-tag"], if enabled { "index, follow" } else { "noindex, nofollow" });
            assert_eq!(image.headers()[header::CACHE_CONTROL], "public, max-age=0, must-revalidate");
            let etag = image.headers()[header::ETAG].to_str().unwrap();
            for method in ["GET", "HEAD"] {
                let cached = image_request(&app, &image_url, "", method, etag).await;
                assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
                assert_eq!(cached.headers()["x-robots-tag"], if enabled { "index, follow" } else { "noindex, nofollow" });
            }
            let robots = html(request(&app, "GET", "/robots.txt", "", "", false).await).await;
            assert!(robots.contains("Disallow:\n"));
            assert!(!robots.contains("Disallow: /"));
            assert_eq!(robots.contains("Sitemap:"), enabled);
            storefront.set_maintenance(&session, &session.csrf, true).await.unwrap();
            let closed = request(&app, "GET", "/", "", "", false).await;
            assert_eq!(closed.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(closed.headers()["x-robots-tag"], "noindex, nofollow");
            storefront.set_maintenance(&session, &session.csrf, false).await.unwrap();
            assert_eq!(storefront.settings().await.unwrap().indexing_enabled, enabled);
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn admin_inherits_saved_theme_on_every_page_and_preserves_it_on_validation_errors() {
    use talocart_platform::storefront::{Settings, Storefront};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let service = Storefront::new(pool.clone());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for palette in ["forest", "ocean", "clay"] {
            for typography in ["editorial", "modern"] {
                for density in ["comfortable", "compact"] {
                    let settings = Settings { palette: palette.into(), typography: typography.into(), density: density.into(), ..Settings::default() };
                    service.save(&session, &session.csrf, settings).await.unwrap();
                    for route in ["/admin", "/admin/status", "/admin/components", "/admin/catalog/products", "/admin/catalog/products/new", "/admin/catalog/categories", "/admin/catalog/categories/new", "/admin/catalog/brands", "/admin/catalog/brands/new", "/admin/catalog/options", "/admin/catalog/options/new", "/admin/media", "/admin/settings", "/admin/settings/appearance", "/admin/settings/storefront", "/admin/settings/taxes", "/admin/settings/taxes/rates", "/admin/settings/taxes/classes", "/admin/users", "/admin/users/new", "/admin/roles", "/admin/roles/new", "/admin/account", "/admin/access", "/admin/login", "/admin/reset", "/"] {
                        let response = request(&app, "GET", route, &cookie, "", false).await;
                        assert_eq!(response.status(), StatusCode::OK, "{route}");
                        let page = html(response).await;
                        assert!(page.contains(&format!("data-palette=\"{palette}\" data-typography=\"{typography}\" data-density=\"{density}\"")), "{route}: saved theme");
                    }
                }
            }
        }
        let form = format!("csrf={}&palette=forest&typography=editorial&density=comfortable&heading=&introduction=&seo_title=&seo_description=&public_origin=invalid", session.csrf);
        let response = request(&app, "POST", "/admin/settings/storefront", &cookie, &form, false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let page = html(response).await;
        assert!(page.contains("data-palette=\"clay\" data-typography=\"modern\" data-density=\"compact\""));
        assert!(page.contains("value=\"forest\" selected"), "Submitted choices remain in the invalid form");
        let fragment = request(&app, "GET", "/admin/status", &cookie, "", true).await;
        assert_eq!(fragment.status(), StatusCode::OK);
        assert!(!html(fragment).await.contains("<body"), "HTMX keeps the existing document theme");
        assert_eq!(request(&app, "GET", "/admin", "", "", false).await.status(), StatusCode::SEE_OTHER);
        let role = identity.create_role(&session, &session.csrf, "Only catalog", &[talocart_platform::identity::Permission::ProductsRead]).await.unwrap();
        identity.create_administrator(&session, &session.csrf, "catalog-theme@example.test", "correct horse reader".into(), talocart_platform::identity::Role::Custom(role)).await.unwrap();
        let (_, anonymous) = identity.anonymous().await.unwrap();
        let raw = identity.login(&anonymous, &anonymous.csrf, "catalog-theme@example.test", "correct horse reader".into()).await.unwrap();
        let cookie = format!("talocart_session={raw}");
        let response = request(&app, "GET", "/admin/catalog/products", &cookie, "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(html(response).await.contains("data-palette=\"clay\""));
        assert_eq!(request(&app, "GET", "/admin/settings/storefront", &cookie, "", false).await.status(), StatusCode::FORBIDDEN);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn storefront_persistence_http_validation_csrf_permissions_and_revocation() {
    use talocart_platform::storefront::{Settings, Storefront, StorefrontError};
    isolated(|pool, _| async move {
        let identity=installed(&pool).await;
        let raw=authenticated(&identity).await;
        let session=identity.session(&raw).await.unwrap().unwrap();
        let cookie=format!("talocart_session={raw}");
        let service=Storefront::new(pool.clone());
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        assert_eq!(request(&app,"GET","/admin/settings/storefront","","",false).await.status(),StatusCode::SEE_OTHER);
        let form=|csrf:&str,palette:&str| format!("csrf={csrf}&palette={palette}&typography=modern&density=compact&show_hero=true&show_featured=true&indexing_enabled=true&heading=Titolo+%3Cscript%3E&introduction=Presentazione&seo_title=Negozio+%26+SEO&seo_description=Descrizione+SEO&public_origin=https%3A%2F%2Fshop.example.test%2F");
        let r=request(&app,"POST","/admin/settings/storefront",&cookie,&form("wrong","ocean"),false).await;
        assert_eq!(r.status(),StatusCode::FORBIDDEN);
        let r=request(&app,"POST","/admin/settings/storefront",&cookie,&form(&session.csrf,"injected"),false).await;
        assert_eq!(r.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert!(html(r).await.contains("aria-describedby=\"palette-error\""));
        let unchanged = service.settings().await.unwrap();
        assert!(!unchanged.indexing_enabled);
        assert_eq!((unchanged.palette.as_str(), unchanged.typography.as_str(), unchanged.density.as_str()), ("ocean", "modern", "compact"));
        let r=request(&app,"POST","/admin/settings/storefront",&cookie,&form(&session.csrf,"ocean"),false).await;
        assert_eq!(r.status(),StatusCode::SEE_OTHER);
        let s=Storefront::new(pool.clone()).settings().await.unwrap();
        assert_eq!(s.palette,"ocean");assert_eq!(s.public_origin,"https://shop.example.test");assert!(s.show_featured);assert!(s.indexing_enabled);
        let home=html(request(&app,"GET","/","","",false).await).await;
        assert!(home.contains("data-palette=\"ocean\"")); assert!(home.contains("Titolo &#60;script&#62;") || home.contains("Titolo &lt;script&gt;"));
        assert!(home.contains("https://shop.example.test/"));assert!(!home.contains("id=\"check-status\""));
        let hidden=Settings { show_hero:false,show_featured:false,..s.clone() };
        service.save(&session,&session.csrf,hidden).await.unwrap();
        let home=html(request(&app,"GET","/","","",false).await).await;
        assert!(!home.contains("class=\"hero\""));assert!(!home.contains("home-featured"));assert!(home.contains("<h1"));
        let role=identity.create_role(&session,&session.csrf,"Theme reader",&[talocart_platform::identity::Permission::SettingsRead]).await.unwrap();
        identity.create_administrator(&session,&session.csrf,"theme-reader@example.test","correct horse reader".into(),talocart_platform::identity::Role::Custom(role)).await.unwrap();
        let (_,anonymous)=identity.anonymous().await.unwrap();
        let reader_raw=identity.login(&anonymous,&anonymous.csrf,"theme-reader@example.test","correct horse reader".into()).await.unwrap();
        let reader=identity.session(&reader_raw).await.unwrap().unwrap();
        let reader_cookie=format!("talocart_session={reader_raw}");
        let read_page=html(request(&app,"GET","/admin/settings/storefront",&reader_cookie,"",false).await).await;
        assert!(read_page.contains("<fieldset disabled>"));assert!(!read_page.contains("Salva tema e SEO"));
        assert_eq!(request(&app,"POST","/admin/settings/storefront",&reader_cookie,&form(&reader.csrf,"clay"),false).await.status(),StatusCode::FORBIDDEN);
        assert!(matches!(service.save(&reader,&reader.csrf,s.clone()).await,Err(StorefrontError::Access(talocart_platform::technical::AccessError::Forbidden))));
        sqlx::query("DELETE FROM admin_sessions WHERE token_hash=$1").bind(digest(&raw)).execute(&pool).await.unwrap();
        assert!(matches!(service.save(&session,&session.csrf,s).await,Err(StorefrontError::Access(talocart_platform::technical::AccessError::Forbidden))));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_metadata_filters_sitemap_pagination_and_live_publication() {
    use talocart_commerce::products::{Field, ProductError, Products};
    isolated(|pool, _| async move {
        let identity=installed(&pool).await;let session=taxonomy_session(&identity).await;
        let (category,brand,class)=product_references(&pool,&session).await;
        let products=Products::new(pool.clone(),identity.clone(),test_media(&pool).await);
        let mut v=product_values("SEO","seo-product");v.status="published".into();v.category_id=category.to_string();v.brand_id=brand.to_string();v.tax_class_id=class.to_string();
        v.seo_title="Titolo <SEO> & negozio".into();v.seo_description="Descrizione <script> sicura".into();
        let id=products.save(&session,&session.csrf,None,&v).await.unwrap();
        let mut invalid=v.clone();invalid.seo_title="x".repeat(121);
        assert!(matches!(products.save(&session,&session.csrf,Some(id),&invalid).await,Err(ProductError::Invalid(e)) if e.contains(&Field::SeoTitle)));
        invalid=v.clone();invalid.seo_description="bad\ntext".into();
        assert!(matches!(products.save(&session,&session.csrf,Some(id),&invalid).await,Err(ProductError::Invalid(e)) if e.contains(&Field::SeoDescription)));
        assert_eq!(products.public_filtered("","categoria-prodotti","marchio-prodotti",true,1).await.unwrap().len(),1);
        assert!(products.public_filtered("%","","",false,1).await.unwrap().is_empty());
        assert!(products.public_filtered("","","missing",false,1).await.unwrap().is_empty());
        assert!(products.public_filtered("","","bad/brand",false,1).await.is_err());
        let app=router(test_app_config(pool.clone(),"Shop".into(), talocart_web::AppStateConfig { public_origin: "http://127.0.0.1:3000".into(), ..Default::default() }).await);
        let r=request(&app,"GET","/products/seo-product?value_1=&photo=","","",false).await;
        assert_eq!(r.status(),StatusCode::OK);assert_eq!(r.headers()["x-robots-tag"],"noindex, nofollow");
        let page=html(r).await;assert!(!page.contains("<SEO>"));assert!(page.contains("rel=\"canonical\" href=\"http://127.0.0.1:3000/products/seo-product\""));assert!(page.contains("name=\"description\""));
        let bad_host=app.clone().oneshot(Request::builder().uri("/products/seo-product").header("host","evil.example").body(Body::empty()).unwrap()).await.unwrap();assert!(!html(bad_host).await.contains("evil.example"));
        let robots=html(request(&app,"GET","/robots.txt","","",false).await).await;assert!(robots.contains("Disallow:\n"));assert!(!robots.contains("Sitemap:"));
        let index=request(&app,"GET","/sitemap.xml","","",false).await;assert_eq!(index.headers()[header::CONTENT_TYPE],"application/xml; charset=utf-8");assert!(html(index).await.contains("/sitemap-products.xml?page=1"));
        assert!(html(request(&app,"GET","/sitemap-products.xml?page=1","","",false).await).await.contains("/products/seo-product"));
        v.stock="0".into();products.save(&session,&session.csrf,Some(id),&v).await.unwrap();assert!(products.public_filtered("","","",true,1).await.unwrap().is_empty());assert_eq!(products.public_list("","",1).await.unwrap().len(),1);
        sqlx::query("UPDATE catalog_brands SET active=false WHERE id=$1").bind(brand).execute(&pool).await.unwrap();assert!(products.public_list("","",1).await.unwrap().is_empty());assert!(products.public_brands("").await.unwrap().is_empty());
        sqlx::query("UPDATE catalog_brands SET active=true WHERE id=$1").bind(brand).execute(&pool).await.unwrap();
        v=products.entry(&session,id).await.unwrap().values();
        v.status="draft".into();products.save(&session,&session.csrf,Some(id),&v).await.unwrap();assert!(products.sitemap_products(1).await.unwrap().is_empty());
        assert_eq!(request(&app,"GET","/products/seo-product","","",false).await.status(),StatusCode::NOT_FOUND);
        // More than one sitemap shard and one catalog page, with no missing tail.
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,tax_class_id,price_cents,stock) SELECT 'Sitemap '||n,'SITE-'||n,'sitemap-'||n,'published',$1,$2,100,1 FROM generate_series(1,1001) n").bind(category).bind(class).execute(&pool).await.unwrap();
        assert_eq!(products.sitemap_count().await.unwrap(),1001);assert_eq!(products.sitemap_products(1).await.unwrap().len(),1000);assert_eq!(products.sitemap_products(2).await.unwrap(),vec!["sitemap-1001"]);
        assert!(html(request(&app,"GET","/sitemap.xml","","",false).await).await.contains("/sitemap-products.xml?page=2"));
        assert_eq!(request(&app,"GET","/sitemap-products.xml?page=3","","",false).await.status(),StatusCode::NOT_FOUND);
        assert_eq!(request(&app,"GET","/sitemap-products.xml?page=0","","",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        let filtered=html(request(&app,"GET","/catalog/categoria-prodotti?available=true","","",false).await).await;
        assert!(filtered.contains("available=true&amp;page=2") || filtered.contains("available=true&#38;page=2"));
        assert_eq!(products.public_filtered("","","",true,2).await.unwrap().len(),22);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn homepage_selects_only_eight_visible_products_in_catalog_order() {
    isolated(|pool, _| async move {
        let (_, session, products, variants, product, options, values) = variant_fixture(&pool).await;
        let entry = products.entry(&session, product).await.unwrap();
        variants.configure(&session, &session.csrf, product, &options).await.unwrap();
        variants.save(&session, &session.csrf, product, None, &variant_values("FEATURED", &[values[0], values[2]], "4.25", "0")).await.unwrap();
        sqlx::query("UPDATE catalog_products SET name='AAA variant' WHERE id=$1").bind(product).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,tax_class_id,price_cents,stock) SELECT CASE WHEN n<=2 THEN 'Equal name' ELSE 'Product '||lpad(n::text,2,'0') END,'HOME-'||n,'home-'||n,'published',$1,$2,100,1 FROM generate_series(1,25) n")
            .bind(entry.category_id).bind(entry.tax_class_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES ('AAA hidden draft','HOME-DRAFT','home-draft',100)").execute(&pool).await.unwrap();
        let catalog = products.public_list("", "", 1).await.unwrap();
        let featured = products.public_featured().await.unwrap();
        assert_eq!(featured.len(), 8);
        assert_eq!(featured.iter().map(|p| p.id).collect::<Vec<_>>(), catalog.iter().take(8).map(|p| p.id).collect::<Vec<_>>());
        assert_eq!(featured.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(), ["model", "home-1", "home-2", "home-3", "home-4", "home-5", "home-6", "home-7"]);
        assert!(featured[0].has_variants);
        assert_eq!(featured[0].quote().unwrap().gross().display(), "4,25");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let home = html(request(&app, "GET", "/", "", "", false).await).await;
        for entry in &featured { assert!(home.contains(&format!("href=\"/products/{}\"", entry.slug))); }
        assert!(!home.contains("href=\"/products/home-8\""));
        assert!(!home.contains("href=\"/products/home-draft\""));
        sqlx::query("UPDATE catalog_brands SET active=false").execute(&pool).await.unwrap();
        assert_eq!(products.public_featured().await.unwrap()[0].slug, "home-1");
        sqlx::query("UPDATE catalog_categories SET active=false").execute(&pool).await.unwrap();
        assert!(products.public_featured().await.unwrap().is_empty());
        assert!(!html(request(&app, "GET", "/", "", "", false).await).await.contains("href=\"/products/"));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn public_query_boundaries_and_unicode_metadata_fallback() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let (category, _, class) = product_references(&pool, &s).await;
        let products = talocart_commerce::products::Products::new(
            pool.clone(),
            identity,
            test_media(&pool).await,
        );
        let mut values = product_values("UNICODE", "unicode");
        values.name = "Caffè & tè".into();
        values.description = format!("  Caffè\n\t{}  ", "è".repeat(400));
        values.status = "published".into();
        values.category_id = category.to_string();
        values.tax_class_id = class.to_string();
        products.save(&s, &s.csrf, None, &values).await.unwrap();
        let app = router(test_app(pool, "Shop".into()).await);
        let page = html(request(&app, "GET", "/products/unicode", "", "", false).await).await;
        let description = page
            .split("<meta name=\"description\" content=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(description.chars().count(), 320);
        assert!(description.starts_with("Caffè èè"));
        assert!(
            page.contains("<title>Caffè &#38; tè ·") || page.contains("<title>Caffè &amp; tè ·")
        );
        for query in [
            "page=0",
            "page=-1",
            "page=1000001",
            "page=9223372036854775807",
            "q=%0A",
            "category=bad--slug",
            "brand=bad%2Fslug",
        ] {
            let response = request(&app, "GET", &format!("/catalog?{query}"), "", "", false).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{query}"
            );
        }
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("/catalog?q={}", "x".repeat(121)),
                "",
                "",
                false
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        for page in ["1", "1000000"] {
            assert_eq!(
                request(&app, "GET", &format!("/catalog?page={page}"), "", "", false)
                    .await
                    .status(),
                StatusCode::OK
            );
        }
        for query in ["page=-1", "page=1000001", "page=9223372036854775807"] {
            assert_eq!(
                request(
                    &app,
                    "GET",
                    &format!("/sitemap-products.xml?{query}"),
                    "",
                    "",
                    false
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn storefront_concurrent_saves_are_atomic_and_database_enforces_limits() {
    use talocart_platform::storefront::{Settings, Storefront};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let service = Storefront::new(pool.clone());
        let first = Settings {
            palette: "ocean".into(),
            heading: "First".into(),
            seo_title: "First SEO".into(),
            show_hero: false,
            ..Settings::default()
        };
        let second = Settings {
            palette: "clay".into(),
            heading: "Second".into(),
            seo_title: "Second SEO".into(),
            show_featured: false,
            ..Settings::default()
        };
        let (a, b) = tokio::join!(
            service.save(&s, &s.csrf, first),
            service.save(&s, &s.csrf, second)
        );
        a.unwrap();
        b.unwrap();
        let saved = service.settings().await.unwrap();
        match saved.palette.as_str() {
            "ocean" => assert_eq!(
                (
                    saved.heading.as_str(),
                    saved.seo_title.as_str(),
                    saved.show_hero,
                    saved.show_featured
                ),
                ("First", "First SEO", false, true)
            ),
            "clay" => assert_eq!(
                (
                    saved.heading.as_str(),
                    saved.seo_title.as_str(),
                    saved.show_hero,
                    saved.show_featured
                ),
                ("Second", "Second SEO", true, false)
            ),
            other => panic!("Unexpected palette: {other}"),
        }
        for sql in [
            "UPDATE storefront_settings SET palette='injected'",
            "UPDATE storefront_settings SET typography='remote'",
            "UPDATE storefront_settings SET density='huge'",
            "UPDATE storefront_settings SET heading=''",
            "UPDATE storefront_settings SET heading=repeat('a',121)",
            "UPDATE storefront_settings SET introduction=repeat('a',501)",
            "UPDATE storefront_settings SET seo_title=repeat('a',121)",
            "UPDATE storefront_settings SET seo_description=repeat('a',321)",
            "UPDATE storefront_settings SET public_origin=repeat('a',256)",
        ] {
            let error = sqlx::query(sql).execute(&pool).await.unwrap_err();
            assert_eq!(
                error.as_database_error().unwrap().code().as_deref(),
                Some("23514")
            );
        }
        assert_eq!(service.settings().await.unwrap().heading, saved.heading);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn public_pages_share_header_search_and_catalog_preserves_filter_controls() {
    isolated(|pool, _| async move {
        variant_fixture(&pool).await;
        let app = router(test_app(pool, "Shop".into()).await);
        for route in ["/", "/catalog", "/products/model", "/status"] {
            let response = request(&app, "GET", route, "", "", false).await;
            assert_eq!(response.status(), StatusCode::OK, "{route}");
            let document = html(response).await;
            let header = document
                .split("<header class=\"site-header\">")
                .nth(1)
                .unwrap()
                .split("</header>")
                .next()
                .unwrap();
            assert!(header.contains("role=\"search\""), "{route}");
            assert!(header.contains("method=\"get\" action=\"/search\""));
            assert!(header.contains("name=\"q\" type=\"search\" value=\"\""));
            assert_eq!(document.matches("name=\"q\"").count(), 1);
            assert!(header.find("wordmark").unwrap() < header.find("role=\"search\"").unwrap());
            assert!(
                header.find("role=\"search\"").unwrap()
                    < header.find("Navigazione principale").unwrap()
            );
        }
        let response = request(
            &app,
            "GET",
            "/search/%3Ctest%3E?category=categoria-prodotti&brand=brand&available=true&page=2",
            "",
            "",
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let document = html(response).await;
        assert!(document.contains("value=\"&#60;test&#62;\""));
        assert!(!document.contains("value=\"<test>\""));
        assert!(document.contains("name=\"category\" value=\"categoria-prodotti\""));
        assert!(document.contains("name=\"brand\" form=\"store-filters-form\""));
        assert!(
            document
                .contains("name=\"available\" value=\"true\" form=\"store-filters-form\" checked")
        );
        assert!(
            !document.contains("name=\"page\""),
            "A new search starts at page one"
        );
        assert!(!document.contains("class=\"store-search\""));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn manual_maintenance_preserves_admin_access_permissions_and_native_http_contracts() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = "/admin/settings/maintenance";
        assert_eq!(
            request(&app, "GET", path, "", "", false).await.status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            request(&app, "GET", "/maintenance/status", "", "", false)
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            request(
                &app,
                "POST",
                path,
                &cookie,
                "csrf=invalid&enabled=true",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let role = identity
            .create_role(
                &session,
                &session.csrf,
                "Maintenance reader",
                &[talocart_platform::identity::Permission::SettingsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &session,
                &session.csrf,
                "maintenance-reader@example.test",
                "correct horse reader".into(),
                talocart_platform::identity::Role::Custom(role),
            )
            .await
            .unwrap();
        let (_, anonymous) = identity.anonymous().await.unwrap();
        let reader_raw = identity
            .login(
                &anonymous,
                &anonymous.csrf,
                "maintenance-reader@example.test",
                "correct horse reader".into(),
            )
            .await
            .unwrap();
        let reader = identity.session(&reader_raw).await.unwrap().unwrap();
        let reader_cookie = format!("talocart_session={reader_raw}");
        let page = html(request(&app, "GET", path, &reader_cookie, "", false).await).await;
        assert!(!page.contains("name=\"enabled\""));
        assert_eq!(
            request(
                &app,
                "POST",
                path,
                &reader_cookie,
                &format!("csrf={}&enabled=true", reader.csrf),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let webhook_before = request(&app, "POST", "/payments/example/webhook", "", "", false)
            .await
            .status();
        let change = |enabled| format!("csrf={}&enabled={enabled}", session.csrf);
        assert_eq!(
            request(&app, "POST", path, &cookie, &change(true), false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        for route in [
            "/",
            "/catalog",
            "/en/catalog",
            "/cart",
            "/checkout",
            "/account",
        ] {
            for htmx in [false, true] {
                let r = request(&app, "GET", route, "", "", htmx).await;
                assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE, "{route}");
                assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
                assert!(r.headers().contains_key(header::RETRY_AFTER));
                if htmx {
                    assert_eq!(r.headers()["hx-redirect"], "/");
                }
                assert!(html(r).await.contains("<em>manutenzione</em>"));
            }
        }
        assert_eq!(
            request(&app, "POST", "/checkout", "", "", false)
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let head = request(&app, "HEAD", "/", "", "", false).await;
        assert_eq!(head.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(html(head).await.is_empty());
        for route in [
            "/admin/login",
            path,
            "/admin",
            "/health/ready",
            "/assets/ui.css",
        ] {
            assert_eq!(
                request(&app, "GET", route, &cookie, "", false)
                    .await
                    .status(),
                StatusCode::OK,
                "{route}"
            );
        }
        let webhook = request(&app, "POST", "/payments/example/webhook", "", "", false).await;
        assert_eq!(webhook.status(), webhook_before);
        assert!(!webhook.headers().contains_key("x-talocart-maintenance"));
        let restarted = router(test_app(pool.clone(), "Shop".into()).await);
        assert_eq!(
            request(&restarted, "GET", "/", "", "", false)
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            request(&app, "POST", path, &cookie, &change(false), false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            request(&app, "GET", "/", "", "", false).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "GET", "/maintenance/status", "", "", false)
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
    })
    .await;
}
