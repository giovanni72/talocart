use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn new_category_and_search_routes_forms_filters_and_text_encoding() {
    isolated(|pool, _| async move {
        let (identity, session, products, variants, model, axes, values) = variant_fixture(&pool).await;
        variants.configure(&session, &session.csrf, model, &axes).await.unwrap();
        variants.save(&session, &session.csrf, model, None, &variant_values("SEARCH-VARIANT", &[values[0],values[2]], "12.34", "5")).await.unwrap();
        let entry = products.entry(&session, model).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock,minimum_quantity,packs) SELECT 'Ricerca '||lpad(n::text,2,'0'),'SEARCH-'||n,'search-'||n,CASE WHEN n<=60 THEN 'published' ELSE 'draft' END,CASE WHEN n=60 THEN NULL ELSE $1 END,$2,$3,100,CASE WHEN n%2=0 THEN 3 ELSE 5 END,3,2 FROM generate_series(1,61) n")
            .bind(entry.category_id).bind(entry.brand_id).bind(entry.tax_class_id).execute(&pool).await.unwrap();
        let catalog = talocart_commerce::Catalog::new(pool.clone(), identity, test_media(&pool).await);
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let category = html(request(&app, "GET", "/catalog/categoria-prodotti?category=missing", "", "", false).await).await;
        assert!(category.contains("<h1>Categoria prodotti</h1>"));
        assert!(category.contains("href=\"/catalog/categoria-prodotti\" aria-current=\"true\""));
        assert!(category.contains("rel=\"canonical\" href=\"http://127.0.0.1:3000/catalog/categoria-prodotti\""));
        assert!(category.contains("id=\"store-filters-form\" method=\"get\" action=\"/catalog/categoria-prodotti\""));
        assert!(category.contains("name=\"category\" value=\"categoria-prodotti\""));
        for old in ["/catalog?category=categoria-prodotti", "/catalog?q=Ricerca", "/catalog?category=categoria-prodotti&q=Ricerca"] {
            let response = request(&app, "GET", old, "", "", false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert!(!response.headers().contains_key(header::LOCATION));
        }
        let submitted = request(&app, "GET", "/search?q=Ricerca&category=categoria-prodotti&brand=marchio-prodotti&available=true&per_page=42&page=2", "", "", false).await;
        assert_eq!(submitted.status(), StatusCode::SEE_OTHER);
        assert_eq!(submitted.headers()[header::LOCATION], "/search/Ricerca?category=categoria-prodotti&brand=marchio-prodotti&available=true&per_page=42");
        let body = html(request(&app, "GET", submitted.headers()[header::LOCATION].to_str().unwrap(), "", "", false).await).await;
        assert!(body.contains("<h1>Risultati per “Ricerca”</h1>"));
        assert!(body.contains("Filtri ricerca"));
        assert_eq!(body.matches("class=\"store-product\"").count(), 30, "Uses minimum/packs with search/category/brand filters");
        assert!(body.contains("href=\"/search/Ricerca?per_page=42\">Rimuovi filtri"));
        assert!(body.contains("rel=\"canonical\" href=\"http://127.0.0.1:3000/search/Ricerca\""));
        assert!(body.contains("id=\"store-search-form\" method=\"get\" action=\"/search\""));
        assert!(body.contains("id=\"store-filters-form\" method=\"get\" action=\"/search/Ricerca\""));
        assert!(body.contains("name=\"category\" form=\"store-filters-form\""));
        assert!(body.contains("name=\"brand\" form=\"store-filters-form\""));
        let mut seen = vec![];
        for page in 1..=3 {
            let body = html(request(&app, "GET", &format!("/search/Ricerca?page={page}"), "", "", false).await).await;
            assert_eq!(body.matches("class=\"store-product\"").count(), if page == 3 { 18 } else { 21 });
            for n in 1..=60 {
                if body.contains(&format!("href=\"/products/search-{n}\"")) { seen.push(n); }
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, (1..=60).collect::<Vec<_>>());
        let all = html(request(&app, "GET", "/search/Ricerca?per_page=63", "", "", false).await).await;
        assert_eq!(all.matches("class=\"store-product\"").count(), 60);
        assert!(all.contains("href=\"/products/search-60\""), "Uncategorized search result is included");
        for (term, href) in [("SEARCH-60", "/products/search-60"), ("SEARCH-VARIANT", "/products/model")] {
            let body = html(request(&app, "GET", &format!("/search/{term}"), "", "", false).await).await;
            assert!(body.contains(&format!("href=\"{href}\"")));
        }
        let term = "Caffè & tè / 50% + #?";
        sqlx::query("UPDATE catalog_products SET name=$1 WHERE model='SEARCH-1'").bind(term).execute(&pool).await.unwrap();
        let encoded = "Caff%C3%A8%20%26%20t%C3%A8%20%2F%2050%25%20%2B%20%23%3F";
        let response = request(&app, "GET", &format!("/search?q={encoded}"), "", "", false).await;
        assert_eq!(response.headers()[header::LOCATION], format!("/search/{encoded}"));
        let body = html(request(&app, "GET", &format!("/search/{encoded}?q=wrong"), "", "", false).await).await;
        assert!(body.contains("Caffè &#38; tè / 50% + #?"));
        assert_eq!(body.matches("class=\"store-product\"").count(), 1);
        let escaped = html(request(&app, "GET", "/search/%3Cscript%3Ealert%281%29%3C%2Fscript%3E", "", "", false).await).await;
        assert!(!escaped.contains("<script>alert(1)</script>"));
        assert!(escaped.contains("&#60;script&#62;alert(1)&#60;/script&#62;"));
        let literal = html(request(&app, "GET", "/search/%25", "", "", false).await).await;
        assert_eq!(literal.matches("class=\"store-product\"").count(), 1, "Percent sign is literal, not an SQL wildcard");
        for (input, expected) in [("/search", "/catalog"), ("/search?q=+&category=categoria-prodotti&per_page=42", "/catalog/categoria-prodotti?per_page=42"), ("/search?brand=marchio-prodotti", "/brands/marchio-prodotti"), ("/search?q=..", "/search/..%20")] {
            let response = request(&app, "GET", input, "", "", false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(response.headers()[header::LOCATION], expected);
        }
        for path in ["/search/%0A", "/search/%20", "/search/Ricerca?category=bad--slug", "/search/Ricerca?brand=bad%2Fslug", "/search/Ricerca?per_page=20", "/search/Ricerca?page=0", "/search?q=%0A"] {
            assert_eq!(request(&app, "GET", path, "", "", false).await.status(), StatusCode::UNPROCESSABLE_ENTITY, "{path}");
        }
        let too_long = format!("/search/{}", "x".repeat(121));
        assert_eq!(request(&app, "GET", &too_long, "", "", false).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(request(&app, "GET", "/catalog/missing", "", "", false).await.status(), StatusCode::NOT_FOUND);
        assert!(catalog.public_category("categoria-prodotti").await.is_ok());
    }).await;
}
