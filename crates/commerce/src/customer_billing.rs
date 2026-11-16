//! Permanent billing data is customer master data, never an order snapshot.
use crate::checkout_details::Details;
use sqlx::{PgPool, Postgres, Transaction};
use talocart_platform::{
    customer_fields,
    identity::{
        CustomerAccount, CustomerError, CustomerSession, Customers, Identity, Permission, Session,
    },
};
type Result<T> = std::result::Result<T, CustomerError>;
#[derive(Clone, Default, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
#[serde(default, deny_unknown_fields)]
pub struct BillingInput {
    pub invoice_requested: bool,
    pub billing_first_name: String,
    pub billing_last_name: String,
    pub billing_company: String,
    pub billing_country: String,
    pub billing_street: String,
    pub billing_number: String,
    pub billing_extra: String,
    pub billing_postal: String,
    pub billing_city: String,
    pub billing_region: String,
    pub vat_id: String,
    pub tax_id: String,
    pub recipient_code: String,
    pub pec: String,
}
#[derive(sqlx::FromRow)]
pub struct Billing {
    pub revision: i64,
    #[sqlx(flatten)]
    pub input: BillingInput,
}
impl BillingInput {
    pub fn validate(&self, kind: &str, policy: &customer_fields::Policy) -> Result<Self> {
        let mut d = Details {
            customer_type: kind.into(),
            email: "billing@example.test".into(),
            invoice_requested: true,
            billing_same: false,
            billing_first_name: self.billing_first_name.trim().to_owned(),
            billing_last_name: self.billing_last_name.trim().to_owned(),
            billing_company: self.billing_company.trim().to_owned(),
            billing_country: self.billing_country.trim().to_owned(),
            billing_street: self.billing_street.trim().to_owned(),
            billing_number: self.billing_number.trim().to_owned(),
            billing_extra: self.billing_extra.trim().to_owned(),
            billing_postal: self.billing_postal.trim().to_owned(),
            billing_city: self.billing_city.trim().to_owned(),
            billing_region: self.billing_region.trim().to_owned(),
            vat_id: self.vat_id.trim().to_owned(),
            tax_id: self.tax_id.trim().to_owned(),
            recipient_code: self.recipient_code.trim().to_owned(),
            pec: self.pec.trim().to_owned(),
            ..Details::default()
        };
        d.billing_country.make_ascii_uppercase();
        d.billing_region.make_ascii_uppercase();
        d.vat_id.make_ascii_uppercase();
        d.tax_id.make_ascii_uppercase();
        d.recipient_code.make_ascii_uppercase();
        if d.billing_country == "IT"
            && let Some(v) = d.vat_id.strip_prefix("IT")
        {
            d.vat_id = v.into();
        }
        let policy = policy.for_administration();
        let code = !d.recipient_code.is_empty()
            || (d.billing_country == "IT"
                && policy.mode("recipient_code", kind) == customer_fields::Mode::Required);
        let pec = !d.pec.is_empty()
            || (d.billing_country == "IT"
                && policy.mode("pec", kind) == customer_fields::Mode::Required);
        d.invoice_channel = match (code, pec) {
            (true, true) => "both",
            (true, false) => "code",
            (false, true) => "pec",
            _ => "none",
        }
        .into();
        if let Err(errors) = d.clone().check_with(&policy) {
            let errors: Vec<_> = errors
                .into_iter()
                .filter(|f| {
                    f.starts_with("billing_")
                        || [
                            "vat_id",
                            "tax_id",
                            "recipient_code",
                            "pec",
                            "invoice_channel",
                            "customer_type",
                        ]
                        .contains(f)
                })
                .collect();
            if !errors.is_empty() {
                return Err(CustomerError::Fields(errors));
            }
        }
        Ok(Self {
            invoice_requested: self.invoice_requested,
            billing_first_name: d.billing_first_name,
            billing_last_name: d.billing_last_name,
            billing_company: d.billing_company,
            billing_country: d.billing_country,
            billing_street: d.billing_street,
            billing_number: d.billing_number,
            billing_extra: d.billing_extra,
            billing_postal: d.billing_postal,
            billing_city: d.billing_city,
            billing_region: d.billing_region,
            vat_id: d.vat_id,
            tax_id: d.tax_id,
            recipient_code: d.recipient_code,
            pec: d.pec,
        })
    }
    /// Copy authorized billing master data into a separate checkout or order draft.
    pub fn apply(&self, d: &mut Details) {
        d.invoice_requested = self.invoice_requested || d.customer_type == "business";
        d.billing_same = false;
        d.billing_first_name = self.billing_first_name.clone();
        d.billing_last_name = self.billing_last_name.clone();
        d.billing_company = self.billing_company.clone();
        d.billing_country = self.billing_country.clone();
        d.billing_street = self.billing_street.clone();
        d.billing_number = self.billing_number.clone();
        d.billing_extra = self.billing_extra.clone();
        d.billing_postal = self.billing_postal.clone();
        d.billing_city = self.billing_city.clone();
        d.billing_region = self.billing_region.clone();
        d.vat_id = self.vat_id.clone();
        d.tax_id = self.tax_id.clone();
        d.recipient_code = self.recipient_code.clone();
        d.pec = self.pec.clone();
    }
}
pub struct CustomerBilling {
    pool: PgPool,
    identity: Identity,
}
impl CustomerBilling {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn get(&self, s: &Session, id: i64) -> Result<Option<Billing>> {
        self.identity
            .authorize(s, Permission::CustomersRead)
            .await?;
        Ok(
            sqlx::query_as("SELECT * FROM customer_billing WHERE customer_id=$1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    pub async fn save(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        input: &BillingInput,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::CustomersWrite).await?;
        let kind: String =
            sqlx::query_scalar("SELECT customer_type FROM customers WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(CustomerError::NotFound)?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM customer_billing WHERE customer_id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if current.unwrap_or(-1) != revision {
            return Err(CustomerError::Stale);
        }
        let policy = customer_fields::load_in(&mut tx).await?;
        let input = input.validate(&kind, &policy)?;
        sqlx::query("INSERT INTO customer_billing(customer_id,invoice_requested,billing_first_name,billing_last_name,\
                billing_company,billing_country,billing_street,billing_number,billing_extra,billing_postal,\
                billing_city,billing_region,vat_id,tax_id,recipient_code,pec) \
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) \
            ON CONFLICT(customer_id) DO UPDATE \
            SET invoice_requested=EXCLUDED.invoice_requested,billing_first_name=EXCLUDED.billing_first_name,\
            billing_last_name=EXCLUDED.billing_last_name,billing_company=EXCLUDED.billing_company,\
            billing_country=EXCLUDED.billing_country,billing_street=EXCLUDED.billing_street,billing_number=EXCLUDED.billing_number,\
            billing_extra=EXCLUDED.billing_extra,billing_postal=EXCLUDED.billing_postal,billing_city=EXCLUDED.billing_city,\
            billing_region=EXCLUDED.billing_region,vat_id=EXCLUDED.vat_id,tax_id=EXCLUDED.tax_id,\
            recipient_code=EXCLUDED.recipient_code,pec=EXCLUDED.pec,revision=nextval('customer_billing_revision')")
            .bind(id).bind(input.invoice_requested).bind(&input.billing_first_name).bind(&input.billing_last_name).bind(&input.billing_company).bind(&input.billing_country).bind(&input.billing_street).bind(&input.billing_number).bind(&input.billing_extra).bind(&input.billing_postal).bind(&input.billing_city).bind(&input.billing_region).bind(&input.vat_id).bind(&input.tax_id).bind(&input.recipient_code).bind(&input.pec).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn delete(&self, s: &Session, csrf: &str, id: i64, revision: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::CustomersWrite).await?;
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT id FROM customers WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Err(CustomerError::NotFound);
        }
        if sqlx::query("DELETE FROM customer_billing WHERE customer_id=$1 AND revision=$2")
            .bind(id)
            .bind(revision)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            != 1
        {
            return Err(CustomerError::Stale);
        }
        tx.commit().await?;
        Ok(())
    }
    /// Seed only an absent draft. Existing cart drafts and order snapshots are never merged.
    pub async fn initial(&self, session: &CustomerSession) -> Result<Details> {
        let mut tx = self.pool.begin().await?;
        let account = Customers::authorize_in(&mut tx, session, None).await?;
        let d = initial_in(&mut tx, &account).await?;
        tx.commit().await?;
        Ok(d)
    }
}
pub(crate) async fn initial_in(
    tx: &mut Transaction<'_, Postgres>,
    account: &CustomerAccount,
) -> Result<Details> {
    let phone: String = sqlx::query_scalar("SELECT phone FROM customers WHERE id=$1")
        .bind(account.id)
        .fetch_one(&mut **tx)
        .await?;
    let mut d = Details {
        email: account.email.clone(),
        first_name: account.first_name.clone(),
        last_name: account.last_name.clone(),
        customer_type: account.customer_type.clone(),
        phone,
        ..Details::default()
    };
    let billing: Option<Billing> =
        sqlx::query_as("SELECT * FROM customer_billing WHERE customer_id=$1")
            .bind(account.id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some(billing) = billing {
        billing.input.apply(&mut d);
    }
    let address: Option<crate::customer_addresses::Address> =
        sqlx::query_as("SELECT * FROM customer_addresses WHERE customer_id=$1 AND is_default")
            .bind(account.id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some(address) = address {
        address.apply_to_checkout(&mut d);
    }
    Ok(d)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fiscal_data_is_checked_and_hidden_values_are_editable() {
        let mut input = BillingInput {
            billing_company: " Example Srl ".into(),
            billing_country: "IT".into(),
            billing_street: "Via Example".into(),
            billing_number: "1".into(),
            billing_postal: "00100".into(),
            billing_city: "Roma".into(),
            billing_region: "RM".into(),
            vat_id: "IT12345678903".into(),
            recipient_code: "abc1234".into(),
            pec: "invoice@example.test".into(),
            ..BillingInput::default()
        };
        let value = input
            .validate("business", &customer_fields::Policy::default())
            .unwrap();
        assert_eq!(value.vat_id, "12345678903");
        assert_eq!(value.recipient_code, "ABC1234");
        assert_eq!(value.billing_company, "Example Srl");
        input.pec = "invalid".into();
        assert!(
            matches!(input.validate("business",&customer_fields::Policy::default()),Err(CustomerError::Fields(f)) if f.contains(&"pec"))
        );
        input.pec.clear();
        input.vat_id = "12345678900".into();
        assert!(
            matches!(input.validate("business",&customer_fields::Policy::default()),Err(CustomerError::Fields(f)) if f.contains(&"vat_id"))
        );
    }
}
