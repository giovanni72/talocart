//! Typed, immutable order-notice snapshots. No order creation, status inference or SMTP here.
use crate::pricing::Money;
use askama::Template;
use serde::Deserialize;
use talocart_platform::mail::{Content, Frame, MailError, Result, Tone};

#[derive(Clone, Copy)]
pub enum Notice {
    OrderUpdated,
    OrderReceived,
    OrderAdmin,
    PaymentReceived,
    OrderCancelled,
    CancellationAdmin,
    Refunded,
    Shipped,
    Delivered,
    Tracking,
}
impl Notice {
    pub const ALL: [Self; 10] = [
        Self::OrderUpdated,
        Self::OrderReceived,
        Self::OrderAdmin,
        Self::PaymentReceived,
        Self::OrderCancelled,
        Self::CancellationAdmin,
        Self::Refunded,
        Self::Shipped,
        Self::Delivered,
        Self::Tracking,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::OrderUpdated => "order_updated",
            Self::OrderReceived => "order_received",
            Self::OrderAdmin => "order_admin",
            Self::PaymentReceived => "payment_received",
            Self::OrderCancelled => "order_cancelled",
            Self::CancellationAdmin => "cancellation_admin",
            Self::Refunded => "payment_refunded",
            Self::Shipped => "order_shipped",
            Self::Delivered => "order_delivered",
            Self::Tracking => "order_tracking",
        }
    }
    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|n| n.key() == key)
    }
}
pub struct Line {
    pub description: String,
    pub quantity: u32,
    pub total: Money,
}
pub struct OrderSnapshot {
    pub fulfillment: crate::orders::Fulfillment,
    pub coupon: Option<(String, Money, Money)>,
    pub reference: String,
    pub store_name: String,
    pub customer_name: String,
    pub currency: String,
    pub lines: Vec<Line>,
    pub merchandise: Money,
    pub shipping: Money,
    pub payment_fee: Money,
    /// Included tax, already calculated and rounded by commerce. Never recalculated by email.
    pub tax: Money,
    pub total: Money,
    pub paid: Money,
    pub payment_method: String,
    pub instructions: String,
}
#[derive(Deserialize)]
struct Words {
    updated: String,
    updated_intro: String,
    shipped: String,
    shipped_intro: String,
    delivered: String,
    delivered_intro: String,
    tracking_updated: String,
    tracking_intro: String,
    tracking_delivered_intro: String,
    carrier: String,
    tracking_number: String,
    track: String,
    received: String,
    admin: String,
    paid: String,
    cancelled: String,
    cancellation_admin: String,
    received_intro: String,
    admin_intro: String,
    paid_intro: String,
    confirmed: String,
    confirmed_intro: String,
    refunded: String,
    refunded_intro: String,
    refunded_shipped_intro: String,
    cancelled_intro: String,
    cancellation_admin_intro: String,
    order: String,
    customer: String,
    product: String,
    quantity: String,
    amount: String,
    merchandise: String,
    coupon: String,
    original: String,
    shipping: String,
    fee: String,
    tax: String,
    total: String,
    payment: String,
    instructions: String,
    recorded: String,
    test: String,
    footer: String,
    staff: String,
    greeting: String,
    summary: String,
    shipment: String,
}
struct Row {
    description: String,
    quantity: u32,
    amount: String,
}
/// Summary line; `kind` is one of "line", "discount", "total" or "tax".
struct Total {
    label: String,
    amount: String,
    kind: &'static str,
}
#[derive(Template)]
#[template(path = "order-mail.html")]
struct Html<'a> {
    frame: Frame<'a>,
    words: &'a Words,
    snapshot: &'a OrderSnapshot,
    title: &'a str,
    intro: &'a str,
    rows: &'a [Row],
    totals: &'a [Total],
    is_payment: bool,
    show_fulfillment: bool,
    show_instructions: bool,
    staff: bool,
    greet: bool,
    paid: String,
}
#[derive(Template)]
#[template(path = "order-mail.txt", escape = "none")]
struct Text<'a> {
    words: &'a Words,
    snapshot: &'a OrderSnapshot,
    title: &'a str,
    intro: &'a str,
    rows: &'a [Row],
    totals: &'a [Total],
    is_payment: bool,
    show_fulfillment: bool,
    show_instructions: bool,
    staff: bool,
    greet: bool,
    paid: String,
    synthetic: bool,
}
pub fn render(
    snapshot: &OrderSnapshot,
    notice: Notice,
    locale: &str,
    synthetic: bool,
) -> Result<Content> {
    render_message(snapshot, notice, locale, synthetic, false)
}
// Online payment uses the event's existing identity and delivery lifecycle,
// with one combined order confirmation and full-payment receipt.
fn render_message(
    snapshot: &OrderSnapshot,
    notice: Notice,
    locale: &str,
    synthetic: bool,
    confirmed: bool,
) -> Result<Content> {
    let source = match locale {
        "it" => include_str!("../templates/mail-it.toml"),
        "en" => include_str!("../templates/mail-en.toml"),
        _ => return Err(MailError::Invalid),
    };
    let w: Words = toml::from_str(source).map_err(|_| MailError::Configuration)?;
    if confirmed && (snapshot.paid.cents() == 0 || snapshot.paid != snapshot.total) {
        return Err(MailError::Invalid);
    }
    if snapshot.reference.is_empty()
        || snapshot.reference.len() > 80
        || snapshot.reference.chars().any(char::is_control)
        || snapshot.store_name.is_empty()
        || snapshot.store_name.chars().count() > 120
        || snapshot.store_name.chars().any(char::is_control)
        || snapshot.customer_name.chars().count() > 201
        || snapshot.currency.len() != 3
        || !snapshot.currency.bytes().all(|b| b.is_ascii_uppercase())
        || snapshot.lines.is_empty()
        || snapshot.lines.len() > 200
        || snapshot
            .lines
            .iter()
            .any(|r| r.quantity == 0 || r.description.is_empty() || r.description.len() > 3000)
        || snapshot.instructions.len() > 20000
        || snapshot.payment_method.chars().count() > 120
    {
        return Err(MailError::Invalid);
    }
    if matches!(
        notice,
        Notice::Shipped | Notice::Delivered | Notice::Tracking
    ) {
        let f = &snapshot.fulfillment;
        if !f.sent()
            || (matches!(notice, Notice::Delivered) && f.state != "delivered")
            || f.carrier.is_empty()
            || f.carrier.chars().count() > 80
            || f.tracking_number.chars().count() > 120
            || f.tracking_url.chars().count() > 2048
            || (!f.tracking_url.is_empty()
                && !url::Url::parse(&f.tracking_url).ok().is_some_and(|u| {
                    u.scheme() == "https"
                        && u.has_host()
                        && u.username().is_empty()
                        && u.password().is_none()
                }))
        {
            return Err(MailError::Invalid);
        }
    }
    let sum = snapshot
        .merchandise
        .cents()
        .checked_add(snapshot.shipping.cents())
        .and_then(|v| v.checked_add(snapshot.payment_fee.cents()));
    let items = snapshot
        .lines
        .iter()
        .try_fold(0i64, |v, r| v.checked_add(r.total.cents()));
    if sum != Some(snapshot.total.cents())
        || items != Some(snapshot.merchandise.cents())
        || snapshot.tax.cents() > snapshot.total.cents()
    {
        return Err(MailError::Invalid);
    }
    let (title, intro) = match notice {
        Notice::OrderUpdated => (&w.updated, &w.updated_intro),
        Notice::OrderReceived => (&w.received, &w.received_intro),
        Notice::OrderAdmin => (&w.admin, &w.admin_intro),
        Notice::PaymentReceived if confirmed => (&w.confirmed, &w.confirmed_intro),
        Notice::PaymentReceived => (&w.paid, &w.paid_intro),
        Notice::Refunded if snapshot.fulfillment.sent() => (&w.refunded, &w.refunded_shipped_intro),
        Notice::Refunded => (&w.refunded, &w.refunded_intro),
        Notice::Shipped => (&w.shipped, &w.shipped_intro),
        Notice::Delivered => (&w.delivered, &w.delivered_intro),
        Notice::Tracking if snapshot.fulfillment.state == "delivered" => {
            (&w.tracking_updated, &w.tracking_delivered_intro)
        }
        Notice::Tracking => (&w.tracking_updated, &w.tracking_intro),
        Notice::OrderCancelled => (&w.cancelled, &w.cancelled_intro),
        Notice::CancellationAdmin => (&w.cancellation_admin, &w.cancellation_admin_intro),
    };
    let money = |m: Money| {
        format!(
            "{}{}{:02} {}",
            m.cents() / 100,
            if locale == "it" { "," } else { "." },
            m.cents() % 100,
            snapshot.currency
        )
    };
    let rows: Vec<_> = snapshot
        .lines
        .iter()
        .map(|r| Row {
            description: r.description.clone(),
            quantity: r.quantity,
            amount: money(r.total),
        })
        .collect();
    let line = |label: &str, amount: Money, kind| Total {
        label: label.into(),
        amount: money(amount),
        kind,
    };
    let mut totals = Vec::with_capacity(7);
    if let Some((code, original, discount)) = &snapshot.coupon {
        totals.push(line(&w.original, *original, "line"));
        totals.push(Total {
            label: format!("{} · {}", w.coupon, code),
            amount: format!("−{}", money(*discount)),
            kind: "discount",
        });
    }
    totals.push(line(&w.merchandise, snapshot.merchandise, "line"));
    totals.push(line(&w.shipping, snapshot.shipping, "line"));
    if snapshot.payment_fee.cents() != 0 {
        totals.push(line(&w.fee, snapshot.payment_fee, "line"));
    }
    totals.push(line(&w.total, snapshot.total, "total"));
    totals.push(line(&w.tax, snapshot.tax, "tax"));
    let is_payment = matches!(notice, Notice::PaymentReceived);
    let show_fulfillment = matches!(
        notice,
        Notice::Shipped | Notice::Delivered | Notice::Tracking
    );
    let show_instructions = matches!(notice, Notice::OrderReceived | Notice::OrderAdmin)
        || (matches!(notice, Notice::OrderUpdated) && snapshot.paid.cents() == 0);
    let staff = matches!(notice, Notice::OrderAdmin | Notice::CancellationAdmin);
    let greet = !staff && !snapshot.customer_name.trim().is_empty();
    let tone = match notice {
        Notice::PaymentReceived | Notice::Shipped | Notice::Delivered => Tone::Positive,
        Notice::OrderCancelled | Notice::CancellationAdmin | Notice::Refunded => Tone::Alert,
        Notice::OrderReceived | Notice::OrderAdmin | Notice::Tracking | Notice::OrderUpdated => {
            Tone::Neutral
        }
    };
    let mut frame = Frame::new(locale, &snapshot.store_name, tone);
    frame.heading = format!("{title} · {}", snapshot.reference);
    frame.preheader = intro;
    frame.test = synthetic.then_some(w.test.as_str());
    frame.footer = &w.footer;
    let html = Html {
        frame,
        words: &w,
        snapshot,
        title,
        intro,
        rows: &rows,
        totals: &totals,
        is_payment,
        show_fulfillment,
        show_instructions,
        staff,
        greet,
        paid: money(snapshot.paid),
    }
    .render()
    .map_err(|_| MailError::Invalid)?;
    let text = Text {
        words: &w,
        snapshot,
        title,
        intro,
        rows: &rows,
        totals: &totals,
        is_payment,
        show_fulfillment,
        show_instructions,
        staff,
        greet,
        paid: money(snapshot.paid),
        synthetic,
    }
    .render()
    .map_err(|_| MailError::Invalid)?;
    Ok(Content {
        locale: locale.into(),
        template: notice.key().into(),
        subject: format!(
            "{}{} · {}",
            if synthetic {
                if locale == "it" {
                    "[PROVA] "
                } else {
                    "[TEST] "
                }
            } else {
                ""
            },
            title,
            snapshot.reference
        ),
        html,
        text,
    })
}
/// Commercial event selected by the future order service, never inferred from status labels.
#[derive(Clone, Copy)]
pub enum Event {
    AdminCreated,
    Updated,
    Shipped,
    Delivered,
    Tracking,
    Created,
    OnlineCreated,
    PaymentRecorded,
    OnlinePaymentRecorded,
    Refunded,
    Cancelled,
}
/// Enqueue every audience atomically with the caller's business transaction.
/// `event_key` identifies one immutable occurrence (e.g. payment transaction), not a status name.
/// Locales and all commercial values must come from the order/event snapshot.
pub async fn enqueue_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_key: &str,
    event: Event,
    snapshot: &OrderSnapshot,
    customer: &str,
    locales: (&str, &str),
) -> Result<Vec<i64>> {
    enqueue_snapshots(
        tx,
        event_key,
        event,
        (snapshot, snapshot),
        customer,
        locales,
    )
    .await
}
/// One immutable event, with labels frozen separately for the customer/shop locale.
pub async fn enqueue_snapshots(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_key: &str,
    event: Event,
    snapshots: (&OrderSnapshot, &OrderSnapshot),
    customer: &str,
    locales: (&str, &str),
) -> Result<Vec<i64>> {
    let (snapshot, shop_snapshot) = snapshots;
    let Some(settings) = talocart_platform::mail::Mail::notification_configuration(tx).await?
    else {
        return Ok(Vec::new());
    };
    let (customer_notice, admin_notice) = match event {
        Event::AdminCreated => (None, Some(Notice::OrderAdmin)),
        Event::Updated => (Some(Notice::OrderUpdated), None),
        Event::Created => (Some(Notice::OrderReceived), Some(Notice::OrderAdmin)),
        Event::OnlineCreated => (None, Some(Notice::OrderAdmin)),
        Event::PaymentRecorded | Event::OnlinePaymentRecorded => {
            (Some(Notice::PaymentReceived), None)
        }
        Event::Refunded => (Some(Notice::Refunded), None),
        Event::Shipped => (Some(Notice::Shipped), None),
        Event::Delivered => (Some(Notice::Delivered), None),
        Event::Tracking => (Some(Notice::Tracking), None),
        Event::Cancelled => (
            Some(Notice::OrderCancelled),
            Some(Notice::CancellationAdmin),
        ),
    };
    let mut ids = Vec::new();
    if let Some(customer_notice) = customer_notice {
        ids.push(
            talocart_platform::mail::Mail::enqueue(
                tx,
                &format!("{event_key}:{}", customer_notice.key()),
                customer,
                &render_message(
                    snapshot,
                    customer_notice,
                    locales.0,
                    false,
                    matches!(event, Event::OnlinePaymentRecorded),
                )?,
            )
            .await?,
        );
    }
    if let Some(notice) = admin_notice {
        let content = render(shop_snapshot, notice, locales.1, false)?;
        let audience_key = format!("{event_key}:{}", notice.key());
        let recipients = talocart_platform::mail::Mail::event_audience(
            tx,
            &audience_key,
            &settings.admin_recipients,
        )
        .await?;
        for recipient in recipients {
            ids.push(
                talocart_platform::mail::Mail::enqueue(tx, &audience_key, &recipient, &content)
                    .await?,
            );
        }
    }
    Ok(ids)
}
/// Preview/test fixture only. Never reads a customer's cart or an existing order.
/// Synthetic data tailored to the selected preview; never reads a customer order.
pub fn sample_for(notice: Notice) -> Result<OrderSnapshot> {
    let mut snapshot = sample()?;
    if matches!(
        notice,
        Notice::Shipped | Notice::Delivered | Notice::Tracking
    ) {
        snapshot.fulfillment = crate::orders::Fulfillment {
            state: if matches!(notice, Notice::Delivered) {
                "delivered"
            } else {
                "shipped"
            }
            .into(),
            carrier: "Example courier / Corriere di esempio".into(),
            tracking_number: "DEMO-TRACK-1".into(),
            tracking_url: "https://example.test/tracking/DEMO-TRACK-1".into(),
            ..Default::default()
        };
    }
    Ok(snapshot)
}
pub fn sample() -> Result<OrderSnapshot> {
    let amount = |c| Money::from_cents(c).map_err(|_| MailError::Invalid);
    Ok(OrderSnapshot {
        fulfillment: crate::orders::Fulfillment::pending(),
        coupon: None,
        reference: "DEMO-0001".into(),
        store_name: "Talocart".into(),
        customer_name: "Alex Example".into(),
        currency: "EUR".into(),
        lines: vec![Line {
            description: "Example product / Prodotto di esempio".into(),
            quantity: 2,
            total: amount(4000)?,
        }],
        merchandise: amount(4000)?,
        shipping: amount(500)?,
        payment_fee: amount(100)?,
        tax: amount(830)?,
        total: amount(4600)?,
        paid: amount(4600)?,
        payment_method: "Demo".into(),
        instructions: "Example payment instructions / Istruzioni di pagamento di esempio".into(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn online_order_confirmation_requires_full_payment_and_is_bilingual() {
        for locale in ["en", "it"] {
            let mut snapshot = sample().unwrap();
            snapshot.customer_name = "<script>unsafe</script>".into();
            let confirmed =
                render_message(&snapshot, Notice::PaymentReceived, locale, false, true).unwrap();
            let title = if locale == "it" {
                "Ordine confermato"
            } else {
                "Order confirmed"
            };
            assert!(confirmed.subject.contains(title));
            assert!(confirmed.html.contains(title));
            assert!(confirmed.text.contains(title));
            assert!(!confirmed.html.contains("<script>"));
            assert!(!confirmed.text.contains(&snapshot.instructions));
            assert_eq!(confirmed.template, "payment_received");
            let offline = render(&snapshot, Notice::PaymentReceived, locale, false).unwrap();
            assert!(!offline.subject.contains(title));
            snapshot.paid = Money::from_cents(0).unwrap();
            assert!(
                render_message(&snapshot, Notice::PaymentReceived, locale, false, true).is_err()
            );
        }
    }
    #[test]
    fn all_templates_bilingual_plain_html_escape_and_event_meaning() {
        for locale in ["it", "en"] {
            for notice in Notice::ALL {
                let mut s = sample().unwrap();
                if matches!(
                    notice,
                    Notice::Shipped | Notice::Delivered | Notice::Tracking
                ) {
                    s.fulfillment = crate::orders::Fulfillment {
                        state: if matches!(notice, Notice::Delivered) {
                            "delivered"
                        } else {
                            "shipped"
                        }
                        .into(),
                        carrier: "Carrier".into(),
                        tracking_number: "T1".into(),
                        tracking_url: "https://example.test/T1".into(),
                        ..Default::default()
                    };
                }
                s.customer_name = "<script>bad()</script>".into();
                s.instructions = "<b>bank</b>\nSecond line".into();
                let m = render(&s, notice, locale, false).unwrap();
                assert!(m.html.contains("&#60;script&#62;"));
                assert!(!m.html.contains("<script>"));
                assert!(m.text.contains("<script>bad()</script>"));
                assert!(m.html.contains(&format!("lang=\"{locale}\"")));
                assert!(m.text.contains(if locale == "it" {
                    "46,00 EUR"
                } else {
                    "46.00 EUR"
                }));
                if matches!(notice, Notice::OrderReceived) {
                    assert!(m.text.contains(if locale == "it" {
                        "non conferma il pagamento"
                    } else {
                        "does not confirm payment"
                    }));
                }
                if matches!(notice, Notice::PaymentReceived) {
                    assert!(!m.text.contains("<b>bank</b>"));
                }
                assert!(!m.subject.contains("[TEST]"));
                assert!(
                    render(&s, notice, locale, true)
                        .unwrap()
                        .subject
                        .contains(if locale == "it" { "[PROVA]" } else { "[TEST]" })
                );
            }
        }
    }
    #[test]
    fn full_order_unicode_labels_and_payment_instructions_fit_outbox_limits() {
        let mut s = sample().unwrap();
        s.customer_name = "é".repeat(201);
        s.payment_method = "é".repeat(120);
        s.instructions = "&".repeat(20000);
        s.lines = (0..200)
            .map(|_| Line {
                description: "&".repeat(3000),
                quantity: 1,
                total: Money::from_cents(20).unwrap(),
            })
            .collect();
        for locale in ["it", "en"] {
            let content = render(&s, Notice::OrderReceived, locale, false).unwrap();
            assert!(content.text.len() <= talocart_platform::mail::MAX_TEXT_BYTES);
            assert!(content.html.len() <= talocart_platform::mail::MAX_HTML_BYTES);
        }
    }
    #[test]
    fn shared_frame_audience_tone_test_banner_and_zero_fee() {
        for locale in ["it", "en"] {
            let (staff, test, fee) = if locale == "it" {
                (
                    "Notifica per il negozio",
                    "PROVA — dati",
                    "Commissione pagamento",
                )
            } else {
                ("Shop notification", "TEST — synthetic", "Payment fee")
            };
            let mut s = sample().unwrap();
            let admin = render(&s, Notice::OrderAdmin, locale, false).unwrap();
            assert!(admin.html.contains(staff) && admin.text.contains(staff));
            assert!(!admin.text.contains("Alex Example,"));
            let customer = render(&s, Notice::OrderReceived, locale, false).unwrap();
            assert!(!customer.html.contains(staff) && !customer.text.contains(staff));
            assert!(
                customer.html.contains("Alex Example,") && customer.text.contains("Alex Example,")
            );
            assert!(customer.html.contains("#243e32") && !customer.html.contains("#882f25"));
            assert!(!customer.html.contains(test) && customer.html.contains(fee));
            let cancelled = render(&s, Notice::OrderCancelled, locale, true).unwrap();
            assert!(cancelled.html.contains("#882f25"));
            assert!(cancelled.html.contains(test) && cancelled.text.contains(test));
            s.payment_fee = Money::from_cents(0).unwrap();
            s.total = Money::from_cents(4500).unwrap();
            s.paid = s.total;
            let free = render(&s, Notice::PaymentReceived, locale, false).unwrap();
            assert!(!free.html.contains(fee) && !free.text.contains(fee));
            assert!(free.html.contains("#345a2c"));
        }
    }
    #[test]
    fn invalid_snapshots_and_locale_fail_closed() {
        let mut s = sample().unwrap();
        assert!(render(&s, Notice::OrderReceived, "fr", false).is_err());
        s.reference = "demo\r\nBcc: attack".into();
        assert!(render(&s, Notice::OrderReceived, "en", false).is_err());
        s = sample().unwrap();
        s.total = Money::from_cents(1).unwrap();
        assert!(render(&s, Notice::OrderReceived, "en", false).is_err());
        s = sample().unwrap();
        s.lines[0].quantity = 0;
        assert!(render(&s, Notice::OrderReceived, "it", false).is_err());
    }
}
