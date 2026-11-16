#[path = "orders/editor.rs"]
mod editor;
include!("orders_support.rs");
#[path = "orders/contention.rs"]
mod contention;
#[path = "orders/coupons.rs"]
mod coupons;
#[path = "orders/customers.rs"]
mod customers;
#[path = "orders/fulfillment.rs"]
mod fulfillment;
#[path = "orders/historical.rs"]
mod historical;
#[path = "orders/list.rs"]
mod list;
#[path = "orders/mail_configuration.rs"]
mod mail_configuration;
#[path = "orders/manual.rs"]
mod manual;
#[path = "orders/online_generic.rs"]
mod online_generic;
#[path = "orders/status.rs"]
mod status;
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_confirmation_replay_concurrency_snapshot_restart_and_new_cart_items() {
    isolated(|pool,options|async move{
 let(p,identity,s)=configured(&pool).await;let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity.clone());let(raw,g,review)=prepare(&carts,(p,0),2,1).await;
 let(a,b)=tokio::join!(confirm(&orders,&g,&review,1),confirm(&orders,&g,&review,1));let a=a.unwrap();assert_eq!(a.id,b.unwrap().id);assert_eq!(a.state,"open");assert_eq!(a.frozen.status_id,1);assert_eq!(a.frozen.payment.iban,"IT60X0542811101000000123456");assert_eq!(count(&pool,"orders").await,1);assert_eq!(count(&pool,"stock").await,1);assert_eq!(count(&pool,"mail").await,2);
 talocart_commerce::orders::check_list_customers(&pool).await.unwrap();
 talocart_commerce::orders::check_current_status(&pool).await.unwrap();
 assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM order_customer_summaries WHERE order_id=$1").bind(a.id).fetch_one(&pool).await.unwrap(),1);
 assert_eq!(sqlx::query_scalar::<_,i64>("SELECT status_id FROM order_current_status WHERE order_id=$1").bind(a.id).fetch_one(&pool).await.unwrap(),a.frozen.status_id);

 assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_products WHERE id=$1").bind(p).fetch_one(&pool).await.unwrap(),8);assert!(carts.quote(&g,lang()).await.unwrap().lines.is_empty());
 sqlx::raw_sql("UPDATE catalog_products SET name='Changed item',price_cents=9000; UPDATE payment_methods SET name_en='Changed method',instructions_en='Changed instructions';").execute(&pool).await.unwrap();let order=orders.get(&s,a.id).await.unwrap();assert_eq!(order.lines[0].name,"Item");assert_ne!(order.frozen.payment.name_en,"Changed method");assert_eq!(order.lines[0].unit_cents,1000);
 let now=carts.session(&raw).await.unwrap().unwrap();carts.add(&now,&now.csrf,now.revision,(p,0),1,lang()).await.unwrap();assert_eq!(confirm(&orders,&g,&review,1).await.unwrap().id,a.id);assert_eq!(carts.quote(&g,lang()).await.unwrap().lines[0].quantity,1);
 let reopened=PgPoolOptions::new().connect_with(options).await.unwrap();let persisted=Orders::new(reopened.clone(),Identity::new(reopened.clone())).for_guest(&g,&a.reference).await.unwrap();assert_eq!(persisted.total_cents,a.total_cents);reopened.close().await;
 let(_,other)=carts.create().await.unwrap();assert!(matches!(orders.for_guest(&other,&a.reference).await,Err(OrderError::NotFound)));
 let outbox:Vec<String>=sqlx::query_scalar("SELECT template FROM email_outbox ORDER BY template").fetch_all(&pool).await.unwrap();assert_eq!(outbox,vec!["order_admin","order_received"]);
}).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_last_unit_only_one_order_and_no_partial_effects() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        sqlx::query("UPDATE catalog_products SET stock=1 WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, a, ra) = prepare(&carts, (p, 0), 1, 1).await;
        let (_, b, rb) = prepare(&carts, (p, 0), 1, 2).await;
        let (x, y) = tokio::join!(confirm(&orders, &a, &ra, 1), confirm(&orders, &b, &rb, 2));
        assert_eq!([x.is_ok(), y.is_ok()].into_iter().filter(|v| *v).count(), 1);
        assert_eq!(count(&pool, "orders").await, 1);
        assert_eq!(count(&pool, "stock").await, 1);
        assert_eq!(count(&pool, "mail").await, 2);
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1")
                .bind(p)
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_variant_stock_and_explicit_cancellation_restore_once() {
    isolated(|pool,_|async move{
 let(p,identity,s)=configured(&pool).await;
 let option:i64=sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id").fetch_one(&pool).await.unwrap();let value:i64=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Large') RETURNING id").bind(option).fetch_one(&pool).await.unwrap();sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)").bind(p).bind(option).execute(&pool).await.unwrap();let variant:i64=sqlx::query_scalar("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],'ORD-VAR',1000,3) RETURNING id").bind(p).bind(value).fetch_one(&pool).await.unwrap();
 let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity);let(_,g,r)=prepare(&carts,(p,variant),2,2).await;let o=confirm(&orders,&g,&r,2).await.unwrap();assert_eq!(o.lines[0].label,"Size: Large");assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_variants WHERE id=$1").bind(variant).fetch_one(&pool).await.unwrap(),1);assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_products WHERE id=$1").bind(p).fetch_one(&pool).await.unwrap(),10);
 carts.prune().await.unwrap();assert_eq!(orders.get(&s,o.id).await.unwrap().state,"open");
 let(a,b)=tokio::join!(orders.cancel(&s,&s.csrf,o.id,0),orders.cancel(&s,&s.csrf,o.id,0));assert_eq!(a.unwrap().state,"cancelled");assert_eq!(b.unwrap().state,"cancelled");orders.cancel(&s,&s.csrf,o.id,0).await.unwrap();assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_variants WHERE id=$1").bind(variant).fetch_one(&pool).await.unwrap(),3);assert_eq!(count(&pool,"stock").await,2);assert_eq!(count(&pool,"mail").await,4);
 assert!(sqlx::query("DELETE FROM catalog_variants WHERE id=$1").bind(variant).execute(&pool).await.is_err());assert!(sqlx::query("UPDATE order_lines SET quantity=1").execute(&pool).await.is_err());
}).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_changed_quote_and_email_failures_rollback_all_business_effects() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, g, r) = prepare(&carts, (p, 0), 1, 2).await;
        sqlx::query("UPDATE catalog_products SET price_cents=2000 WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            confirm(&orders, &g, &r, 2).await,
            Err(OrderError::Review)
        ));
        assert_eq!(count(&pool, "orders").await, 0);
        let q = carts
            .checkout_quote(&g, lang(), "IT", Some(selection(2)))
            .await
            .unwrap();
        sqlx::query("UPDATE email_configuration SET from_email='' WHERE id=1")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            confirm(&orders, &g, &q.review, 2).await,
            Err(OrderError::Notification)
        ));
        assert_eq!(count(&pool, "orders").await, 0);
        assert_eq!(count(&pool, "mail").await, 0);
        assert_eq!(count(&pool, "stock").await, 0);
        assert_eq!(carts.quote(&g, lang()).await.unwrap().lines.len(), 1);
        assert!(carts.details(&g).await.unwrap().is_none());
        sqlx::query("UPDATE email_configuration SET from_email='shop@example.test'")
            .execute(&pool)
            .await
            .unwrap();
        let o = confirm(&orders, &g, &q.review, 2).await.unwrap();
        assert!(
            o.reference.ends_with("-1"),
            "Failed confirmation must not consume a number"
        );
        sqlx::query("UPDATE catalog_products SET stock=999999 WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            orders.cancel(&s, &s.csrf, o.id, 0).await,
            Err(OrderError::Restore)
        ));
        assert_eq!(orders.get(&s, o.id).await.unwrap().state, "open");
        assert_eq!(count(&pool, "mail").await, 2);
        sqlx::query("UPDATE catalog_products SET stock=9 WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE email_configuration SET from_email=''")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            orders.cancel(&s, &s.csrf, o.id, 0).await,
            Err(OrderError::Notification)
        ));
        assert_eq!(orders.get(&s, o.id).await.unwrap().state, "open");
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1")
                .bind(p)
                .fetch_one(&pool)
                .await
                .unwrap(),
            9
        );
        assert_eq!(count(&pool, "stock").await, 1);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_http_bilingual_review_receipt_csrf_permissions_and_cancel() {
    isolated(|pool,_|async move{
 let(p,identity,s)=configured(&pool).await;let carts=Carts::new(pool.clone());let(raw,g,_)=prepare(&carts,(p,0),1,2).await;carts.save_details(&g,&g.csrf,g.revision,data()).await.unwrap();let g=carts.session(&raw).await.unwrap().unwrap();let app=router(test_app(pool.clone(),"Orders".into()).await);let cookie=format!("talocart_cart={raw}");let mut reference=String::new();let mut order_id=0;
 for path in ["/checkout/methods","/it/checkout/methods"] {
  let h=html(request(&app,"GET",path,&cookie,"",false).await).await;
  assert!(h.contains("name=\"action\" value=\"confirm\""));
 }
 let q=carts.checkout_quote(&g,lang(),"IT",Some(selection(2))).await.unwrap();
 let body=format!("csrf={}&revision={}&action=confirm&review={}&customer_type=private&email=forged%40example.test&first_name=Alex&last_name=Example&phone=%2B393331234567&shipping_country=IT&shipping_street=Example&shipping_number=1&shipping_postal=00100&shipping_city=Roma&shipping_region=RM&shipping_method_id=1&payment_method_id=2",g.csrf,g.revision,q.review);
 assert_eq!(request(&app,"POST","/checkout/methods",&cookie,&body.replace(&g.csrf,"bad"),false).await.status(),StatusCode::FORBIDDEN);
 sqlx::query("UPDATE shipping_methods SET fee_value=200 WHERE id=1").execute(&pool).await.unwrap();let r=request(&app,"POST","/checkout/methods",&cookie,&body,false).await;assert_eq!(r.status(),StatusCode::CONFLICT);let h=html(r).await;assert!(h.contains("quotation has changed"));assert!(!h.contains("value=\"confirm\" disabled"));assert_eq!(count(&pool,"orders").await,0);
 let q=carts.checkout_quote(&g,lang(),"IT",Some(selection(2))).await.unwrap();let old=body.split("&review=").nth(1).unwrap().split('&').next().unwrap();let body=body.replace(old,&q.review);
 let r=request(&app,"POST","/checkout/methods",&cookie,&body,false).await;assert_eq!(r.status(),StatusCode::SEE_OTHER);assert_eq!(carts.details(&g).await.unwrap().unwrap().email,"buyer@example.test");let url=r.headers()[header::LOCATION].to_str().unwrap().to_owned();reference.push_str(url.rsplit('/').next().unwrap());
 for prefix in ["","/it"] {let r=request(&app,"GET",&format!("{prefix}/checkout/orders/{reference}"),&cookie,"",false).await;assert_eq!(r.status(),StatusCode::OK);assert_eq!(r.headers()[header::CACHE_CONTROL],"no-store");assert!(html(r).await.contains(&reference));}
 let(_,other)=carts.create().await.unwrap();let o=talocart_commerce::orders::Orders::new(pool.clone(),identity.clone()).for_guest(&other,&reference).await;assert!(matches!(o,Err(OrderError::NotFound)));
 let raw=authenticated(&identity).await;let admin_cookie=format!("talocart_session={raw}");let admin=identity.session(&raw).await.unwrap().unwrap();let rows=Orders::new(pool.clone(),identity.clone()).list(&admin,1).await.unwrap();order_id+=rows.entries[0].id;
 for locale in ["en","it"] {sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();let r=request(&app,"GET","/admin/orders",&admin_cookie,"",false).await;assert_eq!(r.status(),StatusCode::OK);assert_eq!(r.headers()[header::CACHE_CONTROL],"no-store");let h=html(r).await;assert!(h.contains(&reference));assert!(h.contains("class=\"ui-table ui-table--fixed order-table\""));assert_eq!(h.matches("scope=\"col\"").count(),7);assert!(h.contains("scope=\"row\""));assert!(h.contains(if locale=="it" {"Numero ordine"} else {"Order number"}));assert!(h.contains(if locale=="it" {"Non pagato"} else {"Unpaid"}));}
 let path=format!("/admin/orders/{order_id}/cancel");assert_eq!(request(&app,"POST",&path,&admin_cookie,"csrf=bad&revision=0&confirm=true",false).await.status(),StatusCode::FORBIDDEN);
 assert_eq!(request(&app,"POST",&path,&admin_cookie,&format!("csrf={}&revision=0",admin.csrf),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
 assert_eq!(request(&app,"POST",&path,&admin_cookie,&format!("csrf={}&revision=0&confirm=true",admin.csrf),false).await.status(),StatusCode::SEE_OTHER);
 let r=request(&app,"GET",&url,&cookie,"",false).await;let h=html(r).await;assert!(h.contains("Order cancelled"));assert!(h.contains(r#"<h1 id="receipt-title" tabindex="-1">Order cancelled</h1>"#));assert!(h.contains("order-hero--closed"));assert!(!h.contains("Use this payment reference:"));
 for locale in ["en","it"] {
 sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
 let h=html(request(&app,"GET","/admin/settings/email",&admin_cookie,"",false).await).await;
 assert!(h.contains(if locale=="it" {"Superata da un evento successivo"} else {"Superseded by a later event"}));
 let ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM email_outbox WHERE state='superseded'").fetch_all(&pool).await.unwrap();
 for id in ids {assert!(!h.contains(&format!("action=\"/admin/settings/email/{id}/retry\"")));}
 }
 let viewer_role=identity.create_role(&s,&s.csrf,"Order reader",&[Permission::OrdersRead]).await.unwrap();let password="synthetic viewer password";identity.create_administrator(&s,&s.csrf,"reader@example.test",password.into(),talocart_platform::identity::Role::parse(&viewer_role).unwrap()).await.unwrap();
 let login=request(&app,"GET","/admin/login","","",false).await;let login_cookie=cookie_from_response(&login);let login_html=html(login).await;let csrf=csrf(&login_html);let login=request(&app,"POST","/admin/login",&login_cookie,&format!("csrf={csrf}&email=reader%40example.test&password=synthetic+viewer+password"),false).await;let reader_cookie=cookie_from_response(&login);
 assert_eq!(request(&app,"GET","/admin/orders",&reader_cookie,"",false).await.status(),StatusCode::OK);
 assert_eq!(request(&app,"POST",&path,&reader_cookie,"csrf=bad&revision=0&confirm=true",false).await.status(),StatusCode::FORBIDDEN);
}).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_email_audiences_use_frozen_locales_and_fanout_failure_rolls_back() {
    isolated(|pool,_|async move{
 let(p,identity,_)=configured(&pool).await;let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity);let(_,g,_)=prepare(&carts,(p,0),1,2).await;
 let language=Language::new(Locale::parse("it").unwrap(),Locale::parse("en").unwrap());let q=carts.checkout_quote(&g,language,"IT",Some(selection(2))).await.unwrap();
 sqlx::raw_sql("CREATE FUNCTION synthetic_order_mail_fail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.template='order_admin' THEN RAISE EXCEPTION 'Synthetic mail failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER synthetic_order_mail_fail BEFORE INSERT ON email_outbox FOR EACH ROW EXECUTE FUNCTION synthetic_order_mail_fail();").execute(&pool).await.unwrap();
 let mut d=data();d.first_name="<script>Alex</script>".into();
 assert!(matches!(orders.confirm(Confirmation{online_available:false,guest:&g,csrf:&g.csrf,revision:g.revision,data:d.clone(),selection:selection(2),review:&q.review,language}).await,Err(OrderError::Technical(talocart_platform::technical::TechnicalError::Database(ref cause))) if cause.sqlstate()==Some("P0001")));
 assert_eq!(count(&pool,"orders").await,0);assert_eq!(count(&pool,"mail").await,0);assert_eq!(count(&pool,"stock").await,0);
 sqlx::query("DROP TRIGGER synthetic_order_mail_fail ON email_outbox").execute(&pool).await.unwrap();let order=orders.confirm(Confirmation{online_available:false,guest:&g,csrf:&g.csrf,revision:g.revision,data:d,selection:selection(2),review:&q.review,language}).await.unwrap();
 let notices:Vec<(String,String,String)>=sqlx::query_as("SELECT template,locale,body_text FROM email_outbox ORDER BY template").fetch_all(&pool).await.unwrap();assert_eq!(notices[0].0,"order_admin");assert_eq!(notices[0].1,"en");assert!(notices[0].2.contains("Cash on delivery"));assert_eq!(notices[1].1,"it");assert!(notices[1].2.contains("Contrassegno"));assert!(!notices[1].2.contains("Changed"));
 let app=router(test_app(pool.clone(),"Orders".into()).await);let raw=authenticated(&Identity::new(pool.clone())).await;let h=html(request(&app,"GET",&format!("/admin/orders/{}",order.id),&format!("talocart_session={raw}"),"",false).await).await;assert!(!h.contains("<script>Alex</script>"));assert!(h.contains("&#60;script&#62;"));
}).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_multiline_stock_failure_and_open_order_structure_are_atomic() {
    isolated(|pool,_|async move{
 let(p,identity,s)=configured(&pool).await;let second:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Second','ORDER-SECOND','order-second',1000,10,'published',1) RETURNING id").fetch_one(&pool).await.unwrap();let carts=Carts::new(pool.clone());let(raw,g,_)=prepare(&carts,(p,0),1,2).await;carts.add(&g,&g.csrf,g.revision,(second,0),1,lang()).await.unwrap();let g=carts.session(&raw).await.unwrap().unwrap();let q=carts.checkout_quote(&g,lang(),"IT",Some(selection(2))).await.unwrap();let orders=Orders::new(pool.clone(),identity.clone());
 sqlx::raw_sql("CREATE FUNCTION synthetic_stock_fail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.model='ORDER-SECOND' AND NEW.stock<OLD.stock THEN RAISE EXCEPTION 'Synthetic stock failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER synthetic_stock_fail BEFORE UPDATE OF stock ON catalog_products FOR EACH ROW EXECUTE FUNCTION synthetic_stock_fail();").execute(&pool).await.unwrap();assert!(confirm(&orders,&g,&q.review,2).await.is_err());assert_eq!(count(&pool,"orders").await,0);assert_eq!(count(&pool,"stock").await,0);assert_eq!(count(&pool,"mail").await,0);let stocks:Vec<i32>=sqlx::query_scalar("SELECT stock FROM catalog_products ORDER BY id").fetch_all(&pool).await.unwrap();assert_eq!(stocks,vec![10,10]);
 sqlx::query("DROP TRIGGER synthetic_stock_fail ON catalog_products").execute(&pool).await.unwrap();let o=confirm(&orders,&g,&q.review,2).await.unwrap();
 let option:i64=sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Order structure') RETURNING id").fetch_one(&pool).await.unwrap();let variants=talocart_commerce::variants::Variants::new(pool.clone(),identity.clone(),test_media(&pool).await);assert!(matches!(variants.configure(&s,&s.csrf,p,&[option]).await,Err(talocart_commerce::variants::VariantError::InUse)));
 orders.cancel(&s,&s.csrf,o.id,0).await.unwrap();variants.configure(&s,&s.csrf,p,&[option]).await.unwrap();assert_eq!(count(&pool,"stock").await,4);
}).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_tax_only_change_requires_customer_review() {
    isolated(|pool, _| async move {
        let (_, identity, _) = configured(&pool).await;
        sqlx::query("DELETE FROM catalog_products").execute(&pool).await.unwrap();
        sqlx::query("UPDATE pricing_settings SET price_mode='inclusive'")
            .execute(&pool)
            .await
            .unwrap();
        let p:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Tax order','ORD-TAX','ord-tax',1000,10,'published',1) RETURNING id").fetch_one(&pool).await.unwrap();
        let rate: i64 = sqlx::query_scalar(
            "INSERT INTO tax_rates(name,basis_points) VALUES('Shipping VAT',2200) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let class: i64 = sqlx::query_scalar(
            "INSERT INTO tax_classes(name,rate_id) VALUES('Shipping only',$1) RETURNING id",
        )
        .bind(rate)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE shipping_methods SET tax_class_id=$1 WHERE id=1")
            .bind(class)
            .execute(&pool)
            .await
            .unwrap();
        let carts = Carts::new(pool.clone());
        let (_, g, r) = prepare(&carts, (p, 0), 1, 2).await;
        sqlx::query("UPDATE tax_rates SET basis_points=1000 WHERE id=$1")
            .bind(rate)
            .execute(&pool)
            .await
            .unwrap();
        let orders = Orders::new(pool.clone(), identity);
        assert!(matches!(
            confirm(&orders, &g, &r, 2).await,
            Err(OrderError::Review)
        ));
        assert_eq!(count(&pool, "orders").await, 0);
    })
    .await;
}

#[tokio::test]
#[ignore = "Audit reproduction on dedicated PostgreSQL"]
async fn orders_audit_stale_product_form_cannot_overwrite_stock() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let products = talocart_commerce::products::Products::new(
            pool.clone(),
            identity.clone(),
            test_media(&pool).await,
        );
        let mut old_form = products.entry(&s, p).await.unwrap().values();
        assert_eq!(old_form.stock, "10");
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, g, r) = prepare(&carts, (p, 0), 2, 1).await;
        let order = confirm(&orders, &g, &r, 1).await.unwrap();
        old_form.description = "Updated description only".into();
        assert!(
            products
                .save(&s, &s.csrf, Some(p), &old_form)
                .await
                .is_err()
        );
        let stock: i32 = sqlx::query_scalar("SELECT stock FROM catalog_products WHERE id=$1")
            .bind(p)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stock, 8);
        orders.cancel(&s, &s.csrf, order.id, 0).await.unwrap();
        let stock: i32 = sqlx::query_scalar("SELECT stock FROM catalog_products WHERE id=$1")
            .bind(p)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stock, 10);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_audit_cancellation_supersedes_unsent_confirmations() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, g, r) = prepare(&carts, (p, 0), 1, 1).await;
        let o = confirm(&orders, &g, &r, 1).await.unwrap();
        orders.cancel(&s, &s.csrf, o.id, 0).await.unwrap();
        let states: Vec<String> = sqlx::query_scalar("SELECT state FROM email_outbox WHERE template IN ('order_received','order_admin') ORDER BY id").fetch_all(&pool).await.unwrap();
        assert_eq!(states, vec!["superseded", "superseded"]);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_audit_stock_revision_covers_restore_aba_and_variants() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let products = talocart_commerce::products::Products::new(
            pool.clone(),
            identity.clone(),
            test_media(&pool).await,
        );
        let variants = talocart_commerce::variants::Variants::new(
            pool.clone(),
            identity.clone(),
            test_media(&pool).await,
        );
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let old = products.entry(&s, p).await.unwrap().values();
        let (_, g, r) = prepare(&carts, (p, 0), 2, 1).await;
        let o = confirm(&orders, &g, &r, 1).await.unwrap();
        let after_debit = products.entry(&s, p).await.unwrap().values();
        orders.cancel(&s, &s.csrf, o.id, 0).await.unwrap();
        for values in [&old, &after_debit] {
            assert!(matches!(
                products.save(&s, &s.csrf, Some(p), values).await,
                Err(talocart_commerce::products::ProductError::StockChanged)
            ));
        }
        let fresh = products.entry(&s, p).await.unwrap().values();
        products.save(&s, &s.csrf, Some(p), &fresh).await.unwrap();
        let option: i64 =
            sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let value: i64 = sqlx::query_scalar(
            "INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Large') RETURNING id",
        )
        .bind(option)
        .fetch_one(&pool)
        .await
        .unwrap();
        variants.configure(&s, &s.csrf, p, &[option]).await.unwrap();
        let mut v = variant_values("AUDIT-VARIANT", &[value], "10", "10");
        v.stock = "10".into();
        v.price_mode = "exclusive".into();
        let id = variants.save(&s, &s.csrf, p, None, &v).await.unwrap();
        let old = variants.entry(&s, p, id).await.unwrap().values("exclusive");
        let (_, g, r) = prepare(&carts, (p, id), 2, 2).await;
        let o = confirm(&orders, &g, &r, 2).await.unwrap();
        assert!(matches!(
            variants.save(&s, &s.csrf, p, Some(id), &old).await,
            Err(talocart_commerce::variants::VariantError::Product(
                talocart_commerce::products::ProductError::StockChanged
            ))
        ));
        let after_debit = variants.entry(&s, p, id).await.unwrap().values("exclusive");
        orders.cancel(&s, &s.csrf, o.id, 0).await.unwrap();
        for v in [&old, &after_debit] {
            assert!(matches!(
                variants.save(&s, &s.csrf, p, Some(id), v).await,
                Err(talocart_commerce::variants::VariantError::Product(
                    talocart_commerce::products::ProductError::StockChanged
                ))
            ));
        }
        let mut a = variants.entry(&s, p, id).await.unwrap().values("exclusive");
        let mut b = a.clone();
        a.stock = "12".into();
        b.stock = "15".into();
        let (a, b) = tokio::join!(
            variants.save(&s, &s.csrf, p, Some(id), &a),
            variants.save(&s, &s.csrf, p, Some(id), &b)
        );
        assert_eq!([a.is_ok(), b.is_ok()].into_iter().filter(|x| *x).count(), 1);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_audit_email_backoff_and_manual_retry_cannot_revive_creation() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity.clone());
        let mail = talocart_platform::mail::Mail::new(pool.clone(), identity);
        let (_, g, r) = prepare(&carts, (p, 0), 1, 1).await;
        let o = confirm(&orders, &g, &r, 1).await.unwrap();
        let (config, mode, delivered, task) = super::email::smtp(451).await;
        mail.process_one(&config).await.unwrap(); // customer: backoff
        mode.store(550, std::sync::atomic::Ordering::SeqCst);
        mail.process_one(&config).await.unwrap(); // shop: failed
        orders.cancel(&s, &s.csrf, o.id, 0).await.unwrap();
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM email_outbox WHERE template IN ('order_received','order_admin')",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        for id in ids {
            assert!(mail.retry(&s, &s.csrf, id).await.is_err());
        }
        mode.store(250, std::sync::atomic::Ordering::SeqCst);
        for _ in 0..2 {
            assert!(mail.process_one(&config).await.unwrap());
        }
        assert!(!mail.process_one(&config).await.unwrap());
        assert_eq!(delivered.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM email_attempts WHERE outcome IN ('transient','permanent')"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            2
        );
        task.abort();
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_audit_cancellation_waits_for_inflight_success_or_timeout() {
    isolated(|pool, _| async move {
        let (p,identity,s)=configured(&pool).await;
        let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity.clone());
        let mail=talocart_platform::mail::Mail::new(pool.clone(),identity);
        for timeout in [1,10] {
            let (_,g,r)=prepare(&carts,(p,0),1,1).await;
            let o=confirm(&orders,&g,&r,1).await.unwrap();
            let (mut config,mode,delivered,task)=super::email::smtp(999).await;
            config.timeout_seconds=timeout;
            let worker=mail.clone();let smtp=config.clone();
            let sending=tokio::spawn(async move{worker.process_one(&smtp).await.unwrap()});
            tokio::time::timeout(std::time::Duration::from_secs(5),async {
                while delivered.load(std::sync::atomic::Ordering::SeqCst)==0 {tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
            }).await.unwrap();
            orders.cancel(&s,&s.csrf,o.id,0).await.unwrap();
            assert!(!mail.process_one(&config).await.unwrap(),"Cancellation waits for claimed creation attempt");
            assert!(sending.await.unwrap());
            mode.store(250,std::sync::atomic::Ordering::SeqCst);
            for _ in 0..2 {assert!(mail.process_one(&config).await.unwrap());}
            assert!(!mail.process_one(&config).await.unwrap());
            let states:Vec<String>=sqlx::query_scalar("SELECT state FROM email_outbox WHERE order_id=$1 AND template IN ('order_received','order_admin') ORDER BY id").bind(o.id).fetch_all(&pool).await.unwrap();
            assert_eq!(states,vec![if timeout==1 {"superseded"}else{"accepted"},"superseded"]);
            task.abort();
        }
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_annual_numbering_rollover_concurrency_rollback_and_restart() {
    isolated(|pool, options| async move {
        let allocate = |instant: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT next_order_reference($1::text::timestamptz)",
                )
                .bind(instant)
                .fetch_one(&pool)
                .await
                .expect("Allocate annual reference")
            }
        };
        // Rome reaches 2027 one hour before UTC; the connection timezone is irrelevant.
        assert_eq!(allocate("2026-12-31T22:59:59Z").await, "ORD-2026-1");
        assert_eq!(allocate("2026-12-31T22:59:59Z").await, "ORD-2026-2");
        assert_eq!(allocate("2026-12-31T23:00:00Z").await, "ORD-2027-1");
        let (a, b) = tokio::join!(
            allocate("2026-12-31T23:00:01Z"),
            allocate("2026-12-31T23:00:01Z")
        );
        let mut references = [a, b];
        references.sort();
        assert_eq!(references, ["ORD-2027-2", "ORD-2027-3"]);
        let (a, b) = tokio::join!(
            allocate("2030-01-01T00:00:00Z"),
            allocate("2030-01-01T00:00:00Z")
        );
        let mut references = [a, b];
        references.sort();
        assert_eq!(references, ["ORD-2030-1", "ORD-2030-2"]);
        let mut timezone = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL TIME ZONE 'Pacific/Honolulu'")
            .execute(&mut *timezone)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT next_order_reference('2026-12-31T23:00:00Z'::timestamptz)"
            )
            .fetch_one(&mut *timezone)
            .await
            .unwrap(),
            "ORD-2027-4"
        );
        timezone.rollback().await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let rolled_back: String =
            sqlx::query_scalar("SELECT next_order_reference('2027-01-01T00:00:00Z'::timestamptz)")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(rolled_back, "ORD-2027-4");
        tx.rollback().await.unwrap();
        let reopened = PgPoolOptions::new().connect_with(options).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT next_order_reference('2027-01-01T00:00:00Z'::timestamptz)"
            )
            .fetch_one(&reopened)
            .await
            .unwrap(),
            "ORD-2027-4"
        );
        reopened.close().await;
        // Rolling back the first allocation also leaves the next year starting at one.
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT next_order_reference('2028-01-01T00:00:00Z'::timestamptz)")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(allocate("2028-01-01T00:00:00Z").await, "ORD-2028-1");
        sqlx::query("UPDATE order_reference_counters SET last_number=999999 WHERE year=2028")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(allocate("2028-01-01T00:00:00Z").await, "ORD-2028-1000000");
        sqlx::query(
            "UPDATE order_reference_counters SET last_number=9223372036854775807 WHERE year=2028",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            sqlx::query("SELECT next_order_reference('2028-01-01T00:00:00Z'::timestamptz)")
                .execute(&pool)
                .await
                .is_err(),
            "Exhaustion fails without wrapping"
        );
        assert!(
            sqlx::query("INSERT INTO order_reference_counters VALUES (2029,0)")
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("INSERT INTO order_reference_counters VALUES (10000,1)")
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("SELECT next_order_reference('infinity'::timestamptz)")
                .execute(&pool)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_annual_numbering_is_independent_of_id_replay_and_cancellation() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        sqlx::query("SELECT setval(pg_get_serial_sequence('orders','id'),5000)")
            .execute(&pool).await.unwrap();
        let (_, a, ra) = prepare(&carts, (p, 0), 1, 1).await;
        let first = confirm(&orders, &a, &ra, 1).await.unwrap();
        let year: i32 = sqlx::query_scalar("SELECT extract(year FROM created_at AT TIME ZONE 'Europe/Rome')::integer FROM orders WHERE id=$1")
            .bind(first.id).fetch_one(&pool).await.unwrap();
        assert_eq!(first.id, 5001);
        assert_eq!(first.reference, format!("ORD-{year}-1"));
        assert_eq!(confirm(&orders, &a, &ra, 1).await.unwrap().reference, first.reference);
        orders.cancel(&s, &s.csrf, first.id, 0).await.unwrap();
        let (_, b, rb) = prepare(&carts, (p, 0), 1, 2).await;
        let second = confirm(&orders, &b, &rb, 2).await.unwrap();
        assert_eq!(second.reference, format!("ORD-{year}-2"));
        let messages: Vec<String> = sqlx::query_scalar("SELECT subject FROM email_outbox WHERE order_id=$1")
            .bind(second.id).fetch_all(&pool).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|subject| subject.contains(&second.reference)));
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn field_policy_revalidates_final_order_and_keeps_sdi_pec_in_immutable_snapshot() {
    isolated(|pool, _| async move {
        use talocart_platform::customer_fields::{self, Policy};
        let (product, identity, session) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, guest, review) = prepare(&carts, (product, 0), 1, 1).await;
        let mut choices = Policy::default().choices();
        for (key, mode) in &mut choices {
            if key == "business.recipient_code" || key == "business.pec" {
                *mode = "required".into();
            }
        }
        customer_fields::save(&pool, &session, &session.csrf, 0, &choices)
            .await
            .unwrap();
        let mut d = data();
        d.customer_type = "business".into();
        d.billing_company = "Example".into();
        d.vat_id = "12345678903".into();
        let confirmation = |data| Confirmation {
            guest: &guest,
            csrf: &guest.csrf,
            revision: guest.revision,
            data,
            selection: selection(1),
            review: &review,
            language: lang(),
            online_available: false,
        };
        assert!(matches!(
            orders.confirm(confirmation(d.clone())).await,
            Err(OrderError::Cart(
                talocart_commerce::cart::CartError::Invalid
            ))
        ));
        assert_eq!(count(&pool, "orders").await, 0);
        assert_eq!(count(&pool, "stock").await, 0);
        d.recipient_code = "ABC1234".into();
        d.pec = "invoice@example.test".into();
        let order = orders.confirm(confirmation(d)).await.unwrap();
        assert_eq!(order.frozen.customer.recipient_code, "ABC1234");
        assert_eq!(order.frozen.customer.pec, "invoice@example.test");
        assert_eq!(order.frozen.customer.invoice_channel, "both");
        let snapshot: String = sqlx::query_scalar("SELECT snapshot FROM orders WHERE id=$1")
            .bind(order.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        customer_fields::save(
            &pool,
            &session,
            &session.csrf,
            1,
            &Policy::default().choices(),
        )
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT snapshot FROM orders WHERE id=$1")
                .bind(order.id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            snapshot
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_unmanaged_stock_and_policy_changes_restore_only_actual_debits() {
    isolated(|pool, _| async move {
        let (p, identity, session) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        // Existing managed orders must still restore after stock control is disabled.
        let (_, g, review) = prepare(&carts, (p, 0), 2, 1).await;
        let managed = confirm(&orders, &g, &review, 1).await.unwrap();
        sqlx::query("UPDATE catalog_products SET manage_stock=false WHERE id=$1").bind(p).execute(&pool).await.unwrap();
        orders.cancel(&session, &session.csrf, managed.id, 0).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1").bind(p).fetch_one(&pool).await.unwrap(), 10);
        // A mixed order writes a debit only for the managed line.
        let tracked: i64 = sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) SELECT 'Tracked','TRACKED','tracked',price_cents,4,status,tax_class_id FROM catalog_products WHERE id=$1 RETURNING id").bind(p).fetch_one(&pool).await.unwrap();
        sqlx::query("UPDATE catalog_products SET stock=0 WHERE id=$1").bind(p).execute(&pool).await.unwrap();
        let (raw, g) = carts.create().await.unwrap();
        carts.add(&g, &g.csrf, 0, (p, 0), 900, lang()).await.unwrap();
        let g = carts.session(&raw).await.unwrap().unwrap();
        carts.add(&g, &g.csrf, g.revision, (tracked, 0), 2, lang()).await.unwrap();
        let g = carts.session(&raw).await.unwrap().unwrap();
        let quote = carts.checkout_quote(&g, lang(), "IT", Some(selection(1))).await.unwrap();
        let mixed = confirm(&orders, &g, &quote.review, 1).await.unwrap();
        assert_eq!(confirm(&orders, &g, &quote.review, 1).await.unwrap().id, mixed.id);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_stock_moves m JOIN order_lines l ON l.id=m.line_id WHERE l.order_id=$1").bind(mixed.id).fetch_one(&pool).await.unwrap(), 1);
        // Turning control on after purchase must not invent stock on cancellation.
        sqlx::query("UPDATE catalog_products SET manage_stock=true WHERE id=$1").bind(p).execute(&pool).await.unwrap();
        orders.cancel(&session, &session.csrf, mixed.id, 0).await.unwrap();
        orders.cancel(&session, &session.csrf, mixed.id, 0).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1").bind(p).fetch_one(&pool).await.unwrap(), 0);
        assert_eq!(sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1").bind(tracked).fetch_one(&pool).await.unwrap(), 4);
        // Final checkout rechecks the current policy, including old carts.
        sqlx::query("UPDATE catalog_products SET manage_stock=false WHERE id=$1").bind(p).execute(&pool).await.unwrap();
        let (_, stale, review) = prepare(&carts, (p, 0), 5, 1).await;
        sqlx::query("UPDATE catalog_products SET manage_stock=true WHERE id=$1").bind(p).execute(&pool).await.unwrap();
        assert!(confirm(&orders, &stale, &review, 1).await.is_err());
        assert_eq!(count(&pool, "orders").await, 2);
        // Variant purchases inherit the policy; inactive combinations remain unavailable.
        sqlx::query("UPDATE catalog_products SET manage_stock=false WHERE id=$1").bind(p).execute(&pool).await.unwrap();
        let option: i64 = sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id").fetch_one(&pool).await.unwrap();
        let value: i64 = sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Large') RETURNING id").bind(option).fetch_one(&pool).await.unwrap();
        // The earlier simple orders are cancelled; the stale cart has no stock effect.
        sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)").bind(p).bind(option).execute(&pool).await.unwrap();
        let variant: i64 = sqlx::query_scalar("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],'UNMANAGED-VAR',1000,0) RETURNING id").bind(p).bind(value).fetch_one(&pool).await.unwrap();
        let (_, g, review) = prepare(&carts, (p, variant), 100, 1).await;
        let order = confirm(&orders, &g, &review, 1).await.unwrap();
        orders.cancel(&session, &session.csrf, order.id, 0).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_variants WHERE id=$1").bind(variant).fetch_one(&pool).await.unwrap(), 0);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM order_stock_moves m JOIN order_lines l ON l.id=m.line_id WHERE l.order_id=$1").bind(order.id).fetch_one(&pool).await.unwrap(), 0);
        sqlx::query("UPDATE catalog_variants SET active=false WHERE id=$1").bind(variant).execute(&pool).await.unwrap();
        let (_, g) = carts.create().await.unwrap();
        assert!(carts.add(&g, &g.csrf, 0, (p, variant), 1, lang()).await.is_err());
    }).await;
}
