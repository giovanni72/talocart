//! Administrator capabilities for customer master data. No customer impersonation.
use super::*;
use crate::customer_fields::{self, Mode};
type AdminResult<T> = std::result::Result<T, CustomerError>;

#[derive(Clone, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub group_id: Option<i64>,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub customer_type: String,
    pub phone: String,
    pub active: bool,
}
#[derive(FromRow)]
pub struct Customer {
    pub group_id: i64,
    pub id: i64,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub customer_type: String,
    pub phone: String,
    pub active: bool,
    pub revision: i64,
    pub created_at: String,
}
impl Customer {
    pub fn profile(&self) -> Profile {
        Profile {
            group_id: Some(self.group_id),
            email: self.email.clone(),
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            customer_type: self.customer_type.clone(),
            phone: self.phone.clone(),
            active: self.active,
        }
    }
}
/// Overview counts: every account, then those created in the last 30 days.
pub async fn customer_counts(
    pool: &PgPool,
    identity: &Identity,
    session: &Session,
) -> std::result::Result<(i64, i64), IdentityError> {
    identity
        .authorize(session, Permission::CustomersRead)
        .await?;
    Ok(sqlx::query_as(
        "SELECT count(*),count(*) FILTER(WHERE created_at>=clock_timestamp()-interval '30 days') \
            FROM customers",
    )
    .fetch_one(pool)
    .await?)
}
#[derive(Clone)]
pub struct CustomerAdmin {
    pool: PgPool,
    identity: Identity,
}
impl CustomerAdmin {
    /// Bounded picker for authorized administrative workflows.
    pub async fn search(&self, session: &Session, term: &str) -> AdminResult<Vec<Customer>> {
        self.identity
            .authorize(session, Permission::CustomersRead)
            .await?;
        if term.chars().count() > 100 {
            return Err(CustomerError::NotFound);
        }
        Ok(sqlx::query_as("SELECT c.group_id,c.id,c.email,c.first_name,c.last_name,c.customer_type,c.phone,c.active,c.revision,'' AS created_at FROM customers c WHERE c.active AND (position(lower($1) in lower(c.email||' '||c.first_name||' '||c.last_name))>0) ORDER BY c.last_name,c.first_name,c.id LIMIT 25")
            .bind(term.trim()).fetch_all(&self.pool).await?)
    }

    /// Recheck an explicitly selected account inside the caller's transaction.
    pub async fn require_order_customer_in(
        tx: &mut Transaction<'_, Postgres>,
        session: &Session,
        id: i64,
    ) -> AdminResult<()> {
        Identity::authorize_read_transaction(tx, session, Permission::CustomersRead).await?;
        let active: Option<bool> =
            sqlx::query_scalar("SELECT active FROM customers WHERE id=$1 FOR SHARE")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
        if active != Some(true) {
            return Err(CustomerError::NotFound);
        }
        Ok(())
    }
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn get(&self, session: &Session, id: i64) -> AdminResult<Customer> {
        self.identity
            .authorize(session, Permission::CustomersRead)
            .await?;
        sqlx::query_as("SELECT c.group_id,c.id,c.email,c.first_name,c.last_name,c.customer_type,c.phone,c.active,c.revision,platform_datetime(c.created_at,r.time_zone,r.date_format) AS created_at FROM customers c CROSS JOIN platform_regional_settings r WHERE c.id=$1 AND r.singleton")
            .bind(id).fetch_optional(&self.pool).await?.ok_or(CustomerError::NotFound)
    }
    /// Create credentials without impersonation. Hash outside the transaction;
    /// recheck authorization, field policy and group membership before committing.
    pub async fn create(
        &self,
        session: &Session,
        csrf: &str,
        input: &Profile,
        secret: String,
    ) -> AdminResult<i64> {
        session.check_csrf(csrf)?;
        self.identity
            .authorize(session, Permission::CustomersWrite)
            .await?;
        password(&secret).map_err(|_| CustomerError::Fields(vec!["password"]))?;
        let hash = self.identity.hash(secret).await?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::CustomersWrite).await?;
        let policy = customer_fields::load_in(&mut tx)
            .await?
            .for_administration();
        let address = validate(input, &policy)?;
        if let Some(group) = input.group_id
            && sqlx::query_scalar::<_, i64>(
                "SELECT id FROM customer_groups WHERE id=$1 FOR KEY SHARE",
            )
            .bind(group)
            .fetch_optional(&mut *tx)
            .await?
            .is_none()
        {
            return Err(CustomerError::Fields(vec!["group_id"]));
        }
        let id = sqlx::query_scalar("INSERT INTO customers(email,password_hash,first_name,last_name,customer_type,phone,active,group_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(email) DO NOTHING RETURNING id")
            .bind(address).bind(hash).bind(input.first_name.trim()).bind(input.last_name.trim())
            .bind(&input.customer_type).bind(input.phone.trim()).bind(input.active).bind(input.group_id)
            .fetch_optional(&mut *tx).await?.ok_or(CustomerError::Duplicate)?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn save(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        input: &Profile,
    ) -> AdminResult<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::CustomersWrite).await?;
        // Lock group before customer, matching the existing assignment service.
        if let Some(group) = input.group_id
            && sqlx::query_scalar::<_, i64>(
                "SELECT id FROM customer_groups WHERE id=$1 FOR KEY SHARE",
            )
            .bind(group)
            .fetch_optional(&mut *tx)
            .await?
            .is_none()
        {
            return Err(CustomerError::Fields(vec!["group_id"]));
        }
        let previous: (i64, String, bool) =
            sqlx::query_as("SELECT revision,email,active FROM customers WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(CustomerError::NotFound)?;
        if revision != previous.0 {
            return Err(CustomerError::Stale);
        }
        let policy = customer_fields::load_in(&mut tx).await?;
        let address = validate(input, &policy)?;
        sqlx::query("UPDATE customers SET email=$1,first_name=$2,last_name=$3,customer_type=$4,phone=$5,active=$6,group_id=COALESCE($8,group_id),revision=revision+1 WHERE id=$7")
            .bind(&address).bind(input.first_name.trim()).bind(input.last_name.trim()).bind(&input.customer_type).bind(input.phone.trim()).bind(input.active).bind(id).bind(input.group_id)
            .execute(&mut *tx).await.map_err(|e| if e.as_database_error().is_some_and(|e|e.is_unique_violation()){CustomerError::Duplicate}else{e.into()})?;
        if address != previous.1 || input.active != previous.2 {
            Customers::revoke(&mut tx, id).await?;
        }
        crate::mail::Mail::supersede_customer_confirmation(&mut tx, id)
            .await
            .map_err(IdentityError::from)?;
        tx.commit().await?;
        Ok(())
    }
    /// Credential actions use the current account revision: stale tabs cannot revoke
    /// a newer recovery token or create one after concurrent deactivation/email change.
    pub async fn access(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        recovery: bool,
    ) -> AdminResult<Option<String>> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::CustomersWrite).await?;
        let current: (i64, bool) =
            sqlx::query_as("SELECT revision,active FROM customers WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(CustomerError::NotFound)?;
        if current.0 != revision {
            return Err(CustomerError::Stale);
        }
        if recovery && !current.1 {
            return Err(CustomerError::Fields(vec!["active"]));
        }
        Customers::revoke(&mut tx, id).await?;
        let raw = if recovery {
            let raw = token()?;
            sqlx::query("INSERT INTO customer_password_resets(token_hash,customer_id,expires_at) VALUES($1,$2,now()+$3::bigint*interval '1 second')")
                .bind(digest(&raw)).bind(id).bind(ONE_TIME_TOKEN_SECONDS).execute(&mut *tx).await?;
            Some(raw)
        } else {
            None
        };
        sqlx::query("UPDATE customers SET revision=revision+1 WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(raw)
    }
}
fn validate(input: &Profile, policy: &customer_fields::Policy) -> AdminResult<String> {
    let mut errors = Vec::new();
    let address = email(&input.email).map_err(|_| CustomerError::Fields(vec!["email"]))?;
    if !customer_fields::valid_type(&input.customer_type) {
        errors.push("customer_type");
    }
    for (key, value) in [
        ("first_name", &input.first_name),
        ("last_name", &input.last_name),
    ] {
        if (value.trim().is_empty() && policy.mode(key, &input.customer_type) == Mode::Required)
            || value.trim().chars().count() > 100
            || value.chars().any(char::is_control)
        {
            errors.push(key);
        }
    }
    // Phone is optional master data: the shipping policy governs each address, not
    // an existing account that may never have provided a primary phone number.
    let phone = input.phone.trim();
    let digits = phone.bytes().filter(u8::is_ascii_digit).count();
    if phone.len() > 40
        || (!phone.is_empty()
            && (!(7..=15).contains(&digits)
                || !phone
                    .chars()
                    .all(|c| c.is_ascii_digit() || "+()-. ".contains(c))))
    {
        errors.push("phone");
    }
    if !errors.is_empty() {
        return Err(CustomerError::Fields(errors));
    }
    Ok(address)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn administrative_profile_checks_formats_and_requiredness() {
        let mut p = Profile {
            email: " Alex@Example.test ".into(),
            first_name: "Alex".into(),
            last_name: "Buyer".into(),
            customer_type: "private".into(),
            active: true,
            ..Profile::default()
        };
        assert_eq!(
            validate(&p, &customer_fields::Policy::default()).unwrap(),
            "alex@example.test"
        );
        p.phone = "abc".into();
        assert!(
            matches!(validate(&p,&customer_fields::Policy::default()),Err(CustomerError::Fields(f)) if f.contains(&"phone"))
        );
        p.phone.clear();
        p.customer_type = "invalid".into();
        assert!(validate(&p, &customer_fields::Policy::default()).is_err());
    }
}
