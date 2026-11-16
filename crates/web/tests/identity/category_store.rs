use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn category_descendants_share_filters_counts_pagination_and_preserve_assignment() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let (root, brand, class) = product_references(&pool, &session).await;
        let child: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,parent_id) VALUES ('Figlia','figlia',$1) RETURNING id")
            .bind(root).fetch_one(&pool).await.unwrap();
        let grandchild: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,parent_id) VALUES ('Nipote','nipote',$1) RETURNING id")
            .bind(child).fetch_one(&pool).await.unwrap();
        let inactive: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,parent_id,active) VALUES ('Inattiva','inattiva',$1,true) RETURNING id")
            .bind(root).fetch_one(&pool).await.unwrap();
        let below_inactive: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,parent_id) VALUES ('Attiva sotto inattiva','sotto-inattiva',$1) RETURNING id")
            .bind(inactive).fetch_one(&pool).await.unwrap();
        let other: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug) VALUES ('Altrove','altrove') RETURNING id")
            .fetch_one(&pool).await.unwrap();
        let descendant_brand: i64 = sqlx::query_scalar("INSERT INTO catalog_brands(name,slug) VALUES ('Marchio discendenti','marchio-discendenti') RETURNING id")
            .fetch_one(&pool).await.unwrap();
        let inactive_brand: i64 = sqlx::query_scalar("INSERT INTO catalog_brands(name,slug,active) VALUES ('Marchio inattivo','marchio-inattivo',true) RETURNING id")
            .fetch_one(&pool).await.unwrap();
        let mut deepest = grandchild;
        for level in 4..=16 {
            deepest = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,parent_id) VALUES ($1,$2,$3) RETURNING id")
                .bind(format!("Livello {level}")).bind(format!("livello-{level}")).bind(deepest).fetch_one(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock,minimum_quantity,packs) SELECT 'Descendant '||lpad(n::text,2,'0'),'DESC-'||n,'desc-'||n,'published',CASE WHEN n<=20 THEN $1 ELSE $2 END,$3,$4,100,CASE WHEN n%2=0 THEN 3 ELSE 5 END,3,2 FROM generate_series(1,22) n")
            .bind(child).bind(grandchild).bind(descendant_brand).bind(class).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock) VALUES ('Root','ROOT','root','published',$1,$2,$3,100,5),('Deep','DEEP','deep','published',$4,$5,$3,100,5),('Below inactive','BELOW','below','published',$6,$2,$3,100,5),('Inactive','INACTIVE','inactive','published',$7,$2,$3,100,5),('Draft','DRAFT','draft','draft',$4,$5,$3,100,5),('Inactive brand','IB','ib','published',$4,$8,$3,100,5),('Other','OTHER','other','published',$9,$5,$3,100,5),('None','NONE','none','published',NULL,$5,$3,100,5)")
            .bind(root).bind(brand).bind(class).bind(deepest).bind(descendant_brand).bind(below_inactive).bind(inactive).bind(inactive_brand).bind(other).execute(&pool).await.unwrap();
        sqlx::query("UPDATE catalog_categories SET active=false WHERE id=$1").bind(inactive).execute(&pool).await.unwrap();
        sqlx::query("UPDATE catalog_brands SET active=false WHERE id=$1").bind(inactive_brand).execute(&pool).await.unwrap();
        let before: Vec<(i64, Option<i64>)> = sqlx::query_as("SELECT id,category_id FROM catalog_products ORDER BY id").fetch_all(&pool).await.unwrap();
        let products = talocart_commerce::products::Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let size = talocart_commerce::products::CatalogPageSize::default();
        let first = products.public_page("", "categoria-prodotti", "", false, 1, size).await.unwrap();
        assert_eq!(first.total, 25, "Root includes every visible descendant, including depth 16");
        assert_eq!((first.page, first.pages, first.first, first.last, first.entries.len()), (1, 2, 1, 21, 21));
        let last = products.public_page("", "categoria-prodotti", "", false, 999, size).await.unwrap();
        assert_eq!((last.total, last.page, last.pages, last.first, last.last, last.entries.len()), (25, 2, 2, 22, 25, 4));
        let mut ids: Vec<_> = first.entries.iter().chain(&last.entries).map(|p| p.id).collect();
        ids.sort_unstable(); ids.dedup(); assert_eq!(ids.len(), 25);
        let sentinel = products.public_filtered_sized("", "categoria-prodotti", "", false, 1, size).await.unwrap();
        assert_eq!(sentinel.len(), 22);
        for (category, expected) in [("figlia",23), ("nipote",3), ("livello-16",1), ("inattiva",0), ("missing",0)] {
            assert_eq!(products.public_page("",category,"",false,1,size).await.unwrap().total, expected, "{category}");
        }
        assert_eq!(products.public_page("", "categoria-prodotti", "", true, 1, size).await.unwrap().total, 14);
        assert_eq!(products.public_page("", "categoria-prodotti", "marchio-discendenti", true, 1, size).await.unwrap().total, 12);
        assert_eq!(products.public_page("Descendant", "categoria-prodotti", "marchio-discendenti", false, 1, size).await.unwrap().total, 22);
        assert_eq!(products.public_category_brands("categoria-prodotti", "").await.unwrap(), vec![("Marchio discendenti".into(), "marchio-discendenti".into()), ("Marchio prodotti".into(), "marchio-prodotti".into())]);
        assert!(products.public_brand_categories("marchio-discendenti").await.unwrap().iter().any(|c| c.1 == "categoria-prodotti"));
        assert!(products.public_search_categories().await.unwrap().iter().any(|c| c.1 == "livello-15"), "Empty parents are relevant filter choices");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (prefix, locale, count) in [("", "it", "25 prodotti"), ("/en", "en", "25 products")] {
            let path = format!("{prefix}/catalog/categoria-prodotti");
            let response = request(&app,"GET",&path,"","",false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], locale);
            let body = html(response).await;
            assert!(body.contains(count));
            assert!(body.contains("value=\"marchio-discendenti\""));
            assert!(body.contains(&format!("href=\"{prefix}/products/deep\"")));
            let second = html(request(&app,"GET",&format!("{path}?page=2"),"","",false).await).await;
            assert_eq!(second.matches("class=\"store-product\"").count(), 4);
            for (route, expected) in [(format!("{path}?brand=marchio-discendenti&available=true"),12), (format!("{prefix}/search/Descendant?category=categoria-prodotti&brand=marchio-discendenti&per_page=42"),22), (format!("{prefix}/brands/marchio-discendenti?category=categoria-prodotti&per_page=42"),23)] {
                let body = html(request(&app,"GET",&route,"","",false).await).await;
                assert_eq!(body.matches("class=\"store-product\"").count(), expected, "{route}");
            }
        }
        // With no directly assigned products the parent still has its whole subtree.
        sqlx::query("UPDATE catalog_products SET status='draft' WHERE slug='root'").execute(&pool).await.unwrap();
        assert_eq!(products.public_page("", "categoria-prodotti", "", false, 1, size).await.unwrap().total, 24);
        sqlx::query("UPDATE catalog_categories SET active=false WHERE id=$1").bind(root).execute(&pool).await.unwrap();
        assert_eq!(request(&app,"GET","/catalog/categoria-prodotti","","",false).await.status(), StatusCode::NOT_FOUND);
        assert_eq!(products.public_page("", "figlia", "", false, 1, size).await.unwrap().total, 23, "Ancestor visibility contract is preserved");
        let after: Vec<(i64, Option<i64>)> = sqlx::query_as("SELECT id,category_id FROM catalog_products ORDER BY id").fetch_all(&pool).await.unwrap();
        assert_eq!(before, after, "Read operations preserve every backend assignment");
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn category_page_identity_filters_visibility_and_navigation() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let (category, brand, class) = product_references(&pool, &session).await;
        sqlx::query("WITH updated AS (UPDATE catalog_categories SET name='<Categoria & idee>',description='<script>Test</script> & descrizione' WHERE id=$1 RETURNING id,name,description) UPDATE catalog_categories_translations t SET name=u.name,description=u.description FROM updated u WHERE t.entity_id=u.id AND t.locale='it'")
            .bind(category).execute(&pool).await.unwrap();
        let child: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,parent_id,active) VALUES ('Figlia','figlia',$1,true) RETURNING id")
            .bind(category).fetch_one(&pool).await.unwrap();
        let outside: i64 = sqlx::query_scalar("INSERT INTO catalog_brands(name,slug,active) VALUES ('Solo altrove','solo-altrove',true) RETURNING id")
            .fetch_one(&pool).await.unwrap();
        let draft_brand: i64 = sqlx::query_scalar("INSERT INTO catalog_brands(name,slug,active) VALUES ('Solo bozze','solo-bozze',true) RETURNING id")
            .fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock,minimum_quantity,packs) SELECT 'Categoria '||lpad(n::text,2,'0'),'CAT-'||n,'cat-'||n,'published',$1,$2,$3,100,CASE WHEN n%2=0 THEN 3 ELSE 5 END,3,2 FROM generate_series(1,30) n")
            .bind(category).bind(brand).bind(class).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,status,category_id,brand_id,tax_class_id,price_cents,stock) VALUES ('Prodotto figlia','CHILD','child','published',$1,$2,$3,100,1),('Bozza','DRAFT','draft','draft',$4,$5,$3,100,1),('Senza categoria','NONE','none','published',NULL,$2,$3,100,1)")
            .bind(child).bind(outside).bind(class).bind(category).bind(draft_brand).execute(&pool).await.unwrap();
        let products = talocart_commerce::products::Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        assert_eq!(products.public_category_brands("categoria-prodotti", "").await.unwrap(), vec![("Marchio prodotti".into(), "marchio-prodotti".into()), ("Solo altrove".into(), "solo-altrove".into())]);
        assert!(products.public_category_brands("bad--slug", "").await.is_err());
        assert!(products.public_category_brands("categoria-prodotti", "bad--slug").await.is_err());
        let catalog = talocart_commerce::Catalog::new(pool.clone(), identity, test_media(&pool).await);
        let info = catalog.public_category("categoria-prodotti").await.unwrap();
        assert_eq!(info.name, "<Categoria & idee>");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let body = html(request(&app, "GET", "/catalog/categoria-prodotti", "", "", false).await).await;
        assert!(body.contains("<h1>&#60;Categoria &#38; idee&#62;</h1>"));
        assert!(body.contains("<title>&#60;Categoria &#38; idee&#62; · Shop</title>"));
        assert!(body.contains("&#60;script&#62;Test&#60;/script&#62; &#38; descrizione"));
        assert!(!body.contains("<script>Test</script>"));
        assert!(body.contains("Filtri categoria"));
        assert_eq!(body.matches("class=\"store-product\"").count(), 21);
        assert!(!body.contains("href=\"/products/child\""), "Descendant sorts onto the second page");
        assert!(!body.contains("href=\"/products/none\""));
        assert!(body.contains("value=\"solo-altrove\""));
        assert!(!body.contains("value=\"solo-bozze\""));
        assert!(!body.contains(">Rimuovi filtri</a>"));
        let second = html(request(&app, "GET", "/catalog/categoria-prodotti?page=2", "", "", false).await).await;
        assert_eq!(second.matches("class=\"store-product\"").count(), 10);
        assert!(second.contains("href=\"/products/child\""));
        let filtered = html(request(&app, "GET", "/catalog/categoria-prodotti?brand=marchio-prodotti&available=true&per_page=42", "", "", false).await).await;
        assert_eq!(filtered.matches("class=\"store-product\"").count(), 15, "Availability uses minimum and packs, not stock>0");
        assert!(filtered.contains("href=\"/catalog/categoria-prodotti?per_page=42\">Rimuovi filtri"));
        assert!(filtered.contains("rel=\"canonical\" href=\"http://127.0.0.1:3000/catalog/categoria-prodotti\""));
        let empty = html(request(&app, "GET", "/catalog/categoria-prodotti?brand=missing-brand", "", "", false).await).await;
        assert!(!empty.contains("class=\"store-product\""));
        assert!(empty.contains("value=\"missing-brand\" selected>Marchio non presente"));
        assert!(empty.contains("Prova a modificare o rimuovere i filtri di questa categoria."));
        sqlx::query("INSERT INTO catalog_categories(name,slug,active) VALUES ('Vuota','vuota',true)").execute(&pool).await.unwrap();
        let empty = html(request(&app, "GET", "/catalog/vuota", "", "", false).await).await;
        assert!(empty.contains("<h1>Vuota</h1>"));
        assert!(empty.contains("Non ci sono ancora prodotti disponibili in questa categoria."));
        assert!(!empty.contains("category-description"));
        for path in ["/catalog/missing", "/catalog/missing?brand=marchio-prodotti"] {
            assert_eq!(request(&app, "GET", path, "", "", false).await.status(), StatusCode::NOT_FOUND);
        }
        assert_eq!(request(&app, "GET", "/catalog/bad--slug", "", "", false).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        sqlx::query("UPDATE catalog_categories SET active=false WHERE id=$1").bind(category).execute(&pool).await.unwrap();
        assert_eq!(request(&app, "GET", "/catalog/categoria-prodotti", "", "", false).await.status(), StatusCode::NOT_FOUND);
        let child_page = html(request(&app, "GET", "/catalog/figlia", "", "", false).await).await;
        assert!(child_page.contains("href=\"/products/child\""), "Inactive ancestors only hide menu branches, preserving direct visibility");
        sqlx::query("UPDATE catalog_brands SET active=false WHERE id=$1").bind(outside).execute(&pool).await.unwrap();
        assert!(products.public_category_brands("figlia", "").await.unwrap().is_empty());
        let general = html(request(&app, "GET", "/catalog", "", "", false).await).await;
        assert!(general.contains("<h1>Catalogo</h1>"));
    }).await;
}
