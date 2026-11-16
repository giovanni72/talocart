//! Product input validation shared by commands, imports and variants.
use super::*;

pub(super) struct Validated {
    pub(super) name: String,
    pub(super) model: String,
    pub(super) slug: String,
    pub(super) description: String,
    pub(super) seo_title: String,
    pub(super) seo_description: String,
    pub(super) price: i64,
    pub(super) stock: i32,
    pub(super) minimum_quantity: i32,
    pub(super) packs: i32,
    pub(super) status: String,
    pub(super) category: Option<i64>,
    pub(super) brand: Option<i64>,
    pub(super) tax: Option<i64>,
}
fn reference(value: &str, field: Field, errors: &mut Vec<Field>) -> Option<i64> {
    if value.is_empty() {
        return None;
    }
    let parsed = value.parse::<i64>().ok().filter(|n| *n > 0);
    if parsed.is_none() {
        errors.push(field);
    }
    parsed
}
/// Validate external source values with the same field rules as product forms.
/// References and current fiscal settings must still be checked in the write transaction.
pub fn check_import_values(input: &Values) -> Result<()> {
    validate(input).map(|_| ())
}
pub(super) fn validate(input: &Values) -> Result<Validated> {
    let name = input.name.trim().to_owned();
    let slug = input.slug.trim().to_owned();
    let description = input.description.trim().to_owned();
    let mut errors = vec![];
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        errors.push(Field::Name);
    }
    let commercial = validate_commercial_fields(&input.model, &input.price, &input.stock);
    if let Err(ProductError::Invalid(fields)) = &commercial {
        errors.extend(fields);
    }
    if !crate::valid_slug(&slug) {
        errors.push(Field::Slug);
    }
    if description.chars().count() > 4000
        || description
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        errors.push(Field::Description);
    }
    if !talocart_platform::storefront::text_valid(input.seo_title.trim(), 120) {
        errors.push(Field::SeoTitle);
    }
    if !talocart_platform::storefront::text_valid(input.seo_description.trim(), 320) {
        errors.push(Field::SeoDescription);
    }
    if !matches!(input.status.as_str(), "draft" | "published") {
        errors.push(Field::Status);
    }
    let positive_quantity = |value: &str| {
        value
            .parse::<i32>()
            .ok()
            .filter(|n| (1..=MAX_QUANTITY).contains(n))
    };
    let minimum_quantity = positive_quantity(&input.minimum_quantity);
    let packs = positive_quantity(&input.packs);
    if minimum_quantity.is_none() {
        errors.push(Field::MinimumQuantity);
    }
    if packs.is_none() {
        errors.push(Field::Packs);
    }
    let category = reference(&input.category_id, Field::Category, &mut errors);
    let brand = reference(&input.brand_id, Field::Brand, &mut errors);
    let tax = reference(&input.tax_class_id, Field::TaxClass, &mut errors);
    if input.status == "published" && tax.is_none() && !errors.contains(&Field::TaxClass) {
        errors.push(Field::TaxClass);
    }
    if !errors.is_empty() {
        return Err(ProductError::Invalid(errors));
    }
    let description = talocart_platform::rich_text::normalize(&description);
    if description.chars().count() > 4000 {
        return Err(ProductError::Invalid(vec![Field::Description]));
    }
    let (model, price, stock) = commercial?;
    Ok(Validated {
        name,
        model,
        slug,
        description,
        seo_title: input.seo_title.trim().into(),
        seo_description: input.seo_description.trim().into(),
        price,
        stock,
        minimum_quantity: minimum_quantity.unwrap_or_default(),
        packs: packs.unwrap_or_default(),
        status: input.status.clone(),
        category,
        brand,
        tax,
    })
}
/// Common product/variant/seed contract, independent of names, publication and taxonomy.
pub fn validate_commercial_fields(
    code: &str,
    price: &str,
    stock: &str,
) -> Result<(String, i64, i32)> {
    let code = code.trim().to_ascii_uppercase();
    let mut errors = vec![];
    if code.is_empty()
        || code.len() > 64
        || !code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b"._-/+".contains(&b))
        || !code
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        errors.push(Field::Code);
    }
    let price = Money::parse(price).ok().filter(|m| m.cents() <= MAX_PRICE);
    if price.is_none() {
        errors.push(Field::Price);
    }
    let stock = stock
        .parse::<i32>()
        .ok()
        .filter(|n| (0..=999999).contains(n));
    if stock.is_none() {
        errors.push(Field::Stock);
    }
    if !errors.is_empty() {
        return Err(ProductError::Invalid(errors));
    }
    Ok((
        code,
        price.map(Money::cents).unwrap_or_default(),
        stock.unwrap_or_default(),
    ))
}
