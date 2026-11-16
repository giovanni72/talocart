//! Full-order physical dispatch and private, append-only operator history.
use super::*;
#[derive(Clone, Debug, Default, FromRow)]
pub struct Fulfillment {
    pub state: String,
    pub revision: i64,
    pub carrier: String,
    pub tracking_number: String,
    pub tracking_url: String,
}
impl Fulfillment {
    pub fn pending() -> Self {
        Self {
            state: "pending".into(),
            ..Self::default()
        }
    }
    pub fn key(&self) -> &'static str {
        match self.state.as_str() {
            "preparing" => "fulfillment-preparing",
            "shipped" => "fulfillment-shipped",
            "delivered" => "fulfillment-delivered",
            _ => "fulfillment-pending",
        }
    }
    /// Compare the normalized editable fields, independently of private notes.
    pub fn tracking_changed(&self, input: &FulfillmentInput) -> bool {
        self.carrier != input.carrier.trim()
            || self.tracking_number != input.tracking_number.trim()
            || self.tracking_url != input.tracking_url.trim()
    }
    pub fn sent(&self) -> bool {
        matches!(self.state.as_str(), "shipped" | "delivered")
    }
}
#[derive(Clone, Default)]
pub struct FulfillmentInput {
    pub action: String,
    pub body: String,
    pub carrier: String,
    pub tracking_number: String,
    pub tracking_url: String,
}
#[derive(FromRow)]
pub struct FulfillmentEvent {
    pub id: i64,
    pub kind: String,
    pub body: String,
    pub carrier: String,
    pub tracking_number: String,
    pub tracking_url: String,
    pub administrator_id: i64,
    pub administrator_name: String,
    pub created_at: String,
}
fn text(value: &str, max: usize, multiline: bool) -> bool {
    value.chars().count() <= max
        && !value
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t')))
}
fn validated(i: &FulfillmentInput) -> Result<FulfillmentInput> {
    let mut i = i.clone();
    i.carrier = i.carrier.trim().into();
    i.tracking_number = i.tracking_number.trim().into();
    i.tracking_url = i.tracking_url.trim().into();
    let mut fields = vec![];
    if !matches!(
        i.action.as_str(),
        "preparing" | "shipped" | "delivered" | "tracking" | "note"
    ) {
        fields.push("action")
    }
    if i.action == "note" {
        if i.body.trim().is_empty() || !text(&i.body, 2000, true) {
            fields.push("body")
        }
        i.carrier.clear();
        i.tracking_number.clear();
        i.tracking_url.clear();
    } else {
        i.body.clear();
        if matches!(i.action.as_str(), "shipped" | "tracking") {
            if i.carrier.is_empty() || !text(&i.carrier, 80, false) {
                fields.push("carrier")
            }
            if !text(&i.tracking_number, 120, false) {
                fields.push("tracking_number")
            }
            if !i.tracking_url.is_empty() {
                let valid = url::Url::parse(&i.tracking_url).ok().is_some_and(|u| {
                    u.scheme() == "https"
                        && u.has_host()
                        && u.username().is_empty()
                        && u.password().is_none()
                });
                if !text(&i.tracking_url, 2048, false) || !valid || i.tracking_number.is_empty() {
                    fields.push("tracking_url")
                }
            }
        } else {
            i.carrier.clear();
            i.tracking_number.clear();
            i.tracking_url.clear();
        }
    }
    if fields.is_empty() {
        Ok(i)
    } else {
        Err(OrderError::FulfillmentFields(fields))
    }
}
pub(super) async fn load_fulfillment(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
) -> Result<Fulfillment> {
    Ok(sqlx::query_as("SELECT state,revision,carrier,tracking_number,tracking_url FROM order_fulfillment WHERE order_id=$1").bind(id).fetch_optional(&mut **tx).await?.unwrap_or_else(Fulfillment::pending))
}
impl Orders {
    pub async fn fulfillment_history(
        &self,
        s: &Session,
        id: i64,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<FulfillmentEvent>> {
        self.identity.authorize(s, Permission::OrdersRead).await?;
        if !(1..=1000000).contains(&page) {
            return Err(OrderError::Invalid);
        }
        Ok(talocart_platform::pagination::read(&self.pool,sqlx::query("SELECT e.id,e.kind,e.body,e.carrier,e.tracking_number,e.tracking_url,e.administrator_id,\
            e.administrator_name,platform_datetime(e.created_at,r.time_zone,r.date_format) AS created_at \
            FROM order_fulfillment_events e \
            CROSS \
            JOIN platform_regional_settings r \
            WHERE e.order_id=$1 \
            AND r.singleton").bind(id),"id DESC",page,20).await?)
    }
    pub async fn update_fulfillment(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        key: &str,
        input: &FulfillmentInput,
    ) -> Result<Order> {
        crate::transaction_retry::run(
            "fulfillment",
            || self.update_fulfillment_once(s, csrf, id, revision, key, input),
            OrderError::technical,
        )
        .await
    }
    async fn update_fulfillment_once(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        key: &str,
        input: &FulfillmentInput,
    ) -> Result<Order> {
        s.check_csrf(csrf)?;
        s.require(Permission::OrdersWrite)?;
        let i = validated(input)?;
        if !talocart_platform::identity::valid_token(key) {
            return Err(OrderError::FulfillmentFields(vec!["request_key"]));
        }
        let key = key.to_ascii_lowercase();
        let payload = serde_json::to_string(&[
            &i.action,
            &i.body,
            &i.carrier,
            &i.tracking_number,
            &i.tracking_url,
        ])
        .map_err(|_| OrderError::Invalid)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::OrdersWrite).await?;
        sqlx::query("SELECT id FROM orders WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(OrderError::NotFound)?;
        let order = load(&mut tx, id).await?;
        let prior: Option<String> = sqlx::query_scalar(
            "SELECT payload FROM order_fulfillment_events WHERE order_id=$1 AND request_key=$2",
        )
        .bind(id)
        .bind(&key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(prior) = prior {
            if prior != payload {
                return Err(OrderError::FulfillmentConflict);
            }
            tx.commit().await?;
            return Ok(order);
        }
        if order.fulfillment.revision != revision {
            return Err(CartError::Stale.into());
        }
        let current = &order.fulfillment;
        let mut next = current.clone();
        match i.action.as_str() {
            "note" => {}
            "preparing"
                if order.state == "open" && !order.is_refunded() && current.state == "pending" =>
            {
                next.state = "preparing".into()
            }
            "shipped" if order.state == "open" && current.state == "preparing" => {
                let refunding: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 \
                            FROM payment_attempts a \
                            JOIN order_online_payments p ON p.attempt_id=a.id \
                            WHERE p.order_id=$1 \
                            AND (a.refund_reason IS NOT NULL \
                                OR a.state='refund_pending'))",
                )
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
                if order.is_refunded()
                    || order.payment_review_blocks_confirmation()
                    || refunding
                    || (!order.is_paid()
                        && order.total_cents != 0
                        && order.frozen.payment.code != "cash_on_delivery")
                {
                    return Err(OrderError::FulfillmentPayment);
                }
                next.state = "shipped".into();
                next.carrier = i.carrier.clone();
                next.tracking_number = i.tracking_number.clone();
                next.tracking_url = i.tracking_url.clone();
            }
            "delivered" if order.state == "open" && current.state == "shipped" => {
                next.state = "delivered".into()
            }
            "tracking" if order.state == "open" && current.sent() => {
                next.carrier = i.carrier.clone();
                next.tracking_number = i.tracking_number.clone();
                next.tracking_url = i.tracking_url.clone();
            }
            _ => return Err(OrderError::FulfillmentTransition),
        }
        // An unchanged request has no business effect and does not consume its key.
        // Revision is checked above: a later retry cannot overwrite intervening work.
        if i.action == "tracking" && !current.tracking_changed(&i) {
            tx.commit().await?;
            return Ok(order);
        }
        sqlx::query("INSERT INTO order_fulfillment(order_id) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE order_fulfillment \
            SET state=$2,revision=revision+1,carrier=$3,tracking_number=$4,tracking_url=$5 \
            WHERE order_id=$1",
        )
        .bind(id)
        .bind(&next.state)
        .bind(&next.carrier)
        .bind(&next.tracking_number)
        .bind(&next.tracking_url)
        .execute(&mut *tx)
        .await?;
        let actor = s.require(Permission::OrdersWrite)?;
        sqlx::query(
            "INSERT INTO order_fulfillment_events(order_id,request_key,payload,kind,body,carrier,\
                    tracking_number,tracking_url,administrator_id,administrator_name) \
                SELECT $1,$2,$3,$4,$5,$6,$7,$8,id,display_name \
                FROM administrators \
                WHERE id=$9",
        )
        .bind(id)
        .bind(&key)
        .bind(payload)
        .bind(&i.action)
        .bind(&i.body)
        .bind(&next.carrier)
        .bind(&next.tracking_number)
        .bind(&next.tracking_url)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        let order = load(&mut tx, id).await?;
        let event = match i.action.as_str() {
            "shipped" => Some(Event::Shipped),
            "delivered" => Some(Event::Delivered),
            "tracking" => Some(Event::Tracking),
            _ => None,
        };
        if let Some(event) = event {
            enqueue(&mut tx, &order, event).await?
        }
        tx.commit().await?;
        Ok(order)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tracking_and_notes_are_bounded_plain_text() {
        let mut i = FulfillmentInput {
            action: "shipped".into(),
            carrier: "Carrier".into(),
            tracking_number: "T1".into(),
            tracking_url: "https://example.test/track?q=T1".into(),
            ..Default::default()
        };
        assert!(validated(&i).is_ok());
        for bad in [
            "javascript:alert(1)",
            "http://example.test",
            "https://user:secret@example.test",
            "https://exam\nple.test/",
        ] {
            i.tracking_url = bad.into();
            assert!(validated(&i).is_err(), "{bad}")
        }
        i.action = "note".into();
        i.body = "Private <script>\ntext".into();
        assert!(validated(&i).is_ok());
        i.body = "x".repeat(2001);
        assert!(validated(&i).is_err());
    }
}
