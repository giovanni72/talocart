//! Authorized order reads and counted customer history.
use super::*;

impl Orders {
    pub async fn for_guest(&self, guest: &Guest, reference: &str) -> Result<Order> {
        let mut tx = self.pool.begin().await?;
        Carts::authorize(&mut tx, guest)
            .await
            .map_err(|e| match e {
                CartError::Forbidden => OrderError::NotFound,
                e => e.into(),
            })?;
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT o.id \
            FROM orders o \
            JOIN guest_carts c ON c.id=$1 \
            AND c.token_hash=o.owner_hash \
            AND c.expires_at>now() \
            WHERE o.reference=$2 \
            AND o.owner_hash=$3",
        )
        .bind(guest.id)
        .bind(reference)
        .bind(&guest.token_hash)
        .fetch_optional(&mut *tx)
        .await?;
        let order = load(&mut tx, id.ok_or(OrderError::NotFound)?).await?;
        tx.commit().await?;
        Ok(order)
    }
    pub async fn for_customer(
        &self,
        session: &talocart_platform::identity::CustomerSession,
        reference: &str,
    ) -> Result<Order> {
        let mut tx = self.pool.begin().await?;
        let c = talocart_platform::identity::Customers::authorize_in(&mut tx, session, None)
            .await
            .map_err(customer_error)?;
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT o.id \
                FROM orders o \
                JOIN order_current_status cs ON cs.order_id=o.id \
                JOIN order_statuses s ON s.id=cs.status_id \
                JOIN customer_orders c ON c.order_id=o.id \
                WHERE c.customer_id=$1 \
                AND o.reference=$2",
        )
        .bind(c.id)
        .bind(reference)
        .fetch_optional(&mut *tx)
        .await?;
        let order = load(&mut tx, id.ok_or(OrderError::NotFound)?).await?;
        tx.commit().await?;
        Ok(order)
    }
    pub async fn customer_history(
        &self,
        session: &talocart_platform::identity::CustomerSession,
        requested: i64,
    ) -> Result<talocart_platform::pagination::Page<Summary>> {
        // Owner lock protects session validity while reading a stable counted page.
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        let c = talocart_platform::identity::Customers::authorize_in(&mut tx, session, None)
            .await
            .map_err(customer_error)?;
        let total: i64 =
            sqlx::query_scalar("SELECT count(*) FROM customer_orders WHERE customer_id=$1")
                .bind(c.id)
                .fetch_one(&mut *tx)
                .await?;
        let page = requested.clamp(1, if total == 0 { 1 } else { (total - 1) / 20 + 1 });
        let sql = "SELECT s.name_it AS status_it,s.name_en AS status_en,s.color AS status_color,coalesce(f.state,\
                'pending') AS fulfillment_state,o.id,o.reference,o.currency,o.total_cents,o.state,\
            o.payment_state='paid' AS paid,o.payment_state='refunded' AS refunded,platform_datetime(o.created_at,\
                r.time_zone,r.date_format) AS created_at \
            FROM orders o \
            JOIN order_current_status cs ON cs.order_id=o.id \
            JOIN order_statuses s ON s.id=cs.status_id \
            JOIN customer_orders c ON c.order_id=o.id \
            LEFT \
            JOIN order_fulfillment f ON f.order_id=o.id \
            CROSS \
            JOIN platform_regional_settings r \
            WHERE c.customer_id=$1 \
            AND r.singleton \
            ORDER BY o.id DESC \
            LIMIT 20 \
            OFFSET $2";
        let entries = sqlx::query_as(sql)
            .bind(c.id)
            .bind((page - 1) * 20)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(talocart_platform::pagination::Page {
            entries,
            total,
            page,
        })
    }
    pub async fn admin_customer_history(
        &self,
        session: &Session,
        customer_id: i64,
        requested: i64,
    ) -> Result<talocart_platform::pagination::Page<Summary>> {
        self.identity
            .authorize(session, Permission::CustomersRead)
            .await?;
        self.identity
            .authorize(session, Permission::OrdersRead)
            .await?;
        if !(1..=1_000_000).contains(&requested) {
            return Err(OrderError::ListFilters);
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let total: i64 =
            sqlx::query_scalar("SELECT count(*) FROM customer_orders WHERE customer_id=$1")
                .bind(customer_id)
                .fetch_one(&mut *tx)
                .await?;
        let page = requested.clamp(1, if total == 0 { 1 } else { (total - 1) / 20 + 1 });
        let sql = "SELECT s.name_it AS status_it,s.name_en AS status_en,s.color AS status_color,coalesce(f.state,\
                'pending') AS fulfillment_state,o.id,o.reference,o.currency,o.total_cents,o.state,\
            o.payment_state='paid' AS paid,o.payment_state='refunded' AS refunded,platform_datetime(o.created_at,\
                r.time_zone,r.date_format) AS created_at \
            FROM orders o \
            JOIN order_current_status cs ON cs.order_id=o.id \
            JOIN order_statuses s ON s.id=cs.status_id \
            JOIN customer_orders c ON c.order_id=o.id \
            LEFT \
            JOIN order_fulfillment f ON f.order_id=o.id \
            CROSS \
            JOIN platform_regional_settings r \
            WHERE c.customer_id=$1 \
            AND r.singleton \
            ORDER BY o.id DESC \
            LIMIT 20 \
            OFFSET $2";
        let entries = sqlx::query_as(sql)
            .bind(customer_id)
            .bind((page - 1) * 20)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(talocart_platform::pagination::Page {
            entries,
            total,
            page,
        })
    }
    pub async fn list(
        &self,
        s: &Session,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<Summary>> {
        let result = self.filtered_list(s, &ListFilters::default(), page).await?;
        Ok(talocart_platform::pagination::Page {
            entries: result.entries,
            total: result.total,
            page: result.page,
        })
    }

    pub async fn get(&self, s: &Session, id: i64) -> Result<Order> {
        self.identity.authorize(s, Permission::OrdersRead).await?;
        let mut tx = self.pool.begin().await?;
        let order = load(&mut tx, id).await?;
        tx.commit().await?;
        Ok(order)
    }
}
pub(super) async fn load(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<Order> {
    // Hold a shared order lock while reading its lines and receipt, so form
    // revisions and payment state belong to the same committed observation.
    let row:Stored=sqlx::query_as("SELECT id,reference,snapshot,currency,merchandise_cents,shipping_cents,fee_cents,\
        net_cents,tax_cents,total_cents,state,payment_state,orders.revision,platform_datetime(created_at,\
            r.time_zone,r.date_format) AS created_at,coalesce(platform_datetime(cancelled_at,\
                r.time_zone,r.date_format),'') AS cancelled_at \
        FROM orders \
        CROSS \
        JOIN platform_regional_settings r \
        WHERE r.singleton \
        AND id=$1 FOR SHARE OF orders").bind(id).fetch_optional(&mut **tx).await?.ok_or(OrderError::NotFound)?;
    let frozen: Frozen = toml::from_str(&row.snapshot).map_err(|_| OrderError::Invalid)?;
    if frozen.schema_version != 1 {
        return Err(OrderError::Invalid);
    }
    let lines = sqlx::query_as(
        "SELECT * FROM order_lines WHERE order_id=$1 AND active ORDER BY id LIMIT 100",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    let manual_payment = sqlx::query_as(
        "SELECT amount_cents,currency,method_code,reference,administrator_id,administrator_name,\
            platform_datetime(recorded_at,r.time_zone,r.date_format) AS recorded_at \
            FROM order_manual_payments \
            CROSS \
            JOIN platform_regional_settings r \
            WHERE r.singleton \
            AND order_id=$1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    let (online_review, online_review_disputed, online_attempt_state): (
        bool,
        bool,
        Option<String>,
    ) = if crate::payments::is_online_code(&frozen.payment.code) {
        sqlx::query_as(
            "SELECT EXISTS(SELECT 1 \
                    FROM payment_attempts \
                    WHERE order_id=$1 \
                    AND state='review'), EXISTS(SELECT 1 \
                    FROM payment_attempts \
                    WHERE order_id=$1 \
                    AND state='review' \
                    AND last_error<>'temporary'), (SELECT state \
                    FROM payment_attempts \
                    WHERE order_id=$1 \
                    ORDER BY id DESC \
                    LIMIT 1)",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await?
    } else {
        (false, false, None)
    };
    let (status_id, status_revision): (i64, i64) =
        sqlx::query_as("SELECT status_id,revision FROM order_current_status WHERE order_id=$1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    let current_status = OrderStatuses::require(tx, status_id)
        .await
        .map_err(OrderError::from)?;
    Ok(Order {
        current_status,
        status_revision,
        fulfillment: fulfillment::load_fulfillment(tx, id).await?,
        online_review,
        online_review_disputed,
        online_attempt_state,
        id: row.id,
        reference: row.reference,
        frozen,
        currency: row.currency,
        merchandise_cents: row.merchandise_cents,
        shipping_cents: row.shipping_cents,
        fee_cents: row.fee_cents,
        net_cents: row.net_cents,
        tax_cents: row.tax_cents,
        total_cents: row.total_cents,
        state: row.state,
        payment_state: row.payment_state,
        revision: row.revision,
        created_at: row.created_at,
        cancelled_at: row.cancelled_at,
        lines,
        manual_payment,
    })
}
