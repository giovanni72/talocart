//! Authoritative checkout confirmation and its single transaction.
use super::*;

pub(super) fn charge(
    s: charges::ChargeQuote,
    code: String,
    name_it: String,
    name_en: String,
    instructions_it: String,
    instructions_en: String,
) -> ChargeSnapshot {
    let (class_id, tax_name, basis_points, zone_name) =
        s.tax.map_or((None, String::new(), 0, None), |t| {
            (Some(t.class_id), t.class_name, t.basis_points, t.zone_name)
        });
    ChargeSnapshot {
        provider_id: None,
        code,
        name_it,
        name_en,
        instructions_it,
        instructions_en,
        entered_cents: s.entered.cents(),
        net_cents: s.totals.net().cents(),
        tax_cents: s.totals.tax().cents(),
        gross_cents: s.totals.gross().cents(),
        class_id,
        tax_name,
        basis_points,
        zone_name,
        price_mode: s.price_mode.key().into(),
        destination: s.destination,
        beneficiary: String::new(),
        iban: String::new(),
        bic: String::new(),
        delivery_days: None,
    }
}
impl Orders {
    /// The source cart revision is the idempotency identity. A replay returns its
    /// committed order even after this cart has received new items or configuration changed.
    pub async fn confirm(&self, input: Confirmation<'_>) -> Result<Order> {
        self.confirm_as(input, None).await
    }
    /// Attribution is decided once, in the same transaction as stock and notifications.
    pub async fn confirm_for_customer(
        &self,
        input: Confirmation<'_>,
        customer: &talocart_platform::identity::CustomerSession,
    ) -> Result<Order> {
        self.confirm_as(input, Some(customer)).await
    }
    async fn confirm_as(
        &self,
        input: Confirmation<'_>,
        customer: Option<&talocart_platform::identity::CustomerSession>,
    ) -> Result<Order> {
        crate::transaction_retry::run(
            "confirmation",
            || self.confirm_once(&input, customer),
            OrderError::technical,
        )
        .await
    }
    async fn confirm_once(
        &self,
        input: &Confirmation<'_>,
        customer: Option<&talocart_platform::identity::CustomerSession>,
    ) -> Result<Order> {
        let mut timing = timing::ConfirmationTiming::new();
        input.guest.check_csrf(input.csrf)?;
        if input.guest.customer_id().is_some()
            && input.guest.customer_id() != customer.and_then(|s| s.customer_id())
        {
            return Err(CartError::Forbidden.into());
        }
        let mut tx = self.pool.begin().await?;
        timing.acquired();
        // Existing variant write triggers require this exclusive catalog guard. Take
        // it before cart/row locks, rather than upgrading a shared guard mid-checkout.
        crate::taxes::product_write_lock(&mut tx).await?;
        timing.locked();
        let customer = if let Some(s) = customer {
            Some(
                talocart_platform::identity::Customers::authorize_in(&mut tx, s, None)
                    .await
                    .map_err(customer_error)?
                    .id,
            )
        } else {
            None
        };
        Carts::authorize(&mut tx, input.guest).await?;
        let existing:Option<i64>=sqlx::query_scalar("SELECT id FROM orders WHERE source_cart_id=$1 AND source_revision=$2 AND owner_hash=$3").bind(input.guest.id).bind(input.revision).bind(&input.guest.token_hash).fetch_optional(&mut *tx).await?;
        if let Some(id) = existing {
            if let Some(customer) = customer {
                let owner: Option<i64> =
                    sqlx::query_scalar("SELECT customer_id FROM customer_orders WHERE order_id=$1")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await?;
                if owner != Some(customer) {
                    return Err(OrderError::NotFound);
                }
            }
            let order = load(&mut tx, id).await?;
            tx.commit().await?;
            timing.outcome = "replayed";
            return Ok(order);
        }
        Carts::require_checkout_account(&mut tx, input.guest).await?;
        let carts = Carts::new(self.pool.clone());
        carts
            .lock(&mut tx, input.guest, input.csrf, input.revision)
            .await?;
        let policy = talocart_platform::customer_fields::load_in(&mut tx).await?;
        let data = input
            .data
            .clone()
            .validate_with(&policy)
            .map_err(|_| CartError::Invalid)?;
        if input.selection.shipping_method_id.is_none()
            || input.selection.payment_method_id.is_none()
        {
            return Err(CartError::Methods.into());
        }
        // Lock all stock targets in stable order before the authoritative quote.
        sqlx::query(
            "SELECT p.id \
            FROM catalog_products p \
            WHERE p.id IN (SELECT product_id \
                FROM guest_cart_items \
                WHERE cart_id=$1) \
            ORDER BY p.id FOR UPDATE",
        )
        .bind(input.guest.id)
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(
            "SELECT v.id \
            FROM catalog_variants v \
            WHERE v.id IN (SELECT variant_id \
                FROM guest_cart_items \
                WHERE cart_id=$1) \
            ORDER BY v.id FOR UPDATE",
        )
        .bind(input.guest.id)
        .fetch_all(&mut *tx)
        .await?;
        let cart = Carts::quote_in(
            &mut tx,
            input.guest,
            input.language,
            Some(&data.shipping_country),
        )
        .await?;
        let mut providers = self.online_providers.clone();
        if !input.online_available
            && let Some(providers) = &mut providers
        {
            providers.clear();
        }
        let quote = checkout_methods::methods(
            &mut tx,
            cart,
            input.language,
            &data.shipping_country,
            input.selection,
            checkout_methods::OnlineAvailability {
                enabled: input.online_available,
                limits: self.online_limits.as_ref(),
                providers: providers.as_ref(),
            },
        )
        .await?;
        let totals = quote.totals.ok_or(CartError::Unavailable)?;
        if quote.invalid_selection || quote.cart.lines.is_empty() {
            return Err(CartError::Methods.into());
        }
        if input.review.is_empty() || quote.review != input.review {
            return Err(OrderError::Review);
        }
        Carts::store_details(&mut tx, input.guest, data.clone()).await?;
        let goods = quote.cart.totals.ok_or(CartError::Unavailable)?;
        let checkout_methods::SelectedMethods {
            shipping: s,
            shipping_quote: sq,
            payment: p,
            payment_quote: pq,
        } = quote.selected.ok_or(CartError::Methods)?;
        let initial = OrderStatuses::require(&mut tx, p.initial_order_status_id)
            .await
            .map_err(OrderError::from)?;
        if !initial.initial_allowed() {
            return Err(OrderError::Invalid);
        }
        let initial_preparing = initial.role == "preparing";
        let mut shipping = charge(
            sq.breakdown,
            match s.code {
                crate::shipping::MethodCode::NationalExpress => "national_express",
                crate::shipping::MethodCode::FreeShipping => "free_shipping",
            }
            .into(),
            s.name_it,
            s.name_en,
            s.instructions_it,
            s.instructions_en,
        );
        shipping.delivery_days = s.delivery_days;
        let mut payment = charge(
            pq.breakdown,
            p.code.key().into(),
            p.name_it,
            p.name_en,
            p.instructions_it,
            p.instructions_en,
        );
        payment.provider_id = p.provider_id;
        payment.beneficiary = p.beneficiary;
        payment.iban = p.iban;
        payment.bic = p.bic;
        let (store_name, shop_locale): (String, String) = sqlx::query_as(
            "SELECT s.name,f.primary_locale \
            FROM store_settings s \
            CROSS \
            JOIN storefront_settings f \
            WHERE s.id=1 \
            AND f.id=1",
        )
        .fetch_one(&mut *tx)
        .await?;
        let frozen = Frozen {
            commercial_revision: 0,
            group_discount: quote.cart.group_discount.clone(),
            coupon: quote.cart.coupon.clone(),
            schema_version: 1,
            customer: data,
            customer_locale: input.language.current.code().into(),
            shop_locale,
            store_name,
            shipping,
            payment,
            status_id: initial.id,
            status_it: initial.name_it,
            status_en: initial.name_en,
            status_color: initial.color,
        };
        let snapshot = toml::to_string(&frozen).map_err(|_| OrderError::Invalid)?;
        // Capture the numbering year and creation time in the same statement,
        // after the authoritative quote. The annual allocation rolls back with
        // stock and notifications; replay returned above never allocates again.
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO orders(reference,source_cart_id,source_revision,owner_hash,snapshot,currency,\
                    merchandise_cents,shipping_cents,fee_cents,net_cents,tax_cents,total_cents,created_at) \
                VALUES(next_order_reference(statement_timestamp()),$1,$2,$3,$4,$5,$6,$7,$8,$9,$10,\
                    $11,statement_timestamp()) \
                RETURNING id",
        )
        .bind(input.guest.id)
        .bind(input.revision)
        .bind(&input.guest.token_hash)
        .bind(&snapshot)
        .bind(&quote.cart.currency)
        .bind(goods.gross().cents())
        .bind(frozen.shipping.gross_cents)
        .bind(frozen.payment.gross_cents)
        .bind(totals.net().cents())
        .bind(totals.tax().cents())
        .bind(totals.gross().cents())
        .fetch_one(&mut *tx)
        .await?;
        list::insert_customer(&mut tx, id, &frozen.customer).await?;
        if initial_preparing {
            sqlx::query("INSERT INTO order_fulfillment(order_id) VALUES($1)")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE order_fulfillment SET state='preparing',revision=revision+1 WHERE order_id=$1").bind(id).execute(&mut *tx).await?;
        }
        if let Some(coupon) = &frozen.coupon {
            sqlx::query(
                "INSERT INTO coupon_uses(order_id,coupon_id,discount_cents) VALUES($1,$2,$3)",
            )
            .bind(id)
            .bind(coupon.id)
            .bind(coupon.discount_cents)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE guest_carts SET coupon_id=NULL WHERE id=$1")
                .bind(input.guest.id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(customer) = customer {
            sqlx::query("INSERT INTO customer_orders(order_id,customer_id) VALUES($1,$2)")
                .bind(id)
                .bind(customer)
                .execute(&mut *tx)
                .await?;
        }
        let product_ids: Vec<i64> = quote
            .cart
            .lines
            .iter()
            .filter_map(|l| l.current.as_ref().map(|i| i.product_id))
            .collect();
        let fiscal_rows: Vec<(i64, i64, String, i32, Option<String>)> = sqlx::query_as(
            "SELECT p.id,t.class_id,t.tax_name,t.basis_points,CASE \
                WHEN t.explicit_rule THEN t.zone_name END \
                FROM catalog_products p \
                JOIN resolve_tax($1) t ON t.class_id=p.tax_class_id \
                WHERE p.id=ANY($2::bigint[])",
        )
        .bind(&frozen.customer.shipping_country)
        .bind(product_ids)
        .fetch_all(&mut *tx)
        .await?;
        for line in quote.cart.lines {
            let item = line.current.ok_or(CartError::Unavailable)?;
            let amounts = line.totals.ok_or(CartError::Unavailable)?;
            let (_, class_id, tax_name, basis_points, zone_name) = fiscal_rows
                .iter()
                .find(|row| row.0 == item.product_id)
                .ok_or(CartError::Unavailable)?;
            let variant = if item.variant_id == 0 {
                None
            } else {
                Some(item.variant_id)
            };
            let line_id: i64 = sqlx::query_scalar(
                "INSERT INTO order_lines(order_id,product_id,variant_id,quantity,name,label,code,unit_cents,\
                        class_id,tax_name,basis_points,zone_name,price_mode,net_cents,tax_cents,gross_cents) \
                    VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) \
                    RETURNING id",
            )
            .bind(id)
            .bind(item.product_id)
            .bind(variant)
            .bind(line.quantity)
            .bind(&item.name)
            .bind(&item.label)
            .bind(&item.code)
            .bind(item.unit_cents)
            .bind(class_id)
            .bind(tax_name)
            .bind(basis_points)
            .bind(zone_name)
            .bind(&item.price_mode)
            .bind(amounts.net().cents())
            .bind(amounts.tax().cents())
            .bind(amounts.gross().cents())
            .fetch_one(&mut *tx)
            .await?;
            if item.manage_stock {
                let stock =
                    stock_change(&mut tx, item.product_id, variant, line.quantity, false).await?;
                movement(&mut tx, line_id, "debit", line.quantity, stock).await?;
            }
        }
        let order = load(&mut tx, id).await?;
        enqueue(
            &mut tx,
            &order,
            if order.is_online() {
                Event::OnlineCreated
            } else {
                Event::Created
            },
        )
        .await?;
        sqlx::query("DELETE FROM guest_cart_items WHERE cart_id=$1")
            .bind(input.guest.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM checkout_methods WHERE cart_id=$1")
            .bind(input.guest.id)
            .execute(&mut *tx)
            .await?;
        Carts::changed(&mut tx, input.guest.id).await?;
        tx.commit().await?;
        timing.outcome = "committed";
        Ok(order)
    }
}
