use talocart_commerce::products::{CardEntry, Entry, ProductError};

/// Presentation data shared by storefront lists. Prices and availability come from
/// the commerce module; rendering a card never performs queries or calculations.
pub struct ProductCard {
    pub name: String,
    pub href: String,
    pub image: String,
    pub category: Option<String>,
    pub srcset: String,
    pub price: String,
    pub currency: String,
    pub starting: bool,
    pub available: bool,
    pub brand_name: String,
    pub brand_logo: String,
    pub brand_logo_srcset: String,
}

impl TryFrom<Entry> for ProductCard {
    type Error = ProductError;
    fn try_from(entry: Entry) -> Result<Self, Self::Error> {
        Self::try_from(CardEntry::from(entry))
    }
}

impl TryFrom<CardEntry> for ProductCard {
    type Error = ProductError;

    fn try_from(entry: CardEntry) -> Result<Self, Self::Error> {
        Ok(Self {
            image: entry.image_url("card"),
            srcset: talocart_platform::media::responsive_srcset(
                entry.image_id,
                entry.image_width,
                entry.image_height,
            ),
            price: entry.quote()?.gross().display(),
            currency: entry.currency.clone(),
            available: entry.available(),
            starting: entry.has_variants,
            href: super::store_language::path(format!("/products/{}", entry.slug)),
            name: entry.name,
            category: entry.category_name,
            brand_name: entry.brand_name.unwrap_or_default(),
            brand_logo_srcset: brand_logo_srcset(entry.brand_logo_id, entry.brand_logo_edge),
            brand_logo: entry
                .brand_logo_id
                .map(|id| format!("/media/image/{id}/thumbnail"))
                .unwrap_or_default(),
        })
    }
}

// Density descriptors must use the actual no-upscale cap, rather than labeling
// every card as 2x: that would halve the intrinsic size of small source logos.
fn brand_logo_srcset(id: Option<i64>, edge: Option<i32>) -> String {
    use talocart_platform::media::ImageProfile;
    let (Some(id), Some(edge)) = (id, edge) else {
        return String::new();
    };
    let thumbnail = f64::from(ImageProfile::Thumbnail.edge());
    if f64::from(edge) <= thumbnail {
        return String::new();
    }
    let density = f64::from(edge).min(f64::from(ImageProfile::Card.edge())) / thumbnail;
    format!("/media/image/{id}/thumbnail 1x, /media/image/{id}/card {density}x")
}

#[cfg(test)]
mod tests {
    use super::*;
    use askama::Template;

    #[derive(Template)]
    #[template(
        source = "{% import \"storefront/product-card.html\" as card %}{{ card::render(product, nested, false) }}",
        ext = "html"
    )]
    struct CardTemplate<'a> {
        product: &'a ProductCard,
        nested: bool,
    }

    #[test]
    fn logo_density_preserves_native_size_with_small_and_capped_sources() {
        for edge in [32, 80, 128] {
            assert!(brand_logo_srcset(Some(42), Some(edge)).is_empty());
        }
        assert_eq!(
            brand_logo_srcset(Some(42), Some(200)),
            "/media/image/42/thumbnail 1x, /media/image/42/card 1.5625x"
        );
        for edge in [512, 800, 2048] {
            assert_eq!(
                brand_logo_srcset(Some(42), Some(edge)),
                "/media/image/42/thumbnail 1x, /media/image/42/card 4x"
            );
        }
    }

    fn entry() -> Entry {
        Entry {
            modified_at: String::new(),
            id: 1,
            name: "Maglia <cotone> & seta".into(),
            model: "MAGLIA".into(),
            slug: "maglia".into(),
            description: String::new(),
            seo_title: String::new(),
            seo_description: String::new(),
            price_cents: 1000,
            manage_stock: true,
            stock: 5,
            stock_revision: 0,
            minimum_quantity: 5,
            packs: 6,
            has_variants: false,
            variant_count: 0,
            status: "published".into(),
            category_id: Some(1),
            brand_id: None,
            tax_class_id: Some(1),
            image_key: Some("brand/photo original.jpg".into()),
            image_id: Some(7),
            image_width: Some(512),
            image_height: Some(128),
            category_name: Some("Abiti & accessori".into()),
            brand_name: None,
            brand_logo_id: None,
            brand_logo_edge: None,
            brand_slug: None,
            tax_name: Some("Ordinaria".into()),
            basis_points: Some(2200),
            price_mode: "exclusive".into(),
            currency: "EUR".into(),
        }
    }

    #[test]
    fn card_uses_taxed_price_and_purchase_quantity_availability() {
        let mut product = entry();
        let card = ProductCard::try_from(product.clone()).expect("valid commercial data");
        assert_eq!(card.price, "12,20");
        assert_eq!(card.href, "/products/maglia");
        assert_eq!(card.image, "/media/image/7/card");
        assert!(!card.available, "five items cannot fill a pack of six");
        assert!(!card.starting);
        product.manage_stock = false;
        product.stock = 0;
        assert!(
            ProductCard::try_from(product.clone())
                .expect("unmanaged card")
                .available
        );
        product.minimum_quantity = 999999;
        product.packs = 500000;
        assert!(
            !ProductCard::try_from(product.clone())
                .expect("bounded quantity")
                .available
        );
        product.minimum_quantity = 4;
        product.packs = 6;
        product.manage_stock = true;
        product.stock = 6;
        product.has_variants = true;
        product.price_mode = "inclusive".into();
        let card = ProductCard::try_from(product).expect("valid variant projection");
        assert_eq!(card.price, "10,00");
        assert!(card.available);
        assert!(card.starting);
    }

    #[test]
    fn card_rejects_invalid_prices_and_missing_tax_rates() {
        let mut product = entry();
        product.price_cents = -1;
        assert!(ProductCard::try_from(product).is_err());
        let mut product = entry();
        product.basis_points = None;
        assert!(ProductCard::try_from(product).is_err());
    }

    #[test]
    fn shared_template_escapes_content_and_preserves_heading_hierarchy() {
        let card = ProductCard::try_from(entry()).expect("valid card");
        for (nested, heading) in [(false, "h2"), (true, "h3")] {
            let html = CardTemplate {
                product: &card,
                nested,
            }
            .render()
            .expect("render card");
            assert!(html.contains(&format!("<{heading} class=\"store-product-title\">")));
            assert!(html.contains("Maglia &#60;cotone&#62; &#38; seta"));
            assert!(html.contains("Abiti &#38; accessori"));
            assert!(!html.contains("<cotone>"));
            assert!(html.contains("loading=\"lazy\" decoding=\"async\""));
            assert!(html.contains("alt=\"Maglia &#60;cotone&#62; &#38; seta\""));
            assert!(html.contains("12,20 €</span><small>IVA inclusa</small>"));
            assert!(html.contains("product-availability unavailable"));
            assert!(html.contains("Non disponibile"));
            assert!(html.contains("<article class=\"store-product\" role=\"listitem\">"));
            assert!(html.contains("<div class=\"store-product-meta\">"));
        }
    }

    #[test]
    fn shared_template_handles_missing_image_category_and_starting_price() {
        let mut product = entry();
        product.image_key = None;
        product.image_id = None;
        product.category_name = None;
        product.stock = 6;
        product.has_variants = true;
        let card = ProductCard::try_from(product).expect("valid card without optional data");
        let html = CardTemplate {
            product: &card,
            nested: false,
        }
        .render()
        .expect("render card");
        assert!(!html.contains("<img"));
        assert!(!html.contains("store-product-brand-logo"));
        assert!(html.contains("Immagine non disponibile"));
        assert!(!html.contains("class=\"eyebrow\""));
        assert!(!html.contains("store-product-meta"), "no empty meta row");
        assert!(html.contains("Da 12,20 €"));
        assert!(html.contains("class=\"product-availability\">Disponibile</span>"));
    }
    #[test]
    fn brand_logo_is_optional_and_uses_public_media_alias() {
        let mut entry = entry();
        entry.brand_name = Some("Marchio <&>".into());
        entry.brand_logo_id = Some(42);
        entry.brand_logo_edge = Some(800);
        let card = ProductCard::try_from(entry).expect("valid card with brand");
        assert_eq!(card.brand_logo, "/media/image/42/thumbnail");
        assert_eq!(card.brand_name, "Marchio <&>");
        let html = CardTemplate {
            product: &card,
            nested: false,
        }
        .render()
        .expect("render card with brand");
        let meta = html
            .split("<div class=\"store-product-meta\">")
            .nth(1)
            .and_then(|rest| rest.split("</div>").next())
            .expect("meta row");
        assert!(
            meta.contains("class=\"eyebrow\""),
            "category precedes the brand"
        );
        assert!(meta.contains("<img class=\"store-product-brand-logo\" src=\"/media/image/42/thumbnail\" srcset=\"/media/image/42/thumbnail 1x, /media/image/42/card 4x\" alt=\"Logo Marchio &#60;&#38;&#62;\""));
    }
}
