use super::*;
/// One row per ISO destination, aggregated in SQL; never one query per country/class.
#[derive(FromRow)]
pub struct Coverage {
    pub country: String,
    pub enabled: bool,
    pub zone_id: Option<i64>,
    pub zone_name: Option<String>,
    pub zone_active: bool,
    pub classes: i64,
    pub explicit: i64,
}
impl Taxes {
    pub async fn coverage(&self, session: &Session) -> Result<Vec<Coverage>> {
        self.identity
            .authorize(session, Permission::TaxesRead)
            .await?;
        Ok(sqlx::query_as("SELECT c.code AS country,p.country_code IS NOT NULL AS enabled,z.id AS zone_id,z.name AS zone_name,\
            coalesce(z.active,false) AS zone_active,(SELECT count(*) \
                FROM tax_classes cl \
                JOIN tax_rates r ON r.id=cl.rate_id \
                WHERE cl.active \
                AND r.active) AS classes,CASE \
            WHEN z.active THEN (SELECT count(*) \
                FROM tax_zone_rules q \
                JOIN tax_classes cl ON cl.id=q.class_id \
                AND cl.active \
                JOIN tax_rates r ON r.id=q.rate_id \
                AND r.active \
                WHERE q.zone_id=z.id) \
            ELSE 0 END AS explicit \
            FROM countries c \
            LEFT \
            JOIN checkout_countries p ON p.country_code=c.code \
            LEFT \
            JOIN tax_zone_countries tc ON tc.country_code=c.code \
            LEFT \
            JOIN tax_zones z ON z.id=tc.zone_id \
            ORDER BY c.code \
            LIMIT 249").fetch_all(&self.pool).await?)
    }
}
