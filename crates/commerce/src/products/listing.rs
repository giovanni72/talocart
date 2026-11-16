//! Narrow public-card read model: detailed content stays on product/detail reads.
use super::{Entry, LineTotals, MAX_QUANTITY, PurchaseQuantities, Result};
use sqlx::FromRow;

pub struct PublicProductPage {
    pub entries: Vec<CardEntry>,
    pub total: i64,
    pub page: i64,
    pub pages: i64,
    pub first: i64,
    pub last: i64,
}

#[derive(Debug, PartialEq, FromRow)]
pub struct CardEntry {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub price_cents: i64,
    pub manage_stock: bool,
    pub stock: i32,
    pub minimum_quantity: i32,
    pub packs: i32,
    pub has_variants: bool,
    pub image_id: Option<i64>,
    pub image_width: Option<i32>,
    pub image_height: Option<i32>,
    pub category_name: Option<String>,
    pub brand_name: Option<String>,
    pub brand_logo_id: Option<i64>,
    pub brand_logo_edge: Option<i32>,
    pub basis_points: Option<i32>,
    pub price_mode: String,
    pub currency: String,
}
impl CardEntry {
    pub fn available(&self) -> bool {
        PurchaseQuantities::new(self.minimum_quantity, self.packs).is_ok_and(|rules| {
            rules.available(if self.manage_stock {
                self.stock
            } else {
                MAX_QUANTITY
            })
        })
    }
    pub fn quote(&self) -> Result<LineTotals> {
        super::quote(self.price_cents, self.basis_points, &self.price_mode)
    }
    pub fn image_url(&self, profile: &str) -> String {
        self.image_id
            .map(|id| format!("/media/image/{id}/{profile}"))
            .unwrap_or_default()
    }
}
impl From<Entry> for CardEntry {
    fn from(entry: Entry) -> Self {
        Self {
            id: entry.id,
            name: entry.name,
            slug: entry.slug,
            price_cents: entry.price_cents,
            manage_stock: entry.manage_stock,
            stock: entry.stock,
            minimum_quantity: entry.minimum_quantity,
            packs: entry.packs,
            has_variants: entry.has_variants,
            image_id: entry.image_id.filter(|_| entry.image_key.is_some()),
            image_width: entry.image_width,
            image_height: entry.image_height,
            category_name: entry.category_name,
            brand_name: entry.brand_name,
            brand_logo_id: entry.brand_logo_id,
            brand_logo_edge: entry.brand_logo_edge,
            basis_points: entry.basis_points,
            price_mode: entry.price_mode,
            currency: entry.currency,
        }
    }
}
