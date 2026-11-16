use super::*;
use talocart_commerce::cart::{CartError, Issue};

async fn quantity(pool: &PgPool, id: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT coalesce(sum(quantity),0)::bigint FROM guest_cart_items WHERE cart_id=$1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}
async fn merge(carts: &Carts, c: &CustomerSession, g: &Guest) -> Result<(), CartError> {
    carts
        .merge_customer(c, &c.csrf, g, &g.csrf, g.revision)
        .await
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_carts_merge_replay_concurrency_prices_stock_and_restart() {
    isolated(|pool, options| async move {
        let (p, identity, _) = configured(&pool).await;
        let customers = Customers::new(pool.clone(), identity);
        let (_, c) = customer(&customers, "cart@example.test").await;
        let carts = Carts::new(pool.clone());
        let target = carts.ensure_customer(&c).await.unwrap();
        carts
            .add(&target, &target.csrf, 0, (p, 0), 2, lang())
            .await
            .unwrap();
        let (raw, source, _) = prepare(&carts, (p, 0), 3, 1).await;
        sqlx::query("UPDATE catalog_products SET price_cents=1200,stock=4 WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        let (a, b) = tokio::join!(merge(&carts, &c, &source), merge(&carts, &c, &source));
        a.unwrap();
        b.unwrap();
        assert_eq!(quantity(&pool, source.id).await, 0);
        assert_eq!(quantity(&pool, target.id).await, 5);
        let quote = carts.quote(&target, lang()).await.unwrap();
        assert_eq!(quote.lines[0].current.as_ref().unwrap().unit_cents, 1200);
        assert!(quote.lines[0].changed);
        assert!(matches!(quote.lines[0].issue, Some(Issue::Stock)));
        assert!(quote.totals.is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1")
                .bind(p)
                .fetch_one(&pool)
                .await
                .unwrap(),
            4
        );
        assert_eq!(count(&pool, "stock").await, 0);
        let reopened = PgPoolOptions::new().connect_with(options).await.unwrap();
        let persisted = Carts::new(reopened.clone())
            .customer_cart(&c)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((persisted.id, persisted.count), (target.id, 5));
        assert_eq!(carts.session(&raw).await.unwrap().unwrap().count, 0);
        reopened.close().await;
        // A new guest edit is not included by a stale replay.
        let fresh = carts.session(&raw).await.unwrap().unwrap();
        carts
            .add(&fresh, &fresh.csrf, fresh.revision, (p, 0), 1, lang())
            .await
            .unwrap();
        assert!(matches!(
            merge(&carts, &c, &source).await,
            Err(CartError::Stale)
        ));
        assert_eq!(quantity(&pool, target.id).await, 5);
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_carts_limits_are_lossless_and_retry_after_correction() {
    isolated(|pool,_|async move {
        let(p,identity,_)=configured(&pool).await;
        let customers=Customers::new(pool.clone(),identity);
        let(_,c)=customer(&customers,"limit@example.test").await;
        let carts=Carts::new(pool.clone());let target=carts.ensure_customer(&c).await.unwrap();
        let(_,source,_)=prepare(&carts,(p,0),1,1).await;
        // Synthetic deleted observations exercise the representation limit without catalog noise.
        sqlx::query("INSERT INTO guest_cart_items(cart_id,item_key,had_variant,quantity,name,code,unit_cents,basis_points,price_mode,currency) SELECT $1,'deleted:'||n,false,1,'Deleted','',1,0,'exclusive','EUR' FROM generate_series(1,100) n")
            .bind(target.id).execute(&pool).await.unwrap();
        assert!(matches!(merge(&carts,&c,&source).await,Err(CartError::Limit)));
        assert_eq!(quantity(&pool,source.id).await,1);assert_eq!(quantity(&pool,target.id).await,100);
        let id:i64=sqlx::query_scalar("SELECT id FROM guest_cart_items WHERE cart_id=$1 ORDER BY id LIMIT 1").bind(target.id).fetch_one(&pool).await.unwrap();
        carts.update(&target,&target.csrf,0,id,None,lang()).await.unwrap();
        merge(&carts,&c,&source).await.unwrap();
        assert_eq!(quantity(&pool,source.id).await,0);assert_eq!(quantity(&pool,target.id).await,100);
        // Same-key merge at exactly 100 lines must not trip the insert limit trigger.
        let(_,another,_)=prepare(&carts,(p,0),1,1).await;
        merge(&carts,&c,&another).await.unwrap();assert_eq!(quantity(&pool,target.id).await,101);
        sqlx::query("UPDATE guest_cart_items SET quantity=999999 WHERE cart_id=$1 AND product_id=$2").bind(target.id).bind(p).execute(&pool).await.unwrap();
        let(_,overflow,_)=prepare(&carts,(p,0),1,1).await;
        assert!(matches!(merge(&carts,&c,&overflow).await,Err(CartError::Quantity)));
        assert_eq!(quantity(&pool,overflow.id).await,1);
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_carts_revocation_cookie_isolation_and_order_ownership() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let customers = Customers::new(pool.clone(), identity.clone());
        let (_, c) = customer(&customers, "owner@example.test").await;
        let (_, other) = customer(&customers, "other@example.test").await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let target = carts.ensure_customer(&c).await.unwrap();
        let addresses = CustomerAddresses::new(pool.clone());
        let id = addresses
            .save(&other, &other.csrf, None, &address())
            .await
            .unwrap();
        assert!(
            addresses
                .use_in_checkout(
                    &other,
                    &other.csrf,
                    id,
                    (&target, &target.csrf, target.revision)
                )
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE guest_carts SET expires_at=now() WHERE id=$1")
                .bind(target.id)
                .execute(&pool)
                .await
                .is_err()
        );
        assert_ne!(target.id, carts.ensure_customer(&other).await.unwrap().id);
        let (raw, source, review) = prepare(&carts, (p, 0), 1, 1).await;
        let old = confirm(&orders, &source, &review, 1).await.unwrap();
        let source = carts.session(&raw).await.unwrap().unwrap();
        carts
            .add(&source, &source.csrf, source.revision, (p, 0), 2, lang())
            .await
            .unwrap();
        let source = carts.session(&raw).await.unwrap().unwrap();
        assert!(matches!(
            carts
                .merge_customer(&c, "wrong", &source, &source.csrf, source.revision)
                .await,
            Err(CartError::Forbidden)
        ));
        assert!(matches!(
            carts
                .merge_customer(&c, &c.csrf, &source, "wrong", source.revision)
                .await,
            Err(CartError::Forbidden)
        ));
        merge(&carts, &c, &source).await.unwrap();
        assert_eq!(
            orders.for_guest(&source, &old.reference).await.unwrap().id,
            old.id
        );
        assert!(matches!(
            orders.for_customer(&c, &old.reference).await,
            Err(OrderError::NotFound)
        ));
        let target = carts.customer_cart(&c).await.unwrap().unwrap();
        let quote = carts
            .checkout_quote(&target, lang(), "IT", Some(selection(1)))
            .await
            .unwrap();
        assert!(matches!(
            orders.confirm(input(&target, &quote.review)).await,
            Err(OrderError::Cart(CartError::Forbidden))
        ));
        assert!(
            orders
                .confirm_for_customer(input(&target, &quote.review), &other)
                .await
                .is_err()
        );
        let order = orders
            .confirm_for_customer(input(&target, &quote.review), &c)
            .await
            .unwrap();
        assert_eq!(
            orders.for_customer(&c, &order.reference).await.unwrap().id,
            order.id
        );
        // Even knowledge of a synthetic customer-cart raw token cannot create a guest capability.
        let raw = talocart_platform::identity::token().unwrap();
        sqlx::query("UPDATE guest_carts SET token_hash=$1 WHERE id=$2")
            .bind(talocart_platform::identity::digest(&raw))
            .bind(carts.ensure_customer(&other).await.unwrap().id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(carts.session(&raw).await.unwrap().is_none());
        customers.logout(&c, &c.csrf).await.unwrap();
        assert!(carts.quote(&target, lang()).await.is_err());
        assert!(carts.details(&target).await.unwrap().is_none());
        assert!(
            carts
                .add(&target, &target.csrf, target.revision, (p, 0), 1, lang())
                .await
                .is_err()
        );
        assert!(orders.for_guest(&target, &order.reference).await.is_err());
        let (_, anon) = customers.anonymous().await.unwrap();
        let raw = customers
            .login(&anon, &anon.csrf, "owner@example.test", PASSWORD.into())
            .await
            .unwrap();
        let fresh = customers.session(&raw).await.unwrap().unwrap();
        let restored = carts.customer_cart(&fresh).await.unwrap().unwrap();
        assert_eq!(restored.id, target.id);
        assert_eq!(
            orders
                .for_guest(&restored, &order.reference)
                .await
                .unwrap()
                .id,
            order.id
        );
        assert!(
            sqlx::query("UPDATE guest_carts SET customer_id=$1 WHERE id=$2")
                .bind(other.customer_id())
                .bind(target.id)
                .execute(&pool)
                .await
                .is_err()
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_carts_checkout_draft_precedence_and_unavailable_items() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let customers = Customers::new(pool.clone(), identity);
        let (_, c) = customer(&customers, "draft@example.test").await;
        let carts = Carts::new(pool.clone());
        let (raw, source, _) = prepare(&carts, (p, 0), 1, 1).await;
        carts
            .save_details(&source, &source.csrf, source.revision, data())
            .await
            .unwrap();
        let source = carts.session(&raw).await.unwrap().unwrap();
        merge(&carts, &c, &source).await.unwrap();
        let target = carts.customer_cart(&c).await.unwrap().unwrap();
        assert_eq!(
            carts.details(&target).await.unwrap().unwrap().email,
            data().email
        );
        let mut changed = data();
        changed.email = "preserved@example.test".into();
        carts
            .save_details(&target, &target.csrf, target.revision, changed)
            .await
            .unwrap();
        let (raw, source, _) = prepare(&carts, (p, 0), 1, 1).await;
        carts
            .save_details(&source, &source.csrf, source.revision, data())
            .await
            .unwrap();
        let source = carts.session(&raw).await.unwrap().unwrap();
        sqlx::query("UPDATE catalog_products SET status='draft' WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        merge(&carts, &c, &source).await.unwrap();
        let quote = carts.quote(&target, lang()).await.unwrap();
        assert_eq!(quote.lines[0].quantity, 2);
        assert!(matches!(quote.lines[0].issue, Some(Issue::Unavailable)));
        assert_eq!(
            carts.details(&target).await.unwrap().unwrap().email,
            "preserved@example.test"
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_carts_http_merge_is_post_only_and_preserves_guest_receipts() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let customers = Customers::new(pool.clone(), identity.clone());
        let (raw, c) = customer(&customers, "http@example.test").await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (guest_raw, g, review) = prepare(&carts, (p, 0), 1, 1).await;
        let old = confirm(&orders, &g, &review, 1).await.unwrap();
        let g = carts.session(&guest_raw).await.unwrap().unwrap();
        carts
            .add(&g, &g.csrf, g.revision, (p, 0), 1, lang())
            .await
            .unwrap();
        let g = carts.session(&guest_raw).await.unwrap().unwrap();
        let cookie = format!("talocart_customer={raw}; talocart_cart={guest_raw}");
        let app = router(test_app(pool.clone(), "Cart".into()).await);
        for prefix in ["", "/it"] {
            let response =
                request(&app, "GET", &format!("{prefix}/cart"), &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(html(response).await.contains("cart-merge-title"));
            assert_eq!(quantity(&pool, g.id).await, 1);
            assert_eq!(
                request(
                    &app,
                    "GET",
                    &format!("{prefix}/cart/merge"),
                    &cookie,
                    "",
                    false
                )
                .await
                .status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
            let invalid = format!("csrf={}&source_csrf={}&revision=invalid", c.csrf, g.csrf);
            let response = request(
                &app,
                "POST",
                &format!("{prefix}/cart/merge"),
                &cookie,
                &invalid,
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert!(!html(response).await.contains("Failed to deserialize"));
            let f = format!("csrf=wrong&source_csrf={}&revision={}", g.csrf, g.revision);
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("{prefix}/cart/merge"),
                    &cookie,
                    &f,
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        let f = format!(
            "csrf={}&source_csrf={}&revision={}",
            c.csrf, g.csrf, g.revision
        );
        assert_eq!(
            request(&app, "POST", "/cart/merge", &cookie, &f, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(quantity(&pool, g.id).await, 0);
        let response = request(
            &app,
            "GET",
            &format!("/checkout/orders/{}", old.reference),
            &cookie,
            "",
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(matches!(
            orders.for_customer(&c, &old.reference).await,
            Err(OrderError::NotFound)
        ));
    })
    .await;
}

pub(super) async fn browser_pending(pool: &PgPool) -> serde_json::Value {
    let p: i64 = sqlx::query_scalar("SELECT id FROM catalog_products WHERE model='METHODS-1'")
        .fetch_one(pool)
        .await
        .unwrap();
    let service = Customers::new(pool.clone(), Identity::new(pool.clone()));
    let carts = Carts::new(pool.clone());
    let mut profiles = vec![];
    for index in 0..8 {
        let email = format!("pending-{index}@example.test");
        let (_, c) = customer(&service, &email).await;
        let target = carts.ensure_customer(&c).await.unwrap();
        carts
            .add(&target, &target.csrf, 0, (p, 0), 1, lang())
            .await
            .unwrap();
        sqlx::query("UPDATE guest_cart_items SET quantity=999999 WHERE cart_id=$1")
            .bind(target.id)
            .execute(pool)
            .await
            .unwrap();
        let (raw, g) = carts.create().await.unwrap();
        carts.add(&g, &g.csrf, 0, (p, 0), 1, lang()).await.unwrap();
        profiles.push(serde_json::json!({"email":email,"guest":raw}));
    }
    serde_json::json!(profiles)
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_carts_variants_remain_distinct_and_revalidate_packs() {
    isolated(|pool,_|async move {
        let(p,identity,_)=configured(&pool).await;
        let service=Customers::new(pool.clone(),identity);let(_,c)=customer(&service,"variants@example.test").await;
        let class:i64=sqlx::query_scalar("SELECT tax_class_id FROM catalog_products WHERE id=$1").bind(p).fetch_one(&pool).await.unwrap();
        let product:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id,minimum_quantity,packs) VALUES('Variants','MERGE-PARENT','merge-variant',100,100,'published',$1,2,3) RETURNING id").bind(class).fetch_one(&pool).await.unwrap();
        let option:i64=sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)").bind(product).bind(option).execute(&pool).await.unwrap();
        let mut variants=vec![];
        for label in ["Small","Large"] {
            let value:i64=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES($1,$2) RETURNING id").bind(option).bind(label).fetch_one(&pool).await.unwrap();
            let variant:i64=sqlx::query_scalar("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],$3,200,100) RETURNING id").bind(product).bind(value).bind(format!("MERGE-{}",label.to_ascii_uppercase())).fetch_one(&pool).await.unwrap();variants.push(variant);
        }
        let carts=Carts::new(pool.clone());let target=carts.ensure_customer(&c).await.unwrap();
        carts.add(&target,&target.csrf,0,(product,variants[0]),3,lang()).await.unwrap();
        let(raw,g)=carts.create().await.unwrap();
        carts.add(&g,&g.csrf,0,(product,variants[0]),3,lang()).await.unwrap();
        carts.add(&g,&g.csrf,1,(product,variants[1]),3,lang()).await.unwrap();
        let source=carts.session(&raw).await.unwrap().unwrap();
        merge(&carts,&c,&source).await.unwrap();
        let q=carts.quote(&target,lang()).await.unwrap();assert_eq!(q.lines.len(),2);
        assert_eq!(q.lines.iter().map(|l|l.quantity).collect::<Vec<_>>(),[6,3]);assert!(q.totals.is_some());
        sqlx::query("UPDATE catalog_products SET packs=4 WHERE id=$1").bind(product).execute(&pool).await.unwrap();
        let q=carts.quote(&target,lang()).await.unwrap();assert!(q.lines.iter().all(|l|matches!(l.issue,Some(Issue::Quantity))));assert!(q.totals.is_none());
        sqlx::query("DELETE FROM catalog_variants WHERE id=$1").bind(variants[1]).execute(&pool).await.unwrap();
        let q=carts.quote(&target,lang()).await.unwrap();assert!(matches!(q.lines[1].issue,Some(Issue::Unavailable)));
        carts.update(&target,&target.csrf,q.revision,q.lines[1].id,None,lang()).await.unwrap();
        assert_eq!(carts.quote(&target,lang()).await.unwrap().lines.len(),1);
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_carts_concurrent_guest_edits_and_two_devices_preserve_revisions() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        let (_, c) = customer(&service, "concurrent@example.test").await;
        let carts = Carts::new(pool.clone());
        let target = carts.ensure_customer(&c).await.unwrap();
        let (_, g, _) = prepare(&carts, (p, 0), 2, 1).await;
        let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(
                carts.add(&g, &g.csrf, g.revision, (p, 0), 3, lang()),
                merge(&carts, &c, &g)
            )
        })
        .await
        .expect("Concurrent merge completes");
        assert!(a.is_ok() || matches!(a, Err(CartError::Stale)));
        assert!(b.is_ok() || matches!(b, Err(CartError::Stale)));
        assert_eq!(
            quantity(&pool, g.id).await + quantity(&pool, target.id).await,
            2 + if a.is_ok() { 3 } else { 0 }
        );
        let target = carts.customer_cart(&c).await.unwrap().unwrap();
        let (a, b) = tokio::join!(
            carts.add(&target, &target.csrf, target.revision, (p, 0), 1, lang()),
            carts.add(&target, &target.csrf, target.revision, (p, 0), 1, lang())
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(matches!(a, Err(CartError::Stale)) || matches!(b, Err(CartError::Stale)));
        let before = quantity(&pool, target.id).await;
        carts.prune().await.unwrap();
        assert_eq!(quantity(&pool, target.id).await, before);
    })
    .await;
}
