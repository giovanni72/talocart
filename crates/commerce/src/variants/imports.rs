//! Option/value/variant persistence owned by Variants, shared by catalog intake.
use crate::{
    content::Resource,
    imports::{Axis, Error, Issue, Result, Variant, Writer},
};
use std::collections::BTreeSet;
use talocart_platform::{content::Translation, identity::Permission};
impl Writer<'_, '_> {
    pub(crate) async fn create_import_variants(
        &mut self,
        product: i64,
        axes: &[Axis],
        variants: &[Variant],
    ) -> Result<()> {
        self.require(Permission::ProductsWrite)?;
        if variants.is_empty() {
            return Ok(());
        }
        if axes.is_empty() || axes.len() > 3 || variants.len() > 2000 {
            return Err(Error::Invalid(Issue::Variant));
        }
        let mut options = Vec::new();
        for (position, axis) in axes.iter().enumerate() {
            let ids: Vec<i64> = sqlx::query_scalar(
                "SELECT DISTINCT entity_id \
                    FROM catalog_options_translations \
                    WHERE (locale='it' \
                        AND lower(name)=lower($1)) \
                    OR (locale='en' \
                        AND lower(name)=lower($2)) \
                    ORDER BY entity_id \
                    LIMIT 2",
            )
            .bind(&axis.italian)
            .bind(&axis.english)
            .fetch_all(&mut **self.tx)
            .await?;
            let id = match ids.as_slice() {
                [id] => *id,
                [] => {
                    let id = sqlx::query_scalar(
                        "INSERT INTO catalog_options(name) VALUES($1) RETURNING id",
                    )
                    .bind(&axis.italian)
                    .fetch_one(&mut **self.tx)
                    .await?;
                    crate::imports::persist(
                        self.tx,
                        Resource::Option,
                        id,
                        &[
                            Translation {
                                locale: "it".into(),
                                name: axis.italian.clone(),
                                ..Default::default()
                            },
                            Translation {
                                locale: "en".into(),
                                name: axis.english.clone(),
                                ..Default::default()
                            },
                        ],
                    )
                    .await?;
                    id
                }
                _ => return Err(Error::Conflict),
            };
            sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,$2,$3)")
                .bind(product).bind((position+1) as i16).bind(id).execute(&mut **self.tx).await?;
            options.push(id);
        }
        let mut combinations = BTreeSet::new();
        for variant in variants {
            if variant.values.len() != options.len() {
                return Err(Error::Invalid(Issue::Variant));
            }
            let mut ids = Vec::new();
            for (value, option) in variant.values.iter().zip(&options) {
                if value.label.is_empty()
                    || value.label.chars().count() > 80
                    || value.label.chars().any(char::is_control)
                {
                    return Err(Error::Invalid(Issue::VariantLabel));
                }
                let id = match sqlx::query_scalar::<_, i64>(
                    "SELECT t.entity_id \
                    FROM catalog_option_values_translations t \
                    WHERE t.locale=$3 \
                    AND t.option_id=$1 \
                    AND lower(t.name)=lower($2)",
                )
                .bind(option)
                .bind(&value.label)
                .bind(&value.locale)
                .fetch_optional(&mut **self.tx)
                .await?
                {
                    Some(id) => id,
                    None => {
                        let id=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) VALUES($1,$2) RETURNING id").bind(option).bind(&value.label).fetch_one(&mut **self.tx).await?;
                        crate::imports::persist(self.tx, Resource::Value, id, &value.translations)
                            .await?;
                        id
                    }
                };
                ids.push(id);
            }
            ids.sort_unstable();
            if !combinations.insert(ids.clone()) {
                return Err(Error::Invalid(Issue::Combination));
            }
            let (sku, _, stock) = crate::products::validate_commercial_fields(
                &variant.sku,
                &crate::imports::price(variant.price_cents),
                &variant.stock.to_string(),
            )
            .map_err(|_| Error::Invalid(Issue::Variant))?;
            sqlx::query(
                "INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock,active) \
                VALUES($1,$2,$3,$4,$5,$6)",
            )
            .bind(product)
            .bind(ids)
            .bind(sku)
            .bind(variant.price_cents)
            .bind(stock)
            .bind(variant.active)
            .execute(&mut **self.tx)
            .await?;
        }
        Ok(())
    }
    pub(crate) async fn attach_import_variant_image(
        &mut self,
        product: i64,
        sku: &str,
        key: &str,
    ) -> Result<()> {
        self.require(Permission::ProductsWrite)?;
        sqlx::query("UPDATE catalog_variants SET image_key=$1 WHERE product_id=$2 AND sku=$3")
            .bind(key)
            .bind(product)
            .bind(sku)
            .execute(&mut **self.tx)
            .await?;
        Ok(())
    }
}
