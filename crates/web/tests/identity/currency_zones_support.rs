use super::*;



use talocart_commerce::{
    pricing::PriceMode,
    products::{ProductError, Products},
    taxes::{Kind, Preferences, Preview, TaxError, Taxes, ZoneValues},
};

fn preferences(currency: &str, country: &str) -> Preferences {
    Preferences {
        currency: currency.into(),
        country: country.into(),
        mode: "exclusive".into(),
        default_class_id: String::new(),
    }
}
async fn class(
    taxes: &Taxes,
    s: &talocart_platform::identity::Session,
    name: &str,
    percent: &str,
) -> i64 {
    let rate = taxes
        .save(
            s,
            &s.csrf,
            Kind::Rate,
            None,
            &tax_values(name, percent, None),
            taxes.revision().await.unwrap(),
        )
        .await
        .unwrap();
    taxes
        .save(
            s,
            &s.csrf,
            Kind::Class,
            None,
            &tax_values(name, "", Some(rate)),
            taxes.revision().await.unwrap(),
        )
        .await
        .unwrap()
}
fn zone(name: &str, class_id: i64, countries: &str) -> ZoneValues {
    ZoneValues {
        name: name.into(),
        description: "Synthetic zone".into(),
        rules: [(class_id, class_id)].into_iter().collect(),
        countries: countries.into(),
        active: true,
    }
}












fn zone_for(name: &str, class: i64, rate: i64, countries: &str) -> ZoneValues {
    let mut v = zone(name, class, countries);
    v.rules = [(class, rate)].into_iter().collect();
    v
}
