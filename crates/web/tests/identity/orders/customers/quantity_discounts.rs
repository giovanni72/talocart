use super::*;
use talocart_platform::identity::customer_groups::{CustomerGroups, GroupInput, QuantityTierInput};
async fn schedule(pool: &PgPool, identity: &Identity, enabled: bool, percent: &str) {
    let s = taxonomy_session(identity).await;
    let groups = CustomerGroups::new(pool.clone(), identity.clone());
    let (revision, group) = groups.editor(&s, Some(1)).await.unwrap();
    let mut input = group.unwrap().input();
    input.quantity_discounts_enabled = enabled;
    input.quantity_tiers = vec![
        QuantityTierInput {
            min_quantity: "2".into(),
            percent: percent.into(),
        },
        QuantityTierInput {
            min_quantity: "5".into(),
            percent: "20".into(),
        },
    ];
    groups
        .save(&s, &s.csrf, Some(1), revision, &input)
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn quantity_discounts_current_prices_coupon_order_replay_and_immutable_history() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let customers = Customers::new(pool.clone(), identity.clone());
        let (raw, c) = customer(&customers, "tiers@example.test").await;
        let carts = Carts::new(pool.clone());
        let g = carts.ensure_customer(&c).await.unwrap();
        carts.add(&g, &g.csrf, 0, (p, 0), 2, lang()).await.unwrap();
        assert!(
            carts
                .quote(&g, lang())
                .await
                .unwrap()
                .group_discount
                .is_none()
        );
        schedule(&pool, &identity, true, "10").await;
        let q = carts.quote(&g, lang()).await.unwrap();
        assert_eq!(q.totals.unwrap().gross().cents(), 2196);
        assert_eq!(q.lines[0].current.as_ref().unwrap().unit_cents, 900);
        let d = q.group_discount.unwrap();
        assert_eq!((d.original_cents, d.discount_cents), (2440, 244));
        assert_eq!(
            carts
                .preview_product_unit(&g, (p, 0), 3, lang())
                .await
                .unwrap()
                .unwrap()
                .cents(),
            800
        );
        let preview_app = router(test_app(pool.clone(), "Shop".into()).await);
        let preview_cookie = format!("talocart_customer={raw}");
        for (prefix, price) in [("", "9.76"), ("/it", "9,76")] {
            let response = request(
                &preview_app,
                "POST",
                &format!("{prefix}/products/method-item"),
                &preview_cookie,
                &format!("csrf={}&quantity=3", g.csrf),
                true,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = html(response).await;
            assert!(body.contains(price));
            assert!(body.contains("value=\"3\""));
            assert!(
                !body.contains("Group quantity discount")
                    && !body.contains("Sconto quantità del gruppo")
            );
            assert_eq!(
                request(
                    &preview_app,
                    "POST",
                    &format!("{prefix}/products/method-item"),
                    &preview_cookie,
                    "csrf=bad&quantity=3",
                    true
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            carts.quote(&g, lang()).await.unwrap().revision,
            1,
            "Preview does not edit the cart"
        );
        let coupons = talocart_commerce::coupons::Coupons::new(pool.clone(), identity.clone());
        coupons
            .save(
                &s,
                &s.csrf,
                0,
                0,
                &talocart_commerce::coupons::Input {
                    code: "SAVE10".into(),
                    kind: "percent".into(),
                    value: "10".into(),
                    active: true,
                    currency: "EUR".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        carts
            .set_coupon(&g, &g.csrf, 1, "SAVE10", lang())
            .await
            .unwrap();
        let g = carts.customer_cart(&c).await.unwrap().unwrap();
        let q = carts.quote(&g, lang()).await.unwrap();
        assert_eq!(q.coupon.as_ref().unwrap().original_cents, 2196);
        assert_eq!(q.coupon.as_ref().unwrap().discount_cents, 220);
        assert_eq!(q.totals.unwrap().gross().cents(), 1976);
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_customer={raw}");
        // A changed group price invalidates the prior checkout review.
        let old = carts
            .checkout_quote(&g, lang(), "IT", Some(selection(1)))
            .await
            .unwrap();
        schedule(&pool, &identity, true, "15").await;
        let orders = Orders::new(pool.clone(), identity.clone());
        assert!(matches!(
            orders
                .confirm_for_customer(input(&g, &old.review), &c)
                .await,
            Err(OrderError::Review)
        ));
        assert_eq!(count(&pool, "orders").await, 0);
        schedule(&pool, &identity, true, "10").await;
        // Current catalog price is authoritative, never the observation stored on add.
        sqlx::query("UPDATE catalog_products SET price_cents=800 WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        let q = carts
            .checkout_quote(&g, lang(), "IT", Some(selection(1)))
            .await
            .unwrap();
        assert_eq!(q.cart.group_discount.as_ref().unwrap().original_cents, 1952);
        assert_eq!(q.cart.coupon.as_ref().unwrap().original_cents, 1757);
        for prefix in ["", "/it"] {
            let r = request(&app, "GET", &format!("{prefix}/cart"), &cookie, "", false).await;
            assert_eq!(r.status(), StatusCode::OK);
            let body = html(r).await;
            assert!(
                !body.contains("Sconto quantità del gruppo")
                    && !body.contains("Group quantity discount")
            );
            assert!(body.contains("7.20") || body.contains("7,20"));
        }
        let (a, b) = tokio::join!(
            orders.confirm_for_customer(input(&g, &q.review), &c),
            orders.confirm_for_customer(input(&g, &q.review), &c)
        );
        let a = a.unwrap();
        assert_eq!(a.id, b.unwrap().id);
        assert_eq!(a.merchandise_cents, 1581);
        assert_eq!(a.lines[0].gross_cents, 1581);
        assert_eq!(a.lines[0].unit_cents, 720);
        assert_eq!(
            a.frozen.group_discount.as_ref().unwrap().discount_cents,
            195
        );
        schedule(&pool, &identity, false, "90").await;
        let saved = orders.get(&s, a.id).await.unwrap();
        assert_eq!(
            saved.frozen.group_discount.as_ref().unwrap().discount_cents,
            195
        );
        assert_eq!(saved.lines[0].gross_cents, 1581);
        assert_eq!(
            orders
                .confirm_for_customer(input(&g, &q.review), &c)
                .await
                .unwrap()
                .id,
            a.id
        );
        let mail: String =
            sqlx::query_scalar("SELECT body_text FROM email_outbox ORDER BY id LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            !mail.contains("Group quantity discount")
                && !mail.contains("Sconto quantità del gruppo")
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn quantity_discounts_combine_variants_only_within_one_product_and_recalculate() {
    isolated(|pool,_|async move {
        let(simple,identity,_)=configured(&pool).await;let customers=Customers::new(pool.clone(),identity.clone());
        let(_,c)=customer(&customers,"variants@example.test").await;schedule(&pool,&identity,true,"10").await;
        let parent:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Variants','PARENT','tier-variants',1000,100,'published',1) RETURNING id").fetch_one(&pool).await.unwrap();
        let option:i64=sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_product_options VALUES($1,1,$2)").bind(parent).bind(option).execute(&pool).await.unwrap();
        let mut variants=Vec::new();
        for (label,price) in [("Small",1000),("Large",2000)] {
            let value:i64=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES($1,$2) RETURNING id").bind(option).bind(label).fetch_one(&pool).await.unwrap();
            let variant:i64=sqlx::query_scalar("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],$3,$4,100) RETURNING id").bind(parent).bind(value).bind(label.to_ascii_uppercase()).bind(price).fetch_one(&pool).await.unwrap();variants.push(variant);
        }
        let carts=Carts::new(pool.clone());let g=carts.ensure_customer(&c).await.unwrap();
        carts.add(&g,&g.csrf,0,(simple,0),1,lang()).await.unwrap();
        carts.add(&g,&g.csrf,1,(parent,variants[0]),1,lang()).await.unwrap();
        assert!(carts.quote(&g,lang()).await.unwrap().group_discount.is_none(),"Separate products do not combine");
        carts.add(&g,&g.csrf,2,(parent,variants[1]),1,lang()).await.unwrap();
        let q=carts.quote(&g,lang()).await.unwrap();let d=q.group_discount.unwrap();assert_eq!(d.lines.len(),2);assert!(d.lines.iter().all(|l| l.product_quantity==2&&l.product_id==parent));assert_eq!(q.totals.unwrap().gross().cents(),4514);
        let row=q.lines[1].id;
        carts.update(&g,&g.csrf,3,row,Some(4),lang()).await.unwrap();
        let q=carts.quote(&g,lang()).await.unwrap();let d=q.group_discount.unwrap();assert!(d.lines.iter().all(|l| l.product_quantity==5&&l.discount_basis_points==2000));assert_eq!(q.totals.unwrap().gross().cents(),7076);
        carts.update(&g,&g.csrf,4,row,None,lang()).await.unwrap();
        assert!(carts.quote(&g,lang()).await.unwrap().group_discount.is_none());
        let (_,guest)=carts.create().await.unwrap();carts.add(&guest,&guest.csrf,0,(simple,0),5,lang()).await.unwrap();assert!(carts.quote(&guest,lang()).await.unwrap().group_discount.is_none(),"Guest receives no default discount without explicit opt-in");
        // Assignment changes pricing immediately, with no need to log in again.
        let s=taxonomy_session(&identity).await;let groups=CustomerGroups::new(pool.clone(),identity.clone());let revision=groups.editor(&s,None).await.unwrap().0;
        let id=groups.save(&s,&s.csrf,None,revision,&GroupInput{name_it:"Altro".into(),name_en:"Other".into(),..Default::default()}).await.unwrap();
        let member=groups.member_editor(&s,c.customer_id().unwrap()).await.unwrap().0;groups.assign(&s,&s.csrf,member.id,member.revision,id).await.unwrap();
        carts.add(&g,&g.csrf,5,(simple,0),1,lang()).await.unwrap();assert!(carts.quote(&g,lang()).await.unwrap().group_discount.is_none());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn quantity_guests_opt_in_default_switch_and_customer_merge_use_current_identity() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let customers = Customers::new(pool.clone(), identity.clone());
        let (_, customer) = customer(&customers, "guest-merge@example.test").await;
        schedule(&pool, &identity, true, "10").await;
        let carts = Carts::new(pool.clone());
        let (_, guest) = carts.create().await.unwrap();
        carts
            .add(&guest, &guest.csrf, 0, (p, 0), 2, lang())
            .await
            .unwrap();
        assert!(
            carts
                .quote(&guest, lang())
                .await
                .unwrap()
                .group_discount
                .is_none()
        );
        assert!(
            carts
                .preview_product_unit(&guest, (p, 0), 1, lang())
                .await
                .unwrap()
                .is_none()
        );
        let groups = CustomerGroups::new(pool.clone(), identity.clone());
        let (revision, group) = groups.editor(&s, Some(1)).await.unwrap();
        let mut input = group.unwrap().input();
        input.quantity_discounts_for_guests = true;
        groups
            .save(&s, &s.csrf, Some(1), revision, &input)
            .await
            .unwrap();
        assert_eq!(
            carts.quote(&guest, lang()).await.unwrap().lines[0]
                .current
                .as_ref()
                .unwrap()
                .unit_cents,
            900
        );
        assert_eq!(
            carts
                .preview_product_unit(&guest, (p, 0), 3, lang())
                .await
                .unwrap()
                .unwrap()
                .cents(),
            800
        );
        let (_, other_guest) = carts.create().await.unwrap();
        assert_eq!(
            carts
                .preview_product_unit(&other_guest, (p, 0), 1, lang())
                .await
                .unwrap()
                .unwrap()
                .cents(),
            1000,
            "Different guest carts never share quantities"
        );
        let revision = groups.editor(&s, None).await.unwrap().0;
        let mut alternative = GroupInput {
            name_it: "Ospiti".into(),
            name_en: "Guests".into(),
            is_default: true,
            quantity_discounts_enabled: true,
            quantity_discounts_for_guests: false,
            quantity_tiers: vec![QuantityTierInput {
                min_quantity: "1".into(),
                percent: "30".into(),
            }],
        };
        let id = groups
            .save(&s, &s.csrf, None, revision, &alternative)
            .await
            .unwrap();
        assert!(
            carts
                .quote(&guest, lang())
                .await
                .unwrap()
                .group_discount
                .is_none(),
            "A former default's flag is not a fallback"
        );
        let revision = groups.editor(&s, Some(id)).await.unwrap().0;
        alternative.quantity_discounts_for_guests = true;
        groups
            .save(&s, &s.csrf, Some(id), revision, &alternative)
            .await
            .unwrap();
        assert_eq!(
            carts.quote(&guest, lang()).await.unwrap().lines[0]
                .current
                .as_ref()
                .unwrap()
                .unit_cents,
            700
        );
        let target = carts.ensure_customer(&customer).await.unwrap();
        carts
            .add(&target, &target.csrf, 0, (p, 0), 1, lang())
            .await
            .unwrap();
        carts
            .merge_customer(&customer, &customer.csrf, &guest, &guest.csrf, 1)
            .await
            .unwrap();
        let quote = carts.quote(&target, lang()).await.unwrap();
        assert_eq!(quote.lines[0].quantity, 3);
        assert_eq!(
            quote.lines[0].current.as_ref().unwrap().unit_cents,
            900,
            "Login uses the customer's group, not the guest default"
        );
        assert_eq!(quote.group_discount.unwrap().group_id, 1);
        let (revision, group) = groups.editor(&s, Some(1)).await.unwrap();
        let mut input = group.unwrap().input();
        input.quantity_discounts_enabled = false;
        groups
            .save(&s, &s.csrf, Some(1), revision, &input)
            .await
            .unwrap();
        assert!(
            carts
                .quote(&target, lang())
                .await
                .unwrap()
                .group_discount
                .is_none(),
            "A customer with no discounts never inherits guest discounts"
        );
        assert_eq!(
            carts
                .preview_product_unit(&other_guest, (p, 0), 1, lang())
                .await
                .unwrap()
                .unwrap()
                .cents(),
            700
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn quantity_guests_toggle_invalidates_review_and_freezes_order_snapshot() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        schedule(&pool, &identity, true, "10").await;
        let carts = Carts::new(pool.clone());
        let (raw, guest) = carts.create().await.unwrap();
        carts
            .add(&guest, &guest.csrf, 0, (p, 0), 2, lang())
            .await
            .unwrap();
        let guest = carts.session(&raw).await.unwrap().unwrap();
        let groups = CustomerGroups::new(pool.clone(), identity.clone());
        let orders = Orders::new(pool.clone(), identity.clone());
        let old = carts
            .checkout_quote(&guest, lang(), "IT", Some(selection(1)))
            .await
            .unwrap();
        let (revision, group) = groups.editor(&s, Some(1)).await.unwrap();
        let mut settings = group.unwrap().input();
        settings.quantity_discounts_for_guests = true;
        groups
            .save(&s, &s.csrf, Some(1), revision, &settings)
            .await
            .unwrap();
        assert!(matches!(
            orders.confirm(input(&guest, &old.review)).await,
            Err(OrderError::Review)
        ));
        let quote = carts
            .checkout_quote(&guest, lang(), "IT", Some(selection(1)))
            .await
            .unwrap();
        let order = orders.confirm(input(&guest, &quote.review)).await.unwrap();
        assert_eq!(order.lines[0].unit_cents, 900);
        assert_eq!(order.frozen.group_discount.as_ref().unwrap().group_id, 1);
        let revision = groups.editor(&s, Some(1)).await.unwrap().0;
        settings.quantity_discounts_for_guests = false;
        groups
            .save(&s, &s.csrf, Some(1), revision, &settings)
            .await
            .unwrap();
        assert_eq!(
            orders.get(&s, order.id).await.unwrap().lines[0].unit_cents,
            900
        );
        assert_eq!(
            orders
                .confirm(input(&guest, &quote.review))
                .await
                .unwrap()
                .id,
            order.id
        );
        assert!(
            carts
                .preview_product_unit(&guest, (p, 0), 2, lang())
                .await
                .unwrap()
                .is_none()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn quantity_discounts_invalid_other_line_preserves_prices_without_checkout_total() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let customers = Customers::new(pool.clone(), identity.clone());
        let (_, customer) = customer(&customers, "invalid-line@example.test").await;
        let carts = Carts::new(pool.clone());
        let cart = carts.ensure_customer(&customer).await.unwrap();
        let other: i64 = sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) SELECT 'Other','OTHER','other',price_cents,100,'published',tax_class_id FROM catalog_products WHERE id=$1 RETURNING id")
            .bind(p).fetch_one(&pool).await.unwrap();
        carts.add(&cart, &cart.csrf, 0, (p, 0), 2, lang()).await.unwrap();
        carts.add(&cart, &cart.csrf, 1, (other, 0), 1, lang()).await.unwrap();
        schedule(&pool, &identity, true, "10").await;
        for unavailable in [false, true] {
            sqlx::query(if unavailable { "UPDATE catalog_products SET status='draft' WHERE id=$1" } else { "UPDATE catalog_products SET stock=0 WHERE id=$1" })
                .bind(other).execute(&pool).await.unwrap();
            let quote = carts.quote(&cart, lang()).await.unwrap();
            assert!(quote.totals.is_none(), "An invalid cart must remain blocked");
            assert!(quote.group_discount.is_none(), "No partial order discount snapshot");
            let valid = quote.lines.iter().find(|l| l.current.as_ref().is_some_and(|i| i.product_id == p)).unwrap();
            assert_eq!(valid.current.as_ref().unwrap().unit_cents, 900);
            assert_eq!(valid.totals.unwrap().gross().cents(), 2196);
            assert!(quote.lines.iter().any(|l| l.issue.is_some()));
        }
        sqlx::query("UPDATE catalog_products SET stock=100,status='published' WHERE id=$1").bind(other).execute(&pool).await.unwrap();
        let quote = carts.quote(&cart, lang()).await.unwrap();
        assert!(quote.totals.is_some());
        assert!(quote.group_discount.is_some());
    }).await;
}
