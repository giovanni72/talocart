use super::*;
use talocart_commerce::cart::{CartError, Carts, Issue};
use talocart_platform::{content::Language, locale::Locale};
fn language() -> Language {
    Language::new(Locale::parse("en").unwrap(), Locale::parse("en").unwrap())
}
async fn fixture(pool: &PgPool) -> i64 {
    let identity = installed(pool).await;
    let session = taxonomy_session(&identity).await;
    talocart_platform::storefront::Storefront::new(pool.clone())
        .save_languages(&session, &session.csrf, "en", &["it".into(), "en".into()])
        .await
        .unwrap();
    let rate: i64 = sqlx::query_scalar(
        "INSERT INTO tax_rates(name,basis_points) VALUES('Synthetic VAT',5000) RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let class: i64 = sqlx::query_scalar(
        "INSERT INTO tax_classes(name,rate_id) VALUES('Synthetic class',$1) RETURNING id",
    )
    .bind(rate)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE pricing_settings SET price_mode='exclusive'")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,minimum_quantity,packs,status,tax_class_id) VALUES('Synthetic item','CART-1','cart-item',1,100,2,3,'published',$1) RETURNING id").bind(class).fetch_one(pool).await.unwrap()
}
async fn line_id(pool: &PgPool, cart: i64) -> i64 {
    sqlx::query_scalar("SELECT id FROM guest_cart_items WHERE cart_id=$1 ORDER BY id LIMIT 1")
        .bind(cart)
        .fetch_one(pool)
        .await
        .unwrap()
}
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_persistence_line_rounding_merge_and_no_reservation() {
    isolated(|pool, options| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (raw, g) = carts.create().await.unwrap();
        carts
            .add(&g, &g.csrf, 0, (product, 0), 3, language())
            .await
            .unwrap();
        let quote = carts.quote(&g, language()).await.unwrap();
        assert_eq!(quote.revision, 1);
        let total = quote.totals.unwrap();
        assert_eq!(
            (
                total.net().cents(),
                total.tax().cents(),
                total.gross().cents()
            ),
            (3, 2, 5)
        );
        carts
            .add(&g, &g.csrf, 1, (product, 0), 3, language())
            .await
            .unwrap();
        let restarted = PgPoolOptions::new().connect_with(options).await.unwrap();
        let service = Carts::new(restarted.clone());
        let session = service.session(&raw).await.unwrap().unwrap();
        assert_eq!(session.count, 6);
        assert_eq!(session.revision, 2);
        let quote = service.quote(&session, language()).await.unwrap();
        assert_eq!(quote.lines.len(), 1);
        assert_eq!(quote.totals.unwrap().gross().cents(), 9);
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1")
                .bind(product)
                .fetch_one(&pool)
                .await
                .unwrap(),
            100
        );
        assert_ne!(
            sqlx::query_scalar::<_, String>("SELECT token_hash FROM guest_carts WHERE id=$1")
                .bind(g.id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            raw
        );
        restarted.close().await;
    })
    .await;
}
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_invalid_quantity_stock_csrf_and_cross_cart_ownership_rollback() {
    isolated(|pool, _| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (_, g) = carts.create().await.unwrap();
        let (_, other) = carts.create().await.unwrap();
        for quantity in [0, -1, 1, 2, 4, 1000000] {
            assert!(matches!(
                carts
                    .add(&g, &g.csrf, 0, (product, 0), quantity, language())
                    .await,
                Err(CartError::Quantity)
            ));
        }
        assert!(matches!(
            carts
                .add(&g, &g.csrf, 0, (product, 0), 102, language())
                .await,
            Err(CartError::Stock)
        ));
        assert!(matches!(
            carts.add(&g, "wrong", 0, (product, 0), 3, language()).await,
            Err(CartError::Forbidden)
        ));
        assert!(matches!(
            carts
                .add(&g, &g.csrf, 0, (product, 123), 3, language())
                .await,
            Err(CartError::Unavailable)
        ));
        assert_eq!(carts.quote(&g, language()).await.unwrap().revision, 0);
        carts
            .add(&g, &g.csrf, 0, (product, 0), 3, language())
            .await
            .unwrap();
        let id = line_id(&pool, g.id).await;
        assert!(matches!(
            carts
                .update(&other, &other.csrf, 0, id, None, language())
                .await,
            Err(CartError::Unavailable)
        ));
        assert_eq!(
            carts.quote(&g, language()).await.unwrap().lines[0].quantity,
            3
        );
        carts
            .update(&g, &g.csrf, 1, id, Some(6), language())
            .await
            .unwrap();
        assert!(matches!(
            carts.update(&g, &g.csrf, 2, id, Some(4), language()).await,
            Err(CartError::Quantity)
        ));
        assert_eq!(
            carts.quote(&g, language()).await.unwrap().lines[0].quantity,
            6
        );
        carts
            .update(&g, &g.csrf, 2, id, None, language())
            .await
            .unwrap();
        assert!(carts.quote(&g, language()).await.unwrap().lines.is_empty());
    })
    .await;
}
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_concurrent_and_duplicate_submissions_never_double_quantity() {
    isolated(|pool, _| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (_, g) = carts.create().await.unwrap();
        let (a, b) = tokio::join!(
            carts.add(&g, &g.csrf, 0, (product, 0), 3, language()),
            carts.add(&g, &g.csrf, 0, (product, 0), 3, language())
        );
        assert_ne!(a.is_ok(), b.is_ok());
        assert!(matches!(a, Err(CartError::Stale)) || matches!(b, Err(CartError::Stale)));
        assert!(matches!(
            carts.add(&g, &g.csrf, 0, (product, 0), 3, language()).await,
            Err(CartError::Stale)
        ));
        let quote = carts.quote(&g, language()).await.unwrap();
        assert_eq!(quote.lines.len(), 1);
        assert_eq!(quote.lines[0].quantity, 3);
    })
    .await;
}
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_recalculates_prices_tax_stock_rules_and_preserves_deleted_items() {
    isolated(|pool, _| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (_, g) = carts.create().await.unwrap();
        carts
            .add(&g, &g.csrf, 0, (product, 0), 3, language())
            .await
            .unwrap();
        sqlx::query("UPDATE catalog_products SET price_cents=100 WHERE id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tax_rates SET basis_points=2000 WHERE name='Synthetic VAT'")
            .execute(&pool)
            .await
            .unwrap();
        let quote = carts.quote(&g, language()).await.unwrap();
        assert!(quote.lines[0].changed);
        assert_eq!(quote.totals.unwrap().gross().cents(), 360);
        carts
            .update(&g, &g.csrf, 1, quote.lines[0].id, Some(3), language())
            .await
            .unwrap();
        assert!(!carts.quote(&g, language()).await.unwrap().lines[0].changed);
        sqlx::query("UPDATE catalog_products SET stock=2 WHERE id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        let quote = carts.quote(&g, language()).await.unwrap();
        assert!(matches!(quote.lines[0].issue, Some(Issue::Stock)));
        assert!(quote.totals.is_none());
        sqlx::query("UPDATE catalog_products SET stock=100,packs=2 WHERE id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            carts.quote(&g, language()).await.unwrap().lines[0].issue,
            Some(Issue::Quantity)
        ));
        sqlx::query("UPDATE catalog_products SET status='draft' WHERE id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            carts.quote(&g, language()).await.unwrap().lines[0].issue,
            Some(Issue::Unavailable)
        ));
        sqlx::query("DELETE FROM catalog_products WHERE id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        let quote = carts.quote(&g, language()).await.unwrap();
        assert_eq!(quote.lines[0].name, "Synthetic item");
        assert!(quote.lines[0].current.is_none());
        carts
            .update(&g, &g.csrf, 2, quote.lines[0].id, None, language())
            .await
            .unwrap();
    })
    .await;
}
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_expiry_rotation_and_bounded_cleanup() {
    isolated(|pool,_|async move {
        let product=fixture(&pool).await;let carts=Carts::new(pool.clone());let (raw,g)=carts.create().await.unwrap();
        carts.add(&g,&g.csrf,0,(product,0),3,language()).await.unwrap();
        sqlx::query("UPDATE guest_carts SET expires_at=now()-interval '1 second' WHERE id=$1").bind(g.id).execute(&pool).await.unwrap();
        assert!(carts.session(&raw).await.unwrap().is_none());assert!(carts.session("bad").await.unwrap().is_none());
        assert!(matches!(carts.quote(&g,language()).await,Err(CartError::Forbidden)));
        assert!(matches!(carts.update(&g,&g.csrf,1,line_id(&pool,g.id).await,None,language()).await,Err(CartError::Forbidden)));
        let (_,fresh)=carts.create().await.unwrap();assert_ne!(g.csrf,fresh.csrf);assert_eq!(fresh.count,0);
        sqlx::query("INSERT INTO guest_carts(token_hash,csrf,expires_at) SELECT lpad(to_hex(i),64,'0'),repeat('a',64),now()-interval '1 day' FROM generate_series(1,105) i").execute(&pool).await.unwrap();
        assert_eq!(carts.prune().await.unwrap().deleted,106);assert_eq!(carts.prune().await.unwrap().deleted,0);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM guest_cart_items").fetch_one(&pool).await.unwrap(),0);
    }).await;
}
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_http_bilingual_cookie_isolation_errors_and_untrusted_prices() {
    isolated(|pool,_|async move {
        let product=fixture(&pool).await;let app=router(test_app_config(pool.clone(),"Shop".into(), talocart_web::AppStateConfig { secure_cookies: true, ..Default::default() }).await);
        let response=request(&app,"GET","/products/cart-item","","",false).await;assert_eq!(response.status(),StatusCode::OK);
        let cookie=response.headers()[header::SET_COOKIE].to_str().unwrap().to_owned();assert!(cookie.contains("HttpOnly"));assert!(cookie.contains("SameSite=Lax"));assert!(cookie.contains("Secure"));assert!(cookie.contains("Max-Age=2592000"));assert!(cookie.starts_with("talocart_cart="));
        let body=html(response).await;assert!(body.contains("Add to cart"));let csrf=body.split("name=\"csrf\" value=\"").nth(1).unwrap().split('"').next().unwrap();let cookie=cookie.split(';').next().unwrap();
        let sent=format!("csrf={csrf}&revision=0&product_id={product}&variant_id=0&quantity=3&price_cents=1&basis_points=0");
        let response=request(&app,"POST","/cart/add",cookie,&sent,false).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);
        for (path,title,total) in [("/cart","Cart","0.05"),("/it/cart","Carrello","0,05")] {
            let response=request(&app,"GET",path,cookie,"",false).await;assert_eq!(response.status(),StatusCode::OK);assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");let body=html(response).await;let header_html=body.split("</header>").next().unwrap();assert!(!header_html.contains("href=\"/admin\""));assert!(!header_html.contains("href=\"/catalog\""));assert!(!header_html.contains("href=\"/it/catalog\""));assert!(body.contains(title));assert!(body.contains(total));assert!(body.contains("data-cart-count>3"));
        }
        assert_eq!(request(&app,"POST","/cart/add",cookie,&sent,true).await.status(),StatusCode::CONFLICT);
        let response=request(&app,"POST","/it/cart/add",cookie,&sent,true).await;assert!(html(response).await.contains("Il carrello è cambiato"));
        let other=request(&app,"GET","/cart","","",false).await;assert!(html(other).await.contains("Your cart is empty"));
        assert_eq!(request(&app,"POST","/cart/add","",&sent,false).await.status(),StatusCode::FORBIDDEN);
        assert_eq!(request(&app,"POST","/cart/add",cookie,"csrf=wrong&revision=1",false).await.status(),StatusCode::FORBIDDEN);
        assert_eq!(request(&app,"GET","/cart/add",cookie,"",false).await.status(),StatusCode::METHOD_NOT_ALLOWED);
        let id=line_id(&pool,1).await;let response=request(&app,"POST",&format!("/it/cart/items/{id}"),cookie,&format!("csrf={csrf}&revision=1&action=update&quantity=4"),false).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let body=html(response).await;assert!(body.contains("value=\"4\""));assert!(body.contains("aria-invalid=\"true\""));assert!(body.split("</header>").next().unwrap().contains("href=\"/cart\""),"Language switch on an error must target the readable cart page");
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_variant_membership_localization_and_deleted_selection() {
    isolated(|pool,_|async move {
        let simple=fixture(&pool).await;
        let class:i64=sqlx::query_scalar("SELECT tax_class_id FROM catalog_products WHERE id=$1").bind(simple).fetch_one(&pool).await.unwrap();
        let product:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id,minimum_quantity,packs) VALUES('Variant item','CART-PARENT','cart-variant',100,999,'published',$1,2,3) RETURNING id").bind(class).fetch_one(&pool).await.unwrap();
        let option:i64=sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id").fetch_one(&pool).await.unwrap();
        let value:i64=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Large') RETURNING id").bind(option).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)").bind(product).bind(option).execute(&pool).await.unwrap();
        let variant:i64=sqlx::query_scalar("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],'CART-VARIANT',200,6) RETURNING id").bind(product).bind(value).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products_translations(entity_id,locale,name) VALUES($1,'it','Articolo variante')").bind(product).execute(&pool).await.unwrap();
        let carts=Carts::new(pool.clone());let (_,g)=carts.create().await.unwrap();
        assert!(matches!(carts.add(&g,&g.csrf,0,(product,0),3,language()).await,Err(CartError::Unavailable)));
        assert!(matches!(carts.add(&g,&g.csrf,0,(simple,variant),3,language()).await,Err(CartError::Unavailable)));
        assert!(matches!(carts.add(&g,&g.csrf,0,(product,variant),9,language()).await,Err(CartError::Stock)));
        carts.add(&g,&g.csrf,0,(product,variant),3,language()).await.unwrap();
        let it=Language::new(Locale::parse("it").unwrap(),Locale::parse("en").unwrap());
        let quote=carts.quote(&g,it).await.unwrap();assert_eq!(quote.lines[0].name,"Articolo variante");assert_eq!(quote.lines[0].code,"CART-VARIANT");assert_eq!(quote.totals.unwrap().gross().cents(),900);
        assert_eq!(quote.lines[0].current.as_ref().unwrap().label,"Size: Large");
        assert_eq!(quote.lines[0].current.as_ref().unwrap().image_id,None);
        let product_key=format!("{}.png","a".repeat(64));
        let variant_key=format!("{}.png","b".repeat(64));
        let product_image:i64=sqlx::query_scalar("INSERT INTO media_objects(key,content_type,byte_size,width,height,public) VALUES($1,'image/png',1,64,64,true) RETURNING id").bind(&product_key).fetch_one(&pool).await.unwrap();
        let variant_image:i64=sqlx::query_scalar("INSERT INTO media_objects(key,content_type,byte_size,width,height,public) VALUES($1,'image/png',1,64,64,true) RETURNING id").bind(&variant_key).fetch_one(&pool).await.unwrap();
        sqlx::query("UPDATE catalog_products SET image_key=$1 WHERE id=$2").bind(&product_key).bind(product).execute(&pool).await.unwrap();
        assert_eq!(carts.quote(&g,it).await.unwrap().lines[0].current.as_ref().unwrap().image_id,Some(product_image));
        sqlx::query("UPDATE catalog_variants SET image_key=$1 WHERE id=$2").bind(&variant_key).bind(variant).execute(&pool).await.unwrap();
        assert_eq!(carts.quote(&g,it).await.unwrap().lines[0].current.as_ref().unwrap().image_id,Some(variant_image));
        sqlx::query("UPDATE catalog_variants SET image_key=NULL WHERE id=$1").bind(variant).execute(&pool).await.unwrap();
        assert_eq!(carts.quote(&g,it).await.unwrap().lines[0].current.as_ref().unwrap().image_id,Some(product_image));
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let response=request(&app,"GET",&format!("/products/cart-variant?value_1={value}"),"","",false).await;
        assert_eq!(response.status(),StatusCode::OK);let body=html(response).await;assert!(body.contains(&format!("name=\"variant_id\" value=\"{variant}\"")));
        sqlx::query("UPDATE catalog_variants SET active=false WHERE id=$1").bind(variant).execute(&pool).await.unwrap();
        assert!(matches!(carts.quote(&g,language()).await.unwrap().lines[0].issue,Some(Issue::Unavailable)));
        sqlx::query("DELETE FROM catalog_variants WHERE id=$1").bind(variant).execute(&pool).await.unwrap();
        assert!(carts.quote(&g,language()).await.unwrap().lines[0].current.is_none());
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_aggregate_overflow_keeps_lines_readable_and_removable() {
    isolated(|pool,_|async move {
        let product=fixture(&pool).await;
        let tax:i64=sqlx::query_scalar("SELECT tax_class_id FROM catalog_products WHERE id=$1").bind(product).fetch_one(&pool).await.unwrap();
        let carts=Carts::new(pool.clone());let (raw,g)=carts.create().await.unwrap();
        for n in 0..7 {
            let id:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Large amount',$1,$2,999999999999,999999,'published',$3) RETURNING id").bind(format!("LARGE-{n}")).bind(format!("large-{n}")).bind(tax).fetch_one(&pool).await.unwrap();
            carts.add(&g,&g.csrf,n,(id,0),999999,language()).await.unwrap();
        }
        let result=carts.quote(&g,language()).await;
        assert!(result.is_ok(),"Overflow must preserve a readable, removable cart");
        let quote=result.unwrap();assert!(quote.totals.is_none());assert_eq!(quote.lines.len(),7);
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let response=request(&app,"GET","/cart",&format!("talocart_cart={raw}"),"",false).await;
        assert_eq!(response.status(),StatusCode::OK);assert!(html(response).await.contains("Total is too large"));
        carts.update(&g,&g.csrf,7,quote.lines[0].id,None,language()).await.unwrap();
        assert!(carts.quote(&g,language()).await.unwrap().totals.is_some());
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_minimum_applies_to_merged_line_not_each_increment() {
    isolated(|pool, _| async move {
        let product = fixture(&pool).await;
        sqlx::query("UPDATE catalog_products SET minimum_quantity=5 WHERE id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        let carts = Carts::new(pool.clone());
        let (_, g) = carts.create().await.unwrap();
        assert!(matches!(
            carts.add(&g, &g.csrf, 0, (product, 0), 3, language()).await,
            Err(CartError::Quantity)
        ));
        carts
            .add(&g, &g.csrf, 0, (product, 0), 6, language())
            .await
            .unwrap();
        carts
            .add(&g, &g.csrf, 1, (product, 0), 3, language())
            .await
            .expect("The final nine pieces meet minimum and packs");
        assert_eq!(
            carts.quote(&g, language()).await.unwrap().lines[0].quantity,
            9
        );
        for quantity in [0, -3, 1, 1000000] {
            assert!(matches!(
                carts
                    .add(&g, &g.csrf, 2, (product, 0), quantity, language())
                    .await,
                Err(CartError::Quantity)
            ));
        }
        assert_eq!(carts.quote(&g, language()).await.unwrap().revision, 2);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_missing_products_do_not_create_sessions() {
    isolated(|pool, _| async move {
        fixture(&pool).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for path in [
            "/products/missing-cart-product",
            "/it/products/missing-cart-product",
        ] {
            let response = request(&app, "GET", path, "", "", false).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert!(!response.headers().contains_key(header::SET_COOKIE));
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_carts")
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
async fn cart_head_is_read_only_and_matches_get_status() {
    isolated(|pool, _| async move {
        fixture(&pool).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for path in [
            "/cart",
            "/it/cart",
            "/products/cart-item",
            "/it/products/cart-item",
        ] {
            let response = request(&app, "HEAD", path, "", "", false).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert!(!response.headers().contains_key(header::SET_COOKIE));
            assert!(html(response).await.is_empty());
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_carts")
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
async fn cart_stale_forms_require_reload_without_resubmission_controls() {
    isolated(|pool, _| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (raw, g) = carts.create().await.unwrap();
        carts
            .add(&g, &g.csrf, 0, (product, 0), 3, language())
            .await
            .unwrap();
        let id = line_id(&pool, g.id).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for prefix in ["", "/it"] {
            for (path, fields) in [
                (
                    format!("{prefix}/cart/add"),
                    format!("product_id={product}&quantity=3"),
                ),
                (
                    format!("{prefix}/cart/items/{id}"),
                    "action=update&quantity=6".into(),
                ),
                (
                    format!("{prefix}/cart/items/{id}"),
                    "action=remove&quantity=3".into(),
                ),
                (
                    format!("{prefix}/cart/coupon"),
                    "action=remove&code=".into(),
                ),
            ] {
                let response = request(
                    &app,
                    "POST",
                    &path,
                    &format!("talocart_cart={raw}"),
                    &format!("csrf={}&revision=0&{fields}", g.csrf),
                    false,
                )
                .await;
                assert_eq!(response.status(), StatusCode::CONFLICT);
                let body = html(response).await;
                assert!(
                    !body.contains("method=\"post\""),
                    "Stale page must offer reload instead of resubmission"
                );
                assert!(body.contains(&format!("href=\"{prefix}/cart\"")));
            }
        }
        assert_eq!(
            carts.quote(&g, language()).await.unwrap().lines[0].quantity,
            3
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_cleanup_skips_locked_sessions_and_bounds_backlog_work() {
    isolated(|pool, _| async move {
        let carts = Carts::new(pool.clone());
        let (_, active) = carts.create().await.unwrap();
        sqlx::query("INSERT INTO guest_carts(token_hash,csrf,expires_at) SELECT lpad(to_hex(i),64,'0'),repeat('a',64),now()-interval '1 day' FROM generate_series(1,2105) i").execute(&pool).await.unwrap();
        let mut locked = pool.begin().await.unwrap();
        let locked_id: i64 = sqlx::query_scalar("SELECT id FROM guest_carts WHERE expires_at<now() ORDER BY id LIMIT 1 FOR UPDATE").fetch_one(&mut *locked).await.unwrap();
        let report = carts.prune().await.unwrap();
        assert!(report.deleted > 100 && report.deleted <= 2000);
        assert!(report.remaining_at_least > 0 && report.remaining_at_least <= 1000);
        // Finish unlocked backlog; the locked row must survive without blocking.
        let report = carts.prune().await.unwrap();
        assert_eq!(report.remaining_at_least, 1);
        assert!(sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM guest_carts WHERE id=$1)").bind(locked_id).fetch_one(&pool).await.unwrap());
        locked.rollback().await.unwrap();
        let report = carts.prune().await.unwrap();
        assert_eq!(report.deleted, 1);
        assert_eq!(report.remaining_at_least, 0);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT id FROM guest_carts").fetch_one(&pool).await.unwrap(), active.id);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_minicart_removes_one_line_with_csrf_revision_and_page_fallback() {
    isolated(|pool, _| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (raw, g) = carts.create().await.unwrap();
        carts
            .add(&g, &g.csrf, 0, (product, 0), 3, language())
            .await
            .unwrap();
        let quote = carts.quote(&g, language()).await.unwrap();
        let line = quote.lines[0].id;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_cart={raw}");
        let path = format!("/it/cart/items/{line}");
        let form = |csrf: &str, revision: i64| {
            format!("csrf={csrf}&revision={revision}&action=remove&view=minicart")
        };
        // A stale revision keeps the line and explains the conflict inside the dialog.
        let response = request(&app, "POST", &path, &cookie, &form(&g.csrf, 0), true).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = html(response).await;
        assert!(!body.contains("<html"));
        assert!(body.contains("id=\"minicart-notice\""), "{body}");
        assert!(body.contains("data-minicart-count=\"3\""));
        assert!(body.contains(&format!("name=\"revision\" value=\"{}\"", quote.revision)));
        let response = request(
            &app,
            "POST",
            &path,
            &cookie,
            &form("forged", quote.revision),
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(carts.quote(&g, language()).await.unwrap().lines.len(), 1);
        let response = request(
            &app,
            "POST",
            &path,
            &cookie,
            &form(&g.csrf, quote.revision),
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = html(response).await;
        assert!(body.contains("data-minicart-count=\"0\""));
        assert!(body.contains("Articolo rimosso dal carrello."));
        assert!(!body.contains("id=\"minicart-notice\""));
        let quote = carts.quote(&g, language()).await.unwrap();
        assert!(quote.lines.is_empty());
        // Without HTMX the same form keeps the full cart page flow.
        carts
            .add(&g, &g.csrf, quote.revision, (product, 0), 3, language())
            .await
            .unwrap();
        let quote = carts.quote(&g, language()).await.unwrap();
        let path = format!("/cart/items/{}", quote.lines[0].id);
        let response = request(
            &app,
            "POST",
            &path,
            &cookie,
            &form(&g.csrf, quote.revision),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/cart?saved=1");
        assert!(carts.quote(&g, language()).await.unwrap().lines.is_empty());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn cart_minicart_preview_is_localized_isolated_and_recalculates() {
    isolated(|pool, _| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (raw, g) = carts.create().await.unwrap();
        carts
            .add(&g, &g.csrf, 0, (product, 0), 3, language())
            .await
            .unwrap();
        let line = carts.quote(&g, language()).await.unwrap().lines[0].id;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (prefix, empty, quantity, remove) in [
            ("", "Your cart is empty", "Quantity", "Remove"),
            ("/it", "Il carrello è vuoto", "Quantità", "Rimuovi"),
        ] {
            let path = format!("{prefix}/cart?preview=1");
            let response = request(&app, "GET", &path, "", "", true).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!response.headers().contains_key(header::SET_COOKIE));
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let body = html(response).await;
            assert!(body.contains(empty), "{body}");
            assert!(!body.contains("<html"));
            let response = request(
                &app,
                "GET",
                &path,
                &format!("talocart_cart={raw}"),
                "",
                true,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = html(response).await;
            assert!(body.contains("data-minicart-count=\"3\""));
            assert!(body.contains(quantity));
            assert!(body.contains("minicart-thumbnail"));
            for marker in [
                "data-minicart-net",
                "data-minicart-tax",
                "data-minicart-total",
            ] {
                assert!(
                    body.contains(marker),
                    "All three tax breakdown amounts are present"
                );
            }
            assert!(
                !body.contains("<img"),
                "Missing product image uses the accessible placeholder"
            );
            assert!(!body.contains("Le imposte sono stimate"));
            assert!(!body.contains("Taxes are estimated"));
            assert!(!body.contains("non prenotano le scorte"));
            assert!(!body.contains("do not reserve stock"));
            assert!(body.contains("Synthetic item"));
            // The only dialog form removes one line through the shared cart endpoint.
            assert_eq!(body.matches("method=\"post\"").count(), 1);
            assert!(body.contains(&format!(
                "action=\"{prefix}/cart/items/{line}\" hx-post=\"{prefix}/cart/items/{line}\""
            )));
            assert!(body.contains(&format!("name=\"csrf\" value=\"{}\"", g.csrf)));
            assert!(body.contains("name=\"view\" value=\"minicart\""));
            assert!(body.contains(&format!("{remove} Synthetic item")));
            let response = request(&app, "HEAD", &path, "", "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(html(response).await.is_empty());
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_carts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        assert_eq!(carts.quote(&g, language()).await.unwrap().revision, 1);
        sqlx::query("UPDATE catalog_products SET stock=1,price_cents=200 WHERE id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        let body = html(
            request(
                &app,
                "GET",
                "/cart?preview=1",
                &format!("talocart_cart={raw}"),
                "",
                true,
            )
            .await,
        )
        .await;
        assert!(body.contains("minicart-issue-"));
        assert!(body.contains("minicart-changed-"));
        assert!(!body.contains("data-minicart-total"));
        sqlx::query("UPDATE guest_carts SET expires_at=now()-interval '1 second' WHERE id=$1")
            .bind(g.id)
            .execute(&pool)
            .await
            .unwrap();
        let response = request(
            &app,
            "GET",
            "/cart?preview=1",
            &format!("talocart_cart={raw}"),
            "",
            true,
        )
        .await;
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert!(html(response).await.contains("data-minicart-count=\"0\""));
    })
    .await;
}
