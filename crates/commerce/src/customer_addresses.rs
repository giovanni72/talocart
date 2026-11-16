//! Address book owns mutable delivery data; orders keep their original snapshots.
use crate::{
    cart::{CartError, Carts, Guest},
    checkout_details::Details,
    countries::{CheckoutCountries, CountryError},
};
use sqlx::PgPool;
use talocart_platform::identity::{CustomerError, CustomerSession, Customers};
type Result<T> = std::result::Result<T, CustomerError>;

fn checkout_cart_error(error: CartError) -> CustomerError {
    match error {
        CartError::Technical(error) => error.into(),
        CartError::Access(error) => error.into(),
        CartError::Identity(error) => error.into(),
        CartError::Forbidden => talocart_platform::identity::IdentityError::Forbidden.into(),
        CartError::Stale => CustomerError::Stale,
        CartError::Coupon
        | CartError::Invalid
        | CartError::Unavailable
        | CartError::Methods
        | CartError::Quantity
        | CartError::Stock
        | CartError::Limit => CustomerError::Fields(vec!["address"]),
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AddressInput {
    pub label: String,
    #[serde(default, deserialize_with = "crate::checkout_details::form_bool")]
    pub is_default: bool,
    pub first_name: String,
    pub last_name: String,
    pub company: String,
    pub phone: String,
    pub country: String,
    pub street: String,
    pub number: String,
    pub extra: String,
    pub postal: String,
    pub city: String,
    pub region: String,
}
impl AddressInput {
    fn validate_with(
        &self,
        policy: &talocart_platform::customer_fields::Policy,
        kind: &str,
    ) -> Result<Details> {
        let d = Details {
            email: "address@example.test".into(),
            first_name: "Address".into(),
            last_name: "Book".into(),
            recipient_first_name: self.first_name.clone(),
            recipient_last_name: self.last_name.clone(),
            customer_type: kind.into(),
            phone: self.phone.clone(),
            shipping_company: self.company.clone(),
            shipping_country: self.country.clone(),
            shipping_street: self.street.clone(),
            shipping_number: self.number.clone(),
            shipping_extra: self.extra.clone(),
            shipping_postal: self.postal.clone(),
            shipping_city: self.city.clone(),
            shipping_region: self.region.clone(),
            ..Default::default()
        };
        let mut errors = Vec::new();
        if self.label.trim().is_empty()
            || self.label.trim().chars().count() > 80
            || self.label.chars().any(char::is_control)
        {
            errors.push("label");
        }
        if let Err(fields) = d.clone().validate_with(policy) {
            errors.extend(fields.into_iter().filter(|f| {
                *f != "delivery_notes"
                    && (*f == "phone" || f.starts_with("shipping_") || f.starts_with("recipient_"))
            }));
        }
        for (key, value) in [
            ("recipient_first_name", &self.first_name),
            ("recipient_last_name", &self.last_name),
        ] {
            if policy.mode(key, kind) == talocart_platform::customer_fields::Mode::Required
                && value.trim().is_empty()
            {
                errors.push(if key == "recipient_first_name" {
                    "first_name"
                } else {
                    "last_name"
                });
            }
        }
        if !errors.is_empty() {
            return Err(CustomerError::Fields(errors));
        }
        let mut normalized = d.normalize_with(policy);
        normalized.first_name = if policy.mode("recipient_first_name", kind)
            == talocart_platform::customer_fields::Mode::Hidden
        {
            String::new()
        } else {
            self.first_name.trim().into()
        };
        normalized.last_name = if policy.mode("recipient_last_name", kind)
            == talocart_platform::customer_fields::Mode::Hidden
        {
            String::new()
        } else {
            self.last_name.trim().into()
        };
        Ok(normalized)
    }
}
#[derive(sqlx::FromRow)]
pub struct Address {
    pub id: i64,
    pub revision: i64,
    pub label: String,
    pub is_default: bool,
    pub first_name: String,
    pub last_name: String,
    pub company: String,
    pub phone: String,
    pub country: String,
    pub street: String,
    pub number: String,
    pub extra: String,
    pub postal: String,
    pub city: String,
    pub region: String,
}
impl Address {
    pub fn matches_checkout(&self, details: &Details) -> bool {
        self.phone == details.phone
            && self.first_name == details.recipient_first_name
            && self.last_name == details.recipient_last_name
            && self.company == details.shipping_company
            && self.country == details.shipping_country
            && self.street == details.shipping_street
            && self.number == details.shipping_number
            && self.extra == details.shipping_extra
            && self.postal == details.shipping_postal
            && self.city == details.shipping_city
            && self.region == details.shipping_region
    }
    /// Fill delivery/contact fields without changing independent billing data or notes.
    pub fn apply_to_checkout(&self, details: &mut Details) {
        details.phone = self.phone.clone();
        details.recipient_first_name = self.first_name.clone();
        details.recipient_last_name = self.last_name.clone();
        details.shipping_company = self.company.clone();
        details.shipping_country = self.country.clone();
        details.shipping_street = self.street.clone();
        details.shipping_number = self.number.clone();
        details.shipping_extra = self.extra.clone();
        details.shipping_postal = self.postal.clone();
        details.shipping_city = self.city.clone();
        details.shipping_region = self.region.clone();
    }
    pub fn input(&self) -> AddressInput {
        AddressInput {
            label: self.label.clone(),
            is_default: self.is_default,
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            company: self.company.clone(),
            phone: self.phone.clone(),
            country: self.country.clone(),
            street: self.street.clone(),
            number: self.number.clone(),
            extra: self.extra.clone(),
            postal: self.postal.clone(),
            city: self.city.clone(),
            region: self.region.clone(),
        }
    }
}
pub struct CustomerAddresses {
    pool: PgPool,
}
impl CustomerAddresses {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn list(&self, session: &CustomerSession) -> Result<Vec<Address>> {
        let mut tx = self.pool.begin().await?;
        let c = Customers::authorize_in(&mut tx, session, None).await?;
        let rows = sqlx::query_as(
            "SELECT * FROM customer_addresses WHERE customer_id=$1 ORDER BY is_default DESC,id LIMIT 20",
        )
        .bind(c.id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }
    pub async fn get(&self, session: &CustomerSession, id: i64) -> Result<Address> {
        let mut tx = self.pool.begin().await?;
        let c = Customers::authorize_in(&mut tx, session, None).await?;
        let row = sqlx::query_as("SELECT * FROM customer_addresses WHERE customer_id=$1 AND id=$2")
            .bind(c.id)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(CustomerError::NotFound)?;
        tx.commit().await?;
        Ok(row)
    }
    /// Lock the owner to enforce the 20-address limit under concurrent inserts.
    pub async fn save(
        &self,
        session: &CustomerSession,
        csrf: &str,
        target: Option<(i64, i64)>,
        input: &AddressInput,
    ) -> Result<i64> {
        session.check_csrf(csrf)?;
        let mut tx = self.pool.begin().await?;
        let c = Customers::authorize_in(&mut tx, session, Some(csrf)).await?;
        let id = Self::persist(&mut tx, c.id, &c.customer_type, target, input, false).await?;
        tx.commit().await?;
        Ok(id)
    }
    async fn persist(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        owner: i64,
        kind: &str,
        target: Option<(i64, i64)>,
        input: &AddressInput,
        administrative: bool,
    ) -> Result<i64> {
        let policy = talocart_platform::customer_fields::load_in(tx).await?;
        let policy = if administrative {
            policy.for_administration()
        } else {
            policy
        };
        let mut d = input.validate_with(&policy, kind)?;
        if let Some((id, _)) = target {
            let previous: Address = sqlx::query_as(
                "SELECT * FROM customer_addresses WHERE id=$1 AND customer_id=$2 FOR UPDATE",
            )
            .bind(id)
            .bind(owner)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(CustomerError::NotFound)?;
            for (key, old) in [
                ("recipient_first_name", &previous.first_name),
                ("recipient_last_name", &previous.last_name),
                ("phone", &previous.phone),
                ("shipping_company", &previous.company),
                ("shipping_street", &previous.street),
                ("shipping_number", &previous.number),
                ("shipping_extra", &previous.extra),
                ("shipping_postal", &previous.postal),
                ("shipping_city", &previous.city),
                ("shipping_region", &previous.region),
            ] {
                if policy.mode(key, kind) == talocart_platform::customer_fields::Mode::Hidden {
                    match key {
                        "recipient_first_name" => d.first_name = old.clone(),
                        "recipient_last_name" => d.last_name = old.clone(),
                        _ => {
                            if let Some(v) = d.value_mut(key) {
                                *v = old.clone();
                            }
                        }
                    }
                }
            }
        }
        if let Some((id, revision)) = target {
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT revision FROM customer_addresses WHERE id=$1 AND customer_id=$2 FOR UPDATE",
            )
            .bind(id)
            .bind(owner)
            .fetch_optional(&mut **tx)
            .await?;
            if current.ok_or(CustomerError::NotFound)? != revision {
                return Err(CustomerError::Stale);
            }
        } else {
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM customer_addresses WHERE customer_id=$1")
                    .bind(owner)
                    .fetch_one(&mut **tx)
                    .await?;
            if count >= 20 {
                return Err(CustomerError::AddressLimit);
            }
        }
        // The caller holds the owner lock. Switching defaults also invalidates
        // old address forms, and any later failure rolls the whole switch back.
        if input.is_default {
            sqlx::query(
                "UPDATE customer_addresses SET is_default=false,revision=revision+1 \
                 WHERE customer_id=$1 AND is_default AND ($2::bigint IS NULL OR id<>$2)",
            )
            .bind(owner)
            .bind(target.map(|(id, _)| id))
            .execute(&mut **tx)
            .await?;
        }
        let id: i64 = if let Some((id, _)) = target {
            id
        } else {
            sqlx::query_scalar(
                "INSERT INTO customer_addresses(customer_id,label,first_name,last_name,company,phone,\
                        country,street,number,extra,postal,city,region,is_default) \
                    VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) \
                    RETURNING id",
            )
            .bind(owner)
            .bind(input.label.trim())
            .bind(&d.first_name)
            .bind(&d.last_name)
            .bind(&d.shipping_company)
            .bind(&d.phone)
            .bind(&d.shipping_country)
            .bind(&d.shipping_street)
            .bind(&d.shipping_number)
            .bind(&d.shipping_extra)
            .bind(&d.shipping_postal)
            .bind(&d.shipping_city)
            .bind(&d.shipping_region)
            .bind(input.is_default)
            .fetch_one(&mut **tx)
            .await?
        };
        if target.is_some() {
            sqlx::query(
                "UPDATE customer_addresses \
                    SET label=$1,first_name=$2,last_name=$3,company=$4,phone=$5,country=$6,street=$7,\
                    number=$8,extra=$9,postal=$10,city=$11,region=$12,is_default=$15,revision=revision+1 \
                    WHERE id=$13 \
                    AND customer_id=$14",
            )
            .bind(input.label.trim())
            .bind(&d.first_name)
            .bind(&d.last_name)
            .bind(&d.shipping_company)
            .bind(&d.phone)
            .bind(&d.shipping_country)
            .bind(&d.shipping_street)
            .bind(&d.shipping_number)
            .bind(&d.shipping_extra)
            .bind(&d.shipping_postal)
            .bind(&d.shipping_city)
            .bind(&d.shipping_region)
            .bind(id)
            .bind(owner)
            .bind(input.is_default)
            .execute(&mut **tx)
            .await?;
        }
        Ok(id)
    }
    pub async fn admin_list(
        &self,
        identity: &talocart_platform::identity::Identity,
        s: &talocart_platform::identity::Session,
        owner: i64,
    ) -> Result<Vec<Address>> {
        identity
            .authorize(s, talocart_platform::identity::Permission::CustomersRead)
            .await?;
        Ok(sqlx::query_as(
            "SELECT * FROM customer_addresses WHERE customer_id=$1 ORDER BY is_default DESC,id LIMIT 20",
        )
        .bind(owner)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn admin_save(
        &self,
        s: &talocart_platform::identity::Session,
        csrf: &str,
        owner: i64,
        target: Option<(i64, i64)>,
        input: &AddressInput,
    ) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        talocart_platform::identity::Identity::authorize_transaction(
            &mut tx,
            s,
            csrf,
            talocart_platform::identity::Permission::CustomersWrite,
        )
        .await?;
        let kind: String =
            sqlx::query_scalar("SELECT customer_type FROM customers WHERE id=$1 FOR UPDATE")
                .bind(owner)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(CustomerError::NotFound)?;
        let id = Self::persist(&mut tx, owner, &kind, target, input, true).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn admin_delete(
        &self,
        s: &talocart_platform::identity::Session,
        csrf: &str,
        owner: i64,
        id: i64,
        revision: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        talocart_platform::identity::Identity::authorize_transaction(
            &mut tx,
            s,
            csrf,
            talocart_platform::identity::Permission::CustomersWrite,
        )
        .await?;
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT id FROM customers WHERE id=$1 FOR UPDATE")
                .bind(owner)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Err(CustomerError::NotFound);
        }
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM customer_addresses WHERE id=$1 AND customer_id=$2 FOR UPDATE",
        )
        .bind(id)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
        if current.ok_or(CustomerError::NotFound)? != revision {
            return Err(CustomerError::Stale);
        }
        sqlx::query("DELETE FROM customer_addresses WHERE id=$1 AND customer_id=$2")
            .bind(id)
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn delete(
        &self,
        session: &CustomerSession,
        csrf: &str,
        id: i64,
        revision: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let c = Customers::authorize_in(&mut tx, session, Some(csrf)).await?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM customer_addresses WHERE customer_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(c.id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if current.ok_or(CustomerError::NotFound)? != revision {
            return Err(CustomerError::Stale);
        }
        sqlx::query("DELETE FROM customer_addresses WHERE id=$1 AND customer_id=$2")
            .bind(id)
            .bind(c.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Explicit POST copies delivery fields into a draft, retaining independent fiscal data.
    /// No stock/order mutation. Country eligibility is checked again on final confirmation.
    pub async fn use_in_checkout(
        &self,
        session: &CustomerSession,
        csrf: &str,
        id: i64,
        cart: (&Guest, &str, i64),
    ) -> Result<()> {
        let (guest, cart_csrf, revision) = cart;
        if guest.customer_id().is_some() && guest.customer_id() != session.customer_id() {
            return Err(talocart_platform::identity::IdentityError::Forbidden.into());
        }
        let mut tx = self.pool.begin().await?;
        let c = Customers::authorize_in(&mut tx, session, Some(csrf)).await?;
        let a: Address = sqlx::query_as(
            "SELECT * FROM customer_addresses WHERE id=$1 AND customer_id=$2 FOR SHARE",
        )
        .bind(id)
        .bind(c.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CustomerError::NotFound)?;
        CheckoutCountries::require_destination(&mut tx, &a.country)
            .await
            .map_err(|error| match error {
                CountryError::Technical(error) => error.into(),
                CountryError::Access(error) => error.into(),
                CountryError::Identity(error) => CustomerError::from(error),
                CountryError::Stale => CustomerError::Stale,
                CountryError::Invalid | CountryError::NotEnabled => {
                    CustomerError::Fields(vec!["shipping_country"])
                }
            })?;
        let carts = Carts::new(self.pool.clone());
        carts
            .lock(&mut tx, guest, cart_csrf, revision)
            .await
            .map_err(checkout_cart_error)?;
        let draft: Option<Details> =
            sqlx::query_as("SELECT * FROM checkout_details WHERE cart_id=$1")
                .bind(guest.id)
                .fetch_optional(&mut *tx)
                .await?;
        let mut d = match draft {
            Some(d) => d,
            None => crate::customer_billing::initial_in(&mut tx, &c).await?,
        };
        if d.email.is_empty() {
            d.customer_type = c.customer_type;
            d.email = c.email;
        }
        if d.first_name.is_empty() {
            d.first_name = c.first_name;
        }
        if d.last_name.is_empty() {
            d.last_name = c.last_name;
        }
        a.apply_to_checkout(&mut d);
        Carts::store_address_details(&mut tx, guest, d)
            .await
            .map_err(checkout_cart_error)?;
        Carts::changed(&mut tx, guest.id)
            .await
            .map_err(checkout_cart_error)?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn address_uses_checkout_validation_without_destination_allowlist() {
        let mut a = AddressInput {
            label: "Casa".into(),
            first_name: "Alex".into(),
            last_name: "Example".into(),
            phone: "+39 3331234567".into(),
            country: "IT".into(),
            street: "Via Example".into(),
            number: "1".into(),
            postal: "00100".into(),
            city: "Roma".into(),
            region: "RM".into(),
            ..Default::default()
        };
        assert!(
            a.validate_with(
                &talocart_platform::customer_fields::Policy::default(),
                "private"
            )
            .is_ok()
        );
        a.postal = "bad".into();
        assert!(
            a.validate_with(
                &talocart_platform::customer_fields::Policy::default(),
                "private"
            )
            .is_err()
        );
        a.postal = "00100".into();
        a.label = " ".into();
        assert!(
            a.validate_with(
                &talocart_platform::customer_fields::Policy::default(),
                "private"
            )
            .is_err()
        );
    }
}
