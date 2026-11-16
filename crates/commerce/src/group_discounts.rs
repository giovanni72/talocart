//! Group membership is owned by identity; commerce applies its schedule per product.
use crate::{
    cart::{CartError, Guest, Quote},
    pricing::{self, Money, PriceMode, Rate},
};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LineDiscount {
    pub position: usize,
    pub product_id: i64,
    pub product_quantity: i64,
    pub min_quantity: i64,
    pub discount_basis_points: i32,
    pub original_cents: i64,
    pub discount_cents: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub group_id: i64,
    pub name_it: String,
    pub name_en: String,
    pub original_cents: i64,
    pub discount_cents: i64,
    pub lines: Vec<LineDiscount>,
}
pub(crate) async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    guest: &Guest,
    quote: &mut Quote,
) -> crate::cart::Result<()> {
    apply_for(tx, guest.customer_id(), quote).await
}

pub(crate) async fn apply_for(
    tx: &mut Transaction<'_, Postgres>,
    customer: Option<i64>,
    quote: &mut Quote,
) -> crate::cart::Result<()> {
    let original = quote.totals;
    let Some(group) =
        talocart_platform::identity::customer_groups::quantity_schedule_in(tx, customer).await?
    else {
        return Ok(());
    };
    let mut quantities = BTreeMap::<i64, i64>::new();
    for line in &quote.lines {
        if let Some(item) = &line.current {
            let entry = quantities.entry(item.product_id).or_default();
            *entry = entry
                .checked_add(i64::from(line.quantity))
                .ok_or(CartError::Invalid)?;
        }
    }
    let mut discounts = Vec::new();
    let mut amounts = Vec::new();
    for (position, line) in quote.lines.iter_mut().enumerate() {
        let Some(item) = line.current.as_mut() else {
            continue;
        };
        let quantity = quantities
            .get(&item.product_id)
            .copied()
            .ok_or(CartError::Invalid)?;
        let before = line.totals.ok_or(CartError::Unavailable)?;
        if let Some(tier) = group.tier_for(quantity) {
            item.unit_cents = pricing::discounted_unit(
                Money::from_cents(item.unit_cents).map_err(|_| CartError::Invalid)?,
                Rate::from_basis_points(tier.discount_basis_points)
                    .map_err(|_| CartError::Invalid)?,
            )
            .map_err(|_| CartError::Invalid)?
            .cents();
            let after = pricing::line(
                Money::from_cents(item.unit_cents).map_err(|_| CartError::Invalid)?,
                u32::try_from(line.quantity).map_err(|_| CartError::Invalid)?,
                Rate::from_basis_points(item.basis_points).map_err(|_| CartError::Invalid)?,
                PriceMode::parse(&item.price_mode).map_err(|_| CartError::Invalid)?,
            )
            .map_err(|_| CartError::Invalid)?;
            discounts.push(LineDiscount {
                position,
                product_id: item.product_id,
                product_quantity: quantity,
                min_quantity: tier.min_quantity,
                discount_basis_points: tier.discount_basis_points,
                original_cents: before.gross().cents(),
                discount_cents: before.gross().cents() - after.gross().cents(),
            });
            line.totals = Some(after);
        }
        amounts.push(line.totals.ok_or(CartError::Unavailable)?);
    }
    // Invalid carts retain their issues and cannot be checked out. Price their
    // available lines independently, but never synthesize a partial cart total.
    let Some(original) = original else {
        return Ok(());
    };
    let total = pricing::total(&amounts).map_err(|_| CartError::Invalid)?;
    if !discounts.is_empty() {
        quote.group_discount = Some(Snapshot {
            group_id: group.id,
            name_it: group.name_it,
            name_en: group.name_en,
            original_cents: original.gross().cents(),
            discount_cents: original.gross().cents() - total.gross().cents(),
            lines: discounts,
        });
    }
    quote.totals = Some(total);
    Ok(())
}
