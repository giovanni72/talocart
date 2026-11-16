use super::*;
use talocart_commerce::{
    cart::{CartError, Carts},
    checkout_details::Details,
    checkout_methods::Selection,
};
use talocart_platform::{content::Language, locale::Locale};


pub(super) fn lang() -> Language {
    Language::new(Locale::parse("en").unwrap(), Locale::parse("en").unwrap())
}
pub(super) fn data() -> Details {
    Details {
        email: "buyer@example.test".into(),
        first_name: "Alex".into(),
        last_name: "Example".into(),
        phone: "+393331234567".into(),
        shipping_country: "IT".into(),
        shipping_street: "Example".into(),
        shipping_number: "1".into(),
        shipping_postal: "00100".into(),
        shipping_city: "Roma".into(),
        shipping_region: "RM".into(),
        ..Default::default()
    }
}
pub(super) async fn fixture(pool: &PgPool) -> i64 {
    let identity = installed(pool).await;
    let s = taxonomy_session(&identity).await;
    talocart_platform::storefront::Storefront::new(pool.clone())
        .save_languages(&s, &s.csrf, "en", &["en".into(), "it".into()])
        .await
        .unwrap();
    zero_charge_class(pool).await;
    sqlx::raw_sql("INSERT INTO checkout_countries(country_code) VALUES('IT'),('FR'); UPDATE tax_rates SET basis_points=2200 WHERE id=1; UPDATE pricing_settings SET price_mode='exclusive'; UPDATE shipping_methods SET active=true,fee_value=100,tax_class_id=1,delivery_days=2 WHERE id=1; INSERT INTO shipping_method_countries(method_id,country_code) VALUES(1,'IT'),(2,'FR'); UPDATE shipping_methods SET active=true WHERE id=2; UPDATE payment_methods SET active=true,fee_kind='percent',fee_value=1000,tax_class_id=1,beneficiary=CASE WHEN id=1 THEN 'Example' ELSE '' END,iban=CASE WHEN id=1 THEN 'IT60X0542811101000000123456' ELSE '' END,minimum_cents=0 WHERE code IN ('bank_transfer','cash_on_delivery');").execute(pool).await.unwrap();
    sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock,status,tax_class_id) VALUES('Item','METHODS-1','method-item',1000,10,'published',1) RETURNING id").fetch_one(pool).await.unwrap()
}
