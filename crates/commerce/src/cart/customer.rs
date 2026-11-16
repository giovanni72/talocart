//! A customer cart is a private capability, never recoverable from a guest cookie.
use super::*;
use talocart_platform::identity::{CustomerError, CustomerSession, Customers};

fn auth_error(error: CustomerError) -> CartError {
    match error {
        CustomerError::Identity(IdentityError::Forbidden | IdentityError::Unauthorized) => {
            CartError::Forbidden
        }
        CustomerError::Technical(e) => e.into(),
        CustomerError::Access(_) => CartError::Forbidden,
        CustomerError::Identity(e) => CartError::Identity(e),
        _ => CartError::Forbidden,
    }
}
impl Carts {
    /// Read-only product-page price for an addition. Existing variants of the same
    /// product count towards the final threshold, as they do on the actual add.
    pub async fn preview_product_unit(
        &self,
        cart: &Guest,
        selection: (i64, i64),
        quantity: i32,
        language: Language,
    ) -> Result<Option<Money>> {
        if !(1..=999999).contains(&quantity) {
            return Err(CartError::Quantity);
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        Self::authorize(&mut tx, cart).await?;
        let Some(group) = talocart_platform::identity::customer_groups::quantity_schedule_in(
            &mut tx,
            cart.customer_id(),
        )
        .await?
        else {
            tx.commit().await?;
            return Ok(None);
        };
        let item = Products::cart_merchandise(&mut tx, &[selection], language)
            .await
            .map_err(catalog_error)?
            .pop()
            .ok_or(CartError::Unavailable)?;
        // Price presentation remains available even when stock/minimum rules
        // prevent an addition. The mutating add validates the final variant line.
        let product_quantity: i64 = sqlx::query_scalar("SELECT coalesce(sum(quantity),0)::bigint FROM guest_cart_items WHERE cart_id=$1 AND product_id=$2")
            .bind(cart.id).bind(selection.0).fetch_one(&mut *tx).await?;
        let final_product = product_quantity
            .checked_add(i64::from(quantity))
            .ok_or(CartError::Quantity)?;
        let unit = Money::from_cents(item.unit_cents).map_err(|_| CartError::Invalid)?;
        let unit = if let Some(tier) = group.tier_for(final_product) {
            pricing::discounted_unit(
                unit,
                Rate::from_basis_points(tier.discount_basis_points)
                    .map_err(|_| CartError::Invalid)?,
            )
            .map_err(|_| CartError::Invalid)?
        } else {
            unit
        };
        tx.commit().await?;
        Ok(Some(unit))
    }
    /// Recheck both the live session and the immutable cart owner at every boundary.
    /// Shared account locks block revocation through transaction completion without
    /// upgrading the account lock already acquired by order confirmation.
    pub(crate) async fn authorize(tx: &mut Transaction<'_, Postgres>, cart: &Guest) -> Result<()> {
        let owner = if let Some(session) = &cart.customer {
            Some(
                Customers::authorize_in(tx, session, None)
                    .await
                    .map_err(auth_error)?
                    .id,
            )
        } else {
            None
        };
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 \
                    FROM guest_carts c \
                    WHERE id=$1 \
                    AND token_hash=$2 \
                    AND expires_at>now() \
                    AND c.customer_id IS NOT DISTINCT \
                    FROM $3)",
        )
        .bind(cart.id)
        .bind(&cart.token_hash)
        .bind(owner)
        .fetch_one(&mut **tx)
        .await?;
        if !valid {
            return Err(CartError::Forbidden);
        }
        Ok(())
    }
    pub async fn customer_cart(&self, session: &CustomerSession) -> Result<Option<Guest>> {
        let mut tx = self.pool.begin().await?;
        let account = Customers::authorize_in(&mut tx, session, None)
            .await
            .map_err(auth_error)?;
        let cart = Self::load_customer(&mut tx, session, account.id).await?;
        tx.commit().await?;
        Ok(cart)
    }
    async fn load_customer(
        tx: &mut Transaction<'_, Postgres>,
        session: &CustomerSession,
        id: i64,
    ) -> Result<Option<Guest>> {
        let mut cart: Option<Guest> = sqlx::query_as(
            "SELECT c.id,c.csrf,c.revision,c.token_hash,(SELECT coalesce(sum(quantity),0)::bigint \
                    FROM guest_cart_items \
                    WHERE cart_id=c.id) AS count \
                FROM guest_carts c \
                WHERE customer_id=$1",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(c) = &mut cart {
            c.customer = Some(session.clone());
        }
        Ok(cart)
    }
    async fn ensure_customer_in(
        tx: &mut Transaction<'_, Postgres>,
        session: &CustomerSession,
    ) -> Result<Guest> {
        // Serialize first creation and merges before acquiring either cart lock.
        let account = Customers::authorize_in(tx, session, Some(&session.csrf))
            .await
            .map_err(auth_error)?;
        if let Some(cart) = Self::load_customer(tx, session, account.id).await? {
            return Ok(cart);
        }
        let mut cart: Guest = sqlx::query_as(
            "INSERT INTO guest_carts(token_hash,csrf,customer_id,expires_at) \
            VALUES($1,$2,$3,'infinity') \
            RETURNING id,csrf,revision,token_hash,0::bigint AS count",
        )
        .bind(identity::digest(&identity::token()?))
        .bind(identity::token()?)
        .bind(account.id)
        .fetch_one(&mut **tx)
        .await?;
        cart.customer = Some(session.clone());
        Ok(cart)
    }
    /// Establishing an empty capability does not transfer items or alter an existing cart.
    pub async fn ensure_customer(&self, session: &CustomerSession) -> Result<Guest> {
        let mut tx = self.pool.begin().await?;
        let cart = Self::ensure_customer_in(&mut tx, session).await?;
        tx.commit().await?;
        Ok(cart)
    }
    /// Atomic, lossless transfer. Empty sources are successful replays; changed,
    /// nonempty sources require a fresh review. Invalid stock/packs remain visible
    /// in the recalculated quote instead of silently discarding or reducing items.
    pub async fn merge_customer(
        &self,
        session: &CustomerSession,
        csrf: &str,
        source: &Guest,
        source_csrf: &str,
        revision: i64,
    ) -> Result<()> {
        session.check_csrf(csrf).map_err(|_| CartError::Forbidden)?;
        source.check_csrf(source_csrf)?;
        if source.customer.is_some() {
            return Err(CartError::Forbidden);
        }
        let mut tx = self.pool.begin().await?;
        let target = Self::ensure_customer_in(&mut tx, session).await?;
        Self::authorize(&mut tx, source).await?;
        let locked: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT id,revision FROM guest_carts WHERE id IN ($1,$2) ORDER BY id FOR UPDATE",
        )
        .bind(source.id)
        .bind(target.id)
        .fetch_all(&mut *tx)
        .await?;
        let has_items: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM guest_cart_items WHERE cart_id=$1)")
                .bind(source.id)
                .fetch_one(&mut *tx)
                .await?;
        if !has_items {
            tx.commit().await?;
            return Ok(());
        }
        if !locked.contains(&(source.id, revision)) {
            return Err(CartError::Stale);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT item_key) FROM guest_cart_items WHERE cart_id IN ($1,$2)",
        )
        .bind(source.id)
        .bind(target.id)
        .fetch_one(&mut *tx)
        .await?;
        if count > ITEM_LIMIT as i64 {
            return Err(CartError::Limit);
        }
        let excessive: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 \
                FROM guest_cart_items \
                WHERE cart_id IN ($1,$2) \
                GROUP BY item_key \
                HAVING sum(quantity)>999999)",
        )
        .bind(source.id)
        .bind(target.id)
        .fetch_one(&mut *tx)
        .await?;
        if excessive {
            return Err(CartError::Quantity);
        }
        sqlx::query(
            "UPDATE guest_cart_items t \
            SET quantity=t.quantity+s.quantity \
            FROM guest_cart_items s \
            WHERE t.cart_id=$2 \
            AND s.cart_id=$1 \
            AND t.item_key=s.item_key",
        )
        .bind(source.id)
        .bind(target.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO guest_cart_items(cart_id,item_key,product_id,variant_id,had_variant,quantity,\
                    name,code,unit_cents,basis_points,price_mode,currency) \
                SELECT $2,item_key,product_id,variant_id,had_variant,quantity,name,code,unit_cents,\
                basis_points,price_mode,currency \
                FROM guest_cart_items s \
                WHERE cart_id=$1 \
                AND NOT EXISTS(SELECT 1 \
                    FROM guest_cart_items t \
                    WHERE t.cart_id=$2 \
                    AND t.item_key=s.item_key) \
                ORDER BY id",
        )
        .bind(source.id)
        .bind(target.id)
        .execute(&mut *tx)
        .await?;
        // Existing customer draft wins. Copy observations unchanged; checkout repeats
        // all validation, including countries and current shipping/payment methods.
        sqlx::query("INSERT INTO checkout_details \
            SELECT (jsonb_populate_record(NULL::checkout_details,to_jsonb(d)||jsonb_build_object('cart_id',\
                        $2::bigint))).* \
            FROM checkout_details d \
            WHERE cart_id=$1 \
            ON CONFLICT(cart_id) DO NOTHING")
            .bind(source.id).bind(target.id).execute(&mut *tx).await?;
        let source_coupon: Option<i64> =
            sqlx::query_scalar("SELECT coupon_id FROM guest_carts WHERE id=$1")
                .bind(source.id)
                .fetch_one(&mut *tx)
                .await?;
        if let Some(coupon) = source_coupon {
            sqlx::query("UPDATE guest_carts SET coupon_id=coalesce(coupon_id,$2) WHERE id=$1")
                .bind(target.id)
                .bind(coupon)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE guest_carts SET coupon_id=NULL WHERE id=$1")
                .bind(source.id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM guest_cart_items WHERE cart_id=$1")
            .bind(source.id)
            .execute(&mut *tx)
            .await?;
        Self::changed(&mut tx, source.id).await?;
        Self::changed(&mut tx, target.id).await?;
        tx.commit().await?;
        Ok(())
    }
}
