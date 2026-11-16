use super::*;
use std::sync::{Arc, Mutex};
use talocart_commerce::orders::OnlinePayments;
use talocart_platform::payment_gateway::{self as gateway, *};

#[derive(Default)]
struct Neutral {
    catalog: Option<Vec<Arc<Neutral>>>,
    name: Option<&'static str>,
    request: Mutex<Option<Create>>,
}
impl Gateway for Neutral {
    fn providers(&self) -> Vec<String> {
        self.catalog.as_ref().map_or_else(
            || vec![self.provider_id()],
            |items| items.iter().map(|p| p.provider_id()).collect(),
        )
    }
    fn select(&self, id: &str) -> Option<Arc<dyn Gateway>> {
        self.catalog
            .as_ref()?
            .iter()
            .find(|p| p.provider_id() == id)
            .map(|p| p.clone() as Arc<dyn Gateway>)
    }
    fn provider_id(&self) -> String {
        self.name.unwrap_or("neutral").into()
    }
    fn retry_window_seconds(&self) -> u32 {
        3600
    }
    fn decode_event(&self, body: &[u8]) -> gateway::Result<WakeEvent> {
        serde_json::from_slice(body).map_err(|_| Error::Invalid)
    }
    fn verify_webhook(&self, _: &[u8], signature: &str) -> gateway::Result<()> {
        if signature == "valid" {
            Ok(())
        } else {
            Err(Error::Invalid)
        }
    }
    fn webhook_path(&self) -> &'static str {
        if self.catalog.is_some() {
            return "/payments/{provider}/webhook";
        }
        if self.name == Some("other") {
            "/payments/other/webhook"
        } else {
            "/payments/neutral/webhook"
        }
    }
    fn signature_header(&self) -> &'static str {
        "neutral-signature"
    }
    fn checkout_url(&self, value: &str) -> bool {
        value == "https://neutral.example/pay"
    }
    fn identifier(&self, value: &str, _: Identifier) -> bool {
        valid_identifier(value)
    }
    fn create<'a>(&'a self, input: &'a Create) -> Call<'a, gateway::Session> {
        Box::pin(async move {
            *self.request.lock().unwrap() = Some(input.clone());
            self.session("order:123-ABC").await
        })
    }
    fn session<'a>(&'a self, id: &'a str) -> Call<'a, gateway::Session> {
        Box::pin(async move {
            let i = self
                .request
                .lock()
                .unwrap()
                .clone()
                .ok_or(Error::NotFound)?;
            Ok(gateway::Session {
                id: id.into(),
                ui_mode: i.ui_mode,
                client_secret: None,
                state: PaymentState::Paid,
                amount: i.amount,
                currency: i.currency.clone(),
                reference: i.reference,
                request_key: i.key,
                url: None,
                payment_id: Some("CAPTURE-987:XYZ".into()),
                capture: Some(Capture {
                    amount: i.amount,
                    currency: i.currency,
                    amount_refunded: 0,
                }),
            })
        })
    }
    fn refund<'a>(&'a self, payment: &'a str, amount: i64, _: &'a str) -> Call<'a, Refund> {
        Box::pin(async move {
            Ok(Refund {
                id: "REFUND-456:XYZ".into(),
                payment_id: payment.into(),
                amount,
                currency: "EUR".into(),
                state: RefundState::Succeeded,
            })
        })
    }
    fn refunds<'a>(&'a self, _: &'a str) -> Call<'a, RefundList> {
        Box::pin(async {
            Ok(RefundList {
                data: vec![],
                has_more: false,
            })
        })
    }
    fn refund_status<'a>(&'a self, _: &'a str) -> Call<'a, Refund> {
        Box::pin(async { Err(Error::NotFound) })
    }
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn webhook_object_kind_disambiguates_ids_and_rejects_conflicting_references() {
    isolated(|pool, _| async move {
        let (product, identity, _) = configured(&pool).await;
        let method: i64 = sqlx::query_scalar("UPDATE payment_methods SET active=true WHERE code='online_checkout' RETURNING id").fetch_one(&pool).await.unwrap();
        let carts = Carts::new(pool.clone()).with_online(true);
        let orders = Orders::new(pool.clone(), identity);
        let mut attempts = Vec::new();
        for (session, payment, key) in [("123", "456", "a"), ("456", "789", "b")] {
            let (_, guest, review) = prepare(&carts, (product, 0), 1, method).await;
            let order = confirm(&orders, &guest, &review, method).await.unwrap();
            let id: i64 = sqlx::query_scalar("INSERT INTO payment_attempts(provider_id,order_id,request_key,amount_cents,currency,title,return_url,locale,session_id,intent_id,state,action) VALUES('neutral',$1,$2,$3,$4,'Synthetic','http://localhost/','en',$5,$6,'open','none') RETURNING id").bind(order.id).bind(key.repeat(64)).bind(order.total_cents).bind(&order.currency).bind(session).bind(payment).fetch_one(&pool).await.unwrap();
            attempts.push(id);
        }
        let online = OnlinePayments::new(pool.clone(), Arc::new(Neutral::default()), true);
        let mut event = serde_json::json!({"id":"event:typed","kind":"test","request_key":"","object_id":"456","object_kind":"session","payment_id":"","payload_hash":"c".repeat(64)});
        online.event(&serde_json::to_vec(&event).unwrap()).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT attempt_id FROM online_payment_events WHERE id='event:typed'").fetch_one(&pool).await.unwrap(), attempts[1]);
        event["id"] = "event:conflict".into();
        event["request_key"] = "a".repeat(64).into();
        assert!(online.event(&serde_json::to_vec(&event).unwrap()).await.is_err());
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM online_payment_events WHERE id='event:conflict'").fetch_one(&pool).await.unwrap(), 0);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn multiple_providers_keep_checkout_captures_webhooks_and_refunds_separate() {
    isolated(|pool, _| async move {
        let (product, identity, session) = configured(&pool).await;
        sqlx::query("UPDATE payment_methods SET active=true,provider_id='neutral' WHERE id=3").execute(&pool).await.unwrap();
        let other: i64 = sqlx::query_scalar("INSERT INTO payment_methods(code,provider_id,name_it,name_en,active,initial_order_status_id) VALUES('online_checkout','other','Other','Other',true,1) RETURNING id").fetch_one(&pool).await.unwrap();
        let first = Arc::new(Neutral::default());
        let second = Arc::new(Neutral { name: Some("other"), ..Default::default() });
        let gateway = Arc::new(Neutral { catalog: Some(vec![first.clone(), second.clone()]), ..Default::default() });
        let online = OnlinePayments::new(pool.clone(), gateway, true);
        let providers = online.available_providers().await;
        assert_eq!(providers.len(), 2);
        let carts = Carts::new(pool.clone()).with_online_providers(Some(providers.clone()));
        let orders = Orders::new(pool.clone(), identity).with_online_providers(Some(providers));
        let mut saved = vec![];
        for (method, provider) in [(3, "neutral"), (other, "other")] {
            let (_, guest, review) = prepare(&carts, (product, 0), 1, method).await;
            let quote = carts.checkout_quote(&guest, lang(), "IT", Some(selection(method))).await.unwrap();
            assert!(quote.payments.iter().any(|m| m.id == 3));
            assert!(quote.payments.iter().any(|m| m.id == other));
            let order = confirm(&orders, &guest, &review, method).await.unwrap();
            assert_eq!(order.frozen.payment.provider_id.as_deref(), Some(provider));
            assert!(OnlinePayments::provider_busy(&pool,provider).await.unwrap());
            if provider == "neutral" { assert!(!OnlinePayments::provider_busy(&pool,"other").await.unwrap()); }
            assert!(OnlinePayments::require_providers(&pool, &[if provider == "neutral" { "other".into() } else { "neutral".into() }]).await.is_err());
            let attempt = online.start(&guest, &guest.csrf, &order.reference, order.revision, "http://127.0.0.1:3000").await.unwrap();
            assert!(online.process_attempt(attempt).await.unwrap());
            assert_eq!(orders.get(&session, order.id).await.unwrap().payment_state, "paid");
            let event = serde_json::json!({"id":"same-event","kind":"test","request_key":"","object_id":"order:123-ABC","object_kind":"session","payment_id":"CAPTURE-987:XYZ","payload_hash":"a".repeat(64)});
            let headers = std::collections::BTreeMap::from([("neutral-signature".into(),"valid".into())]);
            let path = format!("/payments/{provider}/webhook");
            online.webhook_with_headers(&path, &serde_json::to_vec(&event).unwrap(), &headers).await.unwrap();
            saved.push((order.id,attempt));
        }
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM online_captures").fetch_one(&pool).await.unwrap(),2);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM online_payment_events WHERE id='same-event'").fetch_one(&pool).await.unwrap(),2);
        assert!(online.webhook_headers("/payments/missing/webhook").is_none());
        let original = orders.get(&session, saved[0].0).await.unwrap();
        online.request_refund(&session, &session.csrf, original.id, original.revision).await.unwrap();
        assert!(online.process_attempt(saved[0].1).await.unwrap());
        assert_eq!(orders.get(&session, original.id).await.unwrap().payment_state,"refunded");
        assert_eq!(orders.get(&session, saved[1].0).await.unwrap().payment_state,"paid");
        assert!(sqlx::query("UPDATE payment_methods SET provider_id='other' WHERE id=3").execute(&pool).await.is_err());
        // Availability is provider-specific: hiding the first leaves the second usable.
        let available = std::collections::BTreeMap::from([("other".into(),None)]);
        let carts = Carts::new(pool.clone()).with_online_providers(Some(available));
        let (_,guest,_) = prepare(&carts,(product,0),1,other).await;
        let quote = carts.checkout_quote(&guest,lang(),"IT",Some(selection(other))).await.unwrap();
        assert!(!quote.payments.iter().any(|m|m.id==3));
        assert!(quote.payments.iter().any(|m|m.id==other));
        assert!(!OnlinePayments::provider_busy(&pool,"neutral").await.unwrap());
        let bounded = Carts::new(pool.clone()).with_online_providers(Some(std::collections::BTreeMap::from([
            ("neutral".into(),Some(AmountLimits(std::collections::BTreeMap::from([("EUR".into(),AmountRange { minimum: 5000, maximum: 10000 })])))),
            ("other".into(),None),
        ])));
        let quote = bounded.checkout_quote(&guest,lang(),"IT",Some(selection(other))).await.unwrap();
        assert!(!quote.payments.iter().any(|m|m.id==3),"First provider rejects this amount");
        assert!(quote.payments.iter().any(|m|m.id==other),"Second provider has independent limits");

        let carts = Carts::new(pool.clone()).with_online_providers(Some(online.available_providers().await));
        let (raw,guest,_) = prepare(&carts,(product,0),1,other).await;
        carts.save_checkout(&guest,&guest.csrf,guest.revision,data(),selection(other),lang()).await.unwrap();
        let guest = carts.session(&raw).await.unwrap().unwrap();
        let app = router(test_app_config(pool.clone(),"Multiple providers".into(),talocart_web::AppStateConfig { online: Some(online.clone()), ..Default::default() }).await);
        let cookie = format!("talocart_cart={raw}");
        for prefix in ["", "/it"] {
            let response = request(&app,"GET",&format!("{prefix}/checkout/methods"),&cookie,"",false).await;
            assert_eq!(response.status(),StatusCode::OK);
            let page = html(response).await;
            assert!(page.contains("value=\"3\""));
            assert!(page.contains(&format!("value=\"{other}\"")));
        }
        let quote = carts.checkout_quote(&guest,lang(),"IT",Some(selection(other))).await.unwrap();
        let body = format!("csrf={}&revision={}&action=confirm&shipping_method_id=1&payment_method_id={other}&review={}",guest.csrf,guest.revision,quote.review);
        assert_eq!(request(&app,"POST","/checkout/methods",&cookie,&body,false).await.status(),StatusCode::SEE_OTHER);
        let provider: String = sqlx::query_scalar("SELECT snapshot FROM orders ORDER BY id DESC LIMIT 1").fetch_one(&pool).await.unwrap();
        let frozen: talocart_commerce::orders::Frozen = toml::from_str(&provider).unwrap();
        assert_eq!(frozen.payment.provider_id.as_deref(),Some("other"));
        // Legacy snapshots resolve their original attempt before the current method owner.
        let mut legacy = frozen;
        legacy.payment.provider_id = None;
        let legacy_id: i64 = sqlx::query_scalar("INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents) VALUES('ORD-LEGACY-PROVIDER',987654,0,$1,$2,'EUR',100,0,0,100,0,100) RETURNING id").bind("a".repeat(64)).bind(toml::to_string(&legacy).unwrap()).fetch_one(&pool).await.unwrap();
        assert_eq!(online.for_order(legacy_id).await.unwrap().provider_id(),"neutral", "An unattempted legacy order belongs to the current owner of method 3");
        sqlx::query("INSERT INTO payment_attempts(provider_id,order_id,request_key,amount_cents,currency,title,return_url,locale,state,action) VALUES('other',$1,$2,100,'EUR','Legacy','http://127.0.0.1/','en','expired','none')").bind(legacy_id).bind("e".repeat(64)).execute(&pool).await.unwrap();
        assert_eq!(online.for_order(legacy_id).await.unwrap().provider_id(),"other");
        assert!(!OnlinePayments::provider_busy(&pool,"neutral").await.unwrap(),"A retired provider's expired attempt must not block the current method owner");
        assert!(OnlinePayments::provider_busy(&pool,"other").await.unwrap());


    }).await;
}
