// Direct order fixtures for exercising list projections.
use super::*;
use talocart_commerce::orders::{ChargeSnapshot, Frozen};
pub(super) struct HistoricalOrder {
    pub id: i64,
}
fn charge(code: &str) -> ChargeSnapshot {
    ChargeSnapshot {
        provider_id: None,
        code: code.into(),
        name_it: code.into(),
        name_en: code.into(),
        instructions_it: String::new(),
        instructions_en: String::new(),
        entered_cents: 0,
        net_cents: 0,
        tax_cents: 0,
        gross_cents: 0,
        class_id: None,
        tax_name: String::new(),
        basis_points: 0,
        zone_name: None,
        price_mode: "inclusive".into(),
        destination: "IT".into(),
        beneficiary: String::new(),
        iban: String::new(),
        bic: String::new(),
        delivery_days: None,
    }
}
pub(super) async fn seed(
    pool: &PgPool,
    guest: &Guest,
    product: i64,
    code: &str,
) -> HistoricalOrder {
    let frozen = Frozen {
        commercial_revision: 0,
        group_discount: None,
        schema_version: 1,
        coupon: None,
        customer: data(),
        customer_locale: "en".into(),
        shop_locale: "en".into(),
        store_name: "Historical shop".into(),
        shipping: charge("national_express"),
        payment: charge(code),
        status_id: 1,
        status_it: "Aperto".into(),
        status_en: "Open".into(),
        status_color: "#777777".into(),
    };
    let snapshot = toml::to_string(&frozen).unwrap();
    let mut tx = pool.begin().await.unwrap();
    let owner: String = sqlx::query_scalar("SELECT token_hash FROM guest_carts WHERE id=$1")
        .bind(guest.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let (id,_reference):(i64,String)=sqlx::query_as("INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents) VALUES(next_order_reference(statement_timestamp()),$1,$2,$3,$4,'EUR',1000,0,0,1000,0,1000) RETURNING id,reference").bind(guest.id).bind(guest.revision).bind(&owner).bind(snapshot).fetch_one(&mut *tx).await.unwrap();
    let line:i64=sqlx::query_scalar("INSERT INTO order_lines(order_id,product_id,quantity,name,label,code,unit_cents,class_id,tax_name,basis_points,price_mode,net_cents,tax_cents,gross_cents) VALUES($1,$2,1,'Historical product','','HISTORICAL',1000,1,'Zero',0,'inclusive',1000,0,1000) RETURNING id").bind(id).bind(product).fetch_one(&mut *tx).await.unwrap();
    let stock: i32 =
        sqlx::query_scalar("UPDATE catalog_products SET stock=stock-1 WHERE id=$1 RETURNING stock")
            .bind(product)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO order_stock_moves(line_id,kind,quantity,stock_after) VALUES($1,'debit',1,$2)",
    )
    .bind(line)
    .bind(stock)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("DELETE FROM guest_cart_items WHERE cart_id=$1")
        .bind(guest.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM checkout_methods WHERE cart_id=$1")
        .bind(guest.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE guest_carts SET revision=revision+1 WHERE id=$1")
        .bind(guest.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    HistoricalOrder { id }
}
