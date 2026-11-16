use super::*;
use talocart_commerce::taxes::Taxes;

const SNAPSHOT: &str = "SELECT jsonb_build_array((SELECT jsonb_agg(r ORDER BY id) FROM tax_rates r),(SELECT jsonb_agg(c ORDER BY id) FROM tax_classes c),(SELECT jsonb_agg(z ORDER BY id) FROM tax_zones z),(SELECT jsonb_agg(q ORDER BY zone_id,class_id) FROM tax_zone_rules q),(SELECT jsonb_agg(c ORDER BY country_code) FROM tax_zone_countries c),(SELECT to_jsonb(p) FROM pricing_settings p))::text";

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn defaults_clean_installation_remain_editable_without_reseeding() {
    isolated(|pool, _| async move {
        let identity=test_identity(pool.clone()).await;
        let prepared=identity.prepare_bootstrap("owner@example.com","correct horse battery".into(),"Shop","").await.unwrap();
        talocart_commerce::installation::install(&pool,prepared,None).await.unwrap();
        database::migrate(&pool).await.unwrap();
        let rates: Vec<(i32, bool)> = sqlx::query_as("SELECT basis_points,explicit_rule FROM resolve_tax('IT') ORDER BY basis_points").fetch_all(&pool).await.unwrap();
        assert_eq!(rates, vec![(0,true),(400,true),(500,true),(1000,true),(2200,true)]);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM tax_zone_countries WHERE country_code='IT'").fetch_one(&pool).await.unwrap(),1);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_products").fetch_one(&pool).await.unwrap(),0);
        // Explicit zero-rate selection and an ordinary 22% inclusive amount use existing math.
        let session=taxonomy_session(&identity).await;
        let taxes=Taxes::new(pool.clone(),identity);
        for (rate,net,tax) in [(0,12200,0),(2200,10000,2200)] {
            let class: i64=sqlx::query_scalar("SELECT c.id FROM tax_classes c JOIN tax_rates r ON r.id=c.rate_id WHERE r.basis_points=$1").bind(rate).fetch_one(&pool).await.unwrap();
            let quote=taxes.preview(&session,&session.csrf,&talocart_commerce::taxes::Preview {country:"IT".into(),amount:"122.00".into(),quantity:"1".into(),class_id:class.to_string()}).await.unwrap();
            assert_eq!((quote.totals.net().cents(),quote.totals.tax().cents(),quote.totals.gross().cents()),(net,tax,12200));
        }
        sqlx::query("UPDATE tax_rates SET name='Merchant standard',basis_points=2100 WHERE basis_points=2200").execute(&pool).await.unwrap();
        sqlx::query("UPDATE pricing_settings SET default_class_id=NULL,country_code=NULL").execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM tax_zones").execute(&pool).await.unwrap();
        let before: String = sqlx::query_scalar(SNAPSHOT).fetch_one(&pool).await.unwrap();
        database::migrate(&pool).await.unwrap();
        database::migrate(&pool).await.unwrap();
        database::check(&pool).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>(SNAPSHOT).fetch_one(&pool).await.unwrap(),before);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn defaults_setup_rolls_back_and_concurrent_calls_are_idempotent() {
    isolated(|pool, _| async move {
        sqlx::query("UPDATE pricing_settings SET price_mode='exclusive'")
            .execute(&pool)
            .await
            .unwrap();
        let before: String = sqlx::query_scalar(SNAPSHOT).fetch_one(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        Taxes::initialize_defaults(&mut tx).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>(SNAPSHOT)
                .fetch_one(&pool)
                .await
                .unwrap(),
            before
        );
        async fn initialize(pool: &PgPool) {
            let mut tx = pool.begin().await.unwrap();
            Taxes::initialize_defaults(&mut tx).await.unwrap();
            tx.commit().await.unwrap();
        }
        tokio::join!(initialize(&pool), initialize(&pool));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tax_rates")
                .fetch_one(&pool)
                .await
                .unwrap(),
            5
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tax_classes")
                .fetch_one(&pool)
                .await
                .unwrap(),
            5
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tax_zones")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        let before: String = sqlx::query_scalar(SNAPSHOT).fetch_one(&pool).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT price_mode FROM pricing_settings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "exclusive"
        );
        initialize(&pool).await;
        assert_eq!(
            sqlx::query_scalar::<_, String>(SNAPSHOT)
                .fetch_one(&pool)
                .await
                .unwrap(),
            before
        );
    })
    .await;
}
