//! Read projections owned by Products.
use super::*;

impl Products {
    pub async fn list(
        &self,
        session: &Session,
        q: &str,
        status: &str,
        page: i64,
    ) -> Result<Vec<Entry>> {
        self.admin_filtered(
            session,
            &AdminFilters {
                q,
                status,
                ..Default::default()
            },
            page,
        )
        .await
        .map(|result| result.entries)
    }
    pub async fn admin_filtered(
        &self,
        session: &Session,
        filters: &AdminFilters<'_>,
        page: i64,
    ) -> Result<ProductPage> {
        self.identity
            .authorize(session, Permission::ProductsRead)
            .await?;
        filters.validate(page)?;
        // Count and rows share a short, read-only snapshot, including concurrent catalog edits.
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let (price, stock) = filters.numeric()?;
        let base_predicate = "(strpos(lower(p.name),lower($1))>0 \
                OR strpos(lower(p.model),lower($1))>0 \
                OR strpos(p.slug,lower($1))>0) \
            AND ($2='' \
                OR p.status=$2) \
            AND ($3='' \
                OR EXISTS(SELECT 1 \
                    FROM catalog_product_options a \
                    WHERE a.product_id=p.id)=($3='variants')) \
            AND strpos(lower(p.model),lower($4))>0 \
            AND ($5::bigint IS NULL \
                OR p.category_id=$5) \
            AND ($6::bigint IS NULL \
                OR p.brand_id=$6) \
            AND ($12::bigint IS NULL \
                OR p.id=$12)";
        // Use the same active-variant price/stock definitions as sorting. All user values
        // remain bound; date matching uses the latest event and the configured local day.
        let effective_price = "CASE \
            WHEN EXISTS(SELECT 1 \
                FROM catalog_product_options o \
                WHERE o.product_id=p.id) THEN (SELECT min(v.price_cents) \
                FROM catalog_variants v \
                WHERE v.product_id=p.id \
                AND v.active) \
            ELSE p.price_cents END";
        let effective_stock = "CASE \
            WHEN EXISTS(SELECT 1 \
                FROM catalog_product_options o \
                WHERE o.product_id=p.id) THEN (SELECT sum(v.stock) \
                FROM catalog_variants v \
                WHERE v.product_id=p.id \
                AND v.active) \
            ELSE p.stock END";
        let changed_day = "(SELECT (e.created_at AT TIME ZONE r.time_zone)::date \
                FROM operation_events e \
                CROSS \
                JOIN platform_regional_settings r \
                WHERE r.singleton \
                AND e.resource IN ('catalog_products','catalog_products_translations') \
                AND e.resource_key=p.id::text \
                ORDER BY e.created_at DESC,e.id DESC \
                LIMIT 1)";
        let predicate = format!(
            "{base_predicate} \
                AND ($7::bigint IS NULL \
                    OR CASE $8 \
                    WHEN 'gt' THEN ({effective_price})>$7 \
                    WHEN 'lt' THEN ({effective_price})<$7 \
                    ELSE ({effective_price})=$7 END) \
                AND ($9::bigint IS NULL \
                    OR CASE $10 \
                    WHEN 'gt' THEN ({effective_stock})>$9 \
                    WHEN 'lt' THEN ({effective_stock})<$9 \
                    ELSE ({effective_stock})=$9 END) \
                AND ($11::text='' \
                    OR {changed_day}=NULLIF($11,'')::date)"
        );
        let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
            format!("SELECT count(*) FROM [[products]] p WHERE {predicate}"),
            self.language,
        )))
        .bind(filters.q.trim())
        .bind(filters.status)
        .bind(filters.kind)
        .bind(filters.model.trim())
        .bind(filters.category)
        .bind(filters.brand)
        .bind(price)
        .bind(filters.price_op)
        .bind(stock)
        .bind(filters.stock_op)
        .bind(filters.modified_date)
        .bind(filters.id)
        .fetch_one(&mut *tx)
        .await?;
        let size = i64::from(filters.page_size.value());
        let pages = if total == 0 {
            1
        } else {
            (total - 1) / size + 1
        };
        let page = page.min(pages);
        // Clamping to the counted last page keeps the offset below total, without overflow.
        let offset = (page - 1) * size;
        let column = filters.order.column.sql();
        let direction = filters.order.direction.sql();
        let sql = if filters.order.column == AdminSort::ModifiedAt {
            // The raw instant must participate before LIMIT/OFFSET. Formatting never sorts dates.
            format!(
                "SELECT admin_products.*,coalesce(platform_datetime(modified_instant,regional.time_zone,\
                            regional.date_format),'') AS modified_at \
                    FROM (SELECT base.*,changed.created_at AS modified_instant \
                        FROM ({SELECT} \
                            WHERE {predicate}) base \
                        LEFT \
                        JOIN LATERAL (SELECT created_at \
                            FROM operation_events \
                            WHERE resource IN ('catalog_products','catalog_products_translations') \
                            AND resource_key=base.id::text \
                            ORDER BY created_at DESC,id DESC \
                            LIMIT 1) changed ON true) admin_products \
                    CROSS \
                    JOIN platform_regional_settings regional \
                    WHERE regional.singleton \
                    ORDER BY {column} {direction} NULLS LAST,lower(name) ASC,id ASC \
                    LIMIT $14 \
                    OFFSET $13"
            )
        } else {
            // Other sorts retain the bounded audit lookup after pagination.
            let outer_column = column.replace("admin_products.", "page.");
            format!(
                "SELECT page.*,coalesce(platform_datetime(changed.created_at,regional.time_zone,regional.date_format),\
                        '') AS modified_at \
                    FROM (SELECT * \
                        FROM ({SELECT} \
                            WHERE {predicate}) admin_products \
                        ORDER BY {column} {direction} NULLS LAST,lower(name) ASC,id ASC \
                        LIMIT $14 \
                        OFFSET $13) page \
                    CROSS \
                    JOIN platform_regional_settings regional \
                    LEFT \
                    JOIN LATERAL (SELECT created_at \
                        FROM operation_events \
                        WHERE resource IN ('catalog_products','catalog_products_translations') \
                        AND resource_key=page.id::text \
                        ORDER BY created_at DESC,id DESC \
                        LIMIT 1) changed ON true \
                    WHERE regional.singleton \
                    ORDER BY {outer_column} {direction} NULLS LAST,lower(name) ASC,page.id ASC"
            )
        };
        let entries = sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(filters.q.trim())
        .bind(filters.status)
        .bind(filters.kind)
        .bind(filters.model.trim())
        .bind(filters.category)
        .bind(filters.brand)
        .bind(price)
        .bind(filters.price_op)
        .bind(stock)
        .bind(filters.stock_op)
        .bind(filters.modified_date)
        .bind(filters.id)
        .bind(offset)
        .bind(size)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ProductPage {
            entries,
            total,
            page,
            pages,
            first: if total == 0 { 0 } else { offset + 1 },
            last: offset + size.min(total - offset),
        })
    }
    pub async fn entry(&self, session: &Session, id: i64) -> Result<Entry> {
        self.identity
            .authorize(session, Permission::ProductsRead)
            .await?;
        let sql = format!("{SELECT} WHERE p.id=$1");
        sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ProductError::NotFound)
    }
    pub async fn editor_entry(&self, session: &Session, id: i64) -> Result<EditorEntry> {
        let product = self.entry(session, id).await?;
        let mut attributes =
            crate::attributes::Attributes::new(self.pool.clone(), self.identity.clone());
        if let Some(language) = self.language {
            attributes = attributes.localized(language);
        }
        let attributes = attributes.editor_values(product.id).await?;
        let variant_commerce = if product.has_variants {
            Some(sqlx::query_as::<_, VariantCommerceSummary>(
                "SELECT min(price_cents) AS minimum_price,max(price_cents) AS maximum_price,coalesce(sum(stock),0)::bigint AS stock FROM catalog_variants WHERE product_id=$1 AND active",
            ).bind(id).fetch_one(&self.pool).await?)
        } else {
            None
        };
        Ok(EditorEntry {
            product,
            attributes,
            variant_commerce,
        })
    }
    /// The homepage shares publication and alphabetical ordering with the catalog,
    /// but needs only eight cards and no pagination sentinel.
    pub async fn public_featured(&self) -> Result<Vec<CardEntry>> {
        let sql = format!("{PUBLIC_CARD_SELECT} ORDER BY lower(p.name),p.id LIMIT 8");
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn public_list(&self, q: &str, category: &str, page: i64) -> Result<Vec<Entry>> {
        self.public_filtered(q, category, "", false, page).await
    }
    pub async fn public_filtered(
        &self,
        q: &str,
        category: &str,
        brand: &str,
        available: bool,
        page: i64,
    ) -> Result<Vec<Entry>> {
        self.public_filtered_sized(
            q,
            category,
            brand,
            available,
            page,
            CatalogPageSize::default(),
        )
        .await
    }
    /// Returns at most the selected size plus one pagination sentinel.
    pub async fn public_filtered_sized(
        &self,
        q: &str,
        category: &str,
        brand: &str,
        available: bool,
        page: i64,
        page_size: CatalogPageSize,
    ) -> Result<Vec<Entry>> {
        Self::validate_public_query(q, category, brand, page)?;
        let sql = format!(
            "{PUBLIC_CATEGORY_SCOPE} {PUBLIC_SELECT} WHERE {PUBLIC_FILTER} ORDER BY lower(p.name),p.id LIMIT $6 OFFSET $5"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(q.trim())
        .bind(category)
        .bind(brand)
        .bind(available)
        // validate_query bounds page to one million; size is at most 84.
        .bind((page - 1) * i64::from(page_size.value()))
        .bind(i64::from(page_size.value()) + 1)
        .fetch_all(&self.pool)
        .await?)
    }
    /// Counts and loads the same filtered set in one snapshot; stale page numbers
    /// resolve to the last available page, with an empty set represented as page 1.
    pub async fn public_page(
        &self,
        q: &str,
        category: &str,
        brand: &str,
        available: bool,
        page: i64,
        page_size: CatalogPageSize,
    ) -> Result<PublicProductPage> {
        Self::validate_public_query(q, category, brand, page)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
            format!(
                "{PUBLIC_CATEGORY_SCOPE} \
                SELECT count(*) \
                FROM [[visible_products]] p \
                JOIN [[products]] source ON source.id=p.id \
                WHERE {PUBLIC_FILTER}"
            ),
            self.language,
        )))
        .bind(q.trim())
        .bind(category)
        .bind(brand)
        .bind(available)
        .fetch_one(&mut *tx)
        .await?;
        let size = i64::from(page_size.value());
        let pages = if total == 0 {
            1
        } else {
            (total - 1) / size + 1
        };
        let page = page.min(pages);
        let offset = (page - 1) * size;
        let entries = sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(format!(
            "{PUBLIC_CATEGORY_SCOPE} {PUBLIC_CARD_SELECT} WHERE {PUBLIC_FILTER} ORDER BY lower(p.name),p.id LIMIT $6 OFFSET $5"
        ), self.language)))
        .bind(q.trim())
        .bind(category)
        .bind(brand)
        .bind(available)
        .bind(offset)
        .bind(size)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(PublicProductPage {
            entries,
            total,
            page,
            pages,
            first: if total == 0 { 0 } else { offset + 1 },
            last: offset + size.min(total - offset),
        })
    }
    /// Validate before a web redirect as well as before SQL execution. Both entry
    /// points share the same literal-search, slug and pagination contract.
    pub fn validate_public_query(q: &str, category: &str, brand: &str, page: i64) -> Result<()> {
        validate_query(q, page)?;
        if [brand, category]
            .iter()
            .any(|slug| !slug.is_empty() && !crate::valid_slug(slug))
        {
            return Err(ProductError::Invalid(vec![]));
        }
        Ok(())
    }
    /// Bounded choices plus the active selection, without duplicates or per-row queries.
    pub async fn public_brands(&self, selected: &str) -> Result<Vec<(String, String)>> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT name,slug \
                FROM [[brands]] \
                WHERE active \
                AND (slug=$1 \
                    OR id IN (SELECT id \
                        FROM [[brands]] \
                        WHERE active \
                        ORDER BY lower(name),id \
                        LIMIT 1000)) \
                ORDER BY lower(name),id \
                LIMIT 1001",
            self.language,
        )))
        .bind(selected)
        .fetch_all(&self.pool)
        .await?)
    }
    /// Choices are independent of search/stock filters and contain only brands with
    /// visible products assigned to this category or any descendant. Keep a relevant
    /// selected brand beyond the first 1000; never add a brand outside the subtree.
    pub async fn public_category_brands(
        &self,
        category: &str,
        selected: &str,
    ) -> Result<Vec<(String, String)>> {
        if !crate::valid_slug(category) || (!selected.is_empty() && !crate::valid_slug(selected)) {
            return Err(ProductError::Invalid(vec![]));
        }
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            format!(
                "{PUBLIC_CATEGORY_SCOPE}, relevant AS (SELECT b.id,b.name,b.slug \
                        FROM [[brands]] b \
                        WHERE b.active \
                        AND EXISTS (SELECT 1 \
                            FROM [[visible_products]] p \
                            WHERE p.brand_id=b.id \
                            AND p.category_id IN (SELECT id \
                                FROM category_scope))) \
                    SELECT name,slug \
                    FROM relevant \
                    WHERE slug=$1 \
                    OR id IN (SELECT id \
                        FROM relevant \
                        ORDER BY lower(name),id \
                        LIMIT 1000) \
                    ORDER BY lower(name),id \
                    LIMIT 1001"
            ),
            self.language,
        )))
        .bind(selected)
        .bind(category)
        .fetch_all(&self.pool)
        .await?)
    }
    /// Category taxonomy is bounded by the database to 1000 entries. Include only
    /// categories/ancestors of visible products of this brand, independent of q/stock.
    pub async fn public_brand_categories(&self, brand: &str) -> Result<Vec<(String, String)>> {
        if !crate::valid_slug(brand) {
            return Err(ProductError::Invalid(vec![]));
        }
        self.public_relevant_categories(brand).await
    }
    pub async fn public_search_categories(&self) -> Result<Vec<(String, String)>> {
        self.public_relevant_categories("").await
    }
    pub async fn sitemap_count(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
                "SELECT count(*) FROM [[visible_products]]",
                self.language,
            )))
            .fetch_one(&self.pool)
            .await?,
        )
    }
    pub async fn sitemap_products(&self, page: i64) -> Result<Vec<String>> {
        if !(1..=1_000_000).contains(&page) {
            return Err(ProductError::Invalid(vec![]));
        }
        Ok(
            sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
                "SELECT slug FROM [[visible_products]] ORDER BY id LIMIT 1000 OFFSET $1",
                self.language,
            )))
            .bind((page - 1) * 1000)
            .fetch_all(&self.pool)
            .await?,
        )
    }
    /// Public-link visibility uses the same authoritative view as public_entry,
    /// without loading localized content, media or fiscal presentation data.
    pub async fn public_visible(&self, slug: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM catalog_visible_products WHERE slug=$1)",
        )
        .bind(slug)
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn public_entry(&self, slug: &str) -> Result<Entry> {
        let sql = format!("{PUBLIC_SELECT} WHERE p.slug=$1");
        sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ProductError::NotFound)
    }
    /// Pairs must contain at most 100 selections; 0 denotes a simple product.
    /// Caller owns the transaction so all cart lines and totals share its snapshot.
    pub async fn cart_merchandise(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        pairs: &[(i64, i64)],
        language: talocart_platform::content::Language,
    ) -> Result<Vec<CartMerchandise>> {
        Self::cart_merchandise_for(tx, pairs, language, None).await
    }
    pub(crate) async fn cart_merchandise_for(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        pairs: &[(i64, i64)],
        language: talocart_platform::content::Language,
        destination: Option<&str>,
    ) -> Result<Vec<CartMerchandise>> {
        if pairs.len() > 100 {
            return Err(ProductError::Invalid(vec![]));
        }
        let products: Vec<_> = pairs.iter().map(|p| p.0).collect();
        let variants: Vec<_> = pairs.iter().map(|p| p.1).collect();
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT image.id AS image_id,p.id AS product_id,pick.variant_id,p.name,p.slug,
             CASE WHEN p.has_variants THEN v.sku ELSE p.model END AS code,
             coalesce((SELECT string_agg(o.name||': '||x.name,' / ' ORDER BY a.position)
               FROM catalog_variant_values cv JOIN [[option_values]] x ON x.id=cv.value_id
               JOIN [[options]] o ON o.id=x.option_id JOIN catalog_product_options a ON a.option_id=o.id AND a.product_id=p.id
               WHERE cv.variant_id=v.id),'') AS label,
             CASE WHEN p.has_variants THEN v.price_cents ELSE p.price_cents END AS unit_cents,
             CASE WHEN p.has_variants THEN v.stock ELSE p.stock END AS stock,
             source.manage_stock,source.minimum_quantity,source.packs,CASE WHEN $3::text IS NULL THEN p.basis_points ELSE coalesce(destination_tax.basis_points,0) END AS basis_points,p.price_mode,p.currency
             FROM unnest($1::bigint[],$2::bigint[]) pick(product_id,variant_id)
             JOIN [[visible_products]] p ON p.id=pick.product_id JOIN catalog_products source ON source.id=p.id
             LEFT JOIN resolve_tax($3::text) destination_tax ON destination_tax.class_id=source.tax_class_id
             LEFT JOIN catalog_variants v ON v.id=pick.variant_id AND v.product_id=p.id AND v.active
             LEFT JOIN media_objects image ON image.key=coalesce(v.image_key,p.image_key) AND image.public
             WHERE ($3::text IS NULL OR source.tax_class_id IS NULL OR destination_tax.class_id IS NOT NULL) AND ((NOT p.has_variants AND pick.variant_id=0) OR (p.has_variants AND v.id IS NOT NULL))",
            Some(language),
        )))
        .bind(products).bind(variants).bind(destination).fetch_all(&mut **tx).await?)
    }
}
