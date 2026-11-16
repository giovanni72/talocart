include!("contention_support.rs");

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn contention_notice_database_failures_preserve_cause_and_retry_atomically() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        for (code, persistent) in [("40001", false), ("40P01", false), ("23514", false), ("40001", true)] {
            let (_, guest, review) = prepare(&carts, (p, 0), 1, 1).await;
            let before = (count(&pool, "orders").await, count(&pool, "stock").await, count(&pool, "mail").await);
            // Sequences survive rollback: fail exactly the first INSERT across retries.
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SEQUENCE notice_failure_attempt;
                 CREATE FUNCTION fail_notice_once() RETURNS trigger LANGUAGE plpgsql AS $$
                 DECLARE attempt bigint;
                 BEGIN
                   attempt := nextval('notice_failure_attempt');
                   IF attempt=1 OR {persistent} THEN
                     RAISE EXCEPTION 'synthetic notice failure' USING ERRCODE='{code}';
                   END IF;
                   RETURN NEW;
                 END $$;
                 CREATE TRIGGER fail_notice_once BEFORE INSERT ON email_outbox FOR EACH ROW EXECUTE FUNCTION fail_notice_once();"
            ))).execute(&pool).await.unwrap();
            let result = confirm(&orders, &guest, &review, 1).await;
            if code == "23514" || persistent {
                let error = match result { Err(error) => error, Ok(_) => panic!("Terminal failure must roll back") };
                let cause = error.technical().expect("SQL failure must remain technical");
                assert!(matches!(cause, talocart_platform::technical::TechnicalError::Database(ref failure) if failure.sqlstate()==Some(code)));
                assert_eq!((count(&pool, "orders").await, count(&pool, "stock").await, count(&pool, "mail").await), before);
                assert_eq!(sqlx::query_scalar::<_,i64>("SELECT last_value FROM notice_failure_attempt").fetch_one(&pool).await.unwrap(), if persistent { 3 } else { 1 });
            } else {
                let order = result.unwrap_or_else(|error| panic!("Transient {code} must retry: {error}"));
                assert_eq!(confirm(&orders, &guest, &review, 1).await.unwrap().id, order.id);
                assert_eq!((count(&pool, "orders").await, count(&pool, "stock").await, count(&pool, "mail").await), (before.0+1,before.1+1,before.2+2));
                assert_eq!(sqlx::query_scalar::<_,i64>("SELECT last_value FROM notice_failure_attempt").fetch_one(&pool).await.unwrap(), 3);
            }
            sqlx::raw_sql("DROP TRIGGER fail_notice_once ON email_outbox; DROP FUNCTION fail_notice_once(); DROP SEQUENCE notice_failure_attempt;").execute(&pool).await.unwrap();
        }
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn contention_variant_last_unit_has_one_winner_and_replay_is_side_effect_free() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        sqlx::query("INSERT INTO catalog_options(name) VALUES('Size')").execute(&pool).await.unwrap();
        let value: i64 = sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) SELECT id,'Large' FROM catalog_options WHERE name='Size' RETURNING id").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) SELECT $1,1,option_id FROM catalog_option_values WHERE id=$2").bind(p).bind(value).execute(&pool).await.unwrap();
        let variant: i64 = sqlx::query_scalar("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,ARRAY[$2::bigint],'CONTENTION-VARIANT',1000,1) RETURNING id").bind(p).bind(value).fetch_one(&pool).await.unwrap();
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, a, ar) = prepare(&carts, (p, variant), 1, 1).await;
        let (_, b, br) = prepare(&carts, (p, variant), 1, 1).await;
        let mut gate = pool.begin().await.unwrap();
        commercial_lock::exclusive(&mut gate).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *gate).await.unwrap();
        let service = orders.clone();
        let (ga, gb, ra, rb) = (a.clone(), b.clone(), ar.clone(), br.clone());
        let task = tokio::spawn(async move {
            tokio::join!(confirm(&service, &ga, &ra, 1), confirm(&service, &gb, &rb, 1))
        });
        wait_for_blocked(&pool, pid, 2).await;
        gate.commit().await.unwrap();
        let (x, y) = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();
        assert_eq!(usize::from(x.is_ok()) + usize::from(y.is_ok()), 1);
        let (guest, review) = if x.is_ok() { (&a, &ar) } else { (&b, &br) };
        let winner = x.or(y).unwrap();
        assert_eq!(confirm(&orders, guest, review, 1).await.unwrap().id, winner.id);
        assert_eq!(winner.lines[0].variant_id, Some(variant));
        assert_eq!(count(&pool, "orders").await, 1);
        assert_eq!(count(&pool, "stock").await, 1);
        assert_eq!(count(&pool, "mail").await, 2);
        let stocks: (i32, i32) = sqlx::query_as(
            "SELECT p.stock,v.stock FROM catalog_products p JOIN catalog_variants v ON v.product_id=p.id WHERE v.id=$1",
        ).bind(variant).fetch_one(&pool).await.unwrap();
        assert_eq!(stocks, (10, 0));
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn contention_quote_change_commit_requires_review_but_rollback_keeps_snapshot() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        for fiscal in [false, true] {
            for commit in [false, true] {
                let (_, guest, review) = prepare(&carts, (p, 0), 1, 1).await;
                let previous = count(&pool, "orders").await;
                let mut edit = pool.begin().await.unwrap();
                commercial_lock::exclusive(&mut edit).await.unwrap();
                let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(&mut *edit).await.unwrap();
                if fiscal {
                    sqlx::query("UPDATE tax_rates SET basis_points=basis_points+100 WHERE id=(SELECT rate_id FROM tax_classes WHERE id=(SELECT tax_class_id FROM catalog_products WHERE id=$1))")
                        .bind(p).execute(&mut *edit).await.unwrap();
                } else {
                    sqlx::query("UPDATE catalog_products SET price_cents=price_cents+100 WHERE id=$1")
                        .bind(p).execute(&mut *edit).await.unwrap();
                }
                let service = orders.clone();
                let task = tokio::spawn(async move { confirm(&service, &guest, &review, 1).await });
                wait_for_blocked(&pool, pid, 1).await;
                if commit { edit.commit().await.unwrap(); } else { edit.rollback().await.unwrap(); }
                let result = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();
                if commit {
                    assert!(matches!(result, Err(OrderError::Review)));
                    assert_eq!(count(&pool, "orders").await, previous);
                } else {
                    let order = result.unwrap();
                    assert_eq!(count(&pool, "orders").await, previous + 1);
                    assert_eq!(order.lines.len(), 1);
                }
                let committed = count(&pool, "orders").await;
                assert_eq!(count(&pool, "stock").await, committed);
                assert_eq!(count(&pool, "mail").await, committed * 2);
            }
        }
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn contention_cancelled_confirmation_rolls_back_stock_number_and_partial_outbox() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, guest, review) = prepare(&carts, (p, 0), 1, 1).await;
        // Block the second notice: order, stock movement and first outbox row
        // already exist inside the confirmation transaction at this point.
        sqlx::raw_sql("CREATE FUNCTION pause_second_notice() RETURNS trigger LANGUAGE plpgsql AS $$
          BEGIN
            IF (SELECT count(*) FROM email_outbox)=1 THEN
              IF (SELECT count(*) FROM orders)<>1 OR (SELECT count(*) FROM order_stock_moves)<>1 THEN
                RAISE EXCEPTION 'Expected commercial writes before second notice';
              END IF;
              PERFORM pg_advisory_xact_lock(5426645964561798299);
            END IF;
            RETURN NEW;
          END $$;
          CREATE TRIGGER pause_second_notice BEFORE INSERT ON email_outbox FOR EACH ROW EXECUTE FUNCTION pause_second_notice();")
            .execute(&pool).await.unwrap();
        let mut gate = pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(5426645964561798299)")
            .execute(&mut *gate).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *gate).await.unwrap();
        let service = orders.clone();
        let g = guest.clone();
        let r = review.clone();
        let task = tokio::spawn(async move { confirm(&service, &g, &r, 1).await });
        wait_for_blocked(&pool, pid, 1).await;
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        gate.rollback().await.unwrap();
        // Taking the commercial guard is a fence after the aborted transaction's
        // asynchronous rollback; no sleep is used to guess when cleanup finishes.
        let mut fence = pool.begin().await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), commercial_lock::exclusive(&mut fence))
            .await.unwrap().unwrap();
        fence.rollback().await.unwrap();
        assert_eq!(count(&pool, "orders").await, 0);
        assert_eq!(count(&pool, "stock").await, 0);
        assert_eq!(count(&pool, "mail").await, 0);
        assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_products WHERE id=$1")
            .bind(p).fetch_one(&pool).await.unwrap(), 10);
        assert_eq!(carts.quote(&guest, lang()).await.unwrap().lines.len(), 1);
        sqlx::query("DROP TRIGGER pause_second_notice ON email_outbox").execute(&pool).await.unwrap();
        let order = confirm(&orders, &guest, &review, 1).await.unwrap();
        assert!(order.reference.ends_with("-1"));
        assert_eq!(count(&pool, "orders").await, 1);
        assert_eq!(count(&pool, "stock").await, 1);
        assert_eq!(count(&pool, "mail").await, 2);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn contention_destination_removal_rejects_waiting_confirmation_atomically() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, guest, review) = prepare(&carts, (p, 0), 1, 1).await;
        let mut edit = pool.begin().await.unwrap();
        commercial_lock::exclusive(&mut edit).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *edit)
            .await
            .unwrap();
        sqlx::query("DELETE FROM checkout_countries WHERE country_code='IT'")
            .execute(&mut *edit)
            .await
            .unwrap();
        let task = tokio::spawn(async move { confirm(&orders, &guest, &review, 1).await });
        wait_for_blocked(&pool, pid, 1).await;
        edit.commit().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(OrderError::Cart(_))));
        assert_eq!(count(&pool, "orders").await, 0);
        assert_eq!(count(&pool, "stock").await, 0);
        assert_eq!(count(&pool, "mail").await, 0);
        assert_eq!(
            sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1")
                .bind(p)
                .fetch_one(&pool)
                .await
                .unwrap(),
            10
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn contention_import_writer_commit_or_rollback_releases_waiting_checkout() {
    use talocart_commerce::imports::{Brand, Writer};
    isolated(|pool, _| async move {
        let (p, identity, session) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        for commit in [false, true] {
            let (_, guest, review) = prepare(&carts, (p, 0), 1, 1).await;
            let mut import = pool.begin().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *import)
                .await
                .unwrap();
            let mut writer = Writer::authorize_in(
                &mut import,
                session.administrator_id.unwrap(),
                Permission::BrandsWrite,
            )
            .await
            .unwrap();
            let (brand, _) = writer
                .create_brand(&Brand {
                    name: "Concurrent imported brand".into(),
                    slug: "concurrent-imported-brand".into(),
                    translations: ["en", "it"]
                        .into_iter()
                        .map(|locale| talocart_platform::content::Translation {
                            locale: locale.into(),
                            name: "Concurrent imported brand".into(),
                            ..Default::default()
                        })
                        .collect(),
                })
                .await
                .unwrap();
            let service = orders.clone();
            let task = tokio::spawn(async move { confirm(&service, &guest, &review, 1).await });
            wait_for_blocked(&pool, pid, 1).await;
            if commit {
                import.commit().await.unwrap();
            } else {
                import.rollback().await.unwrap();
            }
            let order = tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(order.lines[0].product_id, p);
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_brands WHERE id=$1)")
                    .bind(brand)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(exists, commit);
        }
        assert_eq!(count(&pool, "orders").await, 2);
        assert_eq!(count(&pool, "stock").await, 2);
        assert_eq!(count(&pool, "mail").await, 4);
    })
    .await;
}
