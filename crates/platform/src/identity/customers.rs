//! Customer identity is isolated from admin roles; only password work and limits are shared.
use super::*;

#[derive(Debug, thiserror::Error)]
pub enum CustomerError {
    #[error(transparent)]
    Technical(#[from] crate::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] crate::technical::AccessError),
    #[error("Email confirmation link invalid or expired")]
    ConfirmationInvalid,
    #[error("Invalid customer fields")]
    Fields(Vec<&'static str>),
    #[error("Customer email already registered")]
    Duplicate,
    #[error("Customer resource not found")]
    NotFound,
    #[error("Customer resource changed")]
    Stale,
    #[error("Address limit reached")]
    AddressLimit,
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for CustomerError {
    fn from(error: sqlx::Error) -> Self {
        Self::Technical(crate::technical::DatabaseFailure::capture(error, module_path!()).into())
    }
}
#[derive(Clone, Copy)]
pub enum CustomerAuthAction {
    Anonymous,
    Register,
    Login,
    Reset,
}
impl CustomerAuthAction {
    fn budget(self) -> (&'static str, i32) {
        match self {
            Self::Anonymous => ("anonymous", 200),
            Self::Register => ("register", 20),
            Self::Login => ("login", 100),
            Self::Reset => ("reset", 30),
        }
    }
}
type CustomerResult<T> = std::result::Result<T, CustomerError>;
/// No Debug or public credentials. Services recheck this capability inside their transaction.
#[derive(Clone, FromRow)]
pub struct CustomerSession {
    pub(super) token_hash: String,
    pub csrf: String,
    pub(super) customer_id: Option<i64>,
}
impl CustomerSession {
    pub fn customer_id(&self) -> Option<i64> {
        self.customer_id
    }
    pub fn check_csrf(&self, value: &str) -> Result<()> {
        check_csrf(&self.csrf, value)
    }
}
#[derive(FromRow)]
pub struct CustomerAccount {
    pub customer_type: String,
    pub id: i64,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub revision: i64,
}
#[derive(Clone, serde::Deserialize)]
#[serde(default)]
pub struct CustomerProfile {
    pub customer_type: String,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
}
impl Default for CustomerProfile {
    fn default() -> Self {
        Self {
            customer_type: "private".into(),
            email: String::new(),
            first_name: String::new(),
            last_name: String::new(),
        }
    }
}
#[derive(Clone)]
pub struct Customers {
    pub(super) pool: PgPool,
    pub(super) identity: Identity,
}
fn names(
    profile: &CustomerProfile,
    policy: &crate::customer_fields::Policy,
    previous: Option<&CustomerAccount>,
) -> CustomerResult<(String, String)> {
    use crate::customer_fields::Mode;
    let mut errors = Vec::new();
    if !crate::customer_fields::valid_type(&profile.customer_type) {
        errors.push("customer_type");
    }
    let mut values = Vec::new();
    for (field, value) in [
        ("first_name", &profile.first_name),
        ("last_name", &profile.last_name),
    ] {
        let mode = policy.mode(field, &profile.customer_type);
        let value = if mode == Mode::Hidden {
            previous
                .map(|c| {
                    if field == "first_name" {
                        c.first_name.as_str()
                    } else {
                        c.last_name.as_str()
                    }
                })
                .unwrap_or("")
        } else {
            value.trim()
        };
        if (mode == Mode::Required && value.is_empty())
            || value.chars().count() > 100
            || value.chars().any(char::is_control)
        {
            errors.push(field);
        }
        values.push(value.to_owned());
    }
    if !errors.is_empty() {
        return Err(CustomerError::Fields(errors));
    }
    Ok((values.remove(0), values.remove(0)))
}
impl Customers {
    /// Pass the application's Identity clone so customer/admin hashing shares one semaphore.
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    /// Trusted transport peer only: never derive this address from forwarded headers.
    /// Session/account budgets remain enforced for every service call.
    pub async fn limit_origin(
        &self,
        origin: std::net::IpAddr,
        action: CustomerAuthAction,
    ) -> CustomerResult<()> {
        let origin = origin.to_canonical();
        let (name, maximum) = action.budget();
        // A shared connection can serve multiple customers; session registration stays at 20.
        let maximum = if matches!(action, CustomerAuthAction::Register) {
            60
        } else {
            maximum
        };
        self.identity
            .limit(
                &format!("customer-origin:{name}:{}", digest(&origin.to_string())),
                maximum,
            )
            .await?;
        Ok(())
    }
    pub(super) async fn limit_session(
        &self,
        session: &CustomerSession,
        action: CustomerAuthAction,
    ) -> CustomerResult<()> {
        let (name, maximum) = action.budget();
        self.identity
            .limit(
                &format!("customer-session:{name}:{}", session.token_hash),
                maximum,
            )
            .await?;
        Ok(())
    }
    pub async fn session(&self, raw: &str) -> CustomerResult<Option<CustomerSession>> {
        if !valid_token(raw) {
            return Ok(None);
        }
        Ok(sqlx::query_as("SELECT s.token_hash,s.csrf,s.customer_id FROM customer_sessions s LEFT JOIN customers c ON c.id=s.customer_id WHERE s.token_hash=$1 AND s.expires_at>now() AND (s.customer_id IS NULL OR c.active)")
            .bind(digest(raw)).fetch_optional(&self.pool).await?)
    }
    pub async fn anonymous(&self) -> CustomerResult<(String, CustomerSession)> {
        // Keep opportunistic cleanup bounded and never wait for a session in use.
        sqlx::query("WITH expired AS (SELECT token_hash FROM customer_sessions WHERE expires_at<=now() ORDER BY expires_at,token_hash LIMIT 100 FOR UPDATE SKIP LOCKED) DELETE FROM customer_sessions s USING expired e WHERE s.token_hash=e.token_hash")
            .execute(&self.pool)
            .await?;
        let raw = token()?;
        let session = CustomerSession {
            token_hash: digest(&raw),
            csrf: token()?,
            customer_id: None,
        };
        sqlx::query("INSERT INTO customer_sessions(token_hash,csrf,expires_at) VALUES($1,$2,now()+$3::bigint*interval '1 second')")
            .bind(&session.token_hash).bind(&session.csrf).bind(ANONYMOUS_SESSION_SECONDS).execute(&self.pool).await?;
        Ok((raw, session))
    }
    async fn rotate(
        tx: &mut Transaction<'_, Postgres>,
        session: &CustomerSession,
        id: i64,
    ) -> CustomerResult<String> {
        let deleted=sqlx::query("DELETE FROM customer_sessions WHERE token_hash=$1 AND customer_id IS NULL AND expires_at>now()")
            .bind(&session.token_hash).execute(&mut **tx).await?.rows_affected();
        if deleted != 1 {
            return Err(IdentityError::Unauthorized.into());
        }
        let raw = token()?;
        sqlx::query("INSERT INTO customer_sessions(token_hash,csrf,customer_id,expires_at) VALUES($1,$2,$3,now()+$4::bigint*interval '1 second')")
            .bind(digest(&raw)).bind(token()?).bind(id).bind(AUTHENTICATED_SESSION_SECONDS).execute(&mut **tx).await?;
        Ok(raw)
    }
    /// Some(session) means immediate activation; None means an inactive account
    /// and confirmation email were committed atomically. No customer cart is created here.
    pub async fn register(
        &self,
        session: &CustomerSession,
        csrf: &str,
        profile: &CustomerProfile,
        secret: String,
        delivery: Option<&customer_registration::RegistrationEmail<'_>>,
    ) -> CustomerResult<Option<String>> {
        session.check_csrf(csrf)?;
        if session.customer_id.is_some() {
            return Err(IdentityError::Forbidden.into());
        }
        let address = email(&profile.email).map_err(|_| CustomerError::Fields(vec!["email"]))?;
        let address =
            crate::mail::address(&address).map_err(|_| CustomerError::Fields(vec!["email"]))?;

        password(&secret).map_err(|_| CustomerError::Fields(vec!["password"]))?;
        self.limit_session(session, CustomerAuthAction::Register)
            .await?;
        let hash = self.identity.hash(secret).await?;
        let mut tx = self.pool.begin().await?;
        let installed: bool = sqlx::query_scalar("SELECT installed FROM identity_state WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        if !installed {
            return Err(IdentityError::Forbidden.into());
        }
        let policy = crate::customer_fields::load_in(&mut tx).await?;
        let (first, last) = names(profile, &policy, None)?;
        let confirmation = customer_registration::required(&mut tx).await?;
        if confirmation {
            Self::anonymous_in(&mut tx, session).await?;
        }
        let id:Option<i64>=sqlx::query_scalar("INSERT INTO customers(email,password_hash,first_name,last_name,customer_type,active) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(email) DO NOTHING RETURNING id")
            .bind(&address).bind(hash).bind(first).bind(last).bind(&profile.customer_type).bind(!confirmation).fetch_optional(&mut *tx).await?;
        let id = id.ok_or(CustomerError::Duplicate)?;
        let raw = if confirmation {
            customer_registration::enqueue(
                &mut tx,
                id,
                &address,
                delivery.ok_or(IdentityError::Unavailable)?,
            )
            .await?;
            None
        } else {
            Some(Self::rotate(&mut tx, session, id).await?)
        };
        tx.commit().await?;
        Ok(raw)
    }
    pub async fn login(
        &self,
        session: &CustomerSession,
        csrf: &str,
        address: &str,
        secret: String,
    ) -> CustomerResult<String> {
        session.check_csrf(csrf)?;
        if session.customer_id.is_some() {
            return Err(IdentityError::Forbidden.into());
        }
        self.limit_session(session, CustomerAuthAction::Login)
            .await?;
        let address = email(address).map_err(|_| IdentityError::Unauthorized)?;
        let bucket = format!("customer-login-failed:{}", digest(&address));
        self.identity.check_login_failures(&bucket).await?;
        if secret.len() > 512 {
            self.identity.record_attempt(&bucket).await?;
            return Err(IdentityError::Unauthorized.into());
        }
        let row: Option<(i64, String)> =
            sqlx::query_as("SELECT id,password_hash FROM customers WHERE email=$1 AND active")
                .bind(&address)
                .fetch_optional(&self.pool)
                .await?;
        if !self
            .identity
            .verify(secret, row.as_ref().map(|r| r.1.clone()))
            .await?
        {
            self.identity.record_attempt(&bucket).await?;
            return Err(IdentityError::Unauthorized.into());
        }
        let (id, hash) = row.ok_or(IdentityError::Unauthorized)?;
        let mut tx = self.pool.begin().await?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM customers WHERE id=$1 AND password_hash=$2 AND email=$3 AND active FOR UPDATE",
        )
        .bind(id)
        .bind(hash)
        .bind(address)
        .fetch_optional(&mut *tx)
        .await?;
        if current.is_none() {
            return Err(IdentityError::Unauthorized.into());
        }
        let raw = Self::rotate(&mut tx, session, id).await?;
        tx.commit().await?;
        Ok(raw)
    }
    /// Account first, then session. A revocation cannot race a committed mutation.
    /// Read callers pass None; mutating callers supply the submitted CSRF token.
    pub async fn authorize_in(
        tx: &mut Transaction<'_, Postgres>,
        session: &CustomerSession,
        csrf: Option<&str>,
    ) -> CustomerResult<CustomerAccount> {
        if let Some(csrf) = csrf {
            session.check_csrf(csrf)?;
        }
        let id = session.customer_id.ok_or(IdentityError::Forbidden)?;
        if csrf.is_none() {
            // Materialize the account lock before locking the session, preserving
            // the account -> session order used by revocation and mutations.
            // Both rows stay locked until the caller commits its transaction.
            #[derive(FromRow)]
            struct Authorized {
                #[sqlx(flatten)]
                account: CustomerAccount,
                session_csrf: String,
            }
            let row: Option<Authorized> = sqlx::query_as("WITH account AS MATERIALIZED (SELECT id,email,first_name,last_name,customer_type,revision FROM customers WHERE id=$1 AND active FOR SHARE) SELECT account.*,s.csrf AS session_csrf FROM account JOIN customer_sessions s ON s.customer_id=account.id WHERE s.token_hash=$2 AND s.expires_at>now() FOR SHARE OF s")
                .bind(id).bind(&session.token_hash).fetch_optional(&mut **tx).await?;
            let row = row.ok_or(IdentityError::Forbidden)?;
            check_csrf(&row.session_csrf, &session.csrf)?;
            return Ok(row.account);
        }
        let account: Option<CustomerAccount> = sqlx::query_as("SELECT id,email,first_name,last_name,customer_type,revision FROM customers WHERE id=$1 AND active FOR UPDATE")
            .bind(id).fetch_optional(&mut **tx).await?;
        let account = account.ok_or(IdentityError::Forbidden)?;
        let valid:Option<String>=sqlx::query_scalar("SELECT csrf FROM customer_sessions WHERE token_hash=$1 AND customer_id=$2 AND expires_at>now() FOR SHARE")
            .bind(&session.token_hash).bind(id).fetch_optional(&mut **tx).await?;
        let valid = valid.ok_or(IdentityError::Forbidden)?;
        check_csrf(&valid, &session.csrf)?;
        Ok(account)
    }
    pub async fn account(&self, session: &CustomerSession) -> CustomerResult<CustomerAccount> {
        let mut tx = self.pool.begin().await?;
        let account = Self::authorize_in(&mut tx, session, None).await?;
        tx.commit().await?;
        Ok(account)
    }
    pub async fn logout(&self, session: &CustomerSession, csrf: &str) -> CustomerResult<()> {
        let mut tx = self.pool.begin().await?;
        Self::authorize_in(&mut tx, session, Some(csrf)).await?;
        sqlx::query("DELETE FROM customer_sessions WHERE token_hash=$1")
            .bind(&session.token_hash)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn credentials(
        &self,
        session: &CustomerSession,
        secret: String,
    ) -> CustomerResult<String> {
        let account = self.account(session).await?;
        self.identity
            .limit(&format!("customer-change:{}", account.id), 10)
            .await?;
        let hash: String =
            sqlx::query_scalar("SELECT password_hash FROM customers WHERE id=$1 AND active")
                .bind(account.id)
                .fetch_one(&self.pool)
                .await?;
        if secret.len() > 512 || !self.identity.verify(secret, Some(hash.clone())).await? {
            return Err(IdentityError::CurrentPassword.into());
        }
        Ok(hash)
    }
    pub(super) async fn revoke(tx: &mut Transaction<'_, Postgres>, id: i64) -> CustomerResult<()> {
        sqlx::query("DELETE FROM customer_password_resets WHERE customer_id=$1")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM customer_sessions WHERE customer_id=$1")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
    pub async fn update(
        &self,
        session: &CustomerSession,
        csrf: &str,
        revision: i64,
        profile: &CustomerProfile,
        secret: String,
    ) -> CustomerResult<bool> {
        session.check_csrf(csrf)?;

        let address = email(&profile.email).map_err(|_| CustomerError::Fields(vec!["email"]))?;
        let expected = self.account(session).await?;
        let changed = expected.email != address;
        let hash = if changed {
            Some(self.credentials(session, secret).await?)
        } else {
            None
        };
        let mut tx = self.pool.begin().await?;
        let current = Self::authorize_in(&mut tx, session, Some(csrf)).await?;
        let policy = crate::customer_fields::load_in(&mut tx).await?;
        let (first, last) = names(profile, &policy, Some(&current))?;
        if current.revision != revision {
            return Err(CustomerError::Stale);
        }
        if let Some(hash) = hash {
            let matches: bool =
                sqlx::query_scalar("SELECT password_hash=$2 FROM customers WHERE id=$1")
                    .bind(current.id)
                    .bind(hash)
                    .fetch_one(&mut *tx)
                    .await?;
            if !matches {
                return Err(IdentityError::Forbidden.into());
            }
        }
        let duplicate: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM customers WHERE email=$1 AND id<>$2)")
                .bind(&address)
                .bind(current.id)
                .fetch_one(&mut *tx)
                .await?;
        if duplicate {
            return Err(CustomerError::Duplicate);
        }
        sqlx::query("UPDATE customers SET first_name=$1,last_name=$2,email=$3,revision=revision+1,customer_type=$5 WHERE id=$4")
            .bind(first).bind(last).bind(address).bind(current.id).bind(&profile.customer_type).execute(&mut *tx).await.map_err(|e| {
                if e.as_database_error().is_some_and(|e|e.is_unique_violation()) { CustomerError::Duplicate } else { e.into() }
            })?;
        if changed {
            Self::revoke(&mut tx, current.id).await?;
        }
        tx.commit().await?;
        Ok(changed)
    }
    pub async fn change_password(
        &self,
        session: &CustomerSession,
        csrf: &str,
        current: String,
        next: String,
    ) -> CustomerResult<()> {
        session.check_csrf(csrf)?;
        password(&next).map_err(|_| CustomerError::Fields(vec!["password"]))?;
        let expected = self.credentials(session, current).await?;
        let hash = self.identity.hash(next).await?;
        let mut tx = self.pool.begin().await?;
        let account = Self::authorize_in(&mut tx, session, Some(csrf)).await?;
        let changed=sqlx::query("UPDATE customers SET password_hash=$1,revision=revision+1 WHERE id=$2 AND password_hash=$3")
            .bind(hash).bind(account.id).bind(expected).execute(&mut *tx).await?.rows_affected();
        if changed != 1 {
            return Err(IdentityError::Forbidden.into());
        }
        Self::revoke(&mut tx, account.id).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Local operator verifies the customer out of band; never claim orders by email.
    pub async fn issue_reset(&self, address: &str) -> CustomerResult<String> {
        let address = email(address)?;
        let raw = token()?;
        let mut tx = self.pool.begin().await?;
        let id: Option<i64> =
            sqlx::query_scalar("SELECT id FROM customers WHERE email=$1 AND active FOR UPDATE")
                .bind(address)
                .fetch_optional(&mut *tx)
                .await?;
        sqlx::query("INSERT INTO customer_password_resets(token_hash,customer_id,expires_at) VALUES($1,$2,now()+$3::bigint*interval '1 second') ON CONFLICT(customer_id) DO UPDATE SET token_hash=EXCLUDED.token_hash,expires_at=EXCLUDED.expires_at")
            .bind(digest(&raw)).bind(id.ok_or(CustomerError::NotFound)?).bind(ONE_TIME_TOKEN_SECONDS).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(raw)
    }
    pub async fn reset(
        &self,
        session: &CustomerSession,
        csrf: &str,
        raw: &str,
        next: String,
    ) -> CustomerResult<()> {
        session.check_csrf(csrf)?;
        if session.customer_id.is_some() || !valid_token(raw) {
            return Err(IdentityError::Unauthorized.into());
        }
        password(&next).map_err(|_| CustomerError::Fields(vec!["password"]))?;
        self.limit_session(session, CustomerAuthAction::Reset)
            .await?;
        let hash = self.identity.hash(next).await?;
        let mut tx = self.pool.begin().await?;
        let id:Option<i64>=sqlx::query_scalar("SELECT c.id FROM customers c JOIN customer_password_resets r ON r.customer_id=c.id WHERE r.token_hash=$1 AND r.expires_at>now() AND c.active FOR UPDATE OF c")
            .bind(digest(raw)).fetch_optional(&mut *tx).await?;
        let id = id.ok_or(IdentityError::Unauthorized)?;
        let consumed=sqlx::query("DELETE FROM customer_password_resets WHERE customer_id=$1 AND token_hash=$2 AND expires_at>now()")
            .bind(id).bind(digest(raw)).execute(&mut *tx).await?.rows_affected();
        if consumed != 1 {
            return Err(IdentityError::Unauthorized.into());
        }
        // Consume the anonymous capability too, preventing reset POST replays.
        let consumed=sqlx::query("DELETE FROM customer_sessions WHERE token_hash=$1 AND customer_id IS NULL AND expires_at>now()")
            .bind(&session.token_hash).execute(&mut *tx).await?.rows_affected();
        if consumed != 1 {
            return Err(IdentityError::Unauthorized.into());
        }
        sqlx::query("UPDATE customers SET password_hash=$1,revision=revision+1 WHERE id=$2")
            .bind(hash)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        Self::revoke(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }
}

impl From<IdentityError> for CustomerError {
    fn from(error: IdentityError) -> Self {
        if let Some(cause) = error.technical() {
            Self::Technical(cause)
        } else if let Some(cause) = error.access() {
            Self::Access(cause)
        } else {
            Self::Identity(error)
        }
    }
}
