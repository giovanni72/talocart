use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn catalog_page_sizes_preserve_boundaries_filters_and_http_navigation() {
    use talocart_commerce::products::{CATALOG_PAGE_SIZES, CatalogPageSize, Products};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let (category, brand, class) = product_references(&pool, &session).await;
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock) SELECT 'Paging '||lpad(n::text,3,'0'),'PAGE-'||n,'paging-'||n,CASE WHEN n<=100 THEN 'published' ELSE 'draft' END,$1,$2,$3,100,CASE WHEN n%2=0 THEN 0 ELSE 1 END FROM generate_series(1,101) n")
            .bind(category).bind(brand).bind(class).execute(&pool).await.unwrap();
        let products = Products::new(pool.clone(), identity, test_media(&pool).await);
        for value in CATALOG_PAGE_SIZES {
            let size = CatalogPageSize::parse(&value.to_string()).unwrap();
            let mut names = Vec::new();
            for page in 1..=5 {
                let entries = products.public_filtered_sized("Paging", "categoria-prodotti", "marchio-prodotti", false, page, size).await.unwrap();
                assert!(entries.len() <= usize::from(value) + 1);
                let more = entries.len() > usize::from(value);
                names.extend(entries.into_iter().take(usize::from(value)).map(|entry| entry.name));
                if !more { break; }
            }
            assert_eq!(names, (1..=100).map(|n| format!("Paging {n:03}")).collect::<Vec<_>>(), "no gaps, duplicates or drafts at size {value}");
        }
        let first = products.public_list("Paging", "", 1).await.unwrap();
        assert_eq!(first.len(), 22, "21 products plus sentinel");
        assert_eq!(first[20].name, "Paging 021");
        let second = products.public_filtered("Paging", "", "", false, 2).await.unwrap();
        assert_eq!(second[0].name, "Paging 022");
        let available = products.public_filtered_sized("Paging", "categoria-prodotti", "marchio-prodotti", true, 2, CatalogPageSize::parse("42").unwrap()).await.unwrap();
        assert_eq!(available.len(), 8);
        assert!(available.iter().all(|entry| entry.available()));
        assert!(products.public_filtered_sized("%", "", "", false, 1, CatalogPageSize::default()).await.unwrap().is_empty());
        assert!(products.public_filtered_sized("", "", "", false, 0, CatalogPageSize::default()).await.is_err());
        assert!(products.public_filtered_sized("", "", "", false, i64::MAX, CatalogPageSize::default()).await.is_err());
        assert!(products.public_filtered_sized("", "", "", false, 1_000_000, CatalogPageSize::parse("84").unwrap()).await.unwrap().is_empty());

        let app = router(test_app(pool, "Shop".into()).await);
        let default = html(request(&app, "GET", "/catalog", "", "", false).await).await;
        assert_eq!(default.matches("class=\"store-product\"").count(), 21);
        assert!(default.contains("value=\"21\" selected"));
        assert!(default.contains("Prodotti 1–21 di 100"));
        assert!(default.contains("Pagina 1 di 5"));
        assert!(!default.contains("rel=\"prev\""));
        for route in ["/catalog", "/catalog/categoria-prodotti", "/brands/marchio-prodotti", "/search/Paging"] {
            let body = html(request(&app, "GET", &format!("{route}?page=1000000&per_page=21"), "", "", false).await).await;
            assert!(body.contains("Prodotti 85–100 di 100"), "{route}");
            assert!(body.contains("class=\"catalog-result-count muted\">100 prodotti</p>"), "{route}");
            assert!(body.contains("Pagina 5 di 5"));
            assert!(body.contains("rel=\"prev\""));
            assert!(!body.contains("rel=\"next\""));
            assert_eq!(body.matches("class=\"store-product\"").count(), 16);
        }
        let url = "/catalog/categoria-prodotti?brand=marchio-prodotti&available=true&per_page=42";
        let body = html(request(&app, "GET", url, "", "", false).await).await;
        assert_eq!(body.matches("class=\"store-product\"").count(), 42);
        assert!(body.contains("Prodotti 1–42 di 50"));
        assert!(body.contains("class=\"catalog-result-count muted\">50 prodotti</p>"));
        assert!(body.contains("Pagina 1 di 2"));
        assert!(body.contains("value=\"42\" selected"));
        assert!(body.contains("page=2&#38;per_page=42"));
        assert!(body.contains("href=\"/catalog/categoria-prodotti?per_page=42\">Rimuovi filtri"));
        assert!(body.contains("name=\"q\" type=\"search\" value=\"\""));
        assert!(body.contains("rel=\"canonical\" href=\"http://127.0.0.1:3000/catalog/categoria-prodotti\""));
        let second = html(request(&app, "GET", &format!("{url}&page=2"), "", "", false).await).await;
        assert_eq!(second.matches("class=\"store-product\"").count(), 8);
        assert!(!second.contains("rel=\"next\""));
        let empty = html(request(&app, "GET", "/search/missing?per_page=84", "", "", false).await).await;
        assert!(empty.contains("value=\"84\" selected"));
        assert!(empty.contains("Nessun prodotto trovato."));
        assert!(empty.contains("Prodotti 0 di 0"));
        assert!(empty.contains("class=\"catalog-result-count muted\">0 prodotti</p>"));
        let single = html(request(&app, "GET", "/search/Paging%20100", "", "", false).await).await;
        assert!(single.contains("class=\"catalog-result-count muted\">1 prodotto</p>"));
        assert!(empty.contains("Pagina 1 di 1"));
        assert!(!empty.contains("rel=\"next\"") && !empty.contains("rel=\"prev\""));
        for invalid in ["0", "20", "22", "100", "65536", "all", "21%3BDROP"] {
            assert_eq!(request(&app, "GET", &format!("/catalog?per_page={invalid}"), "", "", false).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn catalog_preference_persists_across_public_routes() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        product_references(&pool, &session).await;
        let app = router(test_app(pool, "Shop".into()).await);
        let response = request(&app, "GET", "/catalog?per_page=42", "", "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        let saved = response
            .headers()
            .get(header::SET_COOKIE)
            .expect("persistent preference")
            .to_str()
            .unwrap()
            .to_owned();
        assert!(saved.contains("Max-Age=15552000"));
        assert!(saved.contains("HttpOnly"));
        assert!(saved.contains("SameSite=Lax"));
        assert!(saved.contains("Path=/"));
        let cookie = saved.split(';').next().unwrap();
        for route in [
            "/catalog",
            "/catalog/categoria-prodotti",
            "/brands/marchio-prodotti",
            "/search/anything",
        ] {
            let response = request(&app, "GET", route, cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(html(response).await.contains("value=\"42\" selected"));
        }
        let response = request(&app, "GET", "/search?q=anything", cookie, "", false).await;
        assert_eq!(
            response.headers()[header::LOCATION],
            "/search/anything?per_page=42"
        );
        let response = request(&app, "GET", "/catalog?per_page=21", cookie, "", false).await;
        assert!(
            response.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .starts_with("talocart_catalog_page_size=21;")
        );
        assert!(html(response).await.contains("value=\"21\" selected"));
        for value in ["invalid", "100", "0", "42; talocart_catalog_page_size=84"] {
            let response = request(
                &app,
                "GET",
                "/catalog",
                &format!("talocart_catalog_page_size={value}"),
                "",
                false,
            )
            .await;
            assert!(html(response).await.contains("value=\"21\" selected"));
        }
        assert_eq!(
            request(&app, "GET", "/catalog?per_page=100", cookie, "", false)
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    })
    .await;
}
