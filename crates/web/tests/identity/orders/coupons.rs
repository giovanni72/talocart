use super::*;
use talocart_commerce::{
    cart::CartError,
    coupons::{CouponError, Coupons, Input},
};
fn input() -> Input {
    Input {
        code: "save10".into(),
        kind: "percent".into(),
        value: "10".into(),
        active: true,
        currency: "EUR".into(),
        ..Input::default()
    }
}
async fn discounted(carts: &Carts, p: i64) -> (String, Guest, String) {
    let (raw, g, _) = prepare(carts, (p, 0), 2, 1).await;
    carts
        .set_coupon(&g, &g.csrf, g.revision, " Save10 ", lang())
        .await
        .unwrap();
    let g = carts.session(&raw).await.unwrap().unwrap();
    let q = carts
        .checkout_quote(&g, lang(), "IT", Some(selection(1)))
        .await
        .unwrap();
    (raw, g, q.review)
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn coupons_order_snapshot_replay_cancellation_and_permanent_uses() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let service = Coupons::new(pool.clone(), identity.clone());
        let id = service.save(&s, &s.csrf, 0, 0, &input()).await.unwrap();
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, g, review) = discounted(&carts, p).await;
        let q = carts.quote(&g, lang()).await.unwrap();
        let c = q.coupon.unwrap();
        assert_eq!(c.original_cents, 2440);
        assert_eq!(c.discount_cents, 244);
        assert_eq!(q.totals.unwrap().gross().cents(), 2196);
        let (a, b) = tokio::join!(
            confirm(&orders, &g, &review, 1),
            confirm(&orders, &g, &review, 1)
        );
        let a = a.unwrap();
        assert_eq!(a.id, b.unwrap().id);
        assert_eq!(a.merchandise_cents, 2196);
        assert_eq!(a.frozen.coupon.as_ref().unwrap().discount_cents, 244);
        assert_eq!(service.get(&s, id).await.unwrap().uses, 1);
        orders.cancel(&s, &s.csrf, a.id, a.revision).await.unwrap();
        assert_eq!(service.get(&s, id).await.unwrap().uses, 1);
        assert_eq!(confirm(&orders, &g, &review, 1).await.unwrap().id, a.id);
        let mut i = input();
        i.value = "80".into();
        service.save(&s, &s.csrf, id, 0, &i).await.unwrap();
        let frozen = orders.get(&s, a.id).await.unwrap();
        assert_eq!(frozen.frozen.coupon.unwrap().discount_cents, 244);
        assert_eq!(frozen.lines[0].gross_cents, 2196);
        assert!(
            sqlx::query("DELETE FROM coupon_uses")
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE coupon_uses SET discount_cents=0")
                .execute(&pool)
                .await
                .is_err()
        );
        let text: String =
            sqlx::query_scalar("SELECT body_text FROM email_outbox ORDER BY id LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(text.contains("SAVE10"));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn coupons_last_use_concurrency_expiry_revocation_and_rollback() {
    isolated(|pool,_|async move{
 let(p,identity,s)=configured(&pool).await;let service=Coupons::new(pool.clone(),identity.clone());let mut i=input();i.usage_limit="1".into();let id=service.save(&s,&s.csrf,0,0,&i).await.unwrap();let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity);
 let(_,a,ar)=discounted(&carts,p).await;let(_,b,br)=discounted(&carts,p).await;
 sqlx::raw_sql("CREATE FUNCTION fail_coupon_mail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic'; END; $$; CREATE TRIGGER fail_coupon_mail BEFORE INSERT ON email_outbox FOR EACH ROW EXECUTE FUNCTION fail_coupon_mail();").execute(&pool).await.unwrap();assert!(confirm(&orders,&a,&ar,1).await.is_err());assert_eq!(service.get(&s,id).await.unwrap().uses,0);assert_eq!(count(&pool,"orders").await,0);sqlx::query("DROP TRIGGER fail_coupon_mail ON email_outbox").execute(&pool).await.unwrap();
 let(x,y)=tokio::join!(confirm(&orders,&a,&ar,1),confirm(&orders,&b,&br,1));assert_eq!(usize::from(x.is_ok())+usize::from(y.is_ok()),1);assert_eq!(service.get(&s,id).await.unwrap().uses,1);let loser=if x.is_err(){&a}else{&b};assert!(carts.quote(loser,lang()).await.unwrap().coupon_invalid);
 orders.cancel(&s,&s.csrf,x.or(y).unwrap().id,0).await.unwrap();assert!(carts.quote(loser,lang()).await.unwrap().coupon_invalid);
 i.usage_limit.clear();i.expires_on="2000-01-01".into();service.save(&s,&s.csrf,id,0,&i).await.unwrap();assert!(carts.quote(loser,lang()).await.unwrap().coupon_invalid);
 i.expires_on.clear();i.active=false;service.save(&s,&s.csrf,id,1,&i).await.unwrap();assert!(carts.quote(loser,lang()).await.unwrap().coupon_invalid);
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn coupons_validation_auth_revision_currency_and_full_discount() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let service = Coupons::new(pool.clone(), identity);
        assert!(matches!(
            service.save(&s, "bad", 0, 0, &input()).await,
            Err(CouponError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        let mut i = input();
        for bad in ["0", "-1", "100.01", "NaN"] {
            i.value = bad.into();
            assert!(service.save(&s, &s.csrf, 0, 0, &i).await.is_err())
        }
        i.kind = "fixed".into();
        i.value = "999999".into();
        let id = service.save(&s, &s.csrf, 0, 0, &i).await.unwrap();
        assert!(matches!(
            service.save(&s, &s.csrf, 0, 0, &i).await,
            Err(CouponError::Duplicate)
        ));
        assert!(matches!(
            service.save(&s, &s.csrf, id, 5, &i).await,
            Err(CouponError::Stale)
        ));
        assert!(
            sqlx::query("UPDATE store_settings SET currency='USD' WHERE id=1")
                .execute(&pool)
                .await
                .is_err()
        );
        let carts = Carts::new(pool.clone());
        let (raw, g, _) = discounted(&carts, p).await;
        let q = carts.quote(&g, lang()).await.unwrap();
        assert_eq!(q.totals.unwrap().gross().cents(), 0);
        assert_eq!(q.totals.unwrap().tax().cents(), 0);
        assert_eq!(q.coupon.unwrap().discount_cents, 2440);
        assert!(matches!(
            carts.set_coupon(&g, "bad", g.revision, "", lang()).await,
            Err(CartError::Forbidden)
        ));
        assert!(matches!(
            carts.set_coupon(&g, &g.csrf, 0, "", lang()).await,
            Err(CartError::Stale)
        ));
        carts
            .set_coupon(&g, &g.csrf, g.revision, "", lang())
            .await
            .unwrap();
        let fresh = carts.session(&raw).await.unwrap().unwrap();
        assert!(carts.quote(&fresh, lang()).await.unwrap().coupon.is_none());
        assert_eq!(service.get(&s, id).await.unwrap().uses, 0);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL, npm ci and Playwright Chromium"]
async fn coupons_browser_bilingual_desktop_mobile_and_no_js() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let service = Coupons::new(pool.clone(), identity.clone());
        service.save(&s, &s.csrf, 0, 0, &input()).await.unwrap();
        service
            .save(
                &s,
                &s.csrf,
                0,
                0,
                &Input {
                    code: "X".repeat(40),
                    ..input()
                },
            )
            .await
            .unwrap();
        let admin = authenticated(&identity).await;
        let carts = Carts::new(pool.clone());
        let mut profiles = vec![];
        for _ in 0..8 {
            let (raw, g, _) = prepare(&carts, (p, 0), 1, 1).await;
            carts
                .save_details(&g, &g.csrf, g.revision, data())
                .await
                .unwrap();
            profiles.push(raw);
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = test_app_config(
            pool.clone(),
            "Coupon test".into(),
            talocart_web::AppStateConfig {
                public_origin: origin.clone(),
                ..Default::default()
            },
        )
        .await;
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
        let mut child = tokio::process::Command::new("node")
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../scripts/browser-coupons.cjs"),
            )
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use tokio::io::AsyncWriteExt;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                serde_json::json!({"origin":origin,"admin":admin,"profiles":profiles})
                    .to_string()
                    .as_bytes(),
            )
            .await
            .unwrap();
        let result = child.wait_with_output().await.unwrap();
        task.abort();
        let _ = task.await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        println!("{}", String::from_utf8_lossy(&result.stdout));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn coupons_mixed_taxes_thresholds_and_changed_review() {
    isolated(|pool,_|async move{
 let(p,identity,s)=configured(&pool).await;let service=Coupons::new(pool.clone(),identity.clone());let mut i=input();i.kind="fixed".into();i.value="5.00".into();let id=service.save(&s,&s.csrf,0,0,&i).await.unwrap();
 sqlx::raw_sql("INSERT INTO tax_rates(name,basis_points) VALUES('Zero',0); INSERT INTO tax_classes(name,rate_id) SELECT 'Zero',id FROM tax_rates WHERE name='Zero'; UPDATE shipping_methods SET free_from_cents=2000 WHERE id=1; UPDATE payment_methods SET minimum_cents=2000 WHERE id=2;").execute(&pool).await.unwrap();
 let p2:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) SELECT 'Second','SECOND','second',1000,10,'published',id FROM tax_classes WHERE name='Zero' RETURNING id").fetch_one(&pool).await.unwrap();let carts=Carts::new(pool.clone());let(raw,g,_)=prepare(&carts,(p,0),1,1).await;carts.add(&g,&g.csrf,g.revision,(p2,0),1,lang()).await.unwrap();let g=carts.session(&raw).await.unwrap().unwrap();let before=carts.checkout_quote(&g,lang(),"IT",Some(selection(2))).await.unwrap();assert_eq!(before.cart.totals.unwrap().gross().cents(),2220);assert_eq!(before.shipping_cost.unwrap().cents(),0);assert!(!before.invalid_selection);
 carts.set_coupon(&g,&g.csrf,g.revision,"SAVE10",lang()).await.unwrap();let g=carts.session(&raw).await.unwrap().unwrap();let q=carts.checkout_quote(&g,lang(),"IT",Some(selection(2))).await.unwrap();assert_eq!(q.cart.totals.unwrap().gross().cents(),1720);assert_eq!(q.shipping_cost.unwrap().cents(),122);assert!(q.invalid_selection);let c=q.cart.coupon.unwrap();assert_eq!(c.lines.iter().map(|l|l.discount_cents).sum::<i64>(),500);for l in q.cart.lines{let t=l.totals.unwrap();assert_eq!(t.net().cents()+t.tax().cents(),t.gross().cents());}
 let q=carts.checkout_quote(&g,lang(),"IT",Some(selection(1))).await.unwrap();i.value="6".into();service.save(&s,&s.csrf,id,0,&i).await.unwrap();let orders=Orders::new(pool.clone(),identity);assert!(matches!(confirm(&orders,&g,&q.review,1).await,Err(OrderError::Review)));assert_eq!(service.get(&s,id).await.unwrap().uses,0);
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn coupons_http_csrf_bilingual_and_admin_permission() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let service = Coupons::new(pool.clone(), identity.clone());
        service.save(&s, &s.csrf, 0, 0, &input()).await.unwrap();
        let admin = authenticated(&identity).await;
        let carts = Carts::new(pool.clone());
        let (raw, g, _) = prepare(&carts, (p, 0), 1, 1).await;
        let app = router(test_app(pool.clone(), "Coupon".into()).await);
        let cookie = format!("talocart_cart={raw}");
        for prefix in ["", "/it"] {
            let r = request(
                &app,
                "POST",
                &format!("{prefix}/cart/coupon"),
                &cookie,
                "csrf=bad&revision=1&code=SAVE10&action=apply",
                false,
            )
            .await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN);
            let h = html(request(&app, "GET", &format!("{prefix}/cart"), &cookie, "", false).await)
                .await;
            assert!(h.contains("id=\"coupon-code\""));
            assert!(!h.contains("coupon-apply"));
        }
        let r = request(
            &app,
            "POST",
            "/cart/coupon",
            &cookie,
            &format!(
                "csrf={}&revision={}&code=SAVE10&action=apply",
                g.csrf, g.revision
            ),
            false,
        )
        .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            request(&app, "GET", "/cart/coupon", &cookie, "", false)
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_ne!(
            request(&app, "GET", "/admin/settings/coupons", "", "", false)
                .await
                .status(),
            StatusCode::OK
        );
        let ac = format!("talocart_session={admin}");
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/coupons/new",
                &ac,
                "csrf=bad&revision=0&code=HACK&kind=percent&value=10&currency=EUR",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "coupon-reader@example.test",
                "synthetic viewer password".into(),
                talocart_platform::identity::Role::Viewer,
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anon,
                &anon.csrf,
                "coupon-reader@example.test",
                "synthetic viewer password".into(),
            )
            .await
            .unwrap();
        let reader = identity.session(&raw).await.unwrap().unwrap();
        assert!(service.list(&reader, 1).await.is_ok());
        assert!(
            service
                .save(
                    &reader,
                    &reader.csrf,
                    0,
                    0,
                    &Input {
                        code: "VIEWER".into(),
                        ..input()
                    }
                )
                .await
                .is_err()
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn coupons_customer_merge_keeps_customer_code_and_transfers_guest_only_if_absent() {
    isolated(|pool, _| async move {
        use talocart_platform::identity::{CustomerProfile, Customers};
        let (p, identity, s) = configured(&pool).await;
        let service = Coupons::new(pool.clone(), identity.clone());
        service.save(&s, &s.csrf, 0, 0, &input()).await.unwrap();
        service
            .save(
                &s,
                &s.csrf,
                0,
                0,
                &Input {
                    code: "OTHER".into(),
                    value: "20".into(),
                    ..input()
                },
            )
            .await
            .unwrap();
        let customers = Customers::new(pool.clone(), identity);
        let (_, anon) = customers.anonymous().await.unwrap();
        let raw = customers
            .register(
                &anon,
                &anon.csrf,
                &CustomerProfile {
                    email: "coupon-merge@example.test".into(),
                    first_name: "Alex".into(),
                    last_name: "Example".into(),
                    customer_type: "private".into(),
                },
                "synthetic merge password".into(),
                None,
            )
            .await
            .unwrap()
            .expect("Immediate activation");
        let c = customers.session(&raw).await.unwrap().unwrap();
        let carts = Carts::new(pool.clone());
        let (raw, g, _) = discounted(&carts, p).await;
        carts
            .merge_customer(&c, &c.csrf, &g, &g.csrf, g.revision)
            .await
            .unwrap();
        let target = carts.customer_cart(&c).await.unwrap().unwrap();
        assert_eq!(
            carts.quote(&target, lang()).await.unwrap().coupon_code,
            "SAVE10"
        );
        assert!(
            carts
                .quote(&carts.session(&raw).await.unwrap().unwrap(), lang())
                .await
                .unwrap()
                .coupon_code
                .is_empty()
        );
        let (raw, g, _) = prepare(&carts, (p, 0), 1, 1).await;
        carts
            .set_coupon(&g, &g.csrf, g.revision, "OTHER", lang())
            .await
            .unwrap();
        let g = carts.session(&raw).await.unwrap().unwrap();
        carts
            .merge_customer(&c, &c.csrf, &g, &g.csrf, g.revision)
            .await
            .unwrap();
        let target = carts.customer_cart(&c).await.unwrap().unwrap();
        let quote = carts.quote(&target, lang()).await.unwrap();
        assert_eq!(quote.coupon_code, "SAVE10");
        assert_eq!(quote.lines[0].quantity, 3);
        assert_eq!(quote.coupon.unwrap().discount_cents, 366);
    })
    .await;
}
