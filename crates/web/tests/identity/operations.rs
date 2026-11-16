use super::*;
use talocart_platform::identity::IdentityError;
use talocart_platform::identity::{Permission, Role, Session};
use talocart_platform::operations;
async fn fixture(pool: &PgPool) -> (Identity, Session) {
    let identity = installed(pool).await;
    let session = taxonomy_session(&identity).await;
    (identity, session)
}
async fn page(pool: &PgPool, slug: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO information_pages(slug,name) VALUES($1,'PRIVATE BODY') RETURNING id",
    )
    .bind(slug)
    .fetch_one(pool)
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn operations_metadata_is_atomic_immutable_and_transaction_local() {
    isolated(|pool,_| async move {
        let (identity,s)=fixture(&pool).await;
        let mut tx=pool.begin().await.unwrap();
        Identity::authorize_transaction(&mut tx,&s,&s.csrf,Permission::PagesWrite).await.unwrap();
        let id:i64=sqlx::query_scalar("INSERT INTO information_pages(slug,name,description) VALUES('audit','PRIVATE TITLE','SECRET BODY') RETURNING id").fetch_one(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let r=operations::list(&pool,&identity,&s,"pages",1).await.unwrap();
        assert_eq!(r.total,1);
        assert_eq!(r.entries[0].administrator_id,s.administrator_id);
        assert_eq!(r.entries[0].resource_key,id.to_string());
        let data:String=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(e))::text FROM operation_events e").fetch_one(&pool).await.unwrap();
        assert!(!data.contains("PRIVATE")); assert!(!data.contains("SECRET")); assert!(!data.contains(&s.csrf)); assert!(!data.contains(&s.token_hash));
        let mut tx=pool.begin().await.unwrap();
        Identity::authorize_transaction(&mut tx,&s,&s.csrf,Permission::PagesWrite).await.unwrap();
        sqlx::query("UPDATE information_pages SET name='rolled back' WHERE id=$1").bind(id).execute(&mut *tx).await.unwrap();
        tx.rollback().await.unwrap();
        sqlx::query("UPDATE information_pages SET name=name WHERE id=$1").bind(id).execute(&pool).await.unwrap();
        assert_eq!(operations::list(&pool,&identity,&s,"pages",1).await.unwrap().total,1);
        page(&pool,"unattributed").await;
        assert_eq!(operations::list(&pool,&identity,&s,"pages",1).await.unwrap().entries[0].administrator_id,None);
        for sql in ["UPDATE operation_events SET action='DELETE'","DELETE FROM operation_events","TRUNCATE operation_events"] {
            assert!(sqlx::query(sqlx::AssertSqlSafe(sql.to_owned())).execute(&pool).await.is_err());
        }
        // Rejected authorization produces no event.
        let mut tx=pool.begin().await.unwrap();
        assert!(Identity::authorize_transaction(&mut tx,&s,"wrong",Permission::PagesWrite).await.is_err());
        tx.rollback().await.unwrap();
        assert_eq!(operations::list(&pool,&identity,&s,"pages",1).await.unwrap().total,2);
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn operations_filters_pagination_permissions_and_http_are_bounded() {
    isolated(|pool, _| async move {
        let (identity, s) = fixture(&pool).await;
        for n in 0..25 {
            page(&pool, &format!("page-{n}")).await;
        }
        let a = operations::list(&pool, &identity, &s, "pages", 1)
            .await
            .unwrap();
        let b = operations::list(&pool, &identity, &s, "pages", 999)
            .await
            .unwrap();
        assert_eq!(
            (a.total, a.entries.len(), b.page, b.entries.len()),
            (25, 20, 2, 5)
        );
        assert!(a.entries.last().unwrap().id > b.entries[0].id);
        for (area, p) in [("bad", 1), ("pages' OR true--", 1), ("", 0), ("", i64::MAX)] {
            assert!(matches!(
                operations::list(&pool, &identity, &s, area, p).await,
                Err(IdentityError::Invalid)
            ));
        }
        let rid = identity
            .create_role(&s, &s.csrf, "Log reader", &[Permission::OperationsRead])
            .await
            .unwrap();
        let uid = identity
            .create_administrator(
                &s,
                &s.csrf,
                "log@example.test",
                "correct horse battery".into(),
                Role::parse(&rid).unwrap(),
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anon,
                &anon.csrf,
                "log@example.test",
                "correct horse battery".into(),
            )
            .await
            .unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for hx in [false, true] {
            let r = request(
                &app,
                "GET",
                "/admin/operations?area=pages&page=2",
                &cookie,
                "",
                hx,
            )
            .await;
            assert_eq!(r.status(), StatusCode::OK);
            assert!(
                r.headers()[header::CACHE_CONTROL]
                    .to_str()
                    .unwrap()
                    .contains("no-store")
            );
            let h = html(r).await;
            assert!(!h.contains("PRIVATE BODY"));
            assert!(h.contains("area=pages"));
            assert_eq!(
                request(&app, "GET", "/admin/operations?area=bad", &cookie, "", hx)
                    .await
                    .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
            assert_eq!(
                request(&app, "GET", "/admin/operations", "", "", hx)
                    .await
                    .status(),
                if hx {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::SEE_OTHER
                }
            );
        }
        identity
            .update_administrator(&s, &s.csrf, uid, Role::Viewer, true)
            .await
            .unwrap();
        assert_ne!(
            request(&app, "GET", "/admin/operations", &cookie, "", false)
                .await
                .status(),
            StatusCode::OK
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn operations_dashboard_counts_and_permission_isolation() {
    isolated(|pool,_|async move {
        let(identity,s)=fixture(&pool).await;
        sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('Draft','DRAFT','draft',100)").execute(&pool).await.unwrap();
        let id=page(&pool,"published").await;
        sqlx::query("UPDATE information_pages SET description='Info',published=true WHERE id=$1").bind(id).execute(&pool).await.unwrap();
        let rows=talocart_commerce::overview::metrics(&pool,&identity,&s).await.unwrap();
        assert_eq!(rows.iter().find(|r|r.kind==talocart_commerce::overview::MetricKind::ProductsTotal).unwrap().value,1);
        assert_eq!(rows.iter().find(|r|r.kind==talocart_commerce::overview::MetricKind::ProductsPublished).unwrap().value,0);
        assert_eq!(rows.iter().find(|r|r.kind==talocart_commerce::overview::MetricKind::PagesPublished).unwrap().value,1);
        let role=identity.create_role(&s,&s.csrf,"Overview only",&[Permission::AdminRead]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"overview@example.test","correct horse battery".into(),Role::parse(&role).unwrap()).await.unwrap();
        let (_,anon)=identity.anonymous().await.unwrap();
        let raw=identity.login(&anon,&anon.csrf,"overview@example.test","correct horse battery".into()).await.unwrap();
        let limited=identity.session(&raw).await.unwrap().unwrap();
        assert!(talocart_commerce::overview::metrics(&pool,&identity,&limited).await.unwrap().is_empty());
        assert!(matches!(operations::list(&pool,&identity,&limited,"",1).await,Err(IdentityError::Forbidden)));
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let cookie=format!("talocart_session={raw}");
        for path in ["/admin","/admin/status","/admin/modules"] {
            let r=request(&app,"GET",path,&cookie,"",false).await;
            assert_eq!(r.status(),StatusCode::OK);
            let h=html(r).await;
            for marker in ["data-metric=","href=\"/admin/operations\"","PRIVATE BODY"] {assert!(!h.contains(marker),"{path}: {marker}");}
        }
        let overview=html(request(&app,"GET","/admin",&cookie,"",false).await).await;
        assert!(overview.contains("id=\"sidebar-modules-link\" href=\"/admin/modules\""),"Modules entry is reachable with read access only");
        assert!(!overview.contains("href=\"/admin/settings\"")&&!overview.contains("href=\"/admin/settings/taxes\""),"No settings link without settings permissions");
        assert_eq!(request(&app,"GET","/admin/operations",&cookie,"",false).await.status(),StatusCode::FORBIDDEN);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL, npm ci and Playwright Chromium"]
async fn operations_browser_bilingual_desktop_mobile_and_no_js() {
    isolated(|pool, _| async move {
        let (identity, _) = fixture(&pool).await;
        for n in 0..25 { page(&pool,&format!("browser-{n}")).await; }
        sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('Draft','DRAFT','draft',100)").execute(&pool).await.unwrap();
        sqlx::raw_sql("INSERT INTO catalog_products_translations(entity_id,locale,name) SELECT id,'en','PRIVATE TRANSLATION' FROM catalog_products;
            INSERT INTO catalog_categories(name,slug) VALUES('Category','category');
            INSERT INTO catalog_brands(name,slug) VALUES('Brand','brand');
            INSERT INTO catalog_categories_translations(entity_id,locale,name) SELECT id,'en','PRIVATE TRANSLATION' FROM catalog_categories;
            INSERT INTO catalog_brands_translations(entity_id,locale,name) SELECT id,'en','PRIVATE TRANSLATION' FROM catalog_brands;
            INSERT INTO tax_rates(name,basis_points) VALUES('Rate',2000);
            INSERT INTO tax_classes(name,rate_id) SELECT 'Class',id FROM tax_rates;
            INSERT INTO tax_zones(name) VALUES('Zone');
            INSERT INTO tax_zone_countries(country_code,zone_id) SELECT 'IT',id FROM tax_zones;
            INSERT INTO tax_zone_rules(zone_id,class_id,rate_id) SELECT z.id,c.id,c.rate_id FROM tax_zones z CROSS JOIN tax_classes c;")
            .execute(&pool).await.unwrap();
        let admin = authenticated(&identity).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = test_app_config(pool.clone(), "Shop operations".into(), talocart_web::AppStateConfig { public_origin: origin.clone(), ..Default::default() })
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
                    .join("../../scripts/browser-operations.cjs"),
            )
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        use tokio::io::AsyncWriteExt;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                serde_json::json!({"origin":origin,"admin":admin})
                    .to_string()
                    .as_bytes(),
            )
            .await
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            child.wait_with_output(),
        )
        .await
        .expect("Browser timeout")
        .unwrap();
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
async fn operations_dashboard_zero_total_is_not_awaiting_payment() {
    isolated(|pool,_|async move {
        let(identity,s)=fixture(&pool).await;
        sqlx::query("INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents) SELECT 'OPS-'||n,n,0,repeat('a',64),'{}','EUR',n*100,0,0,n*100,0,n*100 FROM generate_series(0,1) n").execute(&pool).await.unwrap();
        let metrics=talocart_commerce::overview::metrics(&pool,&identity,&s).await.unwrap();
        assert_eq!(metrics.iter().find(|m|m.kind==talocart_commerce::overview::MetricKind::OrdersTotal).unwrap().value,2);
        assert_eq!(metrics.iter().find(|m|m.kind==talocart_commerce::overview::MetricKind::OrdersAwaitingPayment).unwrap().value,1,"Zero totals require no collection");
        assert_eq!(metrics.iter().find(|m|m.kind==talocart_commerce::overview::MetricKind::OrdersAwaitingShipment).unwrap().value,2,"Zero totals still require fulfillment");
        assert_eq!(operations::list(&pool,&identity,&s,"orders",1).await.unwrap().total,2);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn operations_related_tax_changes_are_recorded() {
    use talocart_commerce::taxes::{Kind, Taxes, ZoneValues};
    isolated(|pool, _| async move {
        let (identity, s) = fixture(&pool).await;
        let taxes = Taxes::new(pool.clone(), identity.clone());
        let rate = taxes.save(&s, &s.csrf, Kind::Rate, None, &tax_values("Audit rate", "20", None), taxes.revision().await.unwrap()).await.unwrap();
        let class = taxes.save(&s, &s.csrf, Kind::Class, None, &tax_values("Audit class", "", Some(rate)), taxes.revision().await.unwrap()).await.unwrap();
        let mut values = ZoneValues { name: "Audit zone".into(), countries: "IT".into(), active: true, ..Default::default() };
        let id = taxes.save_zone(&s, &s.csrf, None, &values, taxes.revision().await.unwrap()).await.unwrap();
        let before: i64 = sqlx::query_scalar("SELECT max(id) FROM operation_events").fetch_one(&pool).await.unwrap();
        values.countries = "FR".into();
        values.rules.insert(class, rate);
        taxes.save_zone(&s, &s.csrf, Some(id), &values, taxes.revision().await.unwrap()).await.unwrap();
        let events: Vec<(String, String, String, Option<i64>)> = sqlx::query_as("SELECT resource,resource_key,action,administrator_id FROM operation_events WHERE id>$1 ORDER BY id").bind(before).fetch_all(&pool).await.unwrap();
        assert_eq!(events, vec![
            ("tax_zone_countries".into(), id.to_string(), "DELETE".into(), s.administrator_id),
            ("tax_zone_countries".into(), id.to_string(), "INSERT".into(), s.administrator_id),
            ("tax_zone_rules".into(), id.to_string(), "INSERT".into(), s.administrator_id),
        ]);
        let before = operations::list(&pool, &identity, &s, "", 1).await.unwrap().total;
        let mut tx = pool.begin().await.unwrap();
        Identity::authorize_transaction(&mut tx,&s,&s.csrf,Permission::TaxesWrite).await.unwrap();
        sqlx::query("DELETE FROM tax_zone_rules WHERE zone_id=$1").bind(id).execute(&mut *tx).await.unwrap();
        sqlx::query("DELETE FROM tax_zone_countries WHERE zone_id=$1").bind(id).execute(&mut *tx).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(operations::list(&pool, &identity, &s, "", 1).await.unwrap().total, before);
        let raw=authenticated(&identity).await;
        let cookie=format!("talocart_session={raw}");
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        for (locale,labels) in [("it",["Paesi della zona fiscale","Regole della zona fiscale"]),("en",["Tax zone countries","Tax zone rules"])] {
            identity.update_locale(&s,&s.csrf,locale).await.unwrap();
            let response=request(&app,"GET","/admin/operations?area=settings",&cookie,"",false).await;
            assert_eq!(response.status(),StatusCode::OK);
            let body=html(response).await;
            for label in labels { assert!(body.contains(label),"{locale}: {label}"); }
        }
        let before = operations::list(&pool, &identity, &s, "", 1).await.unwrap().total;
        values.countries = "INVALID".into();
        assert!(taxes.save_zone(&s, &s.csrf, Some(id), &values, taxes.revision().await.unwrap()).await.is_err());
        assert_eq!(operations::list(&pool, &identity, &s, "", 1).await.unwrap().total, before);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn operations_related_catalog_translations_are_recorded() {
    use talocart_commerce::content::Resource;
    use talocart_platform::content::Translation;
    isolated(|pool, _| async move {
        let (identity, s) = fixture(&pool).await;
        let primary: String = sqlx::query_scalar("SELECT primary_locale FROM storefront_settings WHERE id=1").fetch_one(&pool).await.unwrap();
        let secondary = if primary == "en" { "it" } else { "en" };
        let product: i64 = sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('Audit product','AUDIT','audit-product',100) RETURNING id").fetch_one(&pool).await.unwrap();
        let category: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug) VALUES('Audit category','audit-category') RETURNING id").fetch_one(&pool).await.unwrap();
        let brand: i64 = sqlx::query_scalar("INSERT INTO catalog_brands(name,slug) VALUES('Audit brand','audit-brand') RETURNING id").fetch_one(&pool).await.unwrap();
        for (resource,id,table) in [(Resource::Product,product,"catalog_products_translations"),(Resource::Category,category,"catalog_categories_translations"),(Resource::Brand,brand,"catalog_brands_translations")] {
            let definition = resource.definition();
            let mut rows = definition.load(&pool,&identity,&s,id).await.unwrap();
            rows.iter_mut().find(|r|r.locale==primary).unwrap().name = "Audit primary".into();
            let translated = rows.iter_mut().find(|r|r.locale==secondary).unwrap();
            *translated = Translation {locale: secondary.into(), name: "PRIVATE TRANSLATION".into(), ..Default::default()};
            definition.save(&pool,&s,&s.csrf,id,&primary,&rows).await.unwrap();
            let before: i64 = sqlx::query_scalar("SELECT max(id) FROM operation_events").fetch_one(&pool).await.unwrap();
            rows.iter_mut().find(|r|r.locale==secondary).unwrap().name = "PRIVATE CHANGED".into();
            definition.save(&pool,&s,&s.csrf,id,&primary,&rows).await.unwrap();
            let events: Vec<(String,String,String,Option<i64>)> = sqlx::query_as("SELECT resource,resource_key,action,administrator_id FROM operation_events WHERE id>$1 ORDER BY id").bind(before).fetch_all(&pool).await.unwrap();
            assert_eq!(events,vec![(table.into(),id.to_string(),"UPDATE".into(),s.administrator_id)]);
            let before = operations::list(&pool,&identity,&s,"",1).await.unwrap().total;
            definition.save(&pool,&s,&s.csrf,id,&primary,&rows).await.unwrap();
            assert_eq!(operations::list(&pool,&identity,&s,"",1).await.unwrap().total,before,"Identical translation save is silent");
            assert!(definition.save(&pool,&s,"wrong",id,&primary,&rows).await.is_err());
        }
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (locale, labels) in [("it", ["Traduzioni prodotti", "Traduzioni categorie", "Traduzioni marchi"]), ("en", ["Product translations", "Category translations", "Brand translations"])] {
            identity.update_locale(&s, &s.csrf, locale).await.unwrap();
            let response = request(&app,"GET","/admin/operations?area=catalog",&cookie,"",false).await;
            assert_eq!(response.status(),StatusCode::OK);
            let body=html(response).await;
            for label in labels { assert!(body.contains(label),"{locale}: {label}"); }
            assert!(!body.contains("PRIVATE"));
        }
        let metadata: String = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(e))::text FROM operation_events e").fetch_one(&pool).await.unwrap();
        assert!(!metadata.contains("PRIVATE"));
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn error_journal_requires_current_owner_and_escapes_records() {
    isolated(|pool, _| async move {
        let (identity, owner) = fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let root = std::env::temp_dir().join(format!("talocart-journal-http-{}", talocart_platform::identity::token().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("errors.2026-10-30.jsonl"), concat!("{\"timestamp\":\"2026-10-30T12:00:00Z\",\"level\":\"ERROR\",\"fields\":{\"message\":\"<script>evil()</script>\"}}", "\n")).unwrap();
        let logs = talocart_platform::error_logs::ErrorLogs::new(root.clone());
        let app = router(test_app(pool.clone(), "Journal".into()).await.with_error_logs(logs.clone()));
        for hx in [false, true] {
            let response = request(&app, "GET", "/admin/error-logs", &cookie, "", hx).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers()[header::CACHE_CONTROL].to_str().unwrap().contains("no-store"));
            let body = html(response).await;
            assert!(!body.contains("<script>evil()"));
            assert!(body.contains("evil()"));
            assert_eq!(request(&app,"GET","/admin/error-logs?date=..%2F..%2Fsecret",&cookie,"",hx).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(request(&app,"GET","/admin/error-logs","","",hx).await.status(),if hx {StatusCode::UNAUTHORIZED} else {StatusCode::SEE_OTHER});
        }
        let uid = identity.create_administrator(&owner,&owner.csrf,"journal-viewer@example.test","correct horse battery".into(),Role::Viewer).await.unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let reader = identity.login(&anon,&anon.csrf,"journal-viewer@example.test","correct horse battery".into()).await.unwrap();
        for hx in [false,true] {
            assert_eq!(request(&app,"GET","/admin/error-logs",&format!("talocart_session={reader}"),"",hx).await.status(),StatusCode::FORBIDDEN);
        }
        // Service authorization rechecks database state even with a stale owner snapshot.
        sqlx::query("DELETE FROM admin_sessions WHERE token_hash=$1").bind(&owner.token_hash).execute(&pool).await.unwrap();
        assert!(matches!(logs.read(&identity,&owner,"",None).await,Err(IdentityError::Forbidden)));
        assert!(uid > 0);
        std::fs::remove_dir_all(root).unwrap();
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn sales_history_filters_current_status_period_currency_and_permissions() {
    use talocart_commerce::overview::{SalesPeriod, sales_history};
    isolated(|pool, _| async move {
        let (identity, owner) = fixture(&pool).await;
        sqlx::query("UPDATE platform_regional_settings SET time_zone='Pacific/Kiritimati'").execute(&pool).await.unwrap();
        let statuses = talocart_commerce::orders::Orders::new(pool.clone(), identity.clone()).status_choices(&owner).await.unwrap();
        let a=statuses[0].id; let b=statuses[1].id;
        // Relative to local midnight: inside/outside each period, future, zero and two currencies.
        for (n, age, cents, currency, status) in [
            (1,0,12345,"EUR",a),(2,0,23456,"EUR",b),(3,0,400,"USD",a),
            (4,8,100,"EUR",a),(5,35,200,"EUR",a),(6,95,300,"EUR",a),
            (7,400,900,"EUR",a),(8,-2,800,"EUR",a),(9,0,0,"EUR",a),
        ] {
            sqlx::query("INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents,created_at) VALUES('SALES-'||$1::bigint::text,$1,0,repeat('a',64),$2,$3,$4,0,0,$4,0,$4,((statement_timestamp() AT TIME ZONE 'Pacific/Kiritimati')::date-$5::int)::timestamp AT TIME ZONE 'Pacific/Kiritimati')")
                .bind(n as i64).bind(format!("status_id = {status}")).bind(currency).bind(cents as i64).bind(age).execute(&pool).await.unwrap();
        }
        let week=sales_history(&pool,&identity,&owner,SalesPeriod::Week,Some(vec![a,a])).await.unwrap();
        assert_eq!(week.selected,vec![a]);
        assert_eq!(week.buckets.len(),14,"Zero-filled week for each currency");
        assert_eq!(week.buckets.iter().map(|b|b.orders).sum::<i64>(),3,"Includes zero totals, excludes other status, old and future orders");
        assert_eq!(week.buckets.iter().filter(|b|b.currency=="EUR").map(|b|b.cents).sum::<i64>(),12345);
        assert_eq!(week.buckets.iter().filter(|b|b.currency=="USD").map(|b|b.cents).sum::<i64>(),400);
        for (period,expected) in [(SalesPeriod::Month,5),(SalesPeriod::Quarter,6),(SalesPeriod::Year,7)] {
            let result=sales_history(&pool,&identity,&owner,period,None).await.unwrap();
            assert_eq!(result.buckets.iter().map(|b|b.orders).sum::<i64>(),expected);
            assert_eq!(result.buckets.len(),if period==SalesPeriod::Year {24} else if period==SalesPeriod::Month {60} else {180});
        }
        let none=sales_history(&pool,&identity,&owner,SalesPeriod::Week,Some(vec![])).await.unwrap();
        assert_eq!(none.buckets.len(),7);
        assert!(none.buckets.iter().all(|b|b.orders==0&&b.cents==0));
        assert!(matches!(sales_history(&pool,&identity,&owner,SalesPeriod::Week,Some(vec![i64::MAX])).await,Err(IdentityError::Invalid)));
        // Current status changes retroactively affect inclusion, without changing the frozen snapshot.
        sqlx::query("UPDATE order_current_status SET status_id=$1,revision=revision+1 WHERE order_id=(SELECT id FROM orders WHERE reference='SALES-2')").bind(a).execute(&pool).await.unwrap();
        let changed=sales_history(&pool,&identity,&owner,SalesPeriod::Week,Some(vec![a])).await.unwrap();
        assert_eq!(changed.buckets.iter().map(|b|b.orders).sum::<i64>(),4);
        let role=identity.create_role(&owner,&owner.csrf,"Sales restricted",&[Permission::AdminRead]).await.unwrap();
        identity.create_administrator(&owner,&owner.csrf,"sales@example.test","correct horse battery".into(),Role::parse(&role).unwrap()).await.unwrap();
        let (_,anon)=identity.anonymous().await.unwrap();
        let token=identity.login(&anon,&anon.csrf,"sales@example.test","correct horse battery".into()).await.unwrap();
        let limited=identity.session(&token).await.unwrap().unwrap();
        assert!(matches!(sales_history(&pool,&identity,&limited,SalesPeriod::Week,None).await,Err(IdentityError::Forbidden)));
        let app=router(test_app(pool.clone(),"Sales".into()).await);
        let response=request(&app,"GET","/admin",&format!("talocart_session={token}"),"",false).await;
        assert_eq!(response.status(),StatusCode::OK);
        assert!(!html(response).await.contains("id=\"sales-title\""));
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn dashboard_sales_fragment_preserves_validation_permissions_and_native_document() {
    isolated(|pool, _| async move {
        let (identity, owner) = fixture(&pool).await;
        let cookie = format!("talocart_session={}", authenticated(&identity).await);
        let app = router(test_app(pool.clone(), "Sales".into()).await);
        for locale in ["it", "en"] {
            identity
                .update_locale(&owner, &owner.csrf, locale)
                .await
                .unwrap();
            for partial in [true, false] {
                let response = request(
                    &app,
                    "GET",
                    "/admin?filtered=1&period=7d&metric=orders",
                    &cookie,
                    "",
                    partial,
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
                assert!(
                    response
                        .headers()
                        .get_all(header::VARY)
                        .iter()
                        .any(|v| v.to_str().unwrap().contains("HX-Request"))
                );
                let body = html(response).await;
                assert_eq!(body.contains("<!doctype html>"), !partial);
                assert!(body.contains("id=\"dashboard-sales\""));
                assert!(body.contains("id=\"sales-period\""));
                assert!(body.contains(if locale == "it" {
                    "Andamento delle vendite"
                } else {
                    "Sales trend"
                }));
                assert_eq!(body.contains("class=\"admin-sidebar\""), !partial);
                assert_eq!(body.contains("id=\"dash-group-orders\""), !partial);
                assert_eq!(
                    request(&app, "GET", "/admin?period=invalid", &cookie, "", partial)
                        .await
                        .status(),
                    StatusCode::UNPROCESSABLE_ENTITY
                );
                assert_eq!(
                    request(
                        &app,
                        "GET",
                        "/admin?status=9223372036854775807",
                        &cookie,
                        "",
                        partial
                    )
                    .await
                    .status(),
                    StatusCode::UNPROCESSABLE_ENTITY
                );
            }
        }
        let history = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin?period=12m")
                    .header(header::COOKIE, &cookie)
                    .header("hx-request", "true")
                    .header("hx-history-restore-request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(history.status(), StatusCode::OK);
        assert!(html(history).await.contains("<!doctype html>"));
        // A filter must not wait for unrelated card or shell reads.
        let mut blocked = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE catalog_products,storefront_settings IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *blocked)
            .await
            .unwrap();
        let fragment = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            request(&app, "GET", "/admin?period=7d", &cookie, "", true),
        )
        .await;
        blocked.rollback().await.unwrap();
        assert_eq!(
            fragment
                .expect("Sales fragment avoids unrelated tables")
                .status(),
            StatusCode::OK
        );
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Overview only",
                &[Permission::AdminRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "fragment@example.test",
                "correct horse battery".into(),
                Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let token = identity
            .login(
                &anon,
                &anon.csrf,
                "fragment@example.test",
                "correct horse battery".into(),
            )
            .await
            .unwrap();
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin",
                &format!("talocart_session={token}"),
                "",
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&app, "GET", "/admin", "", "", true).await.status(),
            StatusCode::UNAUTHORIZED
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn dashboard_customer_counts_use_a_rolling_window_and_their_permission() {
    use talocart_platform::identity::customer_admin::customer_counts;
    isolated(|pool, _| async move {
        let (identity, owner) = fixture(&pool).await;
        for (email, age) in [("now@example.test",0),("recent@example.test",29),("old@example.test",31)] {
            sqlx::query("INSERT INTO customers(email,password_hash,first_name,last_name,created_at) VALUES($1,'synthetic','Synthetic','Customer',clock_timestamp()-make_interval(days=>$2))")
                .bind(email).bind(age).execute(&pool).await.unwrap();
        }
        assert_eq!(customer_counts(&pool,&identity,&owner).await.unwrap(),(3,2));
        let app=router(test_app(pool.clone(),"Customers".into()).await);
        for (name, email, permissions, visible) in [
            ("Customer reader", "customers@example.test", vec![Permission::AdminRead,Permission::CustomersRead], true),
            ("Overview only", "overview@example.test", vec![Permission::AdminRead], false),
        ] {
            let role=identity.create_role(&owner,&owner.csrf,name,&permissions).await.unwrap();
            identity.create_administrator(&owner,&owner.csrf,email,"correct horse battery".into(),Role::parse(&role).unwrap()).await.unwrap();
            // Login rotates the anonymous session, so each administrator starts from a fresh one.
            let (_,anon)=identity.anonymous().await.unwrap();
            let token=identity.login(&anon,&anon.csrf,email,"correct horse battery".into()).await.unwrap();
            let session=identity.session(&token).await.unwrap().unwrap();
            assert_eq!(customer_counts(&pool,&identity,&session).await.is_ok(),visible);
            let body=html(request(&app,"GET","/admin",&format!("talocart_session={token}"),"",false).await).await;
            assert_eq!(body.contains("data-metric=\"dash-customers\""),visible,"{email}");
            assert_eq!(body.contains("data-metric=\"dash-customers-new\""),visible,"{email}");
            assert_eq!(body.contains("id=\"dash-group-customers\""),visible,"{email}");
        }
    }).await;
}
