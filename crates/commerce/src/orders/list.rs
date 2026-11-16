//! Bounded administrative reads. Filters use bound values and ordering only static SQL.
use super::*;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListFilters {
    pub reference: String,
    pub date: String,
    pub customer: String,
    pub state: String,
    pub payment: String,
    pub fulfillment: String,
    pub total: String,
    pub total_op: String,
    pub sort: String,
    pub dir: String,
    pub per_page: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListSort {
    Reference,
    Created,
    Customer,
    State,
    Payment,
    Fulfillment,
    Total,
}
impl ListSort {
    pub fn key(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Created => "created",
            Self::Customer => "customer",
            Self::State => "state",
            Self::Payment => "payment",
            Self::Fulfillment => "fulfillment",
            Self::Total => "total",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ListOrder {
    pub column: ListSort,
    pub descending: bool,
}
impl ListOrder {
    pub fn parse(sort: &str, dir: &str) -> Result<Self> {
        let column = match sort {
            "reference" => ListSort::Reference,
            "" | "created" => ListSort::Created,
            "customer" => ListSort::Customer,
            "state" => ListSort::State,
            "payment" => ListSort::Payment,
            "fulfillment" => ListSort::Fulfillment,
            "total" => ListSort::Total,
            _ => return Err(OrderError::ListFilters),
        };
        let descending = match dir {
            "" => true,
            "asc" => false,
            "desc" => true,
            _ => return Err(OrderError::ListFilters),
        };
        Ok(Self { column, descending })
    }
    pub fn direction(self) -> &'static str {
        if self.descending { "desc" } else { "asc" }
    }
    // Every fragment is static; numeric annual references sort 2 before 10.
    // Nonstandard references fall back to their literal value, then ID.
    fn sql(self) -> &'static str {
        match (self.column, self.descending) {
            (ListSort::Reference, false) => {
                "reference_year ASC NULLS LAST,reference_number ASC NULLS LAST,lower(reference) ASC,id ASC"
            }
            (ListSort::Reference, true) => {
                "reference_year DESC NULLS LAST,reference_number DESC NULLS LAST,lower(reference) DESC,id DESC"
            }
            (ListSort::Created, false) => "issued_at ASC,id ASC",
            (ListSort::Created, true) => "issued_at DESC,id DESC",
            (ListSort::Customer, false) => "customer_sort ASC,customer_email_sort ASC,id ASC",
            (ListSort::Customer, true) => "customer_sort DESC,customer_email_sort DESC,id DESC",
            (ListSort::State, false) => "state_rank ASC,id ASC",
            (ListSort::State, true) => "state_rank DESC,id DESC",
            (ListSort::Payment, false) => "payment_rank ASC,id ASC",
            (ListSort::Payment, true) => "payment_rank DESC,id DESC",
            (ListSort::Fulfillment, false) => "fulfillment_rank ASC,id ASC",
            (ListSort::Fulfillment, true) => "fulfillment_rank DESC,id DESC",
            (ListSort::Total, false) => "total_cents ASC,id ASC",
            (ListSort::Total, true) => "total_cents DESC,id DESC",
        }
    }
}
use talocart_platform::regional::valid_date;

impl ListFilters {
    fn validate(
        &self,
    ) -> Result<(
        ListOrder,
        Option<i64>,
        talocart_platform::pagination::AdminPageSize,
    )> {
        if [&self.reference, &self.customer]
            .iter()
            .any(|v| v.chars().count() > 120 || v.chars().any(char::is_control))
            || (!self.date.is_empty() && !valid_date(&self.date))
            || (!["", "open", "cancelled"].contains(&self.state.as_str())
                && !self.state.parse::<i64>().is_ok_and(|v| v > 0))
            || !["", "unpaid", "paid", "refunded"].contains(&self.payment.as_str())
            || ![
                "",
                "pending",
                "preparing",
                "shipped",
                "delivered",
                "unshipped",
            ]
            .contains(&self.fulfillment.as_str())
            || !["", "eq", "gt", "lt"].contains(&self.total_op.as_str())
        {
            return Err(OrderError::ListFilters);
        }
        let cents = if self.total.trim().is_empty() {
            None
        } else {
            if self.total.len() > 24 {
                return Err(OrderError::ListFilters);
            }
            Some(
                Money::parse(&self.total)
                    .map_err(|_| OrderError::ListFilters)?
                    .cents(),
            )
        };
        Ok((
            ListOrder::parse(&self.sort, &self.dir)?,
            cents,
            self.page_size()?,
        ))
    }
    /// Field identification is used only when rendering rejected input.
    pub fn invalid_total(&self) -> bool {
        !self.total.trim().is_empty()
            && (self.total.len() > 24 || Money::parse(&self.total).is_err())
    }
    pub fn invalid_date(&self) -> bool {
        !self.date.is_empty() && !valid_date(&self.date)
    }
    pub fn page_size(&self) -> Result<talocart_platform::pagination::AdminPageSize> {
        talocart_platform::pagination::AdminPageSize::parse(&self.per_page)
            .map_err(|_| OrderError::ListFilters)
    }
    pub fn active(&self) -> bool {
        [
            &self.reference,
            &self.date,
            &self.customer,
            &self.state,
            &self.payment,
            &self.fulfillment,
            &self.total,
        ]
        .iter()
        .any(|v| !v.trim().is_empty())
    }
}
/// Validated presentation metadata travels with the counted result.
pub struct ListPage {
    pub entries: Vec<Summary>,
    pub total: i64,
    pub page: i64,
    pub order: ListOrder,
    pub page_size: talocart_platform::pagination::AdminPageSize,
}
impl Orders {
    pub async fn filtered_list(
        &self,
        session: &Session,
        filters: &ListFilters,
        page: i64,
    ) -> Result<ListPage> {
        self.identity
            .authorize(session, Permission::OrdersRead)
            .await?;
        let (order, cents, size) = filters.validate()?;
        // Date boundaries are local midnights converted separately, including DST days.
        // The shared formatter omits the zone label while preserving local conversion.
        let sql = "SELECT s.name_it AS status_it,s.name_en AS status_en,s.color AS status_color,o.id,o.reference,o.currency,o.total_cents,o.state,o.created_at AS issued_at,
            platform_datetime(o.created_at,r.time_zone,r.date_format) AS created_at,
            trim(coalesce(c.first_name,'')||' '||coalesce(c.last_name,'')) AS customer_name,coalesce(c.email,'') AS customer_email,
            lower(trim(coalesce(c.first_name,'')||' '||coalesce(c.last_name,''))) AS customer_sort,lower(coalesce(c.email,'')) AS customer_email_sort,
            o.payment_state='paid' AS paid,o.payment_state='refunded' AS refunded,
            CASE WHEN o.state='cancelled' THEN 'unshipped' ELSE coalesce(f.state,'pending') END AS fulfillment_state,
            s.sort_order AS state_rank,
            CASE o.payment_state WHEN 'unpaid' THEN 0 WHEN 'paid' THEN 1 ELSE 2 END AS payment_rank,
            CASE WHEN o.state='cancelled' THEN 4 ELSE CASE coalesce(f.state,'pending') WHEN 'pending' THEN 0 WHEN 'preparing' THEN 1 WHEN 'shipped' THEN 2 ELSE 3 END END AS fulfillment_rank,
            substring(o.reference from '^ORD-([0-9]{4})-[0-9]+$')::integer AS reference_year,
            substring(o.reference from '^ORD-[0-9]{4}-([0-9]+)$')::numeric AS reference_number
            FROM orders o JOIN order_current_status cs ON cs.order_id=o.id JOIN order_statuses s ON s.id=cs.status_id LEFT JOIN order_customer_summaries c ON c.order_id=o.id
            LEFT JOIN order_fulfillment f ON f.order_id=o.id CROSS JOIN platform_regional_settings r
            WHERE r.singleton AND strpos(lower(o.reference),lower($1))>0
            AND ($2::text IS NULL OR (o.created_at >= ($2::date::timestamp AT TIME ZONE r.time_zone) AND o.created_at < (($2::date+1)::timestamp AT TIME ZONE r.time_zone)))
            AND (strpos(lower(trim(coalesce(c.first_name,'')||' '||coalesce(c.last_name,''))),lower($3))>0 OR strpos(lower(coalesce(c.email,'')),lower($3))>0)
            AND ($4='' OR o.state=$4 OR cs.status_id::text=$4) AND ($5='' OR o.payment_state=$5)
            AND ($6='' OR (CASE WHEN o.state='cancelled' THEN 'unshipped' ELSE coalesce(f.state,'pending') END)=$6)
            AND ($7::bigint IS NULL OR CASE $8 WHEN 'gt' THEN o.total_cents>$7 WHEN 'lt' THEN o.total_cents<$7 ELSE o.total_cents=$7 END)";
        let page = talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(sql)
                .bind(filters.reference.trim())
                .bind((!filters.date.is_empty()).then_some(filters.date.as_str()))
                .bind(filters.customer.trim())
                .bind(&filters.state)
                .bind(&filters.payment)
                .bind(&filters.fulfillment)
                .bind(cents)
                .bind(&filters.total_op),
            order.sql(),
            page,
            size.value(),
        )
        .await?;
        Ok(ListPage {
            entries: page.entries,
            total: page.total,
            page: page.page,
            order,
            page_size: size,
        })
    }
}

pub(super) async fn insert_customer(
    tx: &mut Transaction<'_, Postgres>,
    order: i64,
    customer: &Details,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO order_customer_summaries(order_id,first_name,last_name,email) \
        VALUES($1,$2,$3,$4) \
        ON CONFLICT(order_id) DO NOTHING",
    )
    .bind(order)
    .bind(&customer.first_name)
    .bind(&customer.last_name)
    .bind(&customer.email)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
/// Read-only startup check owned by the order module.
pub async fn check_list_customers(pool: &PgPool) -> Result<()> {
    let missing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 \
            FROM orders o \
            WHERE NOT EXISTS(SELECT 1 \
                FROM order_customer_summaries c \
                WHERE c.order_id=o.id))",
    )
    .fetch_one(pool)
    .await?;
    if missing {
        Err(OrderError::ListProjection)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn list_inputs_reject_invalid_dates_money_and_untrusted_sql() {
        for date in ["2024-02-29", "2026-10-18", "0001-01-01", "9999-12-31"] {
            assert!(valid_date(date));
        }
        for date in [
            "2026-02-29",
            "1900-02-29",
            "2026-04-31",
            "0000-01-01",
            "2026-00-01",
            "2026-10-00",
            "2026-1-1",
            "💥💥💥",
            "2026-10-18' OR true",
        ] {
            assert!(!valid_date(date), "{date}");
        }
        for (field, value) in [
            ("sort", "id;DROP TABLE orders"),
            ("dir", "DESC NULLS FIRST"),
            ("state", "other"),
            ("payment", "other"),
            ("fulfillment", "other"),
            ("total", "1.001"),
            ("total", "-1"),
            ("total", "92233720368547758.08"),
            ("total_op", "OR true"),
            ("customer", "line\nbreak"),
        ] {
            let mut f = ListFilters::default();
            match field {
                "sort" => f.sort = value.into(),
                "dir" => f.dir = value.into(),
                "state" => f.state = value.into(),
                "payment" => f.payment = value.into(),
                "fulfillment" => f.fulfillment = value.into(),
                "total" => f.total = value.into(),
                "total_op" => f.total_op = value.into(),
                _ => f.customer = value.into(),
            }
            assert!(
                matches!(f.validate(), Err(OrderError::ListFilters)),
                "{field}: {value}"
            );
        }
        let f = ListFilters {
            total: "13,50".into(),
            ..Default::default()
        };
        assert_eq!(f.validate().unwrap().1, Some(1350));
        for value in ["0", "1", "999", "20;DROP"] {
            assert!(
                ListFilters {
                    per_page: value.into(),
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        for value in ["10", "20", "50", "100"] {
            assert_eq!(
                ListFilters {
                    per_page: value.into(),
                    ..Default::default()
                }
                .page_size()
                .unwrap()
                .value()
                .to_string(),
                value
            );
        }
        assert!(!ListFilters::default().active());
        assert!(f.active());
    }
}
