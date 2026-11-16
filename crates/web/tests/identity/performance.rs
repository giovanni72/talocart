use super::*;
use talocart_platform::identity::IdentityError;
use talocart_platform::{
    identity::{CustomerProfile, Customers, Permission},
    media::library::{Filter, MediaLibrary},
};

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn media_aggregation_preserves_counts_filters_order_pages_and_ownership() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let media = test_media(&pool).await;
        let library = MediaLibrary::new(pool.clone(), identity.clone(), media);
        sqlx::raw_sql("INSERT INTO media_objects(key,content_type,byte_size,width,height) SELECT lpad(to_hex(n),64,'0')||'.png','image/png',n+100,n+10,n+20 FROM generate_series(1,96) n; INSERT INTO media_library(key,folder_id,name) SELECT key,(SELECT id FROM media_folders WHERE path='library'),'asset-'||id FROM media_objects;").execute(&pool).await.unwrap();
        sqlx::query("UPDATE media_library SET owner_id=$1 WHERE key IN(SELECT key FROM media_objects WHERE id%7=0)").bind(session.administrator_id).execute(&pool).await.unwrap();
        sqlx::raw_sql("UPDATE media_library SET trashed_at=now() WHERE key IN(SELECT key FROM media_objects WHERE id%11=0); INSERT INTO catalog_products(name,model,slug,image_key,price_cents) SELECT 'Product '||id,'P-'||id,'product-'||id,key,0 FROM media_objects WHERE id<=8; INSERT INTO media_transforms(source_key,kind,object_key) VALUES(lpad('1',64,'0')||'.png','logo',lpad('2',64,'0')||'.png'),(lpad('1',64,'0')||'.png','favicon',lpad('2',64,'0')||'.png'),(lpad('2',64,'0')||'.png','favicon',lpad('2',64,'0')||'.png');").execute(&pool).await.unwrap();
        for personal in [false, true] {
            for trash in [false, true] {
                for usage in ["", "used", "unused"] {
                    for (order, sql_order) in [("name", "lower(l.name),m.id"), ("newest", "m.id DESC"), ("size", "m.byte_size DESC,m.id"), ("dimensions", "m.width::bigint*m.height DESC,m.id")] {
                        // Independent reference: the pre-optimization OR/IN semantics.
                        let expected: Vec<(i64,i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT m.id,(SELECT count(*) FROM media_usage u WHERE u.key=m.key OR u.key IN(SELECT object_key FROM media_transforms WHERE source_key=m.key)) AS uses FROM media_library l JOIN media_objects m ON m.key=l.key WHERE l.owner_id IS NOT DISTINCT FROM $1 AND (l.trashed_at IS NOT NULL)=$2 AND ($3='' OR EXISTS(SELECT 1 FROM media_usage u WHERE u.key=l.key OR u.key IN(SELECT object_key FROM media_transforms WHERE source_key=l.key))=($3='used')) ORDER BY {sql_order}")))
                            .bind(if personal {session.administrator_id}else{None}).bind(trash).bind(usage).fetch_all(&pool).await.unwrap();
                        for page in [1, 99] {
                            let filter=Filter {personal,trash,usage:usage.into(),order:order.into(),page,..Default::default()};
                            let result=library.assets_counted(&session,&filter).await.unwrap();
                            let pages=((expected.len().max(1)-1)/40+1) as i64;
                            assert_eq!(result.total,expected.len() as i64);
                            assert_eq!(result.page,page.min(pages));
                            let actual:Vec<_>=result.entries.iter().map(|a|(a.id,a.uses)).collect();
                            assert_eq!(actual,expected.iter().skip(((result.page-1)*40) as usize).take(40).copied().collect::<Vec<_>>());
                        }
                        let legacy=library.assets(&session,&Filter {personal,trash,usage:usage.into(),order:order.into(),page:1,..Default::default()}).await.unwrap();
                        assert_eq!(legacy.iter().map(|a|(a.id,a.uses)).collect::<Vec<_>>(),expected.into_iter().take(41).collect::<Vec<_>>());
                    }
                }
            }
        }
        sqlx::query("DELETE FROM admin_sessions WHERE token_hash=$1").bind(&session.token_hash).execute(&pool).await.unwrap();
        assert!(library.assets_counted(&session,&Filter::default()).await.is_err());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn grouped_permissions_recheck_current_role_expiry_and_revocation() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let permissions = [Permission::AdminRead, Permission::ProductsRead];
        assert!(identity.authorize_all(&session, &permissions).await.is_ok());
        let role = identity
            .create_role(&session, &session.csrf, "Performance reader", &permissions)
            .await
            .unwrap();
        identity
            .create_administrator(
                &session,
                &session.csrf,
                "performance-reader@example.test",
                "synthetic reader password".into(),
                talocart_platform::identity::Role::Custom(role.clone()),
            )
            .await
            .unwrap();
        let (_, anonymous) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anonymous,
                &anonymous.csrf,
                "performance-reader@example.test",
                "synthetic reader password".into(),
            )
            .await
            .unwrap();
        let session = identity.session(&raw).await.unwrap().unwrap();
        assert!(identity.authorize_all(&session, &permissions).await.is_ok());
        // The supplied permission snapshot must not override the current role.
        sqlx::query(
            "DELETE FROM admin_role_permissions WHERE role_key=$1 AND permission='products.read'",
        )
        .bind(role)
        .execute(&pool)
        .await
        .unwrap();
        assert!(matches!(
            identity.authorize_all(&session, &permissions).await,
            Err(IdentityError::Forbidden)
        ));
        assert!(
            identity
                .authorize_all(&session, &[Permission::AdminRead])
                .await
                .is_ok()
        );
        sqlx::query(
            "UPDATE admin_sessions SET expires_at=now()-interval '1 second' WHERE token_hash=$1",
        )
        .bind(&session.token_hash)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            identity
                .authorize_all(&session, &[Permission::AdminRead])
                .await
                .is_err()
        );
        sqlx::query("DELETE FROM admin_sessions WHERE token_hash=$1")
            .bind(&session.token_hash)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            identity
                .authorize_all(&session, &[Permission::AdminRead])
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn combined_customer_read_retains_both_locks_and_rechecks_capability() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        let (_, anonymous) = service.anonymous().await.unwrap();
        let raw = service
            .register(
                &anonymous,
                &anonymous.csrf,
                &CustomerProfile {
                    email: "performance@example.test".into(),
                    first_name: "Alex".into(),
                    last_name: "Example".into(),
                    customer_type: "private".into(),
                },
                "synthetic customer password".into(),
                None,
            )
            .await
            .unwrap()
            .expect("Immediate activation");
        let session = service.session(&raw).await.unwrap().unwrap();
        let mut tx = pool.begin().await.unwrap();
        let account = Customers::authorize_in(&mut tx, &session, None)
            .await
            .unwrap();
        assert_eq!(account.email, "performance@example.test");
        for sql in [
            "UPDATE customers SET revision=revision+1 WHERE id=$1",
            "UPDATE customer_sessions SET expires_at=now() WHERE customer_id=$1",
        ] {
            let mut writer = pool.begin().await.unwrap();
            sqlx::query("SET LOCAL lock_timeout='100ms'")
                .execute(&mut *writer)
                .await
                .unwrap();
            let error = sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(account.id)
                .execute(&mut *writer)
                .await
                .unwrap_err();
            assert_eq!(
                error.as_database_error().and_then(|e| e.code()).as_deref(),
                Some("55P03")
            );
            writer.rollback().await.unwrap();
        }
        tx.commit().await.unwrap();
        let mut forged = session.clone();
        forged.csrf = "wrong".into();
        assert!(service.account(&forged).await.is_err());
        sqlx::query(
            "UPDATE customer_sessions SET expires_at=now()-interval '1 second' WHERE token_hash=$1",
        )
        .bind(digest(&raw))
        .execute(&pool)
        .await
        .unwrap();
        assert!(service.account(&session).await.is_err());
        sqlx::query("DELETE FROM customer_sessions WHERE token_hash=$1")
            .bind(digest(&raw))
            .execute(&pool)
            .await
            .unwrap();
        assert!(service.account(&session).await.is_err());
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn translation_join_deduplication_preserves_every_fallback_and_primary_change() {
    use talocart_platform::{content::Language, locale::Locale};
    isolated(|pool, _| async move {
        // Independent oracle retains the former two-join expression. Missing rows,
        // blank fields and primary overrides of original content must stay distinct.
        sqlx::raw_sql("CREATE TABLE performance_translations(entity_id bigint,locale text,name text,description text,PRIMARY KEY(entity_id,locale)); INSERT INTO performance_translations VALUES (1,'it','Nome',''),(1,'en','','Description'),(2,'it','',''),(3,'en','English',''),(4,'it','Override','Descrizione'),(4,'en','Name','Text');").execute(&pool).await.unwrap();
        for primary in [Locale::ITALIAN, Locale::ENGLISH] {
            for current in [Locale::ITALIAN, Locale::ENGLISH] {
                let language = Language::new(current, primary);
                let old = format!("SELECT p.id,coalesce(nullif(ct.name,''),nullif(cp.name,''),p.name),coalesce(nullif(ct.description,''),nullif(cp.description,''),p.description) FROM (SELECT n AS id,'Original '||n AS name,'Base '||n AS description FROM generate_series(1,5) n) p LEFT JOIN performance_translations ct ON ct.entity_id=p.id AND ct.locale='{}' LEFT JOIN performance_translations cp ON cp.entity_id=p.id AND cp.locale='{}' ORDER BY p.id",current.code(),primary.code());
                let optimized = format!("SELECT p.id,{},{} FROM (SELECT n AS id,'Original '||n AS name,'Base '||n AS description FROM generate_series(1,5) n) p {} ORDER BY p.id",language.expression("name","p.name"),language.expression("description","p.description"),language.joins("performance","p.id"));
                let expected: Vec<(i32,String,String)> = sqlx::query_as(sqlx::AssertSqlSafe(old)).fetch_all(&pool).await.unwrap();
                let actual: Vec<(i32,String,String)> = sqlx::query_as(sqlx::AssertSqlSafe(optimized)).fetch_all(&pool).await.unwrap();
                assert_eq!(actual,expected,"current={current:?}, primary={primary:?}");
                assert_eq!(actual[4],(5,"Original 5".into(),"Base 5".into()));
            }
        }
    }).await;
}
