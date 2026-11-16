use super::*;

async fn receipt_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM order_manual_payments")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn manual_payment_full_receipt_replay_frozen_values_and_restart() {
    isolated(|pool, options| async move {
        let (p, identity, s) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity.clone());
        for method in [1, 2] {
            let (raw, g, r) = prepare(&carts, (p, 0), 1, method).await;
            let o = confirm(&orders, &g, &r, method).await.unwrap();
            let snapshot: String = sqlx::query_scalar("SELECT snapshot FROM orders WHERE id=$1")
                .bind(o.id)
                .fetch_one(&pool)
                .await
                .unwrap();
            let (a, b) = tokio::join!(
                orders.record_payment(&s, &s.csrf, o.id, 0, "  TRX-123  "),
                orders.record_payment(&s, &s.csrf, o.id, 0, "TRX-123")
            );
            let a = a.unwrap();
            assert!(b.unwrap().is_paid());
            assert!(a.is_paid());
            assert_eq!(a.revision, 1);
            let receipt = a.manual_payment.as_ref().unwrap();
            assert_eq!(receipt.amount_cents, o.total_cents);
            assert_eq!(receipt.currency, o.currency);
            assert_eq!(receipt.administrator_id, s.administrator_id.unwrap());
            assert_eq!(receipt.reference, "TRX-123");
            assert!(!receipt.recorded_at.is_empty());
            assert_eq!(
                receipt.method_code,
                if method == 1 {
                    "bank_transfer"
                } else {
                    "cash_on_delivery"
                }
            );
            assert!(matches!(
                orders.record_payment(&s, &s.csrf, o.id, 1, "OTHER").await,
                Err(OrderError::AlreadyPaid)
            ));
            assert!(matches!(
                orders.cancel(&s, &s.csrf, o.id, 1).await,
                Err(OrderError::PaidCancellation)
            ));
            assert_eq!(
                snapshot,
                sqlx::query_scalar::<_, String>("SELECT snapshot FROM orders WHERE id=$1")
                    .bind(o.id)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            );
            let mail: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT template,state,body_text FROM email_outbox WHERE order_id=$1 ORDER BY id",
            )
            .bind(o.id)
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(mail.len(), 3);
            assert_eq!(mail[0].1, "superseded");
            assert_eq!(mail[1].1, "pending");
            assert_eq!(mail[2].0, "payment_received");
            assert!(mail[2].2.contains("Payment recorded"));
            assert!(!mail[2].2.contains("IBAN:"));
            let reopened = PgPoolOptions::new()
                .connect_with(options.clone())
                .await
                .unwrap();
            let persisted = Orders::new(reopened.clone(), Identity::new(reopened.clone()))
                .for_guest(&g, &o.reference)
                .await
                .unwrap();
            assert_eq!(persisted.manual_payment.unwrap().reference, "TRX-123");
            reopened.close().await;
            assert!(carts.session(&raw).await.unwrap().is_some());
        }
        assert_eq!(receipt_count(&pool).await, 2);
        assert_eq!(count(&pool, "stock").await, 2);
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1")
                .bind(p)
                .fetch_one(&pool)
                .await
                .unwrap(),
            8
        );
        assert_eq!(
            orders
                .list(&s, 1)
                .await
                .unwrap()
                .entries
                .iter()
                .filter(|o| o.paid)
                .count(),
            2
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn manual_payment_competing_references_and_cancellation_have_one_winner() {
    isolated(|pool,_|async move {
        let (p,identity,s)=configured(&pool).await; let carts=Carts::new(pool.clone()); let orders=Orders::new(pool.clone(),identity);
        let (_,g,r)=prepare(&carts,(p,0),1,1).await; let o=confirm(&orders,&g,&r,1).await.unwrap();
        let (a,b)=tokio::join!(orders.record_payment(&s,&s.csrf,o.id,0,"A"),orders.record_payment(&s,&s.csrf,o.id,0,"B"));
        assert_eq!([a.is_ok(),b.is_ok()].into_iter().filter(|x|*x).count(),1);
        assert!(matches!(a.as_ref().err().or(b.as_ref().err()),Some(OrderError::AlreadyPaid)));
        for _ in 0..4 {
            let (_,g,r)=prepare(&carts,(p,0),1,2).await;let o=confirm(&orders,&g,&r,2).await.unwrap();
            let (pay,cancel)=tokio::join!(orders.record_payment(&s,&s.csrf,o.id,0,"RACE"),orders.cancel(&s,&s.csrf,o.id,0));
            assert_ne!(pay.is_ok(),cancel.is_ok());
            let current=orders.get(&s,o.id).await.unwrap();
            if current.is_paid() {assert_eq!(current.state,"open");assert!(matches!(cancel,Err(OrderError::PaidCancellation)));}
            else {assert_eq!(current.state,"cancelled");assert!(matches!(pay,Err(OrderError::PaymentCancelled)));}
            let credits:i64=sqlx::query_scalar("SELECT count(*) FROM order_stock_moves m JOIN order_lines l ON l.id=m.line_id WHERE l.order_id=$1 AND kind='credit'").bind(o.id).fetch_one(&pool).await.unwrap();
            assert_eq!(credits,if current.is_paid(){0}else{1});
        }
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn manual_payment_validation_stale_csrf_revocation_and_snapshot_amount() {
    isolated(|pool,_|async move {
        let(p,identity,s)=configured(&pool).await;let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity.clone());
        let(_,g,r)=prepare(&carts,(p,0),1,1).await;let o=confirm(&orders,&g,&r,1).await.unwrap();
        for reference in ["".to_owned()," ".into(),"é".repeat(161),"A\nB".into()] {
            assert!(matches!(orders.record_payment(&s,&s.csrf,o.id,0,&reference).await,Err(OrderError::PaymentReference)));
        }
        assert!(matches!(orders.record_payment(&s,"bad",o.id,0,"REF").await,Err(OrderError::Access(talocart_platform::technical::AccessError::Forbidden))));
        assert!(matches!(orders.record_payment(&s,&s.csrf,o.id,99,"REF").await,Err(OrderError::Cart(talocart_commerce::cart::CartError::Stale))));
        sqlx::query("UPDATE administrators SET active=false WHERE id=$1").bind(s.administrator_id).execute(&pool).await.unwrap();
        assert!(matches!(orders.record_payment(&s,&s.csrf,o.id,0,"REF").await,Err(OrderError::Access(talocart_platform::technical::AccessError::Forbidden))));
        sqlx::query("UPDATE administrators SET active=true WHERE id=$1").bind(s.administrator_id).execute(&pool).await.unwrap();
        assert_eq!(receipt_count(&pool).await,0);
        sqlx::raw_sql("UPDATE catalog_products SET price_cents=5000; UPDATE payment_methods SET active=false;").execute(&pool).await.unwrap();
        let paid=orders.record_payment(&s,&s.csrf,o.id,0,"REF").await.unwrap();
        assert_eq!(paid.manual_payment.unwrap().amount_cents,o.total_cents);
        assert_eq!(count(&pool,"stock").await,1);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn manual_payment_email_failure_rolls_back_receipt_revision_and_supersession() {
    isolated(|pool,_|async move {
        let(p,identity,s)=configured(&pool).await;let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity);
        let(_,g,r)=prepare(&carts,(p,0),1,1).await;let o=confirm(&orders,&g,&r,1).await.unwrap();
        sqlx::raw_sql("CREATE FUNCTION fail_payment_mail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.template='payment_received' THEN RAISE EXCEPTION 'synthetic failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_payment_mail BEFORE INSERT ON email_outbox FOR EACH ROW EXECUTE FUNCTION fail_payment_mail();").execute(&pool).await.unwrap();
        assert!(matches!(orders.record_payment(&s,&s.csrf,o.id,0,"ROLLBACK").await,Err(OrderError::Technical(_))));
        let unpaid=orders.get(&s,o.id).await.unwrap();assert!(!unpaid.is_paid());assert_eq!(unpaid.revision,0);
        assert_eq!(receipt_count(&pool).await,0);assert_eq!(count(&pool,"mail").await,2);assert_eq!(count(&pool,"stock").await,1);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM email_outbox WHERE superseded").fetch_one(&pool).await.unwrap(),0);
        sqlx::raw_sql("DROP TRIGGER fail_payment_mail ON email_outbox; DROP FUNCTION fail_payment_mail();").execute(&pool).await.unwrap();
        assert!(orders.record_payment(&s,&s.csrf,o.id,0,"ROLLBACK").await.unwrap().is_paid());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn manual_payment_database_constraints_and_immutable_history() {
    isolated(|pool,_|async move {
        let(p,identity,s)=configured(&pool).await;let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity);
        let(_,g,r)=prepare(&carts,(p,0),1,1).await;let o=confirm(&orders,&g,&r,1).await.unwrap();
        assert!(sqlx::query("UPDATE orders SET payment_state='paid',revision=revision+1 WHERE id=$1").bind(o.id).execute(&pool).await.is_err());
        for (amount,currency) in [(o.total_cents+1,"EUR"),(o.total_cents,"USD")] {
            assert!(sqlx::query("INSERT INTO order_manual_payments(order_id,amount_cents,currency,method_code,reference,administrator_id,administrator_name) VALUES($1,$2,$3,'bank_transfer','DIRECT',$4,'Operator')").bind(o.id).bind(amount).bind(currency).bind(s.administrator_id).execute(&pool).await.is_err());
        }
        assert_eq!(receipt_count(&pool).await,0);
        orders.record_payment(&s,&s.csrf,o.id,0,"IMMUTABLE").await.unwrap();
        for sql in ["DELETE FROM order_manual_payments","UPDATE order_manual_payments SET reference='changed'","UPDATE orders SET state='cancelled',cancelled_at=now(),revision=revision+1","UPDATE orders SET total_cents=0"] {assert!(sqlx::query(sql).execute(&pool).await.is_err());}
        assert_eq!(receipt_count(&pool).await,1);assert_eq!(count(&pool,"stock").await,1);
        let(_,g,r)=prepare(&carts,(p,0),1,2).await;let o=confirm(&orders,&g,&r,2).await.unwrap();orders.cancel(&s,&s.csrf,o.id,0).await.unwrap();
        assert!(matches!(orders.record_payment(&s,&s.csrf,o.id,0,"LATE").await,Err(OrderError::PaymentCancelled)));
        assert_eq!(receipt_count(&pool).await,1);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn manual_payment_variant_stock_zero_total_and_unicode_reference() {
    isolated(|pool,_|async move {
        let(p,identity,s)=configured(&pool).await;let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity);
        let option:i64=sqlx::query_scalar("INSERT INTO catalog_options(name) VALUES('Size') RETURNING id").fetch_one(&pool).await.unwrap();
        let value:i64=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Large') RETURNING id").bind(option).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,1,$2)").bind(p).bind(option).execute(&pool).await.unwrap();
        let variant:i64=sqlx::query_scalar("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],'PAY-VARIANT',1000,3) RETURNING id").bind(p).bind(value).fetch_one(&pool).await.unwrap();
        let(_,g,r)=prepare(&carts,(p,variant),2,2).await;let o=confirm(&orders,&g,&r,2).await.unwrap();
        orders.record_payment(&s,&s.csrf,o.id,0,&"é".repeat(160)).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_variants WHERE id=$1").bind(variant).fetch_one(&pool).await.unwrap(),1);
        assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_products WHERE id=$1").bind(p).fetch_one(&pool).await.unwrap(),10);
        sqlx::raw_sql("UPDATE catalog_variants SET price_cents=0;UPDATE shipping_methods SET fee_value=0;UPDATE payment_methods SET fee_value=0;").execute(&pool).await.unwrap();
        let(_,g,r)=prepare(&carts,(p,variant),1,1).await;let o=confirm(&orders,&g,&r,1).await.unwrap();assert_eq!(o.total_cents,0);
        assert_eq!(orders.record_payment(&s,&s.csrf,o.id,0,"ZERO").await.unwrap().manual_payment.unwrap().amount_cents,0);
        assert_eq!(count(&pool,"stock").await,2);assert_eq!(receipt_count(&pool).await,2);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn manual_payment_http_bilingual_confirmation_permissions_and_private_receipt() {
    isolated(|pool,_|async move {
        let(p,identity,s)=configured(&pool).await;let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity.clone());
        let raw=authenticated(&identity).await;let admin=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let app=router(test_app(pool.clone(),"Manual payments".into()).await);
        let role=identity.create_role(&s,&s.csrf,"Payment reader",&[Permission::OrdersRead]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"payment-reader@example.test","synthetic viewer password".into(),talocart_platform::identity::Role::parse(&role).unwrap()).await.unwrap();
        let(_,anon)=identity.anonymous().await.unwrap();let reader_raw=identity.login(&anon,&anon.csrf,"payment-reader@example.test","synthetic viewer password".into()).await.unwrap();let reader=identity.session(&reader_raw).await.unwrap().unwrap();let reader_cookie=format!("talocart_session={reader_raw}");
        for (locale,method) in [("en",1),("it",2)] {
            sqlx::query("UPDATE administrators SET locale=$1 WHERE id=$2").bind(locale).bind(admin.administrator_id).execute(&pool).await.unwrap();
            let(raw,g,r)=prepare(&carts,(p,0),1,method).await;let o=confirm(&orders,&g,&r,method).await.unwrap();
            let path=format!("/admin/orders/{}/payment",o.id);let detail=format!("/admin/orders/{}",o.id);
            let page=html(request(&app,"GET",&detail,&cookie,"",false).await).await;
            assert!(page.contains(if locale=="it"{"Registra incasso"}else{"Record payment"}));
            let page=html(request(&app,"GET",&detail,&reader_cookie,"",false).await).await;assert!(!page.contains("id=\"manual-payment\""));
            assert_eq!(request(&app,"GET",&path,&cookie,"",false).await.status(),StatusCode::METHOD_NOT_ALLOWED);
            let body=format!("csrf={}&revision=0&reference=%3Cref%3E&confirm=true&amount_cents=1&currency=USD&administrator_id=999",admin.csrf);
            assert_eq!(request(&app,"POST",&path,&cookie,&body.replace(&admin.csrf,"bad"),false).await.status(),StatusCode::FORBIDDEN);
            assert_eq!(request(&app,"POST",&path,&reader_cookie,&body.replace(&admin.csrf,&reader.csrf),false).await.status(),StatusCode::FORBIDDEN);
            let response=request(&app,"POST",&path,&cookie,&body.replace("&confirm=true",""),false).await;assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let page=html(response).await;assert!(page.contains("&#60;ref&#62;"));assert!(page.contains("aria-describedby=\"payment-confirm-error\""));
            let response=request(&app,"POST",&path,&cookie,&body.replace("%3Cref%3E","A%0AB"),false).await;assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let page=html(response).await;assert!(page.contains("aria-invalid=\"true\""));assert!(page.contains("id=\"payment-reference-error\""));
            assert_eq!(request(&app,"POST",&path,&cookie,&body,false).await.status(),StatusCode::SEE_OTHER);
            assert_eq!(request(&app,"POST",&path,&cookie,&body,false).await.status(),StatusCode::SEE_OTHER);
            let receipt=orders.get(&s,o.id).await.unwrap().manual_payment.unwrap();assert_eq!(receipt.amount_cents,o.total_cents);assert_eq!(receipt.currency,"EUR");assert_eq!(receipt.administrator_id,admin.administrator_id.unwrap());
            let response=request(&app,"GET",&format!("{detail}?payment=1"),&cookie,"",false).await;assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");let page=html(response).await;assert!(page.contains("id=\"manual-payment-history\""));assert!(page.contains("&#60;ref&#62;"));assert!(!page.contains("<ref>"));assert!(!page.contains(&format!("action=\"/admin/orders/{}/cancel\"",o.id)));
            for prefix in ["","/it"] {let response=request(&app,"GET",&format!("{prefix}/checkout/orders/{}",o.reference),&format!("talocart_cart={raw}"),"",false).await;assert_eq!(response.status(),StatusCode::OK);let page=html(response).await;assert!(page.contains(if prefix=="/it"{"Pagato"}else{"Paid"}));assert!(!page.contains("Use this payment reference:"));assert!(!page.contains("Usa questo riferimento per il pagamento:"));assert!(!page.contains("&#60;ref&#62;"));assert!(!page.contains("IT60X0542811101000000123456"));}
            assert_eq!(request(&app,"POST",&path,&cookie,&body.replace("%3Cref%3E","OTHER"),false).await.status(),StatusCode::CONFLICT);
            assert_eq!(request(&app,"POST",&format!("/admin/orders/{}/cancel",o.id),&cookie,&format!("csrf={}&revision=1&confirm=true",admin.csrf),false).await.status(),StatusCode::CONFLICT);
        }
        assert_eq!(receipt_count(&pool).await,2);assert_eq!(count(&pool,"stock").await,2);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn manual_payment_supersedes_customer_retry_and_preserves_shop_notification() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity.clone());
        let mail = talocart_platform::mail::Mail::new(pool.clone(), identity);
        let (_, g, r) = prepare(&carts, (p, 0), 1, 1).await;
        let o = confirm(&orders, &g, &r, 1).await.unwrap();
        let (config, mode, delivered, task) = super::super::email::smtp(451).await;
        mail.process_one(&config).await.unwrap();
        orders
            .record_payment(&s, &s.csrf, o.id, 0, "BACKOFF")
            .await
            .unwrap();
        let customer: i64 = sqlx::query_scalar(
            "SELECT id FROM email_outbox WHERE order_id=$1 AND template='order_received'",
        )
        .bind(o.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(mail.retry(&s, &s.csrf, customer).await.is_err());
        mode.store(250, std::sync::atomic::Ordering::SeqCst);
        for _ in 0..2 {
            assert!(mail.process_one(&config).await.unwrap());
        }
        assert!(!mail.process_one(&config).await.unwrap());
        assert_eq!(delivered.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM email_attempts WHERE outcome='transient'"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
        task.abort();
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn manual_payment_waits_for_inflight_confirmation_and_expired_lease() {
    isolated(|pool,_|async move {
        let(p,identity,s)=configured(&pool).await;let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity.clone());let mail=talocart_platform::mail::Mail::new(pool.clone(),identity);
        for timeout in [1,10] {
            let(_,g,r)=prepare(&carts,(p,0),1,1).await;let o=confirm(&orders,&g,&r,1).await.unwrap();
            let(mut config,mode,delivered,task)=super::super::email::smtp(999).await;config.timeout_seconds=timeout;
            let worker=mail.clone();let smtp=config.clone();let sending=tokio::spawn(async move{worker.process_one(&smtp).await.unwrap()});
            tokio::time::timeout(std::time::Duration::from_secs(5),async {while delivered.load(std::sync::atomic::Ordering::SeqCst)==0{tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
            orders.record_payment(&s,&s.csrf,o.id,0,"INFLIGHT").await.unwrap();
            // The shop's new-order notice may proceed; payment receipt must wait.
            sqlx::query("UPDATE email_outbox SET available_at=clock_timestamp()+interval '1 hour' WHERE order_id=$1 AND template='order_admin'").bind(o.id).execute(&pool).await.unwrap();
            assert!(!mail.process_one(&config).await.unwrap());assert!(sending.await.unwrap());mode.store(250,std::sync::atomic::Ordering::SeqCst);
            assert!(mail.process_one(&config).await.unwrap());
            assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM email_outbox WHERE order_id=$1 AND template='payment_received'").bind(o.id).fetch_one(&pool).await.unwrap(),"accepted");
            task.abort();
        }
        let(_,g,r)=prepare(&carts,(p,0),1,1).await;let o=confirm(&orders,&g,&r,1).await.unwrap();
        sqlx::query("UPDATE email_outbox SET state='sending',attempts=1,cycle_attempts=1,lease_token='expired',lease_until=clock_timestamp()-interval '1 second' WHERE order_id=$1 AND template='order_received'").bind(o.id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO email_attempts(message_id,attempt,outcome) SELECT id,1,'sending' FROM email_outbox WHERE order_id=$1 AND template='order_received'").bind(o.id).execute(&pool).await.unwrap();
        orders.record_payment(&s,&s.csrf,o.id,0,"EXPIRED").await.unwrap();
        let(config,_,_,task)=super::super::email::smtp(250).await;assert!(mail.process_one(&config).await.unwrap());
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM email_outbox WHERE order_id=$1 AND template='order_received'").bind(o.id).fetch_one(&pool).await.unwrap(),"superseded");
        task.abort();
    }).await;
}
