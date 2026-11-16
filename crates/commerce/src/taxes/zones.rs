use super::*;
use talocart_platform::geography;

#[derive(Clone, Default)]
pub struct ZoneValues {
    pub name: String,
    pub description: String,
    pub rules: std::collections::BTreeMap<i64, i64>,
    /// Comma/space separated ISO codes, suitable for native forms without JavaScript.
    pub countries: String,
    pub active: bool,
}
#[derive(Clone, FromRow)]
pub struct Zone {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub rule_classes: Vec<i64>,
    pub rule_rates: Vec<i64>,
    pub active: bool,
    pub countries: String,
}
impl Zone {
    pub fn values(&self) -> ZoneValues {
        ZoneValues {
            name: self.name.clone(),
            description: self.description.clone(),
            rules: self
                .rule_classes
                .iter()
                .copied()
                .zip(self.rule_rates.iter().copied())
                .collect(),
            countries: self.countries.clone(),
            active: self.active,
        }
    }
}
const SELECT: &str = "SELECT z.*,ARRAY(SELECT class_id \
        FROM tax_zone_rules \
        WHERE zone_id=z.id \
        ORDER BY class_id) AS rule_classes,ARRAY(SELECT rate_id \
        FROM tax_zone_rules \
        WHERE zone_id=z.id \
        ORDER BY class_id) AS rule_rates,coalesce((SELECT string_agg(country_code,', ' \
                ORDER BY country_code) \
            FROM tax_zone_countries \
            WHERE zone_id=z.id),'') AS countries \
    FROM tax_zones z";
fn countries(value: &str) -> Result<Vec<String>> {
    if value.len() > 2000 {
        return Err(TaxError::Invalid(vec![Field::Countries]));
    }
    let mut codes = std::collections::BTreeSet::new();
    for code in value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|v| !v.is_empty())
    {
        let code = code.to_ascii_uppercase();
        if !geography::country_valid(&code) || !codes.insert(code) {
            return Err(TaxError::Invalid(vec![Field::Countries]));
        }
    }
    if codes.is_empty() {
        return Err(TaxError::Invalid(vec![Field::Countries]));
    }
    Ok(codes.into_iter().collect())
}
impl Taxes {
    pub async fn zones(
        &self,
        session: &Session,
        q: &str,
        active: Option<bool>,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<Zone>> {
        self.identity
            .authorize(session, Permission::TaxesRead)
            .await?;
        if q.chars().count() > 80 || q.chars().any(char::is_control) || !(1..=10000).contains(&page)
        {
            return Err(TaxError::Invalid(vec![]));
        }
        let sql = format!(
            "{SELECT} WHERE strpos(lower(z.name),lower($1))>0 AND ($2::boolean IS NULL OR z.active=$2)"
        );
        Ok(talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(q.trim())
                .bind(active),
            "lower(z.name),z.id",
            page,
            PAGE_SIZE as u16,
        )
        .await?)
    }
    pub async fn zone(&self, session: &Session, id: i64) -> Result<Zone> {
        self.identity
            .authorize(session, Permission::TaxesRead)
            .await?;
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE z.id=$1")))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(TaxError::NotFound)
    }
    pub async fn save_zone(
        &self,
        session: &Session,
        csrf: &str,
        id: Option<i64>,
        input: &ZoneValues,
        revision: i64,
    ) -> Result<i64> {
        session.check_csrf(csrf)?;
        session.require(Permission::TaxesWrite)?;
        let valid = validate(
            &Values {
                name: input.name.clone(),
                description: input.description.clone(),
                percentage: "0".into(),
                ..Values::default()
            },
            Kind::Rate,
        )?;
        let countries = countries(&input.countries)?;
        if input.rules.len() > 100 || input.rules.iter().any(|(c, r)| *c <= 0 || *r <= 0) {
            return Err(TaxError::Invalid(vec![Field::Class]));
        }
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::TaxesWrite).await?;
        check_revision(&mut tx, revision).await?;
        if let Some(id) = id {
            sqlx::query("DELETE FROM tax_zone_rules WHERE zone_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let sql = if id.is_some() {
            "UPDATE tax_zones SET name=$1,description=$2,active=$3 WHERE id=$4 RETURNING id"
        } else {
            "INSERT INTO tax_zones(name,description,active) VALUES($1,$2,$3) RETURNING id"
        };
        let mut query = sqlx::query_scalar::<_, i64>(sql)
            .bind(valid.name)
            .bind(valid.description)
            .bind(input.active);
        if let Some(id) = id {
            query = query.bind(id);
        }
        let id = query
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(TaxError::NotFound)?;
        sqlx::query("DELETE FROM tax_zone_countries WHERE zone_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO tax_zone_countries(zone_id,country_code) SELECT $1,unnest($2::text[])",
        )
        .bind(id)
        .bind(countries)
        .execute(&mut *tx)
        .await?;
        let classes: Vec<_> = input.rules.keys().copied().collect();
        let rates: Vec<_> = input.rules.values().copied().collect();
        sqlx::query(
            "INSERT INTO tax_zone_rules(zone_id,class_id,rate_id) \
            SELECT $1,* \
            FROM unnest($2::bigint[],$3::bigint[])",
        )
        .bind(id)
        .bind(classes)
        .bind(rates)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn delete_zone(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::TaxesWrite).await?;
        check_revision(&mut tx, revision).await?;
        let result = sqlx::query("DELETE FROM tax_zones WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() != 1 {
            return Err(TaxError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn country_groups_validate_real_codes_and_duplicates() {
        assert_eq!(countries("it, FR\nDE").unwrap(), ["DE", "FR", "IT"]);
        for input in ["", "ZZ", "ITA", "IT, it", "UK", "EU", "IT;FR"] {
            assert!(countries(input).is_err(), "{input}");
        }
        assert!(countries(&"IT ".repeat(1000)).is_err());
    }
}
