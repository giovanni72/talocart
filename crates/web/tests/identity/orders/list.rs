use super::*;
use talocart_commerce::orders::ListFilters;

// Synthetic list records retain a real, valid historical snapshot. No production data.
async fn seed(pool: &PgPool) -> (Identity, talocart_platform::identity::Session) {
    let (product, identity, session) = configured(pool).await;
    let (_, guest) = Carts::new(pool.clone()).create().await.unwrap();
    let original = historical::seed(pool, &guest, product, "bank_transfer").await;
    let raw: String = sqlx::query_scalar("SELECT snapshot FROM orders WHERE id=$1")
        .bind(original.id)
        .fetch_one(pool)
        .await
        .unwrap();
    let mut frozen: talocart_commerce::orders::Frozen = toml::from_str(&raw).unwrap();
    sqlx::query("INSERT INTO order_customer_summaries(order_id,first_name,last_name,email) VALUES($1,$2,$3,$4)")
        .bind(original.id).bind(&frozen.customer.first_name).bind(&frozen.customer.last_name).bind(&frozen.customer.email)
        .execute(pool).await.unwrap();
    for n in 1..=25_i64 {
        frozen.customer.first_name = if n == 1 {
            "A<&".into()
        } else {
            format!("Name {n:02}")
        };
        frozen.customer.last_name = "Example".into();
        frozen.customer.email = format!("buyer_{n}%@example.test");
        let instant = match n {
            1 => "2026-03-28T22:59:59Z",
            2 => "2026-03-28T23:00:00Z",
            3 => "2026-03-29T21:59:59Z",
            4 => "2026-03-29T22:00:00Z",
            _ => "2026-04-01T12:00:00Z",
        };
        let id: i64 = sqlx::query_scalar("INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents,created_at,payment_state,state,cancelled_at) VALUES($1,$2,0,repeat('a',64),$3,'EUR',$4,0,0,$4,0,$4,$5::text::timestamptz,$6,$7,CASE WHEN $7='cancelled' THEN now() END) RETURNING id")
            .bind(format!("ORD-2025-{n}")).bind(10000+n).bind(toml::to_string(&frozen).unwrap()).bind(n*100)
            .bind(instant).bind(match n%3 {0=>"unpaid",1=>"paid",_=>"refunded"}).bind(if n==5 {"cancelled"}else{"open"}).fetch_one(pool).await.unwrap();
        sqlx::query("INSERT INTO order_customer_summaries(order_id,first_name,last_name,email) VALUES($1,$2,$3,$4)")
            .bind(id).bind(&frozen.customer.first_name).bind(&frozen.customer.last_name).bind(&frozen.customer.email)
            .execute(pool).await.unwrap();
        if [3, 4, 7].contains(&n) {
            sqlx::query("INSERT INTO order_fulfillment(order_id) VALUES($1)")
                .bind(id)
                .execute(pool)
                .await
                .unwrap();
            sqlx::query(
                "UPDATE order_fulfillment SET state='preparing',revision=1 WHERE order_id=$1",
            )
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
            if n != 3 {
                sqlx::query("UPDATE order_fulfillment SET state='shipped',carrier='Synthetic',revision=2 WHERE order_id=$1").bind(id).execute(pool).await.unwrap();
                if n == 7 {
                    sqlx::query("UPDATE order_fulfillment SET state='delivered',revision=3 WHERE order_id=$1").bind(id).execute(pool).await.unwrap();
                }
            }
        }
    }
    (identity, session)
}
fn filters() -> ListFilters {
    ListFilters {
        reference: "ORD-2025-".into(),
        sort: "reference".into(),
        dir: "asc".into(),
        ..Default::default()
    }
}
fn numbers(rows: &[talocart_commerce::orders::Summary]) -> Vec<i64> {
    rows.iter()
        .map(|r| r.reference.rsplit('-').next().unwrap().parse().unwrap())
        .collect()
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_list_filters_sorting_dates_pagination_and_literal_search() {
    isolated(|pool, _| async move {
        let (identity, session) = seed(&pool).await;
        let orders = Orders::new(pool.clone(), identity);
        let f = filters();
        let first = orders.filtered_list(&session, &f, 1).await.unwrap();
        assert_eq!(first.total, 25);
        assert_eq!(numbers(&first.entries), (1..=20).collect::<Vec<_>>());
        let last = orders.filtered_list(&session, &f, i64::MAX).await.unwrap();
        assert_eq!(last.page, 2);
        assert_eq!(numbers(&last.entries), (21..=25).collect::<Vec<_>>());
        let ten = orders
            .filtered_list(
                &session,
                &ListFilters {
                    per_page: "10".into(),
                    ..f.clone()
                },
                2,
            )
            .await
            .unwrap();
        assert_eq!(numbers(&ten.entries), (11..=20).collect::<Vec<_>>());
        for size in ["50", "100"] {
            let all = orders
                .filtered_list(
                    &session,
                    &ListFilters {
                        per_page: size.into(),
                        ..f.clone()
                    },
                    99,
                )
                .await
                .unwrap();
            assert_eq!(all.page, 1);
            assert_eq!(all.entries.len(), 25);
        }
        let dates = orders
            .filtered_list(
                &session,
                &ListFilters {
                    date: "2026-03-29".into(),
                    ..f.clone()
                },
                1,
            )
            .await
            .unwrap();
        assert_eq!(numbers(&dates.entries), vec![2, 3]);
        assert_eq!(dates.entries[0].created_at, "29/03/2026 00:00:00");
        assert_eq!(dates.entries[1].created_at, "29/03/2026 23:59:59");
        assert_eq!(
            orders
                .filtered_list(
                    &session,
                    &ListFilters {
                        date: "2026-03-30".into(),
                        ..f.clone()
                    },
                    1
                )
                .await
                .unwrap()
                .total,
            1
        );
        for (customer, expected) in [
            ("BUYER_1%@", vec![1]),
            ("A<&", vec![1]),
            ("Name 02", vec![2]),
            ("%", (1..=20).collect()),
            ("_", (1..=20).collect()),
        ] {
            let rows = orders
                .filtered_list(
                    &session,
                    &ListFilters {
                        customer: customer.into(),
                        ..f.clone()
                    },
                    1,
                )
                .await
                .unwrap();
            assert_eq!(numbers(&rows.entries), expected);
        }
        assert_eq!(
            orders
                .filtered_list(
                    &session,
                    &ListFilters {
                        reference: "%".into(),
                        ..f.clone()
                    },
                    1
                )
                .await
                .unwrap()
                .total,
            0
        );
        for (fulfillment, expected) in [
            ("preparing", vec![3]),
            ("shipped", vec![4]),
            ("delivered", vec![7]),
            ("unshipped", vec![5]),
        ] {
            let rows = orders
                .filtered_list(
                    &session,
                    &ListFilters {
                        fulfillment: fulfillment.into(),
                        ..f.clone()
                    },
                    1,
                )
                .await
                .unwrap();
            assert_eq!(numbers(&rows.entries), expected);
        }
        let combined = ListFilters {
            customer: "Name".into(),
            state: "open".into(),
            payment: "paid".into(),
            total: "10,00".into(),
            total_op: "gt".into(),
            ..f.clone()
        };
        assert_eq!(
            numbers(
                &orders
                    .filtered_list(&session, &combined, 1)
                    .await
                    .unwrap()
                    .entries
            ),
            vec![13, 16, 19, 22, 25]
        );
        for (op, expected) in [
            ("eq", vec![10]),
            ("lt", (1..10).collect()),
            ("gt", (11..=25).collect()),
        ] {
            assert_eq!(
                numbers(
                    &orders
                        .filtered_list(
                            &session,
                            &ListFilters {
                                total: "10.00".into(),
                                total_op: op.into(),
                                ..f.clone()
                            },
                            1
                        )
                        .await
                        .unwrap()
                        .entries
                ),
                expected
            );
        }
        for sort in [
            "reference",
            "created",
            "customer",
            "state",
            "payment",
            "fulfillment",
            "total",
        ] {
            let asc = orders
                .filtered_list(
                    &session,
                    &ListFilters {
                        sort: sort.into(),
                        dir: "asc".into(),
                        ..f.clone()
                    },
                    1,
                )
                .await
                .unwrap();
            let desc = orders
                .filtered_list(
                    &session,
                    &ListFilters {
                        sort: sort.into(),
                        dir: "desc".into(),
                        ..f.clone()
                    },
                    1,
                )
                .await
                .unwrap();
            match sort {
                "reference" | "total" | "created" | "customer" => {
                    assert_eq!(numbers(&asc.entries), (1..=20).collect::<Vec<_>>());
                    assert_eq!(numbers(&desc.entries), (6..=25).rev().collect::<Vec<_>>());
                }
                "state" => {
                    assert_eq!(asc.entries[0].status_en, "Awaiting payment");
                    assert_eq!(desc.entries[0].status_en, "Refunded");
                }
                "payment" => {
                    assert!(asc.entries.iter().take(8).all(|r| !r.paid && !r.refunded));
                    assert!(desc.entries.iter().take(8).all(|r| r.refunded));
                }
                _ => {
                    assert!(
                        asc.entries
                            .iter()
                            .take(20)
                            .all(|r| r.fulfillment_state == "pending")
                    );
                    assert_eq!(desc.entries[0].fulfillment_state, "unshipped");
                    assert_eq!(desc.entries[1].fulfillment_state, "delivered");
                }
            }
        }
        for (zone, style, expected) in [
            ("UTC", "ymd", "2026-03-28 23:00:00"),
            ("America/New_York", "mdy", "03/28/2026 19:00:00"),
        ] {
            sqlx::query("UPDATE platform_regional_settings SET time_zone=$1,date_format=$2")
                .bind(zone)
                .bind(style)
                .execute(&pool)
                .await
                .unwrap();
            let rows = orders
                .filtered_list(
                    &session,
                    &ListFilters {
                        customer: "Name 02".into(),
                        ..f.clone()
                    },
                    1,
                )
                .await
                .unwrap();
            assert_eq!(rows.entries[0].created_at, expected);
        }
        assert!(
            sqlx::query("UPDATE order_customer_summaries SET first_name='Changed'")
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM order_customer_summaries")
                .execute(&pool)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn orders_list_http_preserves_filters_sorting_escaping_and_errors() {
    isolated(|pool, _| async move {
        let (identity, _) = seed(&pool).await;
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Orders".into()).await);
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let h = html(
                request(
                    &app,
                    "GET",
                    "/admin/orders?reference=ORD-2025-&sort=reference&dir=asc&total_op=gt&total=0",
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert_eq!(h.matches("scope=\"col\"").count(), 7);
            assert!(h.contains("aria-sort=\"ascending\""));
            assert!(h.contains("&#60;"));
            assert!(!h.contains("A<&"));
            assert!(!h.contains("Europe/Rome"));
            assert!(h.contains("customer="));
            assert!(h.contains("total_op=gt"));
            assert!(h.contains("page=2"));
            assert!(h.contains(if locale == "it" {
                "Nome o email"
            } else {
                "Name or email"
            }));
            let h = html(
                request(
                    &app,
                    "GET",
                    "/admin/orders?customer=NO-SUCH-CUSTOMER",
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(h.contains("colspan=\"7\""));
            assert!(h.contains("id=\"order-customer\""));
            for query in [
                "per_page=999",
                "date=2026-02-29",
                "total=1.001",
                "total=abc",
                "sort=id%3BDROP",
                "dir=other",
                "payment=other",
                "fulfillment=other",
            ] {
                let r = request(
                    &app,
                    "GET",
                    &format!("/admin/orders?{query}"),
                    &cookie,
                    "",
                    false,
                )
                .await;
                assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let h = html(r).await;
                assert!(
                    h.contains("class=\"ui-table ui-table--fixed order-table\""),
                    "invalid filters must retain the page"
                );
                assert!(h.contains("id=\"order-filter-error\""));
                if query == "total=abc" {
                    assert!(h.contains("value=\"abc\""));
                    assert!(h.contains(
                        "aria-invalid=\"true\" aria-describedby=\"order-total-error\" autofocus"
                    ));
                }
                assert!(h.contains(if locale == "it" {
                    "Filtri ordini non validi"
                } else {
                    "Invalid order filters"
                }));
            }
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn configured_order_status_badge_replaces_parallel_fulfillment_column() {
    isolated(|pool, _| async move {
        let (identity, _) = seed(&pool).await;
        let raw = authenticated(&identity).await;
        let app = router(test_app(pool, "Orders".into()).await);
        let h = html(
            request(
                &app,
                "GET",
                "/admin/orders",
                &format!("talocart_session={raw}"),
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(
            h.contains("ui-status-badge"),
            "Order list must render configured status colors"
        );
        assert_eq!(h.matches("scope=\"col\"").count(), 7);
        assert!(!h.contains("id=\"order-fulfillment\""));
    })
    .await;
}
