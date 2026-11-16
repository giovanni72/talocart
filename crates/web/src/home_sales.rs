//! Native dashboard filters and accessible SVG presentation; no client-side totals.
use super::*;
use std::collections::BTreeMap;
use talocart_commerce::overview::{SalesHistory, SalesPeriod};

#[derive(Template)]
#[template(path = "admin-sales.html")]
pub(super) struct SalesFragment {
    pub sales: Option<SalesView>,
}

#[derive(Default)]
pub(super) struct SalesQuery {
    pub metric: String,
    pub period: SalesPeriod,
    pub statuses: Option<Vec<i64>>,
}
impl SalesQuery {
    pub fn parse(
        pairs: Vec<(String, String)>,
    ) -> Result<Self, talocart_platform::identity::IdentityError> {
        let mut query = Self {
            metric: "amount".into(),
            ..Self::default()
        };
        let mut seen = std::collections::HashSet::new();
        for (key, value) in pairs {
            if key != "status" && !seen.insert(key.clone()) {
                return Err(talocart_platform::identity::IdentityError::Invalid);
            }
            match key.as_str() {
                "metric" if matches!(value.as_str(), "amount" | "orders") => query.metric = value,
                "period" => {
                    query.period = match value.as_str() {
                        "7d" => SalesPeriod::Week,
                        "30d" => SalesPeriod::Month,
                        "90d" => SalesPeriod::Quarter,
                        "12m" => SalesPeriod::Year,
                        _ => return Err(talocart_platform::identity::IdentityError::Invalid),
                    }
                }
                "filtered" if value == "1" => {
                    query.statuses.get_or_insert_with(Vec::new);
                }
                "status" => {
                    let id = value
                        .parse::<i64>()
                        .map_err(|_| talocart_platform::identity::IdentityError::Invalid)?;
                    let statuses = query.statuses.get_or_insert_with(Vec::new);
                    if id <= 0 || statuses.len() >= 1000 {
                        return Err(talocart_platform::identity::IdentityError::Invalid);
                    }
                    statuses.push(id);
                }
                _ => return Err(talocart_platform::identity::IdentityError::Invalid),
            }
        }
        Ok(query)
    }
}
pub(super) struct SalesStatus {
    pub id: i64,
    pub name: String,
    pub selected: bool,
}
pub(super) struct SalesPoint {
    pub date: String,
    pub value: String,
    pub x: i64,
    pub y: i64,
}
pub(super) struct SalesChart {
    pub currency: String,
    pub total: String,
    /// Scale top and its exact half, matching the upper and middle grid lines.
    pub maximum: String,
    pub middle: String,
    pub path: String,
    /// Closed line path filled down to the zero grid line.
    pub area: String,
    pub points: Vec<SalesPoint>,
}
pub(super) struct SalesView {
    pub metric: String,
    pub period: &'static str,
    pub statuses: Vec<SalesStatus>,
    pub selected_count: usize,
    pub charts: Vec<SalesChart>,
    pub empty: bool,
}
fn display(value: i64, amount: bool) -> String {
    if amount {
        format!(
            "{}{}{:02}",
            value / 100,
            if i18n::locale() == "it" { ',' } else { '.' },
            value % 100
        )
    } else {
        value.to_string()
    }
}
impl SalesView {
    fn new(
        query: SalesQuery,
        history: SalesHistory,
    ) -> Result<Self, talocart_platform::identity::IdentityError> {
        let amount = query.metric == "amount";
        let selected_count = history.selected.len();
        let statuses = history
            .statuses
            .into_iter()
            .map(|s| SalesStatus {
                id: s.id,
                name: s.name(i18n::locale()).to_owned(),
                selected: history.selected.contains(&s.id),
            })
            .collect();
        let empty = history.buckets.iter().all(|b| b.orders == 0);
        let mut groups: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
        for bucket in history.buckets {
            let dates = groups
                .entry(if amount {
                    bucket.currency
                } else {
                    String::new()
                })
                .or_default();
            let value = dates.entry(bucket.date).or_default();
            *value = value
                .checked_add(if amount { bucket.cents } else { bucket.orders })
                .ok_or(talocart_platform::identity::IdentityError::Unavailable)?;
        }
        let mut charts = Vec::new();
        for (currency, dates) in groups {
            let max = dates.values().copied().max().unwrap_or(0);
            // An even scale keeps the middle grid line on an exact value.
            let top = max.max(1);
            let top = top
                .checked_add(top % 2)
                .ok_or(talocart_platform::identity::IdentityError::Unavailable)?;
            let total = dates
                .values()
                .try_fold(0i64, |sum, value| sum.checked_add(*value))
                .ok_or(talocart_platform::identity::IdentityError::Unavailable)?;
            let steps = i64::try_from(dates.len().saturating_sub(1))
                .unwrap_or(1)
                .max(1);
            let mut points = Vec::new();
            let mut path = String::new();
            for (index, (date, value)) in dates.into_iter().enumerate() {
                let x = 20 + i64::try_from(index).unwrap_or(0) * 960 / steps;
                // i128 keeps integer money exact even at the bigint boundary.
                let height = i128::from(value) * 220 / i128::from(top);
                let y = 240
                    - i64::try_from(height)
                        .map_err(|_| talocart_platform::identity::IdentityError::Unavailable)?;
                path.push_str(&format!("{} {x},{y} ", if index == 0 { "M" } else { "L" }));
                points.push(SalesPoint {
                    date,
                    value: display(value, amount),
                    x,
                    y,
                });
            }
            let area = match (points.first(), points.last()) {
                (Some(first), Some(last)) => format!("{path}L {},240 L {},240 Z", last.x, first.x),
                _ => String::new(),
            };
            charts.push(SalesChart {
                currency,
                total: display(total, amount),
                maximum: display(top, amount),
                middle: display(top / 2, amount),
                path,
                area,
                points,
            });
        }
        Ok(Self {
            metric: query.metric,
            period: query.period.key(),
            statuses,
            selected_count,
            charts,
            empty,
        })
    }
}
pub(super) async fn load(
    state: &AppState,
    session: &Session,
    query: SalesQuery,
) -> Result<Option<SalesView>, talocart_platform::identity::IdentityError> {
    if !session.allows(Permission::OrdersRead) {
        return Ok(None);
    }
    let history = talocart_commerce::overview::sales_history(
        &state.pool,
        &state.identity,
        session,
        query.period,
        query.statuses.clone(),
    )
    .await?;
    Ok(Some(SalesView::new(query, history)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_distinguish_default_and_empty_and_reject_invalid_values() {
        assert!(SalesQuery::parse(vec![]).unwrap().statuses.is_none());
        let pairs = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            SalesQuery::parse(pairs(&[("filtered", "1")]))
                .unwrap()
                .statuses,
            Some(vec![])
        );
        assert_eq!(
            SalesQuery::parse(pairs(&[
                ("status", "2"),
                ("status", "3"),
                ("filtered", "1")
            ]))
            .unwrap()
            .statuses,
            Some(vec![2, 3])
        );
        for bad in [
            vec![("metric", "bad")],
            vec![("period", "1d")],
            vec![("status", "0")],
            vec![("status", "bad")],
            vec![("period", "7d"), ("period", "30d")],
        ] {
            assert!(SalesQuery::parse(pairs(&bad)).is_err());
        }
    }
    #[test]
    fn chart_scale_is_even_so_grid_labels_are_exact_and_area_closes_on_zero() {
        let bucket = |date: &str, orders| talocart_commerce::overview::SalesBucket {
            date: date.into(),
            currency: "EUR".into(),
            orders,
            cents: 0,
        };
        let history = SalesHistory {
            statuses: vec![],
            selected: vec![],
            buckets: vec![bucket("2026-10-01", 3), bucket("2026-10-02", 0)],
        };
        let query = SalesQuery {
            metric: "orders".into(),
            ..SalesQuery::default()
        };
        let view = SalesView::new(query, history).expect("chart");
        let chart = &view.charts[0];
        assert_eq!((chart.maximum.as_str(), chart.middle.as_str()), ("4", "2"));
        assert_eq!(chart.points[0].y, 75);
        assert_eq!(chart.path, "M 20,75 L 980,240 ");
        assert_eq!(chart.area, "M 20,75 L 980,240 L 980,240 L 20,240 Z");
    }
}
