use super::*;
use talocart_platform::mail::{Mail, MailError, Settings};

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn unconfigured_email_allows_orders_replay_and_later_business_events() {
    isolated(|pool, _| async move {
        let p = fixture(&pool).await;
        let identity = Identity::new(pool.clone());
        let session = taxonomy_session(&identity).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity.clone());
        let (_, guest, review) = prepare(&carts, (p, 0), 2, 2).await;
        let (a, b) = tokio::join!(
            confirm(&orders, &guest, &review, 2),
            confirm(&orders, &guest, &review, 2)
        );
        let order = a.expect("An order does not require configured email");
        assert_eq!(order.id, b.unwrap().id);
        assert_eq!(count(&pool, "orders").await, 1);
        assert_eq!(count(&pool, "stock").await, 1);
        assert_eq!(count(&pool, "mail").await, 0);
        assert!(carts.quote(&guest, lang()).await.unwrap().lines.is_empty());
        assert_eq!(
            orders
                .cancel(&session, &session.csrf, order.id, 0)
                .await
                .unwrap()
                .state,
            "cancelled"
        );
        assert_eq!(count(&pool, "stock").await, 2);
        assert_eq!(count(&pool, "mail").await, 0);
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1")
                .bind(p)
                .fetch_one(&pool)
                .await
                .unwrap(),
            10
        );

        let (_, guest, review) = prepare(&carts, (p, 0), 1, 1).await;
        let order = confirm(&orders, &guest, &review, 1).await.unwrap();
        assert!(
            orders
                .record_payment(&session, &session.csrf, order.id, 0, "BANK-1")
                .await
                .unwrap()
                .is_paid()
        );
        for (revision, action) in [(0, "preparing"), (1, "shipped"), (2, "delivered")] {
            let input = talocart_commerce::orders::FulfillmentInput {
                action: action.into(),
                carrier: "Example courier".into(),
                tracking_number: "TRACK-1".into(),
                ..Default::default()
            };
            orders
                .update_fulfillment(
                    &session,
                    &session.csrf,
                    order.id,
                    revision,
                    &talocart_platform::identity::token().unwrap(),
                    &input,
                )
                .await
                .unwrap();
        }
        assert_eq!(count(&pool, "mail").await, 0);

        // Explicit email operations still require a valid sender configuration.
        let mut tx = pool.begin().await.unwrap();
        assert!(matches!(
            Mail::configuration(&mut tx).await,
            Err(MailError::Invalid)
        ));
        tx.rollback().await.unwrap();
        let mail = Mail::new(pool.clone(), identity);
        mail.save(
            &session,
            &session.csrf,
            &Settings {
                from_name: "Shop".into(),
                from_email: "shop@example.test".into(),
                admin_recipients: vec!["admin@example.test".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            confirm(&orders, &guest, &review, 1).await.unwrap().id,
            order.id
        );
        assert_eq!(
            count(&pool, "mail").await,
            0,
            "No retrospective notifications on replay"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn paused_configured_email_still_queues_order_notifications() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        sqlx::query("UPDATE email_configuration SET enabled=false")
            .execute(&pool)
            .await
            .unwrap();
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, guest, review) = prepare(&carts, (p, 0), 1, 2).await;
        let order = confirm(&orders, &guest, &review, 2).await.unwrap();
        let messages: Vec<(String, String, i32)> = sqlx::query_as(
            "SELECT template,state,attempts FROM email_outbox WHERE order_id=$1 ORDER BY template",
        )
        .bind(order.id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            messages,
            vec![
                ("order_admin".into(), "pending".into(), 0),
                ("order_received".into(), "pending".into(), 0),
            ]
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn incomplete_or_missing_email_configuration_is_not_silently_ignored() {
    isolated(|pool, _| async move {
        let p = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), Identity::new(pool.clone()));
        let (_, guest, review) = prepare(&carts, (p, 0), 1, 2).await;
        for sql in [
            "UPDATE email_configuration SET enabled=true",
            "UPDATE email_configuration SET enabled=false,from_name='Incomplete'",
            "DELETE FROM email_configuration",
        ] {
            sqlx::query(sql).execute(&pool).await.unwrap();
            let error = match confirm(&orders, &guest, &review, 2).await {
                Err(error) => error,
                Ok(_) => panic!("Incomplete configuration must roll back"),
            };
            if sql.starts_with("DELETE") {
                assert!(
                    error.technical().is_some(),
                    "Missing configuration row is a storage failure"
                );
            } else {
                assert!(matches!(error, OrderError::Notification));
            }
            assert_eq!(count(&pool, "orders").await, 0);
            assert_eq!(count(&pool, "stock").await, 0);
            assert_eq!(count(&pool, "mail").await, 0);
            assert_eq!(carts.quote(&guest, lang()).await.unwrap().lines.len(), 1);
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_without_configured_email_http_bilingual_receipt_and_csrf() {
    isolated(|pool, _| async move {
        let p = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let app = router(test_app(pool.clone(), "Orders".into()).await);
        for (prefix, locale) in [("", "en"), ("/it", "it")] {
            let (raw, guest, _) = prepare(&carts, (p, 0), 1, 2).await;
            carts.save_details(&guest, &guest.csrf, guest.revision, data()).await.unwrap();
            let guest = carts.session(&raw).await.unwrap().unwrap();
            let language = Language::new(Locale::parse(locale).unwrap(), Locale::parse("en").unwrap());
            let quote = carts.checkout_quote(&guest, language, "IT", Some(selection(2))).await.unwrap();
            let cookie = format!("talocart_cart={raw}");
            let body = format!("csrf={}&revision={}&action=confirm&review={}&shipping_method_id=1&payment_method_id=2",
                guest.csrf, guest.revision, quote.review);
            let route = format!("{prefix}/checkout/methods");
            assert_eq!(request(&app, "POST", &route, &cookie, &body.replace(&guest.csrf, "bad"), false)
                .await.status(), StatusCode::FORBIDDEN);
            let response = request(&app, "POST", &route, &cookie, &body, false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let receipt = response.headers()[header::LOCATION].to_str().unwrap().to_owned();
            assert!(receipt.starts_with(&format!("{prefix}/checkout/orders/ORD-")));
            let response = request(&app, "GET", &receipt, &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], locale);
            let receipt_html = html(response).await;
            assert!(!receipt_html.contains(if locale == "it" {
                "Ti invieremo il riepilogo anche via email."
            } else {
                "We will also email your order summary."
            }), "A receipt without notifications must not promise an email");
            assert_eq!(request(&app, "POST", &route, &cookie, &body, false).await.status(), StatusCode::SEE_OTHER);
        }
        assert_eq!(count(&pool, "orders").await, 2);
        assert_eq!(count(&pool, "stock").await, 2);
        assert_eq!(count(&pool, "mail").await, 0);
    }).await;
}
