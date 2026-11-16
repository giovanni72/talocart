include!("checkout_methods_support.rs");

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_two_steps_load_methods_only_after_saved_addresses() {
    isolated(|pool, _| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (raw, guest) = carts.create().await.unwrap();
        carts
            .add(&guest, &guest.csrf, 0, (product, 0), 1, lang())
            .await
            .unwrap();
        let app = router(test_app(pool.clone(), "Steps".into()).await);
        let cookie = format!("talocart_cart={raw}");
        for prefix in ["", "/it"] {
            let head = request(
                &app,
                "HEAD",
                &format!("{prefix}/checkout/methods"),
                "",
                "",
                false,
            )
            .await;
            assert_eq!(head.status(), StatusCode::OK);
            assert!(head.headers().get(header::SET_COOKIE).is_none());
            assert_eq!(head.headers()[header::CACHE_CONTROL], "no-store");
            let response = request(
                &app,
                "GET",
                &format!("{prefix}/checkout/methods"),
                &cookie,
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(
                response.headers()[header::LOCATION],
                format!("{prefix}/checkout")
            );
        }
        let mut details = data();
        details.shipping_company = "<img src=x onerror=alert(1)>".into();
        carts
            .save_details(&guest, &guest.csrf, 1, details)
            .await
            .unwrap();
        for prefix in ["", "/it"] {
            let first = html(
                request(
                    &app,
                    "GET",
                    &format!("{prefix}/checkout"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(first.contains("value=\"continue\""));
            assert!(!first.contains("id=\"checkout-shipping\""));
            assert!(!first.contains("value=\"confirm\""));
            let response = request(
                &app,
                "GET",
                &format!("{prefix}/checkout/methods"),
                &cookie,
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let second = html(response).await;
            assert!(second.contains("id=\"checkout-methods\""));
            assert!(second.contains("name=\"payment_method_id\""));
            assert!(second.contains("buyer@example.test"));
            assert!(
                second.contains("onerror=alert(1)"),
                "Saved content is shown as text"
            );
            assert!(
                !second.contains("<img src=x"),
                "Composed fragment keeps Askama escaping"
            );
            assert!(!second.contains("name=\"email\""));
            assert!(
                second.find("id=\"checkout-summary\"").unwrap()
                    < second.find("id=\"checkout-confirm\"").unwrap(),
                "Review total precedes the final submit button in document order"
            );
            assert!(second.contains("form=\"checkout-methods\""));
        }
        assert_eq!(carts.session(&raw).await.unwrap().unwrap().revision, 2);
        let update = format!(
            "csrf={}&revision=2&action=update&shipping_method_id=1&payment_method_id=2",
            guest.csrf
        );
        let response = request(&app, "POST", "/checkout/methods", &cookie, &update, false).await;
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "Native update must redirect so reload cannot replay the POST"
        );
        assert_eq!(response.headers()[header::LOCATION], "/checkout/methods");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM orders")
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
async fn checkout_methods_destination_fiscal_base_persistence_and_invalidated_choices() {
    isolated(|pool,options|async move{
 let product=fixture(&pool).await;let carts=Carts::new(pool.clone());let(raw,g)=carts.create().await.unwrap();carts.add(&g,&g.csrf,0,(product,0),1,lang()).await.unwrap();
 let selection=Selection{shipping_method_id:Some(1),payment_method_id:Some(2)};
 let q=carts.checkout_quote(&g,lang(),"IT",Some(selection)).await.unwrap();assert_eq!(q.cart.totals.unwrap().gross().cents(),1220);assert_eq!(q.shipping_cost.unwrap().cents(),122);assert_eq!(q.payment_fee.unwrap().cents(),163);let totals=q.totals.unwrap();assert_eq!((totals.net().cents(),totals.tax().cents(),totals.gross().cents()),(1234,271,1505));assert!(!q.invalid_selection);
 carts.save_checkout(&g,&g.csrf,1,data(),selection,lang()).await.unwrap();
 let restarted=PgPoolOptions::new().connect_with(options).await.unwrap();let service=Carts::new(restarted.clone());let guest=service.session(&raw).await.unwrap().unwrap();assert!(service.checkout_quote(&guest,lang(),"IT",None).await.unwrap().shipping[0].selected);restarted.close().await;
 // A different destination changes merchandise tax and the available shipping method.
 sqlx::raw_sql("INSERT INTO tax_rates(name,basis_points) VALUES('French',2000); INSERT INTO tax_zones(name) VALUES('France'); INSERT INTO tax_zone_countries(zone_id,country_code) SELECT id,'FR' FROM tax_zones WHERE name='France'; INSERT INTO tax_zone_rules(zone_id,class_id,rate_id) SELECT z.id,1,r.id FROM tax_zones z CROSS JOIN tax_rates r WHERE z.name='France' AND r.name='French';").execute(&pool).await.unwrap();
 let q=carts.checkout_quote(&g,lang(),"FR",Some(Selection{shipping_method_id:Some(2),payment_method_id:Some(2)})).await.unwrap();assert_eq!(q.cart.totals.unwrap().gross().cents(),1200);assert_eq!(q.shipping_cost.unwrap().cents(),0);assert_eq!(q.payment_fee.unwrap().cents(),144);assert_eq!(q.totals.unwrap().gross().cents(),1344);
 sqlx::query("UPDATE shipping_methods SET active=false WHERE id=1").execute(&pool).await.unwrap();let q=carts.checkout_quote(&g,lang(),"IT",None).await.unwrap();assert!(q.invalid_selection);assert!(q.totals.is_none());assert!(matches!(carts.save_checkout(&g,&g.csrf,2,data(),selection,lang()).await,Err(CartError::Methods)));assert_eq!(carts.session(&raw).await.unwrap().unwrap().revision,2);
 assert_eq!(sqlx::query_scalar::<_,i32>("SELECT stock FROM catalog_products WHERE id=$1").bind(product).fetch_one(&pool).await.unwrap(),10);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM email_outbox").fetch_one(&pool).await.unwrap(),0);
}).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_methods_threshold_stock_free_shipping_and_atomic_concurrent_save() {
    isolated(|pool,_|async move{
 let p=fixture(&pool).await;let carts=Carts::new(pool.clone());let(raw,g)=carts.create().await.unwrap();carts.add(&g,&g.csrf,0,(p,0),1,lang()).await.unwrap();let s=Selection{shipping_method_id:Some(1),payment_method_id:Some(2)};
 sqlx::query("UPDATE payment_methods SET minimum_cents=1343 WHERE id=2").execute(&pool).await.unwrap();assert!(carts.checkout_quote(&g,lang(),"IT",Some(s)).await.unwrap().invalid_selection);
 sqlx::raw_sql("UPDATE payment_methods SET minimum_cents=1342 WHERE id=2; UPDATE shipping_methods SET free_from_cents=1220 WHERE id=1;").execute(&pool).await.unwrap();let q=carts.checkout_quote(&g,lang(),"IT",Some(s)).await.unwrap();assert_eq!(q.shipping_cost.unwrap().cents(),0);assert!(q.invalid_selection); // payment fee does not help reach its own minimum
 sqlx::query("UPDATE payment_methods SET minimum_cents=1220 WHERE id=2").execute(&pool).await.unwrap();let (a,b)=tokio::join!(carts.save_checkout(&g,&g.csrf,1,data(),s,lang()),carts.save_checkout(&g,&g.csrf,1,data(),s,lang()));assert_eq!([a.is_ok(),b.is_ok()].into_iter().filter(|x|*x).count(),1);
 let(_,other)=carts.create().await.unwrap();assert!(carts.checkout_quote(&other,lang(),"IT",None).await.unwrap().totals.is_none());assert!(matches!(carts.save_checkout(&g,"bad",2,data(),s,lang()).await,Err(CartError::Forbidden)));
 sqlx::query("UPDATE catalog_products SET stock=0 WHERE id=$1").bind(p).execute(&pool).await.unwrap();let q=carts.checkout_quote(&g,lang(),"IT",None).await.unwrap();assert!(q.shipping.is_empty()&&q.payments.is_empty()&&q.totals.is_none());assert!(matches!(carts.save_checkout(&g,&g.csrf,2,data(),s,lang()).await,Err(CartError::Methods)));
 assert_eq!(carts.session(&raw).await.unwrap().unwrap().revision,2);
}).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_methods_http_first_step_does_not_accept_methods_or_confirmation() {
    isolated(|pool,_|async move{
        let p=fixture(&pool).await;let carts=Carts::new(pool.clone());let(raw,g)=carts.create().await.unwrap();carts.add(&g,&g.csrf,0,(p,0),1,lang()).await.unwrap();
        let app=router(test_app(pool.clone(),"Methods".into()).await);let cookie=format!("talocart_cart={raw}");
        let body=format!("csrf={}&revision=1&action=continue&customer_type=private&email=buyer%40example.test&first_name=Alex&last_name=Example&phone=%2B393331234567&shipping_country=IT&shipping_street=Example&shipping_number=1&shipping_postal=00100&shipping_city=Roma&shipping_region=RM&shipping_method_id=999&payment_method_id=2",g.csrf);
        assert_eq!(request(&app,"POST","/checkout",&cookie,&body.replace(&g.csrf,"bad"),false).await.status(),StatusCode::FORBIDDEN);
        assert_eq!(request(&app,"POST","/checkout",&cookie,&body.replace("action=continue","action=confirm"),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert!(carts.details(&g).await.unwrap().is_none());
        for path in ["/checkout", "/it/checkout"] {
            let invalid=body.replace("buyer%40example.test", "invalid");
            let response=request(&app,"POST",path,&cookie,&invalid,false).await;
            assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
            assert!(html(response).await.contains("value=\"Alex\""));
        }
        let response=request(&app,"POST","/it/checkout",&cookie,&body,false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION],"/it/checkout/methods");
        assert_eq!(carts.session(&raw).await.unwrap().unwrap().revision,2);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM checkout_methods").fetch_one(&pool).await.unwrap(),0);
        sqlx::query("UPDATE guest_carts SET expires_at=now()-interval '1 second' WHERE id=$1").bind(g.id).execute(&pool).await.unwrap();
        assert_eq!(request(&app,"POST","/checkout/methods",&cookie,&format!("csrf={}&revision=2&action=update",g.csrf),true).await.status(),StatusCode::FORBIDDEN);
        carts.prune().await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM checkout_details").fetch_one(&pool).await.unwrap(),0);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_methods_quote_retries_when_configuration_commits_while_waiting() {
    isolated(|pool, options| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (_, guest) = carts.create().await.unwrap();
        carts.add(&guest, &guest.csrf, 0, (product, 0), 1, lang()).await.unwrap();
        let reader = PgPoolOptions::new().max_connections(1)
            .connect_with(options.application_name("checkout_quote_retry_test")).await.unwrap();
        let mut writer = pool.begin().await.unwrap();
        let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *writer).await.unwrap();
        sqlx::query("UPDATE shipping_methods SET fee_value=150 WHERE id=1")
            .execute(&mut *writer).await.unwrap();
        let service = Carts::new(reader.clone());
        let task = tokio::spawn(async move {
            service.checkout_quote(&guest, lang(), "IT", Some(Selection {
                shipping_method_id: Some(1), payment_method_id: Some(2),
            })).await
        });
        let mut waiting = false;
        for _ in 0..100 {
            waiting = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='checkout_quote_retry_test' AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(writer_pid).fetch_one(&pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(waiting, "Quote must wait on the configuration writer");
        writer.commit().await.unwrap();
        let quote = task.await.unwrap().expect("Quote resumes using the committed configuration");
        assert_eq!(quote.shipping_cost.unwrap().cents(), 183);
        assert!(quote.totals.is_some());
        reader.close().await;
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_two_steps_recalculate_persist_and_reject_stale_or_forged_updates() {
    isolated(|pool, _| async move {
        let product = fixture(&pool).await;
        sqlx::raw_sql("INSERT INTO shipping_method_countries(method_id,country_code) VALUES(2,'IT'); UPDATE payment_methods SET minimum_cents=1300 WHERE id=2;").execute(&pool).await.unwrap();
        let carts = Carts::new(pool.clone());
        let (raw, guest) = carts.create().await.unwrap();
        carts.add(&guest, &guest.csrf, 0, (product, 0), 1, lang()).await.unwrap();
        carts.save_details(&guest, &guest.csrf, 1, data()).await.unwrap();
        let app = router(test_app(pool.clone(), "Updates".into()).await);
        let cookie = format!("talocart_cart={raw}");
        let (_, quote) = carts.checkout_step(&guest, lang(), None).await.unwrap().unwrap();
        assert!(quote.shipping[0].selected);
        assert_eq!(quote.payments.len(), 2);
        assert!(quote.payments.iter().all(|m| !m.selected));
        assert!(quote.totals.is_none());
        let post = format!("csrf={}&revision=2&action=update&shipping_method_id=1&payment_method_id=2", guest.csrf);
        assert_eq!(request(&app, "POST", "/checkout/methods", &cookie, &post.replace(&guest.csrf, "bad"), true).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(request(&app, "POST", "/checkout/methods", &cookie, &post.replace("shipping_method_id=1", "shipping_method_id=oops"), true).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let response = request(&app, "POST", "/checkout/methods", &cookie, &post, true).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers()[header::VARY].to_str().unwrap().contains("HX-Request"));
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let h = html(response).await;
        assert!(h.contains("id=\"checkout-methods-region\""));
        assert!(!h.contains("<!DOCTYPE") && !h.contains("<html"));
        assert!(h.contains("15.05"));
        assert!(h.contains("name=\"revision\" value=\"3\""));
        let (_, q) = carts.checkout_step(&guest, lang(), None).await.unwrap().unwrap();
        assert_eq!(q.totals.unwrap().gross().cents(), 1505);
        assert!(q.payments.iter().any(|m| m.id == 2 && m.selected));
        assert_eq!(request(&app, "POST", "/checkout/methods", &cookie, &post, true).await.status(), StatusCode::CONFLICT);
        assert_eq!(carts.session(&raw).await.unwrap().unwrap().revision, 3);
        let post = post.replace("revision=2", "revision=3").replace("shipping_method_id=1", "shipping_method_id=2");
        let response = request(&app, "POST", "/it/checkout/methods", &cookie, &post, true).await;
        assert_eq!(response.status(), StatusCode::OK);
        let h = html(response).await;
        assert!(h.contains("scelta precedente"));
        assert!(!h.contains("id=\"payment-2\""));
        let (_, q) = carts.checkout_step(&guest, lang(), None).await.unwrap().unwrap();
        assert!(q.shipping.iter().any(|m| m.id == 2 && m.selected));
        assert!(q.payments.iter().any(|m| m.id == 1 && m.selected));
        assert_eq!(q.shipping_cost.unwrap().cents(), 0);
        assert_eq!(q.totals.unwrap().gross().cents(), 1369);
        assert_eq!(carts.session(&raw).await.unwrap().unwrap().revision, 4);
        let native = post.replace("revision=3", "revision=4");
        let response = request(&app, "POST", "/it/checkout/methods", &cookie, &native, false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/it/checkout/methods?changed=1");
        let response = request(&app, "GET", "/it/checkout/methods?changed=1", &cookie, "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(html(response).await.contains("scelta precedente"));
        // A native refresh is now GET-only and cannot increment the revision again.
        assert_eq!(carts.session(&raw).await.unwrap().unwrap().revision, 5);
        let mut other = guest.clone();
        let (_, alien) = carts.create().await.unwrap();
        other.id = alien.id;
        assert!(carts.checkout_step(&other, lang(), None).await.unwrap().is_none());
        assert!(matches!(carts.update_checkout_choices(&other, &other.csrf, 0, Selection::default(), lang()).await, Err(CartError::Forbidden)));
        sqlx::query("UPDATE shipping_methods SET active=false").execute(&pool).await.unwrap();
        let (_, q) = carts.checkout_step(&guest, lang(), None).await.unwrap().unwrap();
        assert!(q.shipping.is_empty() && q.payments.is_empty() && q.totals.is_none());
        assert!(q.invalid_selection);
        assert_eq!(sqlx::query_scalar::<_, i32>("SELECT stock FROM catalog_products WHERE id=$1").bind(product).fetch_one(&pool).await.unwrap(), 10);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM email_outbox").fetch_one(&pool).await.unwrap(), 0);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM orders").fetch_one(&pool).await.unwrap(), 0);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_two_steps_update_concurrency_and_address_changes_keep_revision_boundary() {
    isolated(|pool, options| async move {
        let product = fixture(&pool).await;
        let carts = Carts::new(pool.clone());
        let (raw, guest) = carts.create().await.unwrap();
        carts
            .add(&guest, &guest.csrf, 0, (product, 0), 1, lang())
            .await
            .unwrap();
        carts
            .save_details(&guest, &guest.csrf, 1, data())
            .await
            .unwrap();
        let selected = Selection {
            shipping_method_id: Some(1),
            payment_method_id: Some(2),
        };
        let (a, b) = tokio::join!(
            carts.update_checkout_choices(&guest, &guest.csrf, 2, selected, lang()),
            carts.update_checkout_choices(&guest, &guest.csrf, 2, selected, lang())
        );
        assert_eq!(
            [a.is_ok(), b.is_ok()].into_iter().filter(|ok| *ok).count(),
            1
        );
        let mut french = data();
        french.shipping_country = "FR".into();
        french.shipping_postal = "75001".into();
        french.shipping_city = "Paris".into();
        french.shipping_region.clear();
        carts
            .save_details(&guest, &guest.csrf, 3, french)
            .await
            .unwrap();
        assert!(matches!(
            carts
                .update_checkout_choices(&guest, &guest.csrf, 3, selected, lang())
                .await,
            Err(CartError::Stale)
        ));
        let restarted = PgPoolOptions::new().connect_with(options).await.unwrap();
        let service = Carts::new(restarted.clone());
        let g = service.session(&raw).await.unwrap().unwrap();
        let (d, q) = service
            .checkout_step(&g, lang(), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(d.shipping_country, "FR");
        assert_eq!(q.cart.revision, 4);
        assert_eq!(q.shipping.len(), 1);
        assert_eq!(q.shipping[0].id, 2);
        assert!(q.shipping[0].selected && q.invalid_selection);
        // GET proposals never modify the saved identifiers or cart revision.
        let stored: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT shipping_method_id,payment_method_id FROM checkout_methods WHERE cart_id=$1",
        )
        .bind(g.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored, (Some(1), Some(2)));
        assert_eq!(g.revision, 4);
        sqlx::query("DELETE FROM checkout_countries WHERE country_code='FR'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            service
                .update_checkout_choices(&g, &g.csrf, 4, selected, lang())
                .await,
            Err(CartError::Unavailable)
        ));
        assert_eq!(service.session(&raw).await.unwrap().unwrap().revision, 4);
        restarted.close().await;
        sqlx::query("INSERT INTO checkout_countries(country_code) VALUES('FR')")
            .execute(&pool)
            .await
            .unwrap();
        // A concurrent address commit must never pair old data with a new revision.
        for revision in 4..8 {
            let before = format!("Version{revision}");
            let after = format!("Version{}", revision + 1);
            sqlx::query("UPDATE checkout_details SET first_name=$1 WHERE cart_id=$2")
                .bind(&before)
                .bind(guest.id)
                .execute(&pool)
                .await
                .unwrap();
            let writer = async {
                let mut tx = pool.begin().await.unwrap();
                sqlx::query("UPDATE guest_carts SET revision=revision+1 WHERE id=$1")
                    .bind(guest.id)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                sqlx::query("UPDATE checkout_details SET first_name=$1 WHERE cart_id=$2")
                    .bind(&after)
                    .bind(guest.id)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            };
            let (step, ()) = tokio::join!(carts.checkout_step(&guest, lang(), None), writer);
            let (details, quote) = step.unwrap().unwrap();
            assert!(matches!(quote.cart.revision, r if r == revision || r == revision + 1));
            assert_eq!(
                details.first_name,
                format!("Version{}", quote.cart.revision)
            );
        }
    })
    .await;
}
