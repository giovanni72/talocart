use super::*;
use talocart_commerce::orders::FulfillmentInput;
async fn role(pool: &PgPool, role: &str) -> i64 {
    sqlx::query_scalar("SELECT status_id FROM order_status_roles WHERE role=$1")
        .bind(role)
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn custom(pool: &PgPool) -> i64 {
    sqlx::query_scalar("INSERT INTO order_statuses(name_it,name_en,color,sort_order) VALUES('Ricevuto <test>','Received <test>','#123abc',5) RETURNING id").fetch_one(pool).await.unwrap()
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn current_status_initialization_color_renames_payment_and_history() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let id = custom(&pool).await;
        sqlx::query("UPDATE payment_methods SET initial_order_status_id=$1 WHERE id=1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let orders = Orders::new(pool.clone(), identity.clone());
        let carts = Carts::new(pool.clone());
        let (_, g, review) = prepare(&carts, (p, 0), 1, 1).await;
        let o = confirm(&orders, &g, &review, 1).await.unwrap();
        assert_eq!(o.current_status.id, id);
        assert_eq!(o.current_status.color, "#123abc");
        assert!(!o.is_paid());
        let snapshot: String = sqlx::query_scalar("SELECT snapshot FROM orders WHERE id=$1")
            .bind(o.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE order_statuses SET name_it='Ricevuto aggiornato',color='#abcdef' WHERE id=$1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        let current = orders.get(&s, o.id).await.unwrap();
        assert_eq!(current.status("it"), "Ricevuto aggiornato");
        assert_eq!(current.current_status.color, "#abcdef");
        assert_eq!(current.frozen.status_color, "#123abc");
        let paid = orders
            .record_payment(&s, &s.csrf, o.id, o.revision, "Bank receipt")
            .await
            .unwrap();
        assert!(paid.is_paid());
        assert_eq!(paid.current_status.id, id);
        assert_eq!(
            snapshot,
            sqlx::query_scalar::<_, String>("SELECT snapshot FROM orders WHERE id=$1")
                .bind(o.id)
                .fetch_one(&pool)
                .await
                .unwrap()
        );
        assert!(
            sqlx::query("DELETE FROM order_statuses WHERE id=$1")
                .bind(id)
                .execute(&pool)
                .await
                .is_err()
        );
        assert_eq!(orders.status_history(&s, o.id, 1).await.unwrap().total, 1);
        let raw = authenticated(&identity).await;
        let app = router(test_app(pool.clone(), "Status".into()).await);
        for locale in ["en", "it"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let h = html(
                request(
                    &app,
                    "GET",
                    &format!("/admin/orders?state={id}"),
                    &format!("talocart_session={raw}"),
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(h.contains("fill=\"#abcdef\""));
            assert!(h.contains(&o.reference));
            assert!(h.contains(if locale == "it" {
                "Ricevuto aggiornato"
            } else {
                "Received &#60;test&#62;"
            }));
        }
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn current_status_transitions_preserve_shipping_guards_and_concurrency() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity);
        let id = custom(&pool).await;
        let carts = Carts::new(pool.clone());
        let (_, g, review) = prepare(&carts, (p, 0), 1, 2).await;
        let o = confirm(&orders, &g, &review, 2).await.unwrap();
        let preparing = role(&pool, "preparing").await;
        let shipped = role(&pool, "shipped").await;
        assert!(orders.set_status(&s, "invalid", o.id, 0, id).await.is_err());
        assert!(
            orders
                .set_status(&s, &s.csrf, o.id, 0, shipped)
                .await
                .is_err()
        );
        let (a, b) = tokio::join!(
            orders.set_status(&s, &s.csrf, o.id, 0, id),
            orders.set_status(&s, &s.csrf, o.id, 0, preparing)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let current = orders.get(&s, o.id).await.unwrap();
        let ready = orders
            .set_status(&s, &s.csrf, o.id, current.status_revision, preparing)
            .await
            .unwrap();
        assert_eq!(ready.fulfillment.state, "preparing");
        let key = talocart_platform::identity::token().unwrap();
        let input = FulfillmentInput {
            action: "shipped".into(),
            carrier: "Courier".into(),
            tracking_number: "T1".into(),
            tracking_url: "https://example.test/T1".into(),
            ..Default::default()
        };
        let sent = orders
            .update_fulfillment(&s, &s.csrf, o.id, ready.fulfillment.revision, &key, &input)
            .await
            .unwrap();
        assert_eq!(sent.current_status.id, shipped);
        assert!(!sent.is_paid());
        assert_eq!(sent.fulfillment.tracking_number, "T1");
        assert!(
            orders
                .set_status(&s, &s.csrf, o.id, sent.status_revision, id)
                .await
                .is_err()
        );
        assert!(
            orders
                .cancel(&s, &s.csrf, o.id, sent.revision)
                .await
                .is_err()
        );
        let done = orders
            .update_fulfillment(
                &s,
                &s.csrf,
                o.id,
                sent.fulfillment.revision,
                &talocart_platform::identity::token().unwrap(),
                &FulfillmentInput {
                    action: "delivered".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(done.current_status.id, role(&pool, "delivered").await);
        assert_eq!(done.fulfillment.tracking_url, "https://example.test/T1");
        assert!(!done.is_paid());
        assert_eq!(count(&pool, "stock").await, 1);
        assert!(
            sqlx::query(
                "UPDATE order_current_status SET status_id=$2,revision=revision+1 WHERE order_id=$1"
            )
            .bind(o.id)
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM order_status_events WHERE order_id=$1")
                .bind(o.id)
                .execute(&pool)
                .await
                .is_err()
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn current_status_preparing_initial_and_cancellation_are_atomic() {
    isolated(|pool, _| async move {
        let (p, identity, s) = configured(&pool).await;
        let orders = Orders::new(pool.clone(), identity);
        let preparing = role(&pool, "preparing").await;
        sqlx::query("UPDATE payment_methods SET initial_order_status_id=$1 WHERE id=2")
            .bind(preparing)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            sqlx::query("UPDATE payment_methods SET initial_order_status_id=$1 WHERE id=1")
                .bind(role(&pool, "shipped").await)
                .execute(&pool)
                .await
                .is_err()
        );
        let carts = Carts::new(pool.clone());
        let (_, g, review) = prepare(&carts, (p, 0), 1, 2).await;
        let o = confirm(&orders, &g, &review, 2).await.unwrap();
        assert_eq!(o.current_status.id, preparing);
        assert_eq!(o.fulfillment.state, "preparing");
        assert!(!o.is_paid());
        let cancelled = orders.cancel(&s, &s.csrf, o.id, o.revision).await.unwrap();
        assert_eq!(cancelled.current_status.id, role(&pool, "cancelled").await);
        orders.cancel(&s, &s.csrf, o.id, o.revision).await.unwrap();
        assert_eq!(count(&pool, "stock").await, 2);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn current_status_http_post_csrf_rejection_context_and_permissions() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let orders = Orders::new(pool.clone(), identity.clone());
        let carts = Carts::new(pool.clone());
        let (_, g, review) = prepare(&carts, (p, 0), 1, 1).await;
        let o = confirm(&orders, &g, &review, 1).await.unwrap();
        let app = router(test_app(pool.clone(), "Status".into()).await);
        let cookie = format!("talocart_session={raw}");
        let path = format!("/admin/orders/{}/status", o.id);
        assert_eq!(
            request(&app, "GET", &path, &cookie, "", false)
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            request(
                &app,
                "POST",
                &path,
                &cookie,
                "csrf=wrong&revision=0&status_id=1",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        for locale in ["en", "it"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let response = request(
                &app,
                "POST",
                &path,
                &cookie,
                &format!(
                    "csrf={}&revision=0&status_id={}",
                    s.csrf,
                    role(&pool, "shipped").await
                ),
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let h = html(response).await;
            assert!(h.contains("aria-describedby=\"order-error status-change-help\""));
            assert!(h.contains(&o.reference));
            assert!(h.contains(if locale == "it" {
                "Impossibile applicare lo stato"
            } else {
                "Unable to apply this status"
            }));
        }
        identity.logout(&s, &s.csrf).await.unwrap();
        assert!(
            orders
                .set_status(&s, &s.csrf, o.id, 0, role(&pool, "preparing").await)
                .await
                .is_err()
        );
        assert!(orders.status_history(&s, o.id, 1).await.is_err());
    })
    .await;
}
