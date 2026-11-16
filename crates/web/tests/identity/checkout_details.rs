use super::*;
use talocart_commerce::{
    cart::{CartError, Carts},
    checkout_details::Details,
};
async fn setup(pool: &PgPool) {
    let identity = installed(pool).await;
    let s = taxonomy_session(&identity).await;
    talocart_platform::storefront::Storefront::new(pool.clone())
        .save_languages(&s, &s.csrf, "en", &["en".into(), "it".into()])
        .await
        .unwrap();
    sqlx::query("INSERT INTO checkout_countries(country_code) VALUES('IT')")
        .execute(pool)
        .await
        .unwrap();
}
fn data() -> Details {
    Details {
        email: "buyer@example.test".into(),
        first_name: "Alex".into(),
        last_name: "Example".into(),
        phone: "+39 333 1234567".into(),
        shipping_country: "IT".into(),
        shipping_street: "Via Example".into(),
        shipping_number: "1".into(),
        shipping_city: "Roma".into(),
        shipping_postal: "00100".into(),
        shipping_region: "RM".into(),
        ..Default::default()
    }
}
fn body(csrf: &str, revision: i64) -> String {
    format!(
        "csrf={csrf}&revision={revision}&customer_type=private&email=buyer%40example.test&first_name=Alex&last_name=Example&phone=%2B39+3331234567&shipping_country=IT&shipping_street=Via+Example&shipping_number=1&shipping_city=Roma&shipping_postal=00100&shipping_region=RM&billing_same=true"
    )
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_details_saved_address_selection_preserves_draft_and_checks_owner() {
    use talocart_commerce::customer_addresses::{AddressInput, CustomerAddresses};
    use talocart_platform::identity::{CustomerProfile, Customers};
    isolated(|pool, _| async move {
        setup(&pool).await;
        let customers = Customers::new(pool.clone(), Identity::new(pool.clone()));
        let (_, anonymous) = customers.anonymous().await.unwrap();
        let raw_customer = customers.register(&anonymous, &anonymous.csrf, &CustomerProfile {
            email: "address-buyer@example.test".into(), first_name: "Alex".into(),
            last_name: "Example".into(), customer_type: "private".into(),
        }, "synthetic customer password".into(), None).await.unwrap().unwrap();
        let customer = customers.session(&raw_customer).await.unwrap().unwrap();
        let addresses = CustomerAddresses::new(pool.clone());
        let id = addresses.save(&customer, &customer.csrf, None, &AddressInput {
            label: "Home <safe>".into(), first_name: "Recipient".into(), last_name: "Example".into(),
            phone: "+39 3331234567".into(), country: "IT".into(), street: "Saved street".into(),
            number: "2".into(), postal: "00100".into(), city: "Roma".into(), region: "RM".into(),
            ..Default::default()
        }).await.unwrap();
        let (_, other_anonymous) = customers.anonymous().await.unwrap();
        let other_raw = customers.register(&other_anonymous, &other_anonymous.csrf, &CustomerProfile {
            email: "other-address-buyer@example.test".into(), first_name: "Other".into(),
            last_name: "Buyer".into(), customer_type: "private".into(),
        }, "synthetic customer password".into(), None).await.unwrap().unwrap();
        let other = customers.session(&other_raw).await.unwrap().unwrap();
        let mut other_input = addresses.get(&customer, id).await.unwrap().input();
        other_input.label = "Another customer's home".into();
        let other_id = addresses.save(&other, &other.csrf, None, &other_input).await.unwrap();
        let carts = Carts::new(pool.clone());
        let (raw_cart, guest) = carts.create().await.unwrap();
        carts.merge_customer(&customer, &customer.csrf, &guest, &guest.csrf, guest.revision).await.unwrap();
        let guest = carts.customer_cart(&customer).await.unwrap().unwrap();
        let cookie = format!("talocart_cart={raw_cart}; talocart_customer={raw_customer}");
        let app = router(test_app(pool.clone(), "Details".into()).await);
        for path in ["/checkout", "/it/checkout"] {
            let response = request(&app, "GET", path, &cookie, "", false).await;
            let h = html(response).await;
            assert!(!h.contains("aria-invalid=\"true\""), "Profile prefill must not validate an unsubmitted checkout");
            assert!(!h.contains("id=\"co-errors\""));
            assert!(h.contains("id=\"checkout-address\""), "Checkout must expose the customer's saved addresses");
            assert!(h.contains("Home &#60;safe&#62;") || h.contains("Home &lt;safe&gt;"));
            assert!(!h.contains("Another customer"));
            let incomplete = format!("csrf={}&revision={}&email=address-buyer%40example.test", guest.csrf, guest.revision);
            let response = request(&app, "POST", path, &cookie, &format!("{incomplete}&action=refresh"), false).await;
            assert_eq!(response.status(), StatusCode::OK);
            let h = html(response).await;
            assert!(!h.contains("aria-invalid=\"true\""), "Refreshing requirements must not validate the incomplete form");
            assert!(!h.contains("id=\"co-errors\""));
            let response = request(&app, "POST", path, &cookie, &format!("{incomplete}&action=continue"), false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert!(html(response).await.contains("aria-invalid=\"true\""), "Continue must still validate the incomplete form");
            let submitted = format!("{}&action=address&address_id={id}&delivery_notes=Keep+my+notes&billing_same=false&billing_street=Fiscal+street", body(&guest.csrf, guest.revision).replace("&billing_same=true", ""));
            let response = request(&app, "POST", path, &cookie, &submitted.replace("first_name=Alex", "first_name="), false).await;
            assert_eq!(response.status(), StatusCode::OK);
            let h = html(response).await;
            assert!(!h.contains("aria-invalid=\"true\""), "Applying an address must not validate unrelated incomplete fields");
            assert!(!h.contains("id=\"co-errors\""));
            let response = request(&app, "POST", path, &cookie, &submitted, false).await;
            assert_eq!(response.status(), StatusCode::OK);
            let h = html(response).await;
            for value in ["Saved street", "+39 3331234567", "Recipient", "Keep my notes", "Fiscal street"] {
                assert!(h.contains(value), "Address selection must preserve unrelated submitted fields");
            }
            assert!(carts.details(&guest).await.unwrap().is_none(), "Selection only fills the form; continue persists it");
            assert_eq!(request(&app, "POST", path, &cookie, &submitted.replace(&guest.csrf, "bad"), false).await.status(), StatusCode::FORBIDDEN);
            let invalid = request(&app, "POST", path, &cookie, &submitted.replace(&format!("address_id={id}"), "address_id="), false).await;
            assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert!(html(invalid).await.contains("Keep my notes"));
            assert_eq!(request(&app, "POST", path, &cookie, &submitted.replace(&format!("address_id={id}"), "address_id=9223372036854775807"), false).await.status(), StatusCode::NOT_FOUND);
            assert_eq!(request(&app, "POST", path, &cookie, &submitted.replace(&format!("address_id={id}"), &format!("address_id={other_id}")), false).await.status(), StatusCode::NOT_FOUND);
            let anonymous_cookie = format!("talocart_cart={raw_cart}");
            assert_eq!(request(&app, "POST", path, &anonymous_cookie, &submitted, false).await.status(), StatusCode::FORBIDDEN);
        }
        carts.save_details(&guest, &guest.csrf, guest.revision, data()).await.unwrap();
        let guest = carts.customer_cart(&customer).await.unwrap().unwrap();
        let submitted = format!("{}&action=address&address_id={id}", body(&guest.csrf, guest.revision));
        assert_eq!(request(&app, "POST", "/checkout", &cookie, &submitted, false).await.status(), StatusCode::OK);
        assert_eq!(carts.details(&guest).await.unwrap().unwrap().shipping_street, "Via Example", "Filling a saved address never replaces the persisted draft before Continue");
        let (other_raw, other_guest) = carts.create().await.unwrap();
        let response = request(&app, "GET", "/checkout", &format!("talocart_cart={other_raw}"), "", false).await;
        assert!(!html(response).await.contains("id=\"checkout-address\""));
        let submitted = format!("{}&action=address&address_id={id}", body(&other_guest.csrf, other_guest.revision));
        assert_eq!(request(&app, "POST", "/checkout", &format!("talocart_cart={other_raw}"), &submitted, false).await.status(), StatusCode::FORBIDDEN);
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_details_http_bilingual_first_step_validation_csrf_and_stale() {
    isolated(|pool,_|async move{
 setup(&pool).await;let carts=Carts::new(pool.clone());let(raw,g)=carts.create().await.unwrap();let cookie=format!("talocart_cart={raw}");let app=router(test_app(pool.clone(),"Details".into()).await);
 for path in ["/checkout","/it/checkout"] {let r=request(&app,"GET",path,&cookie,"",false).await;assert_eq!(r.status(),StatusCode::OK);assert_eq!(r.headers()[header::CACHE_CONTROL],"no-store");let h=html(r).await;assert!(h.contains("id=\"checkout-details\""));assert!(h.contains("name=\"billing_same\""));assert!(h.contains(if path.starts_with("/it"){"Richiedo fattura"}else{"I need an invoice"}));assert!(!h.contains("country-IT"));}
 assert_eq!(request(&app,"POST","/checkout",&cookie,&body("bad",0),false).await.status(),StatusCode::FORBIDDEN);
 let invalid=body(&g.csrf,0).replace("private","business");let r=request(&app,"POST","/checkout",&cookie,&invalid,false).await;assert_eq!(r.status(),StatusCode::UNPROCESSABLE_ENTITY);let h=html(r).await;assert!(h.contains("err-vat_id"));assert!(h.contains("value=\"Alex\""));
 assert_eq!(request(&app,"POST","/checkout",&cookie,&body(&g.csrf,0),false).await.status(),StatusCode::SEE_OTHER);
 let d=carts.details(&g).await.unwrap().unwrap();assert!(!d.invoice_requested);assert!(d.tax_id.is_empty());
 let r=request(&app,"POST","/checkout",&cookie,&body(&g.csrf,0),false).await;assert_eq!(r.status(),StatusCode::CONFLICT);assert!(html(r).await.contains("value=\"continue\" disabled"));
 let invoice=body(&g.csrf,1)+"&invoice_requested=true&tax_id=RSSMRA80A01H501U";
 assert_eq!(request(&app,"POST","/it/checkout",&cookie,&invoice,false).await.status(),StatusCode::SEE_OTHER);
 let d=carts.details(&g).await.unwrap().unwrap();assert!(d.invoice_requested&&d.billing_same);assert_eq!(d.billing_city,"Roma");
 // An unchecked checkbox is absent from a native form, including the no-JS path.
 let separate=body(&g.csrf,2).replace("&billing_same=true","")+"&invoice_requested=true&tax_id=RSSMRA80A01H501U&billing_country=FR&billing_street=Rue+Example&billing_number=2&billing_city=Paris&billing_postal=75001";
 assert_eq!(request(&app,"POST","/checkout",&cookie,&separate,false).await.status(),StatusCode::SEE_OTHER);
 let d=carts.details(&g).await.unwrap().unwrap();assert!(!d.billing_same);assert_eq!(d.billing_country,"FR");
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn checkout_details_identity_concurrency_expiry_restart_and_destination() {
    isolated(|pool, options| async move {
        setup(&pool).await;
        let carts = Carts::new(pool.clone());
        let (raw, a) = carts.create().await.unwrap();
        let (_, b) = carts.create().await.unwrap();
        let (x, y) = tokio::join!(
            carts.save_details(&a, &a.csrf, 0, data()),
            carts.save_details(&a, &a.csrf, 0, data())
        );
        assert_eq!([x.is_ok(), y.is_ok()].into_iter().filter(|b| *b).count(), 1);
        assert!(carts.details(&b).await.unwrap().is_none());
        let mut forged = a.clone();
        forged.id = b.id;
        assert!(carts.details(&forged).await.unwrap().is_none());
        assert!(matches!(
            carts.save_details(&forged, &forged.csrf, 0, data()).await,
            Err(CartError::Forbidden)
        ));
        let restarted = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let service = Carts::new(restarted.clone());
        let guest = service.session(&raw).await.unwrap().unwrap();
        assert_eq!(
            service.details(&guest).await.unwrap().unwrap().first_name,
            "Alex"
        );
        restarted.close().await;
        sqlx::query("DELETE FROM checkout_countries WHERE country_code='IT'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            carts.save_details(&a, &a.csrf, 1, data()).await,
            Err(CartError::Unavailable)
        ));
        assert_eq!(carts.session(&raw).await.unwrap().unwrap().revision, 1);
        sqlx::query("UPDATE guest_carts SET expires_at=now()-interval '1 second' WHERE id=$1")
            .bind(a.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(carts.details(&a).await.unwrap().is_none());
        assert!(matches!(
            carts.save_details(&a, &a.csrf, 1, data()).await,
            Err(CartError::Forbidden)
        ));
        carts.prune().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM checkout_details")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    })
    .await;
}
