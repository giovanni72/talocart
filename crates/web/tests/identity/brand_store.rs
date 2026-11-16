use super::*;

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn brand_menu_and_directory_show_public_logos_after_localized_names() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let (_, brand, _) = product_references(&pool, &session).await;
        let catalog = talocart_commerce::Catalog::new(pool.clone(), identity, test_media(&pool).await);
        catalog.brand_logo(&session, &session.csrf, brand, Some(include_bytes!("../fixtures/webp-lossless.webp").to_vec()), "logo.webp").await.unwrap();
        let logo = catalog.public_brand("marchio-prodotti").await.unwrap().logo_key.unwrap();
        let image = catalog.logo_metadata(&logo).await.unwrap().unwrap();
        sqlx::query("INSERT INTO catalog_brands(name,slug) VALUES ('Without logo','without-logo')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_brands_translations(entity_id,locale,name,description) VALUES ($1,'en','Brand <safe>','') ON CONFLICT(entity_id,locale) DO UPDATE SET name=excluded.name").bind(brand).execute(&pool).await.unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for theme in ["default", "essential"] {
            sqlx::query("UPDATE storefront_settings SET theme_id=$1 WHERE id=1").bind(theme).execute(&pool).await.unwrap();
            for prefix in ["", "/en"] {
                let body = html(request(&app, "GET", &format!("{prefix}/brands"), "", "", false).await).await;
                for (section, profile) in [("class=\"category-panel brand-panel\"", "thumbnail"), ("class=\"brand-directory-list\"", "card")] {
                    let section = body.split(section).nth(1).expect("Brand list").split("</ul>").next().unwrap();
                    let link = section.split(&format!("href=\"{prefix}/brands/marchio-prodotti\"")).nth(1).expect("Brand link").split("</a>").next().unwrap();
                    assert!(link.contains("class=\"brand-link-logo\""), "{theme} {prefix} logo suffix missing");
                    assert!(link.contains(&format!("src=\"/media/image/{}/{profile}\"", image.id)));
                    assert!(link.contains("alt=\"\""), "Adjacent brand name labels the link once");
                    assert!(link.find("<img").unwrap() < link.find("class=\"brand-link-name\"").unwrap(), "Logo precedes brand name");
                    if prefix == "/en" { assert!(link.contains("Brand &#60;safe&#62;")); }
                    let without = section.split(&format!("href=\"{prefix}/brands/without-logo\"")).nth(1).unwrap().split("</a>").next().unwrap();
                    assert!(!without.contains("<img"), "No empty image for a brand without logo");
                    assert!(without.contains("<span class=\"brand-initial\" aria-hidden=\"true\">W</span>"), "Decorative monogram replaces a missing logo");
                }
            }
        }
        sqlx::query("UPDATE media_objects SET public=false WHERE id=$1").bind(image.id).execute(&pool).await.unwrap();
        let body = html(request(&app, "GET", "/brands", "", "", false).await).await;
        assert!(!body.contains("class=\"brand-link-logo\""), "Private images are excluded from both lists");
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn brand_view_fixes_brand_and_preserves_filters_cards_logo_and_visibility() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let (category, brand, class) = product_references(&pool, &session).await;
        let media = test_media(&pool).await;
        let catalog = talocart_commerce::Catalog::new(pool.clone(), identity.clone(), media.clone());
        sqlx::query("UPDATE catalog_brands_translations SET name='<Marchio & idee>',description='<script>Marchio</script> & descrizione' WHERE entity_id=$1 AND locale='it'").bind(brand).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock,minimum_quantity,packs) SELECT 'Marchio '||lpad(n::text,2,'0'),'BRAND-'||n,'brand-product-'||n,'published',$1,$2,$3,100,CASE WHEN n%2=0 THEN 3 ELSE 5 END,3,2 FROM generate_series(1,30) n")
            .bind(category).bind(brand).bind(class).execute(&pool).await.unwrap();
        let other: i64 = sqlx::query_scalar("INSERT INTO catalog_brands(name,slug,active) VALUES ('Altro marchio','altro',true) RETURNING id").fetch_one(&pool).await.unwrap();
        let other_category: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,active) VALUES ('Altra categoria','altra',true) RETURNING id").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock) VALUES ('Senza categoria','BRAND-NONE','brand-none','published',NULL,$1,$3,100,5),('Altro','BRAND-OTHER','brand-other','published',$4,$2,$3,100,5),('Bozza','BRAND-DRAFT','brand-draft','draft',$4,$1,$3,100,5)")
            .bind(brand).bind(other).bind(class).bind(other_category).execute(&pool).await.unwrap();
        let products = talocart_commerce::products::Products::new(pool.clone(), identity, media);
        assert_eq!(products.public_brand_categories("marchio-prodotti").await.unwrap(), vec![("Categoria prodotti".into(), "categoria-prodotti".into())]);
        assert!(products.public_brand_categories("bad--slug").await.is_err());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for path in ["/", "/catalog", "/catalog/categoria-prodotti", "/products/brand-product-1", "/brands", "/brands/marchio-prodotti", "/status"] {
            let response = request(&app, "GET", path, "", "", false).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = html(response).await;
            assert_eq!(body.matches("brand-dropdown").count(), 1, "{path}");
            assert!(body.contains(&format!("Versione build {}", talocart_platform::version::DISPLAY)));
            assert!(body.contains("href=\"/brands/marchio-prodotti\""));
            assert!(body.contains("&#60;Marchio &#38; idee&#62;"));
            assert!(!body.contains("<Marchio & idee>"));
        }
        let detail = html(request(&app, "GET", "/products/brand-product-1", "", "", false).await).await;
        assert!(detail.contains("<a class=\"product-brand\" href=\"/brands/marchio-prodotti\">"), "Product detail links its brand page");
        assert!(detail.contains("<span class=\"brand-initial\" aria-hidden=\"true\">M</span>"));
        assert!(detail.contains("<span class=\"product-brand-name\">&#60;Marchio &#38; idee&#62;</span><span class=\"product-brand-more\">Tutti i prodotti del marchio →</span>"));
        let body = html(request(&app, "GET", "/brands/marchio-prodotti?brand=altro", "", "", false).await).await;
        assert!(body.contains("<h1>&#60;Marchio &#38; idee&#62;</h1>"));
        assert!(body.contains("<title>&#60;Marchio &#38; idee&#62; · Shop</title>"));
        assert!(body.contains("&#60;script&#62;Marchio&#60;/script&#62; &#38; descrizione"));
        assert!(!body.contains("<script>Marchio</script>"));
        assert!(body.contains("method=\"get\" action=\"/search\""));
        assert!(body.contains("href=\"/brands/marchio-prodotti\" aria-current=\"true\""));
        assert!(body.contains("Filtri marchio"));
        assert!(!body.contains("id=\"store-brand\""));
        assert!(!body.contains("store-brand-logo"));
        assert!(!body.contains("value=\"altra\""));
        assert!(!body.contains("href=\"/products/brand-other\""));
        assert!(!body.contains("href=\"/products/brand-draft\""));
        assert_eq!(body.matches("class=\"store-product\"").count(), 21);
        let second = html(request(&app, "GET", "/brands/marchio-prodotti?page=2", "", "", false).await).await;
        assert_eq!(second.matches("class=\"store-product\"").count(), 10);
        assert!(second.contains("href=\"/products/brand-none\""), "Uncategorized products remain available in brand pages");
        let filtered = html(request(&app, "GET", "/brands/marchio-prodotti?category=categoria-prodotti&available=true&per_page=42", "", "", false).await).await;
        assert_eq!(filtered.matches("class=\"store-product\"").count(), 15);
        assert!(filtered.contains("href=\"/brands/marchio-prodotti?per_page=42\">Rimuovi filtri"));
        assert!(filtered.contains("name=\"brand\" value=\"marchio-prodotti\""));
        assert!(filtered.contains("rel=\"canonical\" href=\"http://127.0.0.1:3000/brands/marchio-prodotti\""));
        let all = html(request(&app, "GET", "/brands/marchio-prodotti?per_page=42", "", "", false).await).await;
        assert_eq!(all.matches("class=\"store-product\"").count(), 31);
        assert!(!all.contains(">Rimuovi filtri</a>"));
        let empty = html(request(&app, "GET", "/brands/marchio-prodotti?category=altra", "", "", false).await).await;
        assert!(empty.contains("value=\"altra\" selected>Categoria non presente"));
        assert!(empty.contains("Prova a modificare o rimuovere i filtri di questo marchio."));
        catalog.brand_logo(&session, &session.csrf, brand, Some(include_bytes!("../fixtures/webp-lossless.webp").to_vec()), "Logo & nome.webp").await.unwrap();
        let logo = catalog.public_brand("marchio-prodotti").await.unwrap().logo_key.unwrap();
        let metadata = catalog.logo_metadata(&logo).await.unwrap().unwrap();
        let body = html(request(&app, "GET", "/brands/marchio-prodotti", "", "", false).await).await;
        assert!(body.contains("class=\"store-brand-logo\""));
        assert!(body.find("class=\"store-brand-logo\"").unwrap() < body.find("<h1>").unwrap(), "Logo before brand heading");
        assert!(body.contains("class=\"store-product-brand-logo\""));
        assert!(body.contains(&format!("src=\"/media/image/{}/thumbnail\"", metadata.id)));
        assert!(body.contains(&format!("srcset=\"/media/image/{}/thumbnail 1x, /media/image/{}/card 4x\"", metadata.id, metadata.id)));
        assert_eq!(image_follow(&app, &format!("/media/image/{}/card", metadata.id), "", "GET", "").await.status(), StatusCode::OK);
        let detail = html(request(&app, "GET", "/products/brand-product-1", "", "", false).await).await;
        assert!(detail.contains(&format!("<span class=\"product-brand-media\"><img src=\"/media/image/{}/thumbnail\" alt=\"\"", metadata.id)));
        for path in ["/", "/catalog", "/catalog/categoria-prodotti", "/search/Marchio"] {
            let cards = html(request(&app, "GET", path, "", "", false).await).await;
            assert!(cards.contains("class=\"store-product-brand-logo\""), "{path}");
            assert!(cards.contains("alt=\"Logo &#60;Marchio &#38; idee&#62;\""), "escaped logo name on {path}");
        }
        let logo_path = format!("/media/public/{}/{}/card", metadata.id, logo);
        assert_eq!(image_follow(&app, &logo_path, "", "GET", "").await.status(), StatusCode::OK);
        sqlx::query("INSERT INTO catalog_brands(name,slug,active) VALUES ('Vuoto','vuoto',true)").execute(&pool).await.unwrap();
        let empty = html(request(&app, "GET", "/brands/vuoto", "", "", false).await).await;
        assert!(empty.contains("<h1>Vuoto</h1>"));
        assert!(empty.contains("Non ci sono ancora prodotti disponibili per questo marchio."));
        for path in ["/brands/missing", "/brands/missing?category=categoria-prodotti"] {
            assert_eq!(request(&app, "GET", path, "", "", false).await.status(), StatusCode::NOT_FOUND);
        }
        for path in ["/brands/bad--slug", "/brands/marchio-prodotti?category=bad--slug", "/brands/marchio-prodotti?per_page=20", "/brands/marchio-prodotti?page=0"] {
            assert_eq!(request(&app, "GET", path, "", "", false).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
        sqlx::query("UPDATE catalog_categories SET active=false WHERE id=$1").bind(category).execute(&pool).await.unwrap();
        assert!(products.public_brand_categories("marchio-prodotti").await.unwrap().is_empty());
        let without_category = html(request(&app, "GET", "/brands/marchio-prodotti?per_page=42", "", "", false).await).await;
        assert_eq!(without_category.matches("class=\"store-product\"").count(), 1);
        assert!(without_category.contains("href=\"/products/brand-none\""));
        sqlx::query("UPDATE catalog_brands SET active=false WHERE id=$1").bind(brand).execute(&pool).await.unwrap();
        assert_eq!(request(&app, "GET", "/brands/marchio-prodotti", "", "", false).await.status(), StatusCode::NOT_FOUND);
        let body = html(request(&app, "GET", "/", "", "", false).await).await;
        assert!(!body.contains("href=\"/brands/marchio-prodotti\""));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn brand_menu_and_directory_reach_all_active_brands_with_bounded_pages() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let catalog = talocart_commerce::Catalog::new(pool.clone(), identity, test_media(&pool).await);
        let empty_app = router(test_app(pool.clone(), "Shop".into()).await);
        let empty = html(request(&empty_app, "GET", "/brands", "", "", false).await).await;
        assert!(empty.contains("Nessun marchio presente."));
        assert!(empty.contains("I marchi saranno disponibili qui appena attivati."));
        sqlx::query("INSERT INTO catalog_brands(name,slug) SELECT 'Brand '||lpad(n::text,4,'0'),'directory-brand-'||n FROM generate_series(1,1001) n").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_brands(name,slug,active) VALUES ('zz <Marchio & idee>','ultimo',true),('Nascosto','nascosto',false)").execute(&pool).await.unwrap();
        let mut names = vec![];
        for page in 1..=11 {
            let rows = catalog.public_brand_page(page).await.unwrap();
            assert!(rows.len() <= 101);
            names.extend(rows.into_iter().take(100).map(|brand| brand.0));
        }
        assert_eq!(names.len(), 1002);
        assert_eq!(names.first().unwrap(), "Brand 0001");
        assert_eq!(names.last().unwrap(), "zz <Marchio & idee>");
        let app = router(test_app(pool, "Shop".into()).await);
        let body = html(request(&app, "GET", "/", "", "", false).await).await;
        let panel = body.split("class=\"category-panel brand-panel\"").nth(1).unwrap().split("</details>").next().unwrap();
        assert_eq!(panel.matches("href=\"/brands/directory-brand-").count(), 100);
        assert!(panel.contains("href=\"/brands?page=2\">Altri marchi"));
        assert!(!panel.contains("href=\"/brands/nascosto\""));
        let second = html(request(&app, "GET", "/brands?page=2", "", "", false).await).await;
        let directory = second.split("class=\"brand-directory-list\"").nth(1).unwrap().split("</ul>").next().unwrap();
        assert_eq!(directory.matches("<li>").count(), 100);
        assert!(directory.contains("/brands/directory-brand-101"));
        let last = html(request(&app, "GET", "/brands?page=11", "", "", false).await).await;
        let directory = last.split("class=\"brand-directory-list\"").nth(1).unwrap().split("</ul>").next().unwrap();
        assert_eq!(directory.matches("<li>").count(), 2);
        assert!(directory.contains("zz &#60;Marchio &#38; idee&#62;"));
        assert!(!last.contains("rel=\"next\""));
        let beyond_menu = html(request(&app, "GET", "/brands/ultimo", "", "", false).await).await;
        assert!(beyond_menu.contains("<h1>zz &#60;Marchio &#38; idee&#62;</h1>"));
        assert!(beyond_menu.contains("brand-menu-item is-selected"));
        let beyond = html(request(&app, "GET", "/brands?page=12", "", "", false).await).await;
        assert!(beyond.contains("Pagina 11 di 11"));
        assert!(beyond.contains("Marchi 1001–1002 di 1002"));
        assert!(!beyond.contains("Nessun marchio presente."));
        assert!(beyond.contains("rel=\"prev\""));
        assert!(!beyond.contains("rel=\"next\""));
        for page in [0, i64::MAX] {
            assert_eq!(request(&app, "GET", &format!("/brands?page={page}"), "", "", false).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
    }).await;
}
