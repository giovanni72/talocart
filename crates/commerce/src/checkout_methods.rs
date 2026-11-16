//! Checkout quotation and draft choices. Every amount is recalculated, never persisted as authority.
use crate::{
    cart::{CartError, Carts, Guest, Quote as CartQuote, Result},
    checkout_details::Details,
    payments::{PaymentError, Payments},
    pricing::{self, LineTotals, Money},
    shipping::{Shipping, ShippingError},
    taxes::charges,
};
use sqlx::{Postgres, Transaction};
use talocart_platform::content::Language;
#[derive(Clone, Copy, Default, sqlx::FromRow)]
pub struct Selection {
    pub shipping_method_id: Option<i64>,
    pub payment_method_id: Option<i64>,
}
pub struct OptionQuote {
    pub online: bool,
    pub id: i64,
    pub name: String,
    pub instructions: String,
    pub amount: Money,
    pub selected: bool,
    pub delivery_days: Option<i32>,
    pub bank: Option<(String, String, String)>,
}
/// Method metadata and amounts from this transaction's authoritative quotation.
pub(crate) struct SelectedMethods {
    pub shipping: crate::shipping::Method,
    pub shipping_quote: crate::shipping::Quote,
    pub payment: crate::payments::Method,
    pub payment_quote: crate::payments::Quote,
}
pub(crate) struct OnlineAvailability<'a> {
    pub(crate) providers: Option<&'a crate::payments::OnlineProviders>,
    pub(crate) enabled: bool,
    pub(crate) limits: Option<&'a talocart_platform::payment_gateway::AmountLimits>,
}

pub struct Quote {
    pub(crate) selected: Option<SelectedMethods>,
    pub cart: CartQuote,
    pub shipping: Vec<OptionQuote>,
    pub payments: Vec<OptionQuote>,
    pub shipping_cost: Option<Money>,
    pub payment_fee: Option<Money>,
    pub totals: Option<LineTotals>,
    pub invalid_selection: bool,
    pub destination_valid: bool,
    pub review: String,
}
fn shipping_error(e: ShippingError) -> CartError {
    match e {
        ShippingError::Technical(e) => e.into(),
        ShippingError::Access(e) => e.into(),
        ShippingError::Identity(e) => e.into(),
        _ => CartError::Invalid,
    }
}
fn payment_error(e: PaymentError) -> CartError {
    match e {
        PaymentError::Technical(e) => e.into(),
        PaymentError::Access(e) => e.into(),
        PaymentError::Identity(e) => e.into(),
        _ => CartError::Invalid,
    }
}
fn charge_error(e: charges::ChargeError) -> CartError {
    match e {
        charges::ChargeError::Technical(e) => CartError::Technical(e),
        charges::ChargeError::Access(e) => CartError::Access(e),
        charges::ChargeError::Identity(e) => e.into(),
        charges::ChargeError::Unavailable => CartError::Unavailable,
        charges::ChargeError::Price(_) => CartError::Invalid,
    }
}
impl Carts {
    /// Call after cart authorization and before any checkout write. Account ownership
    /// comes from the live customer capability, never a submitted customer ID/email.
    pub(crate) async fn require_checkout_account(
        tx: &mut Transaction<'_, Postgres>,
        guest: &Guest,
    ) -> Result<()> {
        if guest.customer_id().is_none()
            && !talocart_platform::identity::customer_registration::RegistrationSettings::guests_allowed_in(tx).await?
        {
            return Err(CartError::Forbidden);
        }
        Ok(())
    }

    /// Preview does not mutate the cart or retain submitted personal data. One snapshot
    /// covers merchandise, destination tax, shipping and payment thresholds. A snapshot
    /// taken while a writer held the guard may need a fresh transaction; at most
    /// three attempts handle serialization conflicts/deadlocks without any side effects.
    pub async fn checkout_quote(
        &self,
        guest: &Guest,
        language: Language,
        destination: &str,
        selection: Option<Selection>,
    ) -> Result<Quote> {
        self.quote_with_defaults(guest, language, destination, selection, false)
            .await
    }
    /// Read saved addresses, choices and amounts from one snapshot. Compatible
    /// choices survive; otherwise propose the first shipping and sole payment.
    /// Strict order confirmation never applies defaults.
    pub async fn checkout_step(
        &self,
        guest: &Guest,
        language: Language,
        selection: Option<Selection>,
    ) -> Result<Option<(Details, Quote)>> {
        crate::transaction_retry::run(
            "checkout_step",
            || self.checkout_step_once(guest, language, selection),
            CartError::technical,
        )
        .await
    }
    async fn checkout_step_once(
        &self,
        guest: &Guest,
        language: Language,
        selection: Option<Selection>,
    ) -> Result<Option<(Details, Quote)>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        charges::lock(&mut tx).await.map_err(charge_error)?;
        match Self::authorize(&mut tx, guest).await {
            Ok(()) => {}
            Err(CartError::Forbidden) => return Ok(None),
            Err(e) => return Err(e),
        }
        Self::require_checkout_account(&mut tx, guest).await?;
        let data: Option<Details> = sqlx::query_as(
            "SELECT d.* \
            FROM checkout_details d \
            JOIN guest_carts c ON c.id=d.cart_id \
            WHERE c.id=$1 \
            AND c.token_hash=$2 \
            AND c.expires_at>now()",
        )
        .bind(guest.id)
        .bind(&guest.token_hash)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(data) = data else {
            return Ok(None);
        };
        let policy = talocart_platform::customer_fields::load_in(&mut tx).await?;
        let data = match data.validate_with(&policy) {
            Ok(d) => d,
            Err(_) => return Ok(None),
        };
        let cart = Self::quote_in(&mut tx, guest, language, Some(&data.shipping_country)).await?;
        let chosen = match selection {
            Some(s) => s,
            None => sqlx::query_as("SELECT shipping_method_id,payment_method_id FROM checkout_methods WHERE cart_id=$1")
                .bind(guest.id).fetch_optional(&mut *tx).await?.unwrap_or_default(),
        };
        let quote = methods_with_defaults(
            &mut tx,
            cart,
            language,
            &data.shipping_country,
            chosen,
            OnlineAvailability {
                enabled: self.online_available,
                limits: self.online_limits.as_ref(),
                providers: self.online_providers.as_ref(),
            },
            true,
        )
        .await?;
        tx.commit().await?;
        Ok(Some((data, quote)))
    }
    async fn quote_with_defaults(
        &self,
        guest: &Guest,
        language: Language,
        destination: &str,
        selection: Option<Selection>,
        defaults: bool,
    ) -> Result<Quote> {
        crate::transaction_retry::run(
            "checkout_quote",
            || self.checkout_quote_once(guest, language, destination, selection, defaults),
            CartError::technical,
        )
        .await
    }
    async fn checkout_quote_once(
        &self,
        guest: &Guest,
        language: Language,
        destination: &str,
        selection: Option<Selection>,
        defaults: bool,
    ) -> Result<Quote> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        charges::lock(&mut tx).await.map_err(charge_error)?;
        let cart = Self::quote_in(
            &mut tx,
            guest,
            language,
            if talocart_platform::geography::country_valid(destination) {
                Some(destination)
            } else {
                None
            },
        )
        .await?;
        Self::require_checkout_account(&mut tx, guest).await?;
        let chosen=match selection {Some(s)=>s,None=>sqlx::query_as("SELECT shipping_method_id,payment_method_id FROM checkout_methods WHERE cart_id=$1").bind(guest.id).fetch_optional(&mut *tx).await?.unwrap_or_default()};
        let result = methods_with_defaults(
            &mut tx,
            cart,
            language,
            destination,
            chosen,
            OnlineAvailability {
                enabled: self.online_available,
                limits: self.online_limits.as_ref(),
                providers: self.online_providers.as_ref(),
            },
            defaults,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }
    /// Persist only method identifiers, using the saved address under the cart lock.
    /// No order, stock move or email is created. The returned quote includes the new
    /// cart revision and its review fingerprint from the same transaction.
    pub async fn update_checkout_choices(
        &self,
        guest: &Guest,
        csrf: &str,
        revision: i64,
        selection: Selection,
        language: Language,
    ) -> Result<(Details, Quote)> {
        let mut tx = self.pool.begin().await?;
        charges::lock(&mut tx).await.map_err(charge_error)?;
        self.lock(&mut tx, guest, csrf, revision).await?;
        Self::require_checkout_account(&mut tx, guest).await?;
        let data: Details = sqlx::query_as("SELECT * FROM checkout_details WHERE cart_id=$1")
            .bind(guest.id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(CartError::Invalid)?;
        let policy = talocart_platform::customer_fields::load_in(&mut tx).await?;
        let data = data
            .validate_with(&policy)
            .map_err(|_| CartError::Invalid)?;
        crate::countries::CheckoutCountries::require_destination(&mut tx, &data.shipping_country)
            .await
            .map_err(|e| match e {
                crate::countries::CountryError::Technical(e) => e.into(),
                crate::countries::CountryError::Access(e) => e.into(),
                crate::countries::CountryError::Identity(e) => CartError::Identity(e),
                _ => CartError::Unavailable,
            })?;
        Self::changed(&mut tx, guest.id).await?;
        let cart = Self::quote_in(&mut tx, guest, language, Some(&data.shipping_country)).await?;
        let quote = methods_with_defaults(
            &mut tx,
            cart,
            language,
            &data.shipping_country,
            selection,
            OnlineAvailability {
                enabled: self.online_available,
                limits: self.online_limits.as_ref(),
                providers: self.online_providers.as_ref(),
            },
            true,
        )
        .await?;
        let shipping = quote.shipping.iter().find(|m| m.selected).map(|m| m.id);
        let payment = quote.payments.iter().find(|m| m.selected).map(|m| m.id);
        sqlx::query("INSERT INTO checkout_methods(cart_id,shipping_method_id,payment_method_id) \
            VALUES($1,$2,$3) \
            ON CONFLICT(cart_id) DO UPDATE \
            SET shipping_method_id=EXCLUDED.shipping_method_id,payment_method_id=EXCLUDED.payment_method_id")
            .bind(guest.id).bind(shipping).bind(payment).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok((data, quote))
    }
    pub async fn save_checkout(
        &self,
        guest: &Guest,
        csrf: &str,
        revision: i64,
        data: Details,
        selection: Selection,
        language: Language,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        // Fiscal guard precedes all method/country locks, matching configuration writers.
        charges::lock(&mut tx).await.map_err(charge_error)?;
        self.lock(&mut tx, guest, csrf, revision).await?;
        Self::require_checkout_account(&mut tx, guest).await?;
        let policy = talocart_platform::customer_fields::load_in(&mut tx).await?;
        let data = data
            .validate_with(&policy)
            .map_err(|_| CartError::Invalid)?;
        let destination = data.shipping_country.clone();
        Self::store_details(&mut tx, guest, data).await?;
        let cart = Self::quote_in(&mut tx, guest, language, Some(&destination)).await?;
        let quote = methods(
            &mut tx,
            cart,
            language,
            &destination,
            selection,
            OnlineAvailability {
                enabled: self.online_available,
                limits: self.online_limits.as_ref(),
                providers: self.online_providers.as_ref(),
            },
        )
        .await?;
        if quote.invalid_selection {
            return Err(CartError::Methods);
        }
        sqlx::query("INSERT INTO checkout_methods(cart_id,shipping_method_id,payment_method_id) \
            VALUES($1,$2,$3) \
            ON CONFLICT(cart_id) DO UPDATE \
            SET shipping_method_id=EXCLUDED.shipping_method_id,payment_method_id=EXCLUDED.payment_method_id")
            .bind(guest.id).bind(selection.shipping_method_id).bind(selection.payment_method_id).execute(&mut *tx).await?;
        Self::changed(&mut tx, guest.id).await?;
        tx.commit().await?;
        Ok(())
    }
}
pub(crate) async fn methods(
    tx: &mut Transaction<'_, Postgres>,
    cart: CartQuote,
    language: Language,
    destination: &str,
    selection: Selection,
    online: OnlineAvailability<'_>,
) -> Result<Quote> {
    methods_with_defaults(tx, cart, language, destination, selection, online, false).await
}
async fn methods_with_defaults(
    tx: &mut Transaction<'_, Postgres>,
    cart: CartQuote,
    language: Language,
    destination: &str,
    mut selection: Selection,
    online: OnlineAvailability<'_>,
    defaults: bool,
) -> Result<Quote> {
    let destination_valid: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM checkout_countries WHERE country_code=$1)")
            .bind(destination)
            .fetch_one(&mut **tx)
            .await?;
    let mut shipping_candidates = vec![];
    let mut payment_candidates = vec![];
    let mut shipping = vec![];
    let mut payments = vec![];
    let mut shipping_cost = None;
    let mut payment_fee = None;
    let mut breakdown = vec![];
    let mut base = None;
    let mut selected_shipping = None;
    let mut selected_payment = None;
    if destination_valid
        && !cart.lines.is_empty()
        && let Some(goods) = cart.totals
    {
        breakdown.push(goods);
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM shipping_methods ORDER BY sort_order,id LIMIT 2")
                .fetch_all(&mut **tx)
                .await?;
        for id in ids {
            match Shipping::require_method(tx, id, goods.gross(), destination).await {
                Ok((m, q)) => {
                    shipping_candidates.push((
                        OptionQuote {
                            online: false,
                            id,
                            name: if language.current.code() == "it" {
                                m.name_it.clone()
                            } else {
                                m.name_en.clone()
                            },
                            instructions: if language.current.code() == "it" {
                                m.instructions_it.clone()
                            } else {
                                m.instructions_en.clone()
                            },
                            amount: q.cost,
                            selected: false,
                            delivery_days: m.delivery_days,
                            bank: None,
                        },
                        m,
                        q,
                    ));
                }
                Err(ShippingError::Unavailable | ShippingError::NotFound) => {}
                Err(e) => return Err(shipping_error(e)),
            }
        }
    }
    let shipping_invalid = selection.shipping_method_id.is_some()
        && !shipping_candidates
            .iter()
            .any(|(m, _, _)| Some(m.id) == selection.shipping_method_id);
    if defaults
        && !shipping_candidates
            .iter()
            .any(|(m, _, _)| Some(m.id) == selection.shipping_method_id)
    {
        selection.shipping_method_id = shipping_candidates.first().map(|(m, _, _)| m.id);
    }
    for (mut method, metadata, quote) in shipping_candidates {
        method.selected = Some(method.id) == selection.shipping_method_id;
        if method.selected {
            shipping_cost = Some(method.amount);
            base = Some(quote.total);
            breakdown.push(quote.breakdown.totals);
            selected_shipping = Some((metadata, quote));
        }
        shipping.push(method);
    }
    if let Some(base) = base {
        let provider_ids = online
            .providers
            .map(|p| p.keys().cloned().collect::<Vec<_>>());
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM payment_methods WHERE active AND (code<>'online_checkout' OR $1::text[] IS NULL OR provider_id=ANY($1)) ORDER BY sort_order,id LIMIT 34")
                .bind(provider_ids)
                .fetch_all(&mut **tx)
                .await?;
        for id in ids {
            match Payments::require_method(tx, id, base, destination).await {
                Ok((m, q)) => {
                    let provider = online.providers.and_then(|providers| {
                        m.provider_id.as_ref().and_then(|id| providers.get(id))
                    });
                    let enabled = if online.providers.is_some() {
                        provider.is_some()
                    } else {
                        online.enabled
                    };
                    let limits = if online.providers.is_some() {
                        provider.and_then(Option::as_ref)
                    } else {
                        online.limits
                    };
                    if m.code == crate::payments::MethodCode::OnlineCheckout
                        && (!enabled
                            || q.total.cents() == 0
                            || limits.is_some_and(|limits| {
                                !limits.allows(&cart.currency, q.total.cents())
                            }))
                    {
                        continue;
                    }
                    let bank = if m.code == crate::payments::MethodCode::BankTransfer {
                        Some((m.beneficiary.clone(), m.iban.clone(), m.bic.clone()))
                    } else {
                        None
                    };
                    payment_candidates.push((
                        OptionQuote {
                            online: m.code == crate::payments::MethodCode::OnlineCheckout,
                            id,
                            name: if language.current.code() == "it" {
                                m.name_it.clone()
                            } else {
                                m.name_en.clone()
                            },
                            instructions: if language.current.code() == "it" {
                                m.instructions_it.clone()
                            } else {
                                m.instructions_en.clone()
                            },
                            amount: q.fee,
                            selected: false,
                            delivery_days: None,
                            bank,
                        },
                        m,
                        q,
                    ));
                }
                Err(PaymentError::Unavailable | PaymentError::NotFound) => {}
                Err(e) => return Err(payment_error(e)),
            }
        }
    }
    let payment_invalid = selection.payment_method_id.is_some()
        && !payment_candidates
            .iter()
            .any(|(m, _, _)| Some(m.id) == selection.payment_method_id);
    if defaults
        && !payment_candidates
            .iter()
            .any(|(m, _, _)| Some(m.id) == selection.payment_method_id)
    {
        selection.payment_method_id = if payment_candidates.len() == 1 {
            payment_candidates.first().map(|(m, _, _)| m.id)
        } else {
            None
        };
    }
    for (mut method, metadata, quote) in payment_candidates {
        method.selected = Some(method.id) == selection.payment_method_id;
        if method.selected {
            payment_fee = Some(method.amount);
            breakdown.push(quote.breakdown.totals);
            selected_payment = Some((metadata, quote));
        }
        payments.push(method);
    }
    let invalid_selection = shipping_invalid || payment_invalid;
    let totals = if shipping_cost.is_some() && payment_fee.is_some() {
        Some(pricing::total(&breakdown).map_err(|_| CartError::Invalid)?)
    } else {
        None
    };
    let mut fingerprint = format!(
        "{}:{destination}:{:?}:{:?}",
        cart.revision, selection.shipping_method_id, selection.payment_method_id
    );
    if let Some(discount) = &cart.group_discount {
        fingerprint.push_str(&format!("group:{discount:?}"));
    }
    if let Some(coupon) = &cart.coupon {
        fingerprint.push_str(&format!("coupon:{coupon:?}"));
    }
    let revisions: (i64, i64, i64) = sqlx::query_as(
        "SELECT p.revision,s.revision,t.revision \
            FROM payment_configuration p \
            CROSS \
            JOIN shipping_configuration s \
            CROSS \
            JOIN tax_configuration t \
            WHERE p.id=1 \
            AND s.id=1 \
            AND t.id=1",
    )
    .fetch_one(&mut **tx)
    .await?;
    fingerprint.push_str(&format!("{revisions:?}"));
    for line in &cart.lines {
        if let Some(item) = &line.current {
            for text in [&item.name, &item.label, &item.code] {
                fingerprint.push_str(&format!("{}:{text}", text.len()));
            }
            fingerprint.push_str(&format!(
                "{}:{}:{}:{}:{}:{}:{}:{}",
                item.product_id,
                item.variant_id,
                line.quantity,
                item.unit_cents,
                item.basis_points,
                item.price_mode,
                item.currency,
                line.id
            ));
        }
    }
    for method in shipping
        .iter()
        .chain(payments.iter())
        .filter(|m| m.selected)
    {
        for text in [&method.name, &method.instructions] {
            fingerprint.push_str(&format!("{}:{text}", text.len()));
        }
        fingerprint.push_str(&format!(
            "{}:{}:{:?}:{:?}",
            method.id,
            method.amount.cents(),
            method.delivery_days,
            method.bank
        ));
    }
    let review = if totals.is_some() {
        talocart_platform::identity::digest(&fingerprint)
    } else {
        String::new()
    };
    let selected = selected_shipping.zip(selected_payment).map(
        |((shipping, shipping_quote), (payment, payment_quote))| SelectedMethods {
            shipping,
            shipping_quote,
            payment,
            payment_quote,
        },
    );
    Ok(Quote {
        selected,
        cart,
        shipping,
        payments,
        shipping_cost,
        payment_fee,
        totals,
        invalid_selection,
        destination_valid,
        review,
    })
}
