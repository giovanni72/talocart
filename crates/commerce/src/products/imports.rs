//! Product intake uses the same field rules, SQL constraints and transaction as its caller.
use crate::{
    content::Resource,
    imports::{Error, Images, Issue, Product, Result, Writer},
};
use talocart_platform::identity::Permission;
impl Writer<'_, '_> {
    pub async fn create_product(&mut self, input: &Product) -> Result<i64> {
        self.require(Permission::ProductsWrite)?;
        let validated =
            super::validate(&input.values()).map_err(|_| Error::Invalid(Issue::Product))?;
        let id = sqlx::query_scalar(
            "INSERT INTO catalog_products(name,model,slug,description,price_cents,stock,minimum_quantity,\
                    packs,brand_id,category_id,tax_class_id,status) \
                VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'draft') \
                RETURNING id",
        )
        .bind(validated.name)
        .bind(validated.model)
        .bind(validated.slug)
        .bind(validated.description)
        .bind(validated.price)
        .bind(if input.variants.is_empty() {
            validated.stock
        } else {
            0
        })
        .bind(validated.minimum_quantity)
        .bind(validated.packs)
        .bind(validated.brand)
        .bind(validated.category)
        .bind(validated.tax)
        .fetch_one(&mut **self.tx)
        .await?;
        crate::imports::persist(self.tx, Resource::Product, id, &input.translations).await?;
        self.create_import_variants(id, &input.axes, &input.variants)
            .await?;
        Ok(id)
    }
    pub async fn attach_product_images(&mut self, product: i64, images: &Images) -> Result<()> {
        self.require(Permission::ProductsWrite)?;
        if images.gallery.len() > super::gallery::MAX_GALLERY_IMAGES {
            return Err(Error::Invalid(Issue::Image));
        }
        if let Some(key) = &images.primary {
            sqlx::query("UPDATE catalog_products SET image_key=$1 WHERE id=$2")
                .bind(key)
                .bind(product)
                .execute(&mut **self.tx)
                .await?;
        }
        for (position, key) in images.gallery.iter().enumerate() {
            sqlx::query("INSERT INTO catalog_product_images(product_id,image_key,position) VALUES($1,$2,$3)")
                .bind(product).bind(key).bind((position+1) as i32).execute(&mut **self.tx).await?;
        }
        for (sku, key) in &images.variants {
            self.attach_import_variant_image(product, sku, key).await?;
        }
        Ok(())
    }
}

impl Product {
    pub fn validate(&self) -> Result<()> {
        super::check_import_values(&self.values()).map_err(|_| Error::Invalid(Issue::Product))
    }
    fn values(&self) -> super::Values {
        super::Values {
            name: self.name.clone(),
            model: self.model.clone(),
            slug: self.slug.clone(),
            description: self.description.clone(),
            price: crate::imports::price(self.price_cents),
            stock: self.stock.to_string(),
            minimum_quantity: self.minimum.to_string(),
            packs: self.packs.to_string(),
            brand_id: self.brand_id.to_string(),
            category_id: self
                .category_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            tax_class_id: self
                .tax_class_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}
