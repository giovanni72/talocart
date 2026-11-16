use super::checkout_methods::{data, fixture, lang};
use super::*;
use talocart_commerce::{
    cart::{Carts, Guest},
    checkout_methods::Selection,
    orders::{Confirmation, Order, OrderError, Orders},
};
use talocart_platform::{content::Language, identity::Permission, locale::Locale};










fn selection(payment: i64) -> Selection {
    Selection {
        shipping_method_id: Some(1),
        payment_method_id: Some(payment),
    }
}
async fn configured(pool: &PgPool) -> (i64, Identity, talocart_platform::identity::Session) {
    let product = fixture(pool).await;
    let identity = Identity::new(pool.clone());
    let s = taxonomy_session(&identity).await;
    sqlx::query("UPDATE email_configuration SET from_name='Shop',from_email='shop@example.test',admin_recipients=ARRAY['admin@example.test'],enabled=true").execute(pool).await.unwrap();
    (product, identity, s)
}
async fn prepare(
    carts: &Carts,
    pair: (i64, i64),
    quantity: i32,
    payment: i64,
) -> (String, Guest, String) {
    let (raw, g) = carts.create().await.unwrap();
    carts
        .add(&g, &g.csrf, 0, pair, quantity, lang())
        .await
        .unwrap();
    let g = carts.session(&raw).await.unwrap().unwrap();
    let q = carts
        .checkout_quote(&g, lang(), "IT", Some(selection(payment)))
        .await
        .unwrap();
    (raw, g, q.review)
}
async fn confirm(
    orders: &Orders,
    g: &Guest,
    review: &str,
    payment: i64,
) -> std::result::Result<Order, OrderError> {
    orders
        .confirm(Confirmation {
            online_available: true,
            guest: g,
            csrf: &g.csrf,
            revision: g.revision,
            data: data(),
            selection: selection(payment),
            review,
            language: lang(),
        })
        .await
}
async fn count(pool: &PgPool, table: &str) -> i64 {
    match table {
        "orders" => sqlx::query_scalar("SELECT count(*) FROM orders")
            .fetch_one(pool)
            .await
            .unwrap(),
        "mail" => sqlx::query_scalar("SELECT count(*) FROM email_outbox")
            .fetch_one(pool)
            .await
            .unwrap(),
        _ => sqlx::query_scalar("SELECT count(*) FROM order_stock_moves")
            .fetch_one(pool)
            .await
            .unwrap(),
    }
}
