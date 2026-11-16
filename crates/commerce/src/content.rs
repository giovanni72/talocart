//! Catalog content ownership: static definitions and bounded SQL projections.
use sqlx::PgPool;
use talocart_platform::{
    content::{ContentError, Definition, Language, Translation},
    identity::{Identity, Permission, Session},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Category,
    Brand,
    Product,
    Option,
    Value,
    AttributeGroup,
    Attribute,
    ProductAttribute,
}
impl Resource {
    pub fn definition(self) -> Definition {
        let (table, limit, description, read, write) = match self {
            Self::Category => (
                "catalog_categories",
                120,
                true,
                Permission::CategoriesRead,
                Permission::CategoriesWrite,
            ),
            Self::Brand => (
                "catalog_brands",
                120,
                true,
                Permission::BrandsRead,
                Permission::BrandsWrite,
            ),
            Self::Product => (
                "catalog_products",
                120,
                true,
                Permission::ProductsRead,
                Permission::ProductsWrite,
            ),
            Self::Option => (
                "catalog_options",
                80,
                false,
                Permission::ProductsRead,
                Permission::ProductsWrite,
            ),
            Self::AttributeGroup | Self::Attribute | Self::ProductAttribute => (
                match self {
                    Self::AttributeGroup => "catalog_attribute_groups",
                    Self::Attribute => "catalog_attributes",
                    _ => "catalog_product_attributes",
                },
                if self == Self::ProductAttribute {
                    500
                } else {
                    80
                },
                false,
                Permission::ProductsRead,
                Permission::ProductsWrite,
            ),
            Self::Value => (
                "catalog_option_values",
                80,
                false,
                Permission::ProductsRead,
                Permission::ProductsWrite,
            ),
        };
        Definition {
            table,
            name_column: "name",
            description_column: description.then_some("description"),
            name_limit: limit,
            description_limit: if description { 4000 } else { 0 },
            seo: self == Self::Product,
            read,
            write,
        }
    }
}
/// Demo seed boundary: copy the newly inserted source into the explicitly selected
/// primary language. Callers hold the commercial lock before inserting resources.
impl Resource {
    pub(crate) async fn seed_primary(
        self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: i64,
    ) -> Result<(), sqlx::Error> {
        let definition = self.definition();
        let description = if definition.description_column.is_some() {
            "r.description"
        } else {
            "''::text"
        };
        let (title, summary) = if definition.seo {
            ("r.seo_title", "r.seo_description")
        } else {
            ("''::text", "''::text")
        };
        let (extra_column, extra_value) = if self == Self::Value {
            (",option_id", ",r.option_id")
        } else {
            ("", "")
        };
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO {table}_translations(entity_id,locale,name,description,seo_title,seo_description{extra_column}) \
                SELECT r.id,s.primary_locale,r.name,{description},{title},{summary}{extra_value} \
                FROM {table} r \
                CROSS \
                JOIN storefront_settings s \
                WHERE r.id=$1 \
                AND s.id=1",
            table=definition.table
        ))).bind(id).execute(&mut **tx).await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct Content {
    pool: PgPool,
    identity: Identity,
}
impl Content {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    pub async fn load(
        &self,
        session: &Session,
        resource: Resource,
        id: i64,
    ) -> Result<Vec<Translation>, ContentError> {
        resource
            .definition()
            .load(&self.pool, &self.identity, session, id)
            .await
    }
    pub async fn load_many(
        &self,
        session: &Session,
        resource: Resource,
        ids: &[i64],
    ) -> Result<Vec<talocart_platform::content::StoredTranslation>, ContentError> {
        resource
            .definition()
            .load_many(&self.pool, &self.identity, session, ids)
            .await
    }
    pub async fn save(
        &self,
        session: &Session,
        csrf: &str,
        resource: Resource,
        id: i64,
        primary: &str,
        rows: &[Translation],
    ) -> Result<(), ContentError> {
        resource
            .definition()
            .save(&self.pool, session, csrf, id, primary, rows)
            .await
    }
}

fn projection(table: &str, columns: &str, fields: &[&str], language: Language) -> String {
    let columns = columns
        .split(',')
        .map(|column| {
            if fields.contains(&column) {
                format!(
                    "{} AS {column}",
                    language.expression(column, &format!("original.{column}"))
                )
            } else {
                format!("original.{column}")
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "(SELECT {columns} FROM {table} original {})",
        language.joins(table, "original.id")
    )
}
/// Only explicit `[[products]]`, `[[categories]]`, etc. source slots are expanded.
/// Ordinary identifiers and SQL literals are never rewritten. Sources/columns are static;
/// values retain the caller's bindings. Callers opt in at each FROM/JOIN site.
/// Localized text participates in filtering, counting and ordering before LIMIT, never after paging.
pub(crate) fn query(query: impl AsRef<str>, language: Option<Language>) -> String {
    let tables = [
        (
            "catalog_visible_products",
            "id,name,model,slug,description,price_cents,stock,status,category_id,brand_id,tax_class_id,image_key,category_name,category_slug,brand_name,tax_name,basis_points,price_mode,has_variants,currency",
            vec!["name", "description"],
        ),
        (
            "catalog_products",
            "id,name,model,slug,description,price_cents,stock,status,category_id,brand_id,tax_class_id,image_key,seo_title,seo_description,minimum_quantity,packs,stock_revision,manage_stock",
            vec!["name", "description", "seo_title", "seo_description"],
        ),
        (
            "catalog_categories",
            "id,name,slug,description,parent_id,sort_order,active",
            vec!["name", "description"],
        ),
        (
            "catalog_brands",
            "id,name,slug,description,sort_order,active,logo_key",
            vec!["name", "description"],
        ),
        (
            "catalog_option_values",
            "id,option_id,name,sort_order",
            vec!["name"],
        ),
        ("catalog_options", "id,name,sort_order", vec!["name"]),
        (
            "catalog_attribute_groups",
            "id,name,sort_order,revision",
            vec!["name"],
        ),
        (
            "catalog_attributes",
            "id,name,group_id,sort_order,revision",
            vec!["name"],
        ),
        (
            "catalog_product_attributes",
            "id,product_id,attribute_id,name,sort_order,revision",
            vec!["name"],
        ),
    ];
    let mut result = query.as_ref().to_owned();
    for (table, columns, fields) in &tables {
        let token = format!("[[{}]]", table.trim_start_matches("catalog_"));
        if !result.contains(&token) {
            continue;
        }
        let Some(language) = language else {
            result = result.replace(&token, table);
            continue;
        };
        let replacement = if *table == "catalog_visible_products" {
            let mut columns = columns
                .split(',')
                .map(|column| match column {
                    "name" | "description" => format!(
                        "{} AS {column}",
                        language.expression(column, &format!("original.{column}"))
                    ),
                    "category_name" => format!(
                        "{} AS category_name",
                        language.aliased_expression("name", "original.category_name", "cct", "ccp")
                    ),
                    "brand_name" => format!(
                        "{} AS brand_name",
                        language.aliased_expression("name", "original.brand_name", "bct", "bcp")
                    ),
                    _ => format!("original.{column}"),
                })
                .collect::<Vec<_>>()
                .join(",");
            columns.push_str(&format!(
                " FROM catalog_visible_products original {} {} {}",
                language.joins("catalog_products", "original.id"),
                language.aliased_joins("catalog_categories", "original.category_id", "cct", "ccp"),
                language.aliased_joins("catalog_brands", "original.brand_id", "bct", "bcp"),
            ));
            format!("(SELECT {columns})")
        } else {
            projection(table, columns, fields, language)
        };
        result = result.replace(&token, &replacement);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visible_products_deduplicate_all_primary_translation_joins() {
        use talocart_platform::locale::Locale;
        for locale in [Locale::ENGLISH, Locale::ITALIAN] {
            let sql = query(
                "SELECT * FROM [[visible_products]]",
                Some(Language::new(locale, locale)),
            );
            assert_eq!(sql.matches("_translations ").count(), 3);
            assert!(sql.contains("coalesce(nullif(cct.name,''),original.category_name)"));
            assert!(sql.contains("coalesce(nullif(bct.name,''),original.brand_name)"));
            assert!(!sql.contains("ccp."));
        }
        let secondary = query(
            "SELECT * FROM [[visible_products]]",
            Some(Language::new(Locale::ITALIAN, Locale::ENGLISH)),
        );
        assert_eq!(secondary.matches("_translations ").count(), 6);
        assert!(
            secondary.contains(
                "coalesce(nullif(cct.name,''),nullif(ccp.name,''),original.category_name)"
            )
        );
    }
    #[test]
    fn explicit_sources_do_not_rewrite_identifiers_or_literals() {
        let language = Language::new(
            talocart_platform::locale::Locale::ENGLISH,
            talocart_platform::locale::Locale::ITALIAN,
        );
        let sql = query(
            "SELECT 'catalog_products' AS label \
                FROM [[products]] p \
                JOIN catalog_products_translations raw ON raw.entity_id=p.id",
            Some(language),
        );
        assert!(sql.contains("'catalog_products' AS label"));
        assert!(sql.contains("JOIN catalog_products_translations raw"));
        assert!(sql.contains("LEFT JOIN catalog_products_translations ct"));
        assert!(!sql.contains("[["));
        assert_eq!(
            query("SELECT * FROM [[brands]]", None),
            "SELECT * FROM catalog_brands"
        );
    }
}
