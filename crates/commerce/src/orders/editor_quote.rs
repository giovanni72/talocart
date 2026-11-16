//! One authoritative quotation for preview and publication; no persisted commercial effects.
use super::*;
use crate::{
    cart::{CartLine, Quote as CartQuote},
    products::CartMerchandise,
};
use talocart_platform::{content::Language, locale::Locale};

fn price_error(_: pricing::PriceError) -> OrderError {
    fields(&["price"])
}
pub(super) fn product_error(error: crate::products::ProductError) -> OrderError {
    match error {
        crate::products::ProductError::Technical(e) => OrderError::Technical(e),
        crate::products::ProductError::Access(e) => OrderError::Access(e),
        crate::products::ProductError::Identity(e) => e.into(),
        _ => CartError::Unavailable.into(),
    }
}
fn line_amount(item: &CartMerchandise, quantity: i32) -> Result<LineTotals> {
    pricing::line(
        Money::from_cents(item.unit_cents).map_err(price_error)?,
        u32::try_from(quantity).map_err(|_| CartError::Quantity)?,
        Rate::from_basis_points(item.basis_points).map_err(price_error)?,
        PriceMode::parse(&item.price_mode).map_err(price_error)?,
    )
    .map_err(price_error)
}
fn override_charge(mut value: charges::ChargeQuote, entered: &str) -> Result<charges::ChargeQuote> {
    if !entered.trim().is_empty() {
        value.entered = Money::parse(entered).map_err(price_error)?;
        if value.entered.cents() > 0 && value.tax.is_none() {
            return Err(fields(&["price"]));
        }
        value.totals = pricing::line(
            value.entered,
            1,
            Rate::from_basis_points(value.tax.as_ref().map_or(0, |t| t.basis_points))
                .map_err(price_error)?,
            value.price_mode,
        )
        .map_err(price_error)?;
    }
    Ok(value)
}
pub(super) async fn quote(
    tx: &mut Transaction<'_, Postgres>,
    session: &Session,
    draft: &Draft,
    old: Option<&Order>,
) -> Result<Preview> {
    let data = &draft.data;
    payload(data)?;
    let policy = talocart_platform::customer_fields::load_in(tx).await?;
    let customer = data
        .customer
        .clone()
        .validate_with(&policy)
        .map_err(|names| {
            OrderError::EditorFields(names.into_iter().map(str::to_owned).collect())
        })?;
    if data.lines.is_empty() {
        return Err(fields(&["lines"]));
    }
    if let Some(customer) = data.customer_id.filter(|_| old.is_none()) {
        CustomerAdmin::require_order_customer_in(tx, session, customer)
            .await
            .map_err(customer_error)?;
    }
    let locale = Locale::parse(&data.locale).ok_or(OrderError::Invalid)?;
    let language = Language::new(locale, locale);
    let pairs: Vec<_> = data
        .lines
        .iter()
        .map(|l| (l.product_id, l.variant_id))
        .collect();
    let unique: std::collections::BTreeSet<_> = pairs.iter().copied().collect();
    if pairs.len() != unique.len() {
        return Err(fields(&["lines"]));
    }
    let current =
        Products::cart_merchandise_for(tx, &pairs, language, Some(&customer.shipping_country))
            .await
            .map_err(product_error)?;
    let debited: Vec<i64> = if let Some(order) = old {
        sqlx::query_scalar("SELECT m.line_id FROM order_stock_moves m JOIN order_lines l ON l.id=m.line_id WHERE l.order_id=$1 AND l.active AND m.kind='debit'").bind(order.id).fetch_all(&mut **tx).await?
    } else {
        vec![]
    };
    let mut cart_lines = vec![];
    for (position, input) in data.lines.iter().enumerate() {
        let source = if let Some(id) = input.source_line_id {
            Some(
                old.and_then(|o| {
                    o.lines.iter().find(|l| {
                        l.id == id
                            && l.product_id == input.product_id
                            && l.variant_id.unwrap_or(0) == input.variant_id
                    })
                })
                .ok_or(OrderError::Invalid)?,
            )
        } else {
            None
        };
        let mut item = current
            .iter()
            .find(|i| i.product_id == input.product_id && i.variant_id == input.variant_id)
            .cloned()
            .ok_or(CartError::Unavailable)?;
        PurchaseQuantities::new(item.minimum_quantity, item.packs)
            .map_err(|_| CartError::Unavailable)?
            .validate(input.quantity)
            .map_err(|_| CartError::Quantity)?;
        let returned = old.map_or(0, |o| {
            o.lines
                .iter()
                .filter(|l| {
                    l.product_id == input.product_id
                        && l.variant_id.unwrap_or(0) == input.variant_id
                        && debited.contains(&l.id)
                })
                .map(|l| i64::from(l.quantity))
                .sum::<i64>()
        });
        if let Some(source) = source {
            item.manage_stock = debited.contains(&source.id);
            item.name = source.name.clone();
            item.label = source.label.clone();
            item.code = source.code.clone();
        }
        if item.manage_stock && i64::from(input.quantity) > i64::from(item.stock) + returned {
            return Err(CartError::Stock.into());
        }
        let amounts = line_amount(&item, input.quantity)?;
        cart_lines.push(CartLine {
            id: i64::try_from(position).map_err(|_| OrderError::Invalid)?,
            quantity: input.quantity,
            name: item.name.clone(),
            code: item.code.clone(),
            current: Some(item),
            totals: Some(amounts),
            issue: None,
            changed: false,
        });
    }
    let initial_amounts: Vec<_> = cart_lines.iter().filter_map(|l| l.totals).collect();
    let currency = current
        .first()
        .ok_or(CartError::Unavailable)?
        .currency
        .clone();
    if old.is_some_and(|o| o.currency != currency) || current.iter().any(|i| i.currency != currency)
    {
        return Err(fields(&["currency"]));
    }
    let mut cart = CartQuote {
        group_discount: None,
        coupon: None,
        coupon_code: String::new(),
        coupon_invalid: false,
        overflow: false,
        revision: draft.revision,
        lines: cart_lines,
        totals: Some(pricing::total(&initial_amounts).map_err(price_error)?),
        currency: currency.clone(),
    };
    // Automatic prices use the selected account's current quantity schedule.
    crate::group_discounts::apply_for(tx, data.customer_id, &mut cart).await?;
    for (input, line) in data.lines.iter().zip(&mut cart.lines) {
        if !input.unit_price.trim().is_empty() {
            let item = line.current.as_mut().ok_or(CartError::Unavailable)?;
            item.unit_cents = Money::parse(&input.unit_price)
                .map_err(price_error)?
                .cents();
            line.totals = Some(line_amount(item, input.quantity)?);
        }
    }
    let amounts: Vec<_> = cart.lines.iter().filter_map(|l| l.totals).collect();
    let goods_before = pricing::total(&amounts).map_err(price_error)?;
    cart.totals = Some(goods_before);
    if let Some(group) = &mut cart.group_discount {
        group
            .lines
            .retain(|l| data.lines[l.position].unit_price.trim().is_empty());
        group.discount_cents = group
            .lines
            .iter()
            .try_fold(0i64, |n, l| n.checked_add(l.discount_cents))
            .ok_or(OrderError::Invalid)?;
        group.original_cents = goods_before
            .gross()
            .cents()
            .checked_add(group.discount_cents)
            .ok_or(OrderError::Invalid)?;
        if group.lines.is_empty() {
            cart.group_discount = None;
        }
    }
    if let Some(previous) = old.and_then(|o| o.frozen.coupon.as_ref()) {
        if data.coupon_code.trim() != previous.code {
            return Err(fields(&["coupon_code"]));
        }
        crate::coupons::apply_snapshot(&mut cart, previous.clone())?;
    } else if !data.coupon_code.trim().is_empty() {
        let code = crate::coupons::normalize(&data.coupon_code).ok_or(CartError::Coupon)?;
        let id: Option<i64> = sqlx::query_scalar("SELECT id FROM coupons WHERE code=$1")
            .bind(code)
            .fetch_optional(&mut **tx)
            .await?;
        crate::coupons::apply_id(tx, id.ok_or(CartError::Coupon)?, &mut cart).await?;
        if cart.coupon_invalid {
            return Err(CartError::Coupon.into());
        }
    }
    if old.is_some()
        && old.and_then(|o| o.frozen.coupon.as_ref()).is_none()
        && cart.coupon.is_some()
    {
        return Err(fields(&["coupon_code"]));
    }
    let goods = cart.totals.ok_or(CartError::Unavailable)?;
    let (s, sq) = crate::shipping::Shipping::require_method(
        tx,
        data.shipping_method_id.ok_or(CartError::Methods)?,
        goods.gross(),
        &customer.shipping_country,
    )
    .await
    .map_err(|e| match e {
        crate::shipping::ShippingError::Technical(e) => OrderError::Technical(e),
        crate::shipping::ShippingError::Access(e) => OrderError::Access(e),
        crate::shipping::ShippingError::Identity(e) => e.into(),
        _ => CartError::Methods.into(),
    })?;
    let shipping_amount = override_charge(sq.breakdown, &data.shipping_price)?;
    let base = pricing::total(&[goods, shipping_amount.totals]).map_err(price_error)?;
    let (p, pq) = crate::payments::Payments::require_method(
        tx,
        data.payment_method_id.ok_or(CartError::Methods)?,
        base.gross(),
        &customer.shipping_country,
    )
    .await
    .map_err(|e| match e {
        crate::payments::PaymentError::Technical(e) => OrderError::Technical(e),
        crate::payments::PaymentError::Access(e) => OrderError::Access(e),
        crate::payments::PaymentError::Identity(e) => e.into(),
        _ => CartError::Methods.into(),
    })?;
    if !matches!(
        p.code,
        crate::payments::MethodCode::BankTransfer | crate::payments::MethodCode::CashOnDelivery
    ) {
        return Err(OrderError::EditBlocked);
    }
    let payment_amount = override_charge(pq.breakdown, &data.payment_price)?;
    let totals = pricing::total(&[goods, shipping_amount.totals, payment_amount.totals])
        .map_err(price_error)?;
    let initial = if let Some(order) = old {
        OrderStatuses::require(tx, order.frozen.status_id).await?
    } else {
        OrderStatuses::require(tx, p.initial_order_status_id).await?
    };
    if old.is_none() && !initial.initial_allowed() {
        return Err(OrderError::Invalid);
    }
    let mut shipping = confirmation::charge(
        shipping_amount,
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
    let mut payment = confirmation::charge(
        payment_amount,
        p.code.key().into(),
        p.name_it,
        p.name_en,
        p.instructions_it,
        p.instructions_en,
    );
    payment.beneficiary = p.beneficiary;
    payment.iban = p.iban;
    payment.bic = p.bic;
    let (store_name,shop_locale):(String,String)=sqlx::query_as("SELECT s.name,f.primary_locale FROM store_settings s CROSS JOIN storefront_settings f WHERE s.id=1 AND f.id=1").fetch_one(&mut **tx).await?;
    let frozen = Frozen {
        commercial_revision: old.map_or(0, |o| o.revision + 1),
        group_discount: cart.group_discount,
        coupon: cart.coupon,
        schema_version: 1,
        customer,
        customer_locale: data.locale.clone(),
        shop_locale,
        store_name,
        shipping,
        payment,
        status_id: initial.id,
        status_it: initial.name_it,
        status_en: initial.name_en,
        status_color: initial.color,
    };
    let product_ids: Vec<i64> = data.lines.iter().map(|l| l.product_id).collect();
    let fiscal_rows: Vec<(i64,i64,String,i32,Option<String>)> = sqlx::query_as(
        "SELECT p.id,t.class_id,t.tax_name,t.basis_points,CASE WHEN t.explicit_rule THEN t.zone_name END FROM catalog_products p JOIN resolve_tax($1) t ON t.class_id=p.tax_class_id WHERE p.id=ANY($2::bigint[])"
    ).bind(&frozen.customer.shipping_country).bind(product_ids).fetch_all(&mut **tx).await?;
    let mut lines = vec![];
    for (position, line) in cart.lines.into_iter().enumerate() {
        let item = line.current.ok_or(CartError::Unavailable)?;
        let input = &data.lines[position];
        let (_, class_id, tax_name, _, zone_name) = fiscal_rows
            .iter()
            .find(|r| r.0 == item.product_id)
            .ok_or(CartError::Unavailable)?;
        let amounts = line.totals.ok_or(CartError::Unavailable)?;
        lines.push(PreviewLine {
            product_id: item.product_id,
            variant_id: (item.variant_id != 0).then_some(item.variant_id),
            quantity: input.quantity,
            name: item.name,
            label: item.label,
            code: item.code,
            unit_cents: item.unit_cents,
            catalog_unit_cents: current
                .iter()
                .find(|i| i.product_id == input.product_id && i.variant_id == input.variant_id)
                .ok_or(CartError::Unavailable)?
                .unit_cents,
            class_id: *class_id,
            tax_name: tax_name.clone(),
            basis_points: item.basis_points,
            zone_name: zone_name.clone(),
            price_mode: item.price_mode,
            net_cents: amounts.net().cents(),
            tax_cents: amounts.tax().cents(),
            gross_cents: amounts.gross().cents(),
            manage_stock: item.manage_stock,
        });
    }
    let fingerprint = serde_json::to_string(&(
        draft.id,
        draft.revision,
        draft.target_order_id,
        draft.target_revision,
        data,
        &frozen,
        &lines,
        totals.gross().cents(),
    ))
    .map_err(|_| OrderError::Invalid)?;
    Ok(Preview {
        frozen,
        currency,
        lines,
        merchandise_cents: goods.gross().cents(),
        totals,
        review: talocart_platform::identity::digest(&fingerprint),
    })
}
