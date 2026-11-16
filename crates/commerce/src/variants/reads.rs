//! Read projections owned by Variants.
use super::*;

impl Variants {
    /// Complete choices for the product selectors. Administrative lists use `options_page`.
    pub async fn options(&self, s: &Session) -> Result<Vec<OptionEntry>> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT id,name,sort_order FROM [[options]] ORDER BY sort_order,lower(name),id",
            self.language,
        )))
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn option(&self, s: &Session, id: i64) -> Result<OptionEntry> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT id,name,sort_order FROM [[options]] WHERE id=$1",
            self.language,
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(VariantError::NotFound)
    }
    /// Complete values for callers that need every choice, without a catalog cap.
    pub async fn option_values(&self, s: &Session, id: i64) -> Result<Vec<ValueEntry>> {
        self.option(s, id).await?;
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT id,option_id,name,sort_order \
            FROM [[option_values]] \
            WHERE option_id=$1 \
            ORDER BY sort_order,lower(name),id",
            self.language,
        )))
        .bind(id)
        .fetch_all(&self.pool)
        .await?)
    }
    /// Counted option list; total catalog size remains unrestricted.
    pub async fn options_counted(
        &self,
        s: &Session,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<OptionEntry>> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        page_offset(page)?;
        Ok(talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(sqlx::AssertSqlSafe(crate::content::query(
                "SELECT id,name,sort_order FROM [[options]]",
                self.language,
            ))),
            "sort_order,lower(name),id",
            page,
            20,
        )
        .await?)
    }

    pub async fn values_counted(
        &self,
        s: &Session,
        option: i64,
        page: i64,
        search: &ValueSearch,
    ) -> Result<talocart_platform::pagination::Page<ValueEntry>> {
        self.option(s, option).await?;
        value_page_offset(page)?;
        Ok(talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(sqlx::AssertSqlSafe(crate::content::query(
                format!(
                    "SELECT v.id,v.option_id,v.name,v.sort_order FROM [[option_values]] v WHERE v.option_id=$1 AND {}",
                    value_filter("v", 2, 3)
                ),
                self.language,
            )))
            .bind(option)
            .bind(search.text())
            .bind(&search.locales),
            "sort_order,lower(name),id",
            page,
            VALUE_PAGE_SIZE,
        )
        .await?)
    }

    /// Locate the error target and read its page from one snapshot, so a
    /// concurrent reorder or rename cannot separate deletion feedback from its value.
    /// On success, clears `search` only if the target still exists outside it;
    /// missing targets retain the filter. Position, count and rows share a snapshot.
    pub async fn values_counted_at_value(
        &self,
        s: &Session,
        option: i64,
        value: i64,
        fallback_page: i64,
        search: &mut ValueSearch,
    ) -> Result<talocart_platform::pagination::Page<ValueEntry>> {
        self.option(s, option).await?;
        value_page_offset(fallback_page)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let mut effective_search = search.clone();
        let mut located: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(
            crate::content::query(value_page_query(), self.language),
        ))
        .bind(option)
        .bind(value)
        .bind(i64::from(VALUE_PAGE_SIZE))
        .bind(search.text())
        .bind(&search.locales)
        .fetch_optional(&mut *tx)
        .await?;
        if located.is_none() && !search.is_empty() {
            let all = ValueSearch::default();
            let unfiltered: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(
                crate::content::query(value_page_query(), self.language),
            ))
            .bind(option)
            .bind(value)
            .bind(i64::from(VALUE_PAGE_SIZE))
            .bind(all.text())
            .bind(&all.locales)
            .fetch_optional(&mut *tx)
            .await?;
            if unfiltered.is_some() {
                located = unfiltered;
                effective_search = all;
            }
        }
        let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
            format!(
                "SELECT count(*) FROM [[option_values]] v WHERE v.option_id=$1 AND {}",
                value_filter("v", 2, 3)
            ),
            self.language,
        )))
        .bind(option)
        .bind(effective_search.text())
        .bind(&effective_search.locales)
        .fetch_one(&mut *tx)
        .await?;
        let size = i64::from(VALUE_PAGE_SIZE);
        let pages = if total == 0 {
            1
        } else {
            (total - 1) / size + 1
        };
        let page = located.unwrap_or(fallback_page).min(pages);
        let entries = sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            format!(
                "SELECT v.id,v.option_id,v.name,v.sort_order \
                    FROM [[option_values]] v \
                    WHERE v.option_id=$1 \
                    AND {} \
                    ORDER BY v.sort_order,lower(v.name),v.id \
                    LIMIT $3 \
                    OFFSET $2",
                value_filter("v", 4, 5)
            ),
            self.language,
        )))
        .bind(option)
        .bind(value_page_offset(page)?)
        .bind(size)
        .bind(effective_search.text())
        .bind(&effective_search.locales)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        *search = effective_search;
        Ok(talocart_platform::pagination::Page {
            entries,
            total,
            page,
        })
    }

    pub async fn list_counted(
        &self,
        s: &Session,
        product: i64,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<Entry>> {
        self.list_filtered_counted(s, product, &AdminFilters::default(), page)
            .await
    }

    pub async fn list_filtered_counted(
        &self,
        s: &Session,
        product: i64,
        filters: &AdminFilters<'_>,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<Entry>> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        page_offset_sized(page, filters.page_size.value())?;
        let active = filters.validate()?;
        Ok(talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(sqlx::AssertSqlSafe(crate::content::query(
                format!(
                    "SELECT * \
                        FROM ({SELECT} \
                            WHERE v.product_id=$1) variant_rows \
                        WHERE ($2='' \
                            OR strpos(lower(label),lower($2))>0) \
                        AND ($3='' \
                            OR strpos(lower(sku),lower($3))>0) \
                        AND ($4::boolean IS NULL \
                            OR active=$4)"
                ),
                self.language,
            )))
            .bind(product)
            .bind(filters.combination.trim())
            .bind(filters.sku.trim())
            .bind(active),
            "id",
            page,
            filters.page_size.value(),
        )
        .await?)
    }

    /// Twenty displayed rows plus one lookahead; retained sentinel API.
    pub async fn options_page(&self, s: &Session, page: i64) -> Result<Vec<OptionEntry>> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        let offset = page_offset(page)?;
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT id,name,sort_order FROM [[options]] ORDER BY sort_order,lower(name),id LIMIT 21 OFFSET $1",
            self.language,
        )))
        .bind(offset)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn option_values_page(
        &self,
        s: &Session,
        option: i64,
        page: i64,
    ) -> Result<Vec<ValueEntry>> {
        self.option(s, option).await?;
        let offset = value_page_offset(page)?;
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT id,option_id,name,sort_order \
            FROM [[option_values]] \
            WHERE option_id=$1 \
            ORDER BY sort_order,lower(name),id \
            LIMIT $3 \
            OFFSET $2",
            self.language,
        )))
        .bind(option)
        .bind(offset)
        .bind(i64::from(VALUE_PAGE_SIZE) + 1)
        .fetch_all(&self.pool)
        .await?)
    }
    /// Locate a saved or invalid value using its persisted ordering, without loading all values.
    /// `NotFound` also covers a value outside `search`; callers may retry unfiltered.
    pub async fn option_value_page(
        &self,
        s: &Session,
        option: i64,
        value: i64,
        search: &ValueSearch,
    ) -> Result<i64> {
        self.option(s, option).await?;
        sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
            value_page_query(),
            self.language,
        )))
        .bind(option)
        .bind(value)
        .bind(i64::from(VALUE_PAGE_SIZE))
        .bind(search.text())
        .bind(&search.locales)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(VariantError::NotFound)
    }
    pub(super) async fn axes(&self, product: i64, public_only: bool) -> Result<Vec<Axis>> {
        #[derive(FromRow)]
        struct AxisRow {
            position: i16,
            id: i64,
            name: String,
            value_id: Option<i64>,
            value_name: Option<String>,
            value_sort_order: Option<i32>,
        }
        let rows:Vec<AxisRow>=sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query("SELECT a.position,o.id,o.name,v.id AS value_id,v.name AS value_name,v.sort_order AS value_sort_order \
            FROM catalog_product_options a \
            JOIN [[options]] o ON o.id=a.option_id \
            LEFT \
            JOIN [[option_values]] v ON v.option_id=o.id \
            AND (NOT $2 \
                OR EXISTS (SELECT 1 \
                    FROM catalog_variant_values selected \
                    JOIN catalog_variants variant ON variant.id=selected.variant_id \
                    WHERE selected.value_id=v.id \
                    AND variant.product_id=$1 \
                    AND variant.active)) \
            WHERE a.product_id=$1 \
            ORDER BY CASE \
            WHEN $2 THEN o.sort_order \
            ELSE 0 END,a.position,v.sort_order,lower(v.name),v.id", self.language))).bind(product).bind(public_only).fetch_all(&self.pool).await?;
        let mut axes: Vec<Axis> = vec![];
        for row in rows {
            if axes.last().is_none_or(|a| a.id != row.id) {
                axes.push(Axis {
                    position: row.position,
                    id: row.id,
                    name: row.name,
                    values: vec![],
                });
            }
            if let (Some(id), Some(name), Some(axis)) =
                (row.value_id, row.value_name, axes.last_mut())
            {
                axis.values.push(ValueEntry {
                    id,
                    option_id: row.id,
                    name,
                    sort_order: row.value_sort_order.unwrap_or(0),
                });
            }
        }
        Ok(axes)
    }
    /// Product editors already loaded the variant count with their product.
    pub async fn configuration_axes(&self, s: &Session, product: i64) -> Result<Vec<Axis>> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        self.axes(product, false).await
    }
    pub async fn configuration(&self, s: &Session, product: i64) -> Result<Configuration> {
        let axes = self.configuration_axes(s, product).await?;
        let count = sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT count(*) FROM catalog_variants WHERE product_id=$1",
            self.language,
        )))
        .bind(product)
        .fetch_one(&self.pool)
        .await?;
        Ok(Configuration { axes, count })
    }
    pub async fn list(&self, s: &Session, product: i64, page: i64) -> Result<Vec<Entry>> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        let offset = page_offset(page)?;
        let sql = format!("{SELECT} WHERE v.product_id=$1 ORDER BY v.id LIMIT 21 OFFSET $2");
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(product)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn entry(&self, s: &Session, product: i64, id: i64) -> Result<Entry> {
        self.identity.authorize(s, Permission::ProductsRead).await?;
        let sql = format!("{SELECT} WHERE v.product_id=$1 AND v.id=$2");
        sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(product)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(VariantError::NotFound)
    }
    /// Only called after the product owner checked public visibility; chosen values resolve on the server.
    pub async fn public_selection(
        &self,
        product: i64,
        chosen: &[i64],
    ) -> Result<(Vec<Axis>, Option<Entry>)> {
        let axes = self.axes(product, true).await?;
        let chosen = &chosen[..axes.len().min(chosen.len())];
        let mut ids = chosen.to_vec();
        ids.sort_unstable();
        if axes.len() != ids.len()
            || !axes.iter().all(|axis| {
                axis.position
                    .checked_sub(1)
                    .and_then(|position| usize::try_from(position).ok())
                    .and_then(|index| chosen.get(index))
                    .is_some_and(|id| axis.values.iter().any(|value| value.id == *id))
            })
        {
            return Ok((axes, None));
        }
        let sql = format!(
            "{SELECT} \
                JOIN [[visible_products]] p ON p.id=v.product_id \
                WHERE v.product_id=$1 \
                AND v.value_ids=$2 \
                AND v.active"
        );
        let entry = sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(product)
        .bind(ids)
        .fetch_optional(&self.pool)
        .await?;
        Ok((axes, entry))
    }
}
