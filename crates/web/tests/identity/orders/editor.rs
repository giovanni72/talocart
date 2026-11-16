use super::*;
use talocart_commerce::orders::editor::{DraftData, Publish};

async fn draft(orders: &Orders, s: &talocart_platform::identity::Session, p: i64) -> i64 {
    let id = orders.create_draft(s, &s.csrf, None, "en").await.unwrap();
    let item = orders.editor_product(s, p, 0, "en").await.unwrap();
    let data = DraftData {
        customer: data(),
        locale: "en".into(),
        lines: vec![item],
        shipping_method_id: Some(1),
        payment_method_id: Some(1),
        reason: "Telephone order".into(),
        ..Default::default()
    };
    orders.save_draft(s, &s.csrf, id, 0, &data).await.unwrap();
    id
}
async fn publish(
    orders: &Orders,
    s: &talocart_platform::identity::Session,
    id: i64,
    notify: bool,
) -> Order {
    let d = orders.draft(s, id).await.unwrap();
    let q = orders.preview_draft(s, id).await.unwrap();
    orders
        .publish_draft(Publish {
            session: s,
            csrf: &s.csrf,
            draft_id: id,
            revision: d.revision,
            review: &q.review,
            notify,
        })
        .await
        .unwrap()
}
async fn stock(pool: &PgPool, p: i64) -> i32 {
    sqlx::query_scalar("SELECT stock FROM catalog_products WHERE id=$1")
        .bind(p)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn editor_create_replay_revision_prices_stock_history_and_cancellation() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity);
        let id = draft(&orders, &s, p).await;
        assert_eq!(stock(&pool, p).await, 10);
        assert_eq!(count(&pool, "orders").await, 0);
        assert_eq!(count(&pool, "mail").await, 0);
        let q = orders.preview_draft(&s, id).await.unwrap();
        let d = orders.draft(&s, id).await.unwrap();
        let input = || Publish {
            session: &s,
            csrf: &s.csrf,
            draft_id: id,
            revision: d.revision,
            review: &q.review,
            notify: false,
        };
        let (a, b) = tokio::join!(orders.publish_draft(input()), orders.publish_draft(input()));
        let first = a.unwrap();
        assert_eq!(first.id, b.unwrap().id);
        assert_eq!(stock(&pool, p).await, 9);
        assert_eq!(count(&pool, "orders").await, 1);
        assert_eq!(count(&pool, "mail").await, 1);
        assert_eq!(first.lines[0].unit_cents, 1000);
        sqlx::query("UPDATE catalog_products SET price_cents=9000 WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        let edit = orders
            .create_draft(&s, &s.csrf, Some((first.id, first.revision)), "en")
            .await
            .unwrap();
        let mut d = orders.draft(&s, edit).await.unwrap();
        assert_eq!(d.data.lines[0].unit_price, "10,00");
        d.data.lines[0].quantity = 3;
        d.data.customer.shipping_street = "Updated street".into();
        d.data.reason = "Customer requested three".into();
        orders
            .save_draft(&s, &s.csrf, edit, d.revision, &d.data)
            .await
            .unwrap();
        let q = orders.preview_draft(&s, edit).await.unwrap();
        assert_eq!(q.lines[0].unit_cents, 1000);
        assert_eq!(q.lines[0].catalog_unit_cents, 9000);
        let revised = publish(&orders, &s, edit, true).await;
        assert_eq!(revised.id, first.id);
        assert_eq!(revised.reference, first.reference);
        assert_eq!(revised.revision, 1);
        assert_eq!(revised.lines.len(), 1);
        assert_eq!(revised.lines[0].quantity, 3);
        assert_eq!(stock(&pool, p).await, 7);
        assert_eq!(revised.frozen.customer.shipping_street, "Updated street");
        let history = orders.edit_history(&s, first.id, 1).await.unwrap().entries;
        assert_eq!(history.len(), 2);
        assert_eq!(
            history[0].administrator_id,
            s.require(Permission::OrdersWrite).unwrap()
        );
        let previous: serde_json::Value=serde_json::from_str(&history[0].before_data).unwrap();
        assert_eq!(previous["lines"][0]["product_id"].as_i64(),Some(p));
        assert_eq!(previous["lines"][0]["id"].as_i64(),Some(first.lines[0].id));
        assert_eq!(previous["lines"][0]["tax_cents"].as_i64(),Some(first.lines[0].tax_cents));
        assert_eq!(previous["net_cents"].as_i64(),Some(first.net_cents));
        assert!(history[0].before_data.contains("Example"));
        assert!(history[0].after_data.contains("Updated street"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM order_lines WHERE order_id=$1 AND NOT active"
            )
            .bind(first.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
        assert!(
            sqlx::query("UPDATE order_lines SET quantity=2 WHERE order_id=$1")
                .bind(first.id)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE orders SET snapshot=snapshot||' ' WHERE id=$1")
                .bind(first.id)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM order_edit_events WHERE order_id=$1")
                .bind(first.id)
                .execute(&pool)
                .await
                .is_err()
        );
        orders.send_summary(&s, &s.csrf, first.id, 1).await.unwrap();
        orders.send_summary(&s, &s.csrf, first.id, 1).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM email_outbox WHERE order_id=$1 AND template='order_updated'"
            )
            .bind(first.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
        let replacement:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Replacement','EDITOR-REPLACEMENT','editor-replacement',500,5,'published',1) RETURNING id").fetch_one(&pool).await.unwrap();
        let replace=orders.create_draft(&s,&s.csrf,Some((first.id,1)),"en").await.unwrap();
        let mut draft=orders.draft(&s,replace).await.unwrap();
        let mut item=orders.editor_product(&s,replacement,0,"en").await.unwrap();item.quantity=2;
        draft.data.lines=vec![item];draft.data.reason="Replace the ordered product".into();
        orders.save_draft(&s,&s.csrf,replace,0,&draft.data).await.unwrap();
        let replaced=publish(&orders,&s,replace,false).await;
        assert_eq!(replaced.lines[0].product_id,replacement);
        assert_eq!(stock(&pool,p).await,10);assert_eq!(stock(&pool,replacement).await,3);
        orders.cancel(&s, &s.csrf, first.id, 2).await.unwrap();
        assert_eq!(stock(&pool,replacement).await,5);
        assert_eq!(stock(&pool, p).await, 10);
        assert_eq!(orders.publish_draft(input()).await.unwrap().id, first.id);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn editor_stale_review_stock_failure_and_payment_race_are_atomic() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity);
        let id = draft(&orders, &s, p).await;
        let q = orders.preview_draft(&s, id).await.unwrap();
        let d = orders.draft(&s, id).await.unwrap();
        sqlx::query("UPDATE catalog_products SET price_cents=1100 WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            orders
                .publish_draft(Publish {
                    session: &s,
                    csrf: &s.csrf,
                    draft_id: id,
                    revision: d.revision,
                    review: &q.review,
                    notify: true
                })
                .await,
            Err(OrderError::Review)
        ));
        assert_eq!(stock(&pool, p).await, 10);
        assert_eq!(count(&pool, "orders").await, 0);
        let first = publish(&orders, &s, id, false).await;
        let edit = orders
            .create_draft(&s, &s.csrf, Some((first.id, 0)), "en")
            .await
            .unwrap();
        let mut d = orders.draft(&s, edit).await.unwrap();
        d.data.lines[0].quantity = 99;
        d.data.reason = "Too many".into();
        orders
            .save_draft(&s, &s.csrf, edit, 0, &d.data)
            .await
            .unwrap();
        assert!(matches!(
            orders.preview_draft(&s, edit).await,
            Err(OrderError::Cart(talocart_commerce::cart::CartError::Stock))
        ));
        assert_eq!(stock(&pool, p).await, 9);
        assert_eq!(orders.get(&s, first.id).await.unwrap().lines[0].quantity, 1);
        d.data.lines[0].quantity = 2;
        orders
            .save_draft(&s, &s.csrf, edit, 1, &d.data)
            .await
            .unwrap();
        assert!(
            orders
                .save_draft(&s, &s.csrf, edit, 1, &d.data)
                .await
                .is_err()
        );
        let q = orders.preview_draft(&s, edit).await.unwrap();
        let (a, b) = tokio::join!(
            orders.publish_draft(Publish {
                session: &s,
                csrf: &s.csrf,
                draft_id: edit,
                revision: 2,
                review: &q.review,
                notify: false
            }),
            orders.record_payment(&s, &s.csrf, first.id, 0, "BANK")
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let current = orders.get(&s, first.id).await.unwrap();
        if current.is_paid() {
            assert_eq!(current.lines[0].quantity, 1);
            assert_eq!(stock(&pool, p).await, 9);
            assert!(!current.editable());
            assert!(matches!(
                orders
                    .create_draft(&s, &s.csrf, Some((first.id, current.revision)), "en")
                    .await,
                Err(OrderError::EditBlocked)
            ));
        } else {
            assert_eq!(current.lines[0].quantity, 2);
            assert_eq!(stock(&pool, p).await, 8);
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn editor_variants_custom_prices_coupon_and_original_stock_policy() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity);
        sqlx::raw_sql(
            "INSERT INTO coupons(code,kind,value,currency,active) VALUES('ADMIN10','percent',1000,'EUR',true)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let id = draft(&orders, &s, p).await;
        let mut d = orders.draft(&s, id).await.unwrap();
        d.data.lines[0].unit_price = "8,00".into();
        d.data.coupon_code = "ADMIN10".into();
        orders
            .save_draft(&s, &s.csrf, id, 1, &d.data)
            .await
            .unwrap();
        let first = publish(&orders, &s, id, true).await;
        assert_eq!(first.lines[0].unit_cents, 800);
        assert_eq!(first.frozen.coupon.as_ref().unwrap().discount_cents, 98);
        sqlx::raw_sql(
            "UPDATE coupons SET active=false; UPDATE catalog_products SET manage_stock=false",
        )
        .execute(&pool)
        .await
        .unwrap();
        let edit = orders
            .create_draft(&s, &s.csrf, Some((first.id, 0)), "en")
            .await
            .unwrap();
        let mut d = orders.draft(&s, edit).await.unwrap();
        d.data.lines[0].quantity = 2;
        d.data.reason = "Revised with original coupon".into();
        orders
            .save_draft(&s, &s.csrf, edit, 0, &d.data)
            .await
            .unwrap();
        let revised = publish(&orders, &s, edit, false).await;
        assert_eq!(revised.frozen.coupon.as_ref().unwrap().discount_cents, 195);
        assert_eq!(stock(&pool, p).await, 8);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM coupon_uses")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        orders.cancel(&s, &s.csrf, first.id, 1).await.unwrap();
        assert_eq!(stock(&pool, p).await, 10);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn editor_http_csrf_permissions_forms_and_escaping() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity.clone());
        let (raw, admin) = {
            let raw = authenticated(&identity).await;
            let admin = identity.session(&raw).await.unwrap().unwrap();
            (raw, admin)
        };
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        assert_eq!(
            fiscal_request(&app, "GET", "/admin/orders/new", &cookie, "", false)
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            fiscal_request(
                &app,
                "POST",
                "/admin/orders/new",
                &cookie,
                "csrf=bad",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let r = fiscal_request(
            &app,
            "POST",
            "/admin/orders/new",
            &cookie,
            &format!("csrf={}", admin.csrf),
            false,
        )
        .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        let path = r.headers()["location"].to_str().unwrap().to_owned();
        let body = html(fiscal_request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(body.contains("order-editor-form"));
        let id = draft(&orders, &s, p).await;
        let mut d = orders.draft(&s, id).await.unwrap();
        d.data.customer.first_name = "<script>".into();
        orders
            .save_draft(&s, &s.csrf, id, 1, &d.data)
            .await
            .unwrap();
        let body = html(
            fiscal_request(
                &app,
                "GET",
                &format!("/admin/orders/drafts/{id}"),
                &cookie,
                "",
                true,
            )
            .await,
        )
        .await;
        assert!(body.contains("&#60;script&#62;"));
        assert!(!body.contains("value=\"<script>"));
        let role = identity
            .create_role(&s, &s.csrf, "Order reader", &[Permission::OrdersRead])
            .await
            .unwrap();
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "reader@example.com",
                "correct horse battery".into(),
                talocart_platform::identity::Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let reader_raw = login_as(&identity, "reader@example.com").await;
        let reader = identity.session(&reader_raw).await.unwrap().unwrap();
        let reader_cookie = format!("talocart_session={reader_raw}");
        assert_eq!(
            fiscal_request(
                &app,
                "GET",
                &format!("/admin/orders/drafts/{id}"),
                &reader_cookie,
                "",
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fiscal_request(
                &app,
                "POST",
                "/admin/orders/new",
                &reader_cookie,
                &format!("csrf={}", reader.csrf),
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn editor_initial_schema_and_repeated_migration_preserve_orders() {
    isolated_before(1, |pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let (_, guest) = carts.create().await.unwrap();
        let old = super::historical::seed(&pool, &guest, p, "bank_transfer").await;
        let before: String = sqlx::query_scalar("SELECT snapshot FROM orders WHERE id=$1")
            .bind(old.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let stock_before = stock(&pool, p).await;
        database::migrate(&pool).await.unwrap();
        database::migrate(&pool).await.unwrap();
        database::check(&pool).await.unwrap();
        assert_eq!(
            before,
            sqlx::query_scalar::<_, String>("SELECT snapshot FROM orders WHERE id=$1")
                .bind(old.id)
                .fetch_one(&pool)
                .await
                .unwrap()
        );
        let orders = Orders::new(pool.clone(), identity);
        let order = orders.get(&s, old.id).await.unwrap();
        assert_eq!(order.lines.len(), 1);
        assert_eq!(order.lines[0].quantity, 1);
        assert_eq!(stock(&pool, p).await, stock_before);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT origin FROM orders WHERE id=$1")
                .bind(old.id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "storefront"
        );
        let edit = orders
            .create_draft(&s, &s.csrf, Some((order.id, order.revision)), "en")
            .await
            .unwrap();
        assert_eq!(
            orders.draft(&s, edit).await.unwrap().data.lines[0].unit_price,
            "10,00"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn editor_linked_customer_group_prices_and_variant_stock() {
    isolated(|pool,_|async move {
        use talocart_platform::identity::{CustomerProfile,Customers};
        use talocart_platform::identity::customer_groups::{CustomerGroups,QuantityTierInput};
        let(p,identity,s)=configured(&pool).await;
        let groups=CustomerGroups::new(pool.clone(),identity.clone());let(revision,group)=groups.editor(&s,Some(1)).await.unwrap();let mut input=group.unwrap().input();input.quantity_discounts_enabled=true;input.quantity_tiers=vec![QuantityTierInput {min_quantity:"2".into(),percent:"10".into()}];groups.save(&s,&s.csrf,Some(1),revision,&input).await.unwrap();
        let customers=Customers::new(pool.clone(),identity.clone());let(_,anon)=customers.anonymous().await.unwrap();let raw=customers.register(&anon,&anon.csrf,&CustomerProfile {email:"admin-buyer@example.test".into(),first_name:"Alex".into(),last_name:"Example".into(),customer_type:"private".into()},"synthetic password for orders".into(),None).await.unwrap().unwrap();let customer=customers.session(&raw).await.unwrap().unwrap();
        let orders=Orders::new(pool.clone(),identity);let id=draft(&orders,&s,p).await;let mut d=orders.draft(&s,id).await.unwrap();d.data.customer_id=customer.customer_id();d.data.lines[0].quantity=2;orders.save_draft(&s,&s.csrf,id,1,&d.data).await.unwrap();let order=publish(&orders,&s,id,false).await;assert_eq!(order.lines[0].unit_cents,900);assert!(order.frozen.group_discount.is_some());assert_eq!(orders.for_customer(&customer,&order.reference).await.unwrap().id,order.id);
        let option:i64=sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id").fetch_one(&pool).await.unwrap();let val:i64=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Large') RETURNING id").bind(option).fetch_one(&pool).await.unwrap();
        let product:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Variant item','ADMIN-V','admin-v',1000,9,'published',1) RETURNING id").fetch_one(&pool).await.unwrap();sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)").bind(product).bind(option).execute(&pool).await.unwrap();let variant:i64=sqlx::query_scalar("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],'ADMIN-L',1000,5) RETURNING id").bind(product).bind(val).fetch_one(&pool).await.unwrap();
        let id=orders.create_draft(&s,&s.csrf,None,"en").await.unwrap();let mut d=orders.draft(&s,id).await.unwrap();d.data.customer=data();d.data.reason="Variant phone order".into();d.data.shipping_method_id=Some(1);d.data.payment_method_id=Some(2);let mut item=orders.editor_product(&s,product,variant,"en").await.unwrap();item.quantity=2;d.data.lines.push(item);orders.save_draft(&s,&s.csrf,id,0,&d.data).await.unwrap();let order=publish(&orders,&s,id,false).await;
        assert_eq!(order.lines[0].variant_id,Some(variant));assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_variants WHERE id=$1").bind(variant).fetch_one(&pool).await.unwrap(),3);assert_eq!(stock(&pool,product).await,9);
        let edit=orders.create_draft(&s,&s.csrf,Some((order.id,0)),"en").await.unwrap();let mut d=orders.draft(&s,edit).await.unwrap();d.data.lines[0].quantity=1;d.data.reason="One variant".into();orders.save_draft(&s,&s.csrf,edit,0,&d.data).await.unwrap();let order=publish(&orders,&s,edit,false).await;
        assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_variants WHERE id=$1").bind(variant).fetch_one(&pool).await.unwrap(),4);
        let key=talocart_platform::identity::token().unwrap();orders.update_fulfillment(&s,&s.csrf,order.id,0,&key,&talocart_commerce::orders::FulfillmentInput {action:"preparing".into(),..Default::default()}).await.unwrap();
        let key=talocart_platform::identity::token().unwrap();let shipped=orders.update_fulfillment(&s,&s.csrf,order.id,1,&key,&talocart_commerce::orders::FulfillmentInput {action:"shipped".into(),carrier:"Synthetic courier".into(),..Default::default()}).await.unwrap();assert!(!shipped.editable());assert!(matches!(orders.create_draft(&s,&s.csrf,Some((order.id,shipped.revision)),"en").await,Err(OrderError::EditBlocked)));
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn editor_native_multiline_reason_is_saved_published_and_audited() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity);
        let id = draft(&orders, &s, p).await;
        let mut d = orders.draft(&s, id).await.unwrap();
        d.data.reason = "Telephone order\r\nConfirmed by operator\tAB".into();
        orders
            .save_draft(&s, &s.csrf, id, d.revision, &d.data)
            .await
            .unwrap();
        let order = publish(&orders, &s, id, false).await;
        let history = orders.edit_history(&s, order.id, 1).await.unwrap().entries;
        assert_eq!(history[0].reason, d.data.reason);
        let snapshot: serde_json::Value = serde_json::from_str(&history[0].after_data).unwrap();
        assert_eq!(snapshot["lines"][0]["basis_points"].as_i64(), Some(2200));
        assert_eq!(snapshot["tax_cents"].as_i64(), Some(order.tax_cents));
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn editor_drafts_and_audit_history_remain_reachable_beyond_first_page() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity.clone());
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let first = orders.create_draft(&s, &s.csrf, None, "en").await.unwrap();
        for _ in 0..50 {
            orders.create_draft(&s, &s.csrf, None, "en").await.unwrap();
        }
        let first_page =
            html(fiscal_request(&app, "GET", "/admin/orders/drafts", &cookie, "", false).await)
                .await;
        assert!(first_page.contains("/admin/orders/drafts?page=2"));
        let second_page = html(
            fiscal_request(
                &app,
                "GET",
                "/admin/orders/drafts?page=2",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(second_page.contains(&format!("href=\"/admin/orders/drafts/{first}\"")));
        let id = draft(&orders, &s, p).await;
        let mut order = publish(&orders, &s, id, false).await;
        for i in 1..=50 {
            let edit = orders
                .create_draft(&s, &s.csrf, Some((order.id, order.revision)), "en")
                .await
                .unwrap();
            let mut d = orders.draft(&s, edit).await.unwrap();
            d.data.reason = format!("Audit revision {i}");
            orders
                .save_draft(&s, &s.csrf, edit, d.revision, &d.data)
                .await
                .unwrap();
            order = publish(&orders, &s, edit, false).await;
        }
        let path = format!("/admin/orders/{}", order.id);
        let body = html(fiscal_request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(body.contains("edit_page=2"));
        let body = html(
            fiscal_request(
                &app,
                "GET",
                &format!("{path}?edit_page=2"),
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(
            body.contains("Telephone order"),
            "The original audit event remains reachable"
        );
    })
    .await;
}
