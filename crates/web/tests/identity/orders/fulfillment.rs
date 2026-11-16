use super::*;
use talocart_commerce::orders::FulfillmentInput;
fn key() -> String {
    talocart_platform::identity::token().unwrap()
}
fn action(action: &str) -> FulfillmentInput {
    FulfillmentInput {
        action: action.into(),
        carrier: "Example courier".into(),
        tracking_number: "TRACK-1".into(),
        tracking_url: "HTTPS://example.test/track/TRACK-1".into(),
        ..Default::default()
    }
}
async fn fresh(pool: &PgPool, p: i64, orders: &Orders, payment: i64) -> (String, Guest, Order) {
    let carts = Carts::new(pool.clone());
    let (raw, g, review) = prepare(&carts, (p, 0), 1, payment).await;
    let o = confirm(orders, &g, &review, payment).await.unwrap();
    (raw, g, o)
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn fulfillment_lifecycle_notes_tracking_replay_and_immutable_snapshots() {
    isolated(|pool,options|async move{
 let(p,identity,s)=configured(&pool).await;let orders=Orders::new(pool.clone(),identity);let(_,g,o)=fresh(&pool,p,&orders,2).await;let snapshot:String=sqlx::query_scalar("SELECT snapshot FROM orders WHERE id=$1").bind(o.id).fetch_one(&pool).await.unwrap();
 let n=FulfillmentInput{action:"note".into(),body:"Private <script>\nKeep internal".into(),..Default::default()};let k=key();let(a,b)=tokio::join!(orders.update_fulfillment(&s,&s.csrf,o.id,0,&k,&n),orders.update_fulfillment(&s,&s.csrf,o.id,0,&k,&n));assert_eq!(a.unwrap().fulfillment.revision,1);b.unwrap();assert_eq!(orders.fulfillment_history(&s,o.id,1).await.unwrap().total,1);
 assert!(matches!(orders.update_fulfillment(&s,&s.csrf,o.id,0,&k,&action("preparing")).await,Err(OrderError::FulfillmentConflict)));
 orders.update_fulfillment(&s,&s.csrf,o.id,1,&key(),&action("preparing")).await.unwrap();let k=key();let shipped=orders.update_fulfillment(&s,&s.csrf,o.id,2,&k,&action("shipped")).await.unwrap();assert_eq!(shipped.fulfillment.state,"shipped");assert_eq!(shipped.revision,0);assert!(!shipped.is_paid());assert_eq!(orders.update_fulfillment(&s,&s.csrf,o.id,2,&k,&action("shipped")).await.unwrap().fulfillment.revision,3);
 assert!(matches!(orders.cancel(&s,&s.csrf,o.id,0).await,Err(OrderError::Shipped)));let mut tracking=action("tracking");tracking.tracking_number="CORRECTED".into();tracking.tracking_url="https://example.test/corrected".into();orders.update_fulfillment(&s,&s.csrf,o.id,3,&key(),&tracking).await.unwrap();orders.update_fulfillment(&s,&s.csrf,o.id,4,&key(),&action("delivered")).await.unwrap();
 let final_order=orders.for_guest(&g,&o.reference).await.unwrap();assert_eq!(final_order.fulfillment.state,"delivered");assert_eq!(final_order.fulfillment.tracking_number,"CORRECTED");assert_eq!(count(&pool,"stock").await,1);assert_eq!(snapshot,sqlx::query_scalar::<_,String>("SELECT snapshot FROM orders WHERE id=$1").bind(o.id).fetch_one(&pool).await.unwrap());
 let mails:Vec<(String,String,bool)>=sqlx::query_as("SELECT template,body_text,superseded FROM email_outbox WHERE order_id=$1 ORDER BY id").bind(o.id).fetch_all(&pool).await.unwrap();assert_eq!(mails.iter().filter(|m|m.0=="order_shipped").count(),1);assert!(mails.iter().all(|m|!m.1.contains("Keep internal")));assert!(mails.iter().find(|m|m.0=="order_tracking").unwrap().2);assert!(!mails.iter().find(|m|m.0=="order_delivered").unwrap().2);
 assert!(sqlx::query("DELETE FROM order_fulfillment_events").execute(&pool).await.is_err());assert!(sqlx::query("UPDATE order_fulfillment SET state='pending',revision=revision+1 WHERE order_id=$1").bind(o.id).execute(&pool).await.is_err());assert!(sqlx::query("UPDATE orders SET state='cancelled',revision=revision+1,cancelled_at=now() WHERE id=$1").bind(o.id).execute(&pool).await.is_err());
 let reopened=PgPoolOptions::new().connect_with(options).await.unwrap();assert_eq!(Orders::new(reopened.clone(),Identity::new(reopened.clone())).for_guest(&g,&o.reference).await.unwrap().fulfillment.state,"delivered");reopened.close().await;
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn fulfillment_payment_validation_and_outbox_rollback() {
    isolated(|pool,_|async move{
 let(p,identity,s)=configured(&pool).await;let orders=Orders::new(pool.clone(),identity);let(_,_,o)=fresh(&pool,p,&orders,1).await;orders.update_fulfillment(&s,&s.csrf,o.id,0,&key(),&action("preparing")).await.unwrap();assert!(matches!(orders.update_fulfillment(&s,&s.csrf,o.id,1,&key(),&action("shipped")).await,Err(OrderError::FulfillmentPayment)));assert_eq!(orders.get(&s,o.id).await.unwrap().fulfillment.revision,1);
 orders.record_payment(&s,&s.csrf,o.id,0,"BANK-1").await.unwrap();let mut invalid=action("shipped");invalid.tracking_url="javascript:alert(1)".into();assert!(matches!(orders.update_fulfillment(&s,&s.csrf,o.id,1,&key(),&invalid).await,Err(OrderError::FulfillmentFields(_))));
 sqlx::raw_sql("CREATE FUNCTION fail_dispatch_mail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic'; END; $$; CREATE TRIGGER fail_dispatch_mail BEFORE INSERT ON email_outbox FOR EACH ROW EXECUTE FUNCTION fail_dispatch_mail();").execute(&pool).await.unwrap();let k=key();assert!(orders.update_fulfillment(&s,&s.csrf,o.id,1,&k,&action("shipped")).await.is_err());assert_eq!(orders.get(&s,o.id).await.unwrap().fulfillment.state,"preparing");assert_eq!(orders.fulfillment_history(&s,o.id,1).await.unwrap().total,1);sqlx::query("DROP TRIGGER fail_dispatch_mail ON email_outbox").execute(&pool).await.unwrap();assert_eq!(orders.update_fulfillment(&s,&s.csrf,o.id,1,&k,&action("shipped")).await.unwrap().fulfillment.state,"shipped");
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn fulfillment_cancel_ship_race_stale_requests_and_private_pagination() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity.clone());
        let (_, _, o) = fresh(&pool, p, &orders, 2).await;
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 0, &key(), &action("preparing"))
            .await
            .unwrap();
        let k = key();
        let shipment = action("shipped");
        let (a, b) = tokio::join!(
            orders.cancel(&s, &s.csrf, o.id, 0),
            orders.update_fulfillment(&s, &s.csrf, o.id, 1, &k, &shipment)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let now = orders.get(&s, o.id).await.unwrap();
        let stock: i32 = sqlx::query_scalar("SELECT stock FROM catalog_products WHERE id=$1")
            .bind(p)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stock, if now.state == "cancelled" { 10 } else { 9 });
        for n in 0..22 {
            let current = orders.get(&s, o.id).await.unwrap();
            orders
                .update_fulfillment(
                    &s,
                    &s.csrf,
                    o.id,
                    current.fulfillment.revision,
                    &key(),
                    &FulfillmentInput {
                        action: "note".into(),
                        body: format!("Internal {n}"),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
        let h = orders.fulfillment_history(&s, o.id, 1).await.unwrap();
        assert_eq!(h.entries.len(), 20);
        assert!(h.total >= 23);
        assert!(
            !orders
                .fulfillment_history(&s, o.id, 2)
                .await
                .unwrap()
                .entries
                .is_empty()
        );
        assert!(matches!(
            orders
                .update_fulfillment(&s, &s.csrf, o.id, 0, &key(), &action("preparing"))
                .await,
            Err(OrderError::Cart(talocart_commerce::cart::CartError::Stale))
        ));
        assert!(
            orders
                .update_fulfillment(&s, "bad", o.id, 0, &key(), &action("preparing"))
                .await
                .is_err()
        );
        let role = identity
            .create_role(&s, &s.csrf, "Fulfillment reader", &[Permission::OrdersRead])
            .await
            .unwrap();
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "fulfillment-reader@example.test",
                "synthetic reader password".into(),
                talocart_platform::identity::Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anon,
                &anon.csrf,
                "fulfillment-reader@example.test",
                "synthetic reader password".into(),
            )
            .await
            .unwrap();
        let reader = identity.session(&raw).await.unwrap().unwrap();
        assert!(orders.fulfillment_history(&reader, o.id, 1).await.is_ok());
        assert!(
            orders
                .update_fulfillment(&reader, &reader.csrf, o.id, 0, &key(), &action("preparing"))
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL, npm ci and Playwright Chromium"]
async fn fulfillment_browser_bilingual_desktop_mobile_no_js_and_private_notes() {
    isolated(|pool,_|async move{
 use talocart_platform::identity::{Customers,CustomerProfile};
 let(p,identity,s)=configured(&pool).await;let admin=authenticated(&identity).await;let customers=Customers::new(pool.clone(),identity.clone());let(_,anon)=customers.anonymous().await.unwrap();let customer=customers.register(&anon,&anon.csrf,&CustomerProfile{email:"fulfillment-buyer@example.test".into(),first_name:"Alex".into(),last_name:"Example".into(),customer_type:"private".into()},"synthetic fulfillment password".into(), None).await.unwrap().expect("Immediate activation");let session=customers.session(&customer).await.unwrap().unwrap();let carts=Carts::new(pool.clone());carts.ensure_customer(&session).await.unwrap();let orders=Orders::new(pool.clone(),identity);let mut profiles=vec![];
 for _ in 0..8{let g=carts.customer_cart(&session).await.unwrap().unwrap();carts.add(&g,&g.csrf,g.revision,(p,0),1,lang()).await.unwrap();let g=carts.customer_cart(&session).await.unwrap().unwrap();let q=carts.checkout_quote(&g,lang(),"IT",Some(selection(2))).await.unwrap();let o=orders.confirm_for_customer(Confirmation{guest:&g,csrf:&g.csrf,revision:g.revision,data:data(),selection:selection(2),review:&q.review,language:lang(),online_available:false},&session).await.unwrap();profiles.push(serde_json::json!({"id":o.id,"reference":o.reference}));}
 let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let origin=format!("http://{}",listener.local_addr().unwrap());let state=test_app_config(pool.clone(),"Fulfillment test".into(), talocart_web::AppStateConfig { public_origin: origin.clone(), ..Default::default() }).await;let task=tokio::spawn(async move{axum::serve(listener,router(state).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});let mut child=tokio::process::Command::new("node").arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/browser-fulfillment.cjs")).current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();use tokio::io::AsyncWriteExt;child.stdin.take().unwrap().write_all(serde_json::json!({"origin":origin,"admin":admin,"customer":customer,"profiles":profiles}).to_string().as_bytes()).await.unwrap();let result=child.wait_with_output().await.unwrap();task.abort();let _=task.await;assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));println!("{}",String::from_utf8_lossy(&result.stdout));
 let private:i64=sqlx::query_scalar("SELECT count(*) FROM email_outbox WHERE body_text LIKE '%PRIVATE-NOTE%'").fetch_one(&pool).await.unwrap();assert_eq!(private,0);let _=s;
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn fulfillment_zero_total_and_tracking_after_delivery_keep_physical_truth() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity);
        sqlx::raw_sql(
            "UPDATE catalog_products SET price_cents=0; UPDATE shipping_methods SET fee_value=0;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (_, _, o) = fresh(&pool, p, &orders, 1).await;
        assert_eq!(o.total_cents, 0);
        assert!(!o.is_paid());
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 0, &key(), &action("preparing"))
            .await
            .unwrap();
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 1, &key(), &action("shipped"))
            .await
            .unwrap();
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 2, &key(), &action("delivered"))
            .await
            .unwrap();
        let mut corrected = action("tracking");
        corrected.tracking_number = "CORRECTED-AFTER-DELIVERY".into();
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 3, &key(), &corrected)
            .await
            .unwrap();
        assert_eq!(
            orders.get(&s, o.id).await.unwrap().fulfillment.state,
            "delivered"
        );
        let body: String = sqlx::query_scalar(
            "SELECT body_text FROM email_outbox WHERE template='order_tracking'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(body.contains("remains delivered") || body.contains("resta consegnato"));
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT superseded FROM email_outbox WHERE template='order_delivered'"
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL and loopback SMTP fixture"]
async fn fulfillment_delivery_mail_waits_for_inflight_shipment_and_preserves_notes() {
    isolated(|pool,_|async move{
 let(p,identity,s)=configured(&pool).await;let orders=Orders::new(pool.clone(),identity.clone());let(_,_,o)=fresh(&pool,p,&orders,2).await;let mail=talocart_platform::mail::Mail::new(pool.clone(),identity);let(config,_,delivered,task)=super::super::email::smtp(250).await;mail.process_one(&config).await.unwrap();mail.process_one(&config).await.unwrap();assert_eq!(delivered.load(std::sync::atomic::Ordering::SeqCst),2);
 let body="  PRIVATE-NOTE\nIndented\n";orders.update_fulfillment(&s,&s.csrf,o.id,0,&key(),&FulfillmentInput{action:"note".into(),body:body.into(),..Default::default()}).await.unwrap();assert_eq!(orders.fulfillment_history(&s,o.id,1).await.unwrap().entries[0].body,body);
 orders.update_fulfillment(&s,&s.csrf,o.id,1,&key(),&action("preparing")).await.unwrap();orders.update_fulfillment(&s,&s.csrf,o.id,2,&key(),&action("shipped")).await.unwrap();sqlx::query("UPDATE email_outbox SET state='sending',lease_token=$1,lease_until=clock_timestamp()+interval '2 minutes' WHERE template='order_shipped'").bind(key()).execute(&pool).await.unwrap();orders.update_fulfillment(&s,&s.csrf,o.id,3,&key(),&action("delivered")).await.unwrap();assert!(!mail.process_one(&config).await.unwrap());sqlx::query("UPDATE email_outbox SET state='superseded',lease_token=NULL,lease_until=NULL WHERE template='order_shipped'").execute(&pool).await.unwrap();assert!(mail.process_one(&config).await.unwrap());assert_eq!(delivered.load(std::sync::atomic::Ordering::SeqCst),3);assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM email_outbox WHERE template='order_delivered'").fetch_one(&pool).await.unwrap(),"accepted");task.abort();
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn fulfillment_http_csrf_post_only_and_no_private_note_in_customer_receipts() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let admin = authenticated(&identity).await;
        let orders = Orders::new(pool.clone(), identity);
        let (raw, _, o) = fresh(&pool, p, &orders, 2).await;
        let app = router(test_app(pool.clone(), "Fulfillment".into()).await);
        let cookie = format!("talocart_session={admin}");
        let url = format!("/admin/orders/{}/fulfillment", o.id);
        assert_eq!(
            request(&app, "GET", &url, &cookie, "", false)
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            request(
                &app,
                "POST",
                &url,
                &cookie,
                "csrf=bad&action=note&body=SECRET",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        orders
            .update_fulfillment(
                &s,
                &s.csrf,
                o.id,
                0,
                &key(),
                &FulfillmentInput {
                    action: "note".into(),
                    body: "PRIVATE-NOTE secret".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        for prefix in ["", "/it"] {
            let response = request(
                &app,
                "GET",
                &format!("{prefix}/checkout/orders/{}", o.reference),
                &format!("talocart_cart={raw}"),
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let h = html(response).await;
            assert!(!h.contains("PRIVATE-NOTE"));
            assert!(!h.contains("data-private-note"));
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn audit_fulfillment_unchanged_tracking_is_noop() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity);
        sqlx::raw_sql(
            "UPDATE catalog_products SET price_cents=0; UPDATE shipping_methods SET fee_value=0;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (_, _, o) = fresh(&pool, p, &orders, 1).await;
        assert_eq!(o.total_cents, 0);
        assert!(!o.is_paid());
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 0, &key(), &action("preparing"))
            .await
            .unwrap();
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 1, &key(), &action("shipped"))
            .await
            .unwrap();
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 2, &key(), &action("delivered"))
            .await
            .unwrap();
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 3, &key(), &action("tracking"))
            .await
            .unwrap();
        assert_eq!(
            orders.get(&s, o.id).await.unwrap().fulfillment.state,
            "delivered"
        );
        assert_eq!(
            orders.get(&s, o.id).await.unwrap().fulfillment.revision,
            3,
            "Unchanged tracking must not increment revision or send an update"
        );
        assert_eq!(
            orders.fulfillment_history(&s, o.id, 1).await.unwrap().total,
            3
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM email_outbox WHERE template='order_tracking'"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            0
        );
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 3, &key(), &action("tracking"))
            .await
            .unwrap();
        assert_eq!(orders.get(&s, o.id).await.unwrap().fulfillment.revision, 3);
        let k = key();
        let mut normalized = action("tracking");
        normalized.carrier = format!("  {}  ", normalized.carrier);
        let (a, b) = tokio::join!(
            orders.update_fulfillment(&s, &s.csrf, o.id, 3, &k, &normalized),
            orders.update_fulfillment(&s, &s.csrf, o.id, 3, &k, &normalized)
        );
        assert_eq!(a.unwrap().fulfillment.revision, 3);
        assert_eq!(b.unwrap().fulfillment.revision, 3);
        let mut changed = action("tracking");
        changed.tracking_number = "NEW".into();
        orders
            .update_fulfillment(&s, &s.csrf, o.id, 3, &k, &changed)
            .await
            .unwrap();
        assert_eq!(
            orders
                .update_fulfillment(&s, &s.csrf, o.id, 3, &k, &changed)
                .await
                .unwrap()
                .fulfillment
                .revision,
            4
        );
        assert!(matches!(
            orders
                .update_fulfillment(&s, &s.csrf, o.id, 3, &key(), &normalized)
                .await,
            Err(OrderError::Cart(_))
        ));
        assert!(matches!(
            orders
                .update_fulfillment(&s, &s.csrf, o.id, 4, &k, &normalized)
                .await,
            Err(OrderError::FulfillmentConflict)
        ));
    })
    .await;
}
