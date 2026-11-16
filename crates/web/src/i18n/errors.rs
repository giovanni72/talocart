//! Error variants map to stable IDs at the HTTP presentation boundary.
use super::text;
pub fn identity_error(error: &talocart_platform::identity::IdentityError) -> String {
    use talocart_platform::identity::IdentityError as E;
    match error {
        E::InvalidLocale => text("admin-choose-an-available-language"),
        E::Invalid => text("admin-invalid-data-check-email-name-and-password-12-to-128-characters"),
        E::InvalidSettings(..) => text("admin-check-the-highlighted-store-settings-fields"),
        E::CurrentPassword => text("admin-the-current-password-is-incorrect"),
        E::InvalidAvatar => {
            text("admin-choose-a-valid-static-png-jpeg-or-webp-image-up-to-2-mib-and-4096-4096-p")
        }
        E::InvalidBranding => {
            text("admin-choose-a-valid-static-png-jpeg-or-webp-image-up-to-10-mib-and-8192-8192")
        }
        E::Storage => text("admin-media-storage-temporarily-unavailable"),
        E::PasswordMismatch => text("admin-the-new-passwords-do-not-match"),
        E::Unauthorized => text("admin-invalid-expired-or-already-used-credentials-or-token"),
        E::Forbidden => text("admin-operation-not-allowed"),
        E::Installed => text("admin-installation-already-completed"),
        E::LastOwner => {
            text("admin-at-least-one-active-owner-must-remain-assign-the-role-to-another-adminis")
        }
        E::DuplicateEmail => text("admin-an-administrator-with-this-email-already-exists"),
        E::NotFound => text("admin-administrator-not-found"),
        E::ProtectedRole => {
            text("admin-the-owner-role-is-protected-and-cannot-be-edited-or-deleted")
        }
        E::RoleInUse => {
            text("admin-this-role-is-assigned-to-administrators-reassign-all-accounts-before-del")
        }
        E::DuplicateRole => text("admin-a-role-with-this-name-already-exists"),
        E::RoleNotFound => text("admin-role-not-found"),
        E::RoleLimit => text("admin-the-limit-of-100-roles-has-been-reached"),
        E::Limited => text("admin-too-many-attempts-retry-after-the-15-minute-window"),
        E::Busy => text("admin-service-is-busy-retry-in-a-few-seconds"),
        E::Unavailable => text("admin-service-temporarily-unavailable"),
        E::Technical(error) => technical_error(error),
    }
}
pub(crate) fn catalog_error(error: &talocart_commerce::CatalogError) -> String {
    use talocart_commerce::CatalogError as E;
    match error {
        E::Content(error) => crate::content::message(error),
        E::Invalid(..) => text("admin-check-the-highlighted-fields"),
        E::DuplicateSlug => text("admin-this-slug-is-already-in-use"),
        E::DuplicateName => text("admin-a-brand-with-this-name-already-exists"),
        E::Hierarchy => {
            text("admin-invalid-parent-category-avoid-cycles-and-hierarchies-deeper-than-16-leve")
        }
        E::Limit => text("admin-the-limit-of-1000-categories-has-been-reached"),
        E::NotFound => text("admin-entry-not-found"),
        E::InUse => text("admin-this-entry-is-in-use-move-subcategories-or-linked-resources-first"),
        E::Technical(e) => technical_error(e),
        E::Access(e) => access_error(e),
        E::Identity(e) => identity_error(e),
    }
}
pub(crate) fn product_error(error: &talocart_commerce::products::ProductError) -> String {
    use talocart_commerce::products::ProductError as E;
    match error {
        E::Content(error) => crate::content::message(error),
        E::Invalid(..) => text("admin-check-the-highlighted-fields"),
        E::DuplicateModel => text("admin-this-model-is-already-in-use"),
        E::DuplicateSlug => text("admin-this-slug-is-already-in-use"),
        E::Publication => {
            text("admin-choose-a-tax-class-with-an-active-tax-rate-to-publish-category-and-brand")
        }
        E::StockChanged => text("admin-stock-changed"),
        E::ModeChanged => {
            text("admin-the-price-mode-changed-while-you-were-editing-the-product-check-the-amou")
        }
        E::NotFound => text("admin-product-not-found"),
        E::InUse => text("admin-the-product-is-linked-to-other-resources-and-cannot-be-deleted"),
        E::Selection => text("admin-select-1-to-500-products-with-valid-unique-ids"),
        E::BulkConfirmation => text("admin-confirm-the-action-on-all-selected-products"),
        E::BulkAction => {
            text("admin-choose-valid-actions-delete-runs-alone-and-publish-and-draft-are-alterna")
        }
        E::Confirmation => text("admin-confirm-product-deletion"),
        E::Image => text("admin-upload-a-valid-static-png-jpeg-or-webp-image-up-to-10-mib"),
        E::GalleryFull => text("admin-the-gallery-can-contain-at-most-12-additional-images"),
        E::GalleryAction => text("admin-invalid-gallery-action"),
        E::Technical(e) => technical_error(e),
        E::Access(e) => access_error(e),
        E::Identity(e) => identity_error(e),
    }
}
pub(crate) fn variant_error(error: &talocart_commerce::variants::VariantError) -> String {
    use talocart_commerce::variants::VariantError as E;
    match error {
        E::Content(error) => crate::content::message(error),
        E::Name => text("admin-enter-a-name-of-1-to-80-characters-without-control-characters-2"),
        E::SortOrder => text("options-invalid-sort-order"),
        E::DuplicateName => text("admin-this-name-is-already-in-use"),
        E::DuplicateSku => text("admin-this-sku-is-already-used-by-another-variant"),
        E::DuplicateCombination => text("admin-this-combination-already-exists-in-the-product"),
        E::Combination => text("admin-choose-a-value-for-each-product-option-without-duplicates"),
        E::Options => {
            text("admin-choose-one-to-three-different-options-or-none-for-a-simple-product")
        }
        E::InUse => {
            text("admin-delete-variants-before-changing-options-options-and-values-in-use-cannot")
        }
        E::NotFound => text("admin-resource-not-found-in-the-specified-product-or-option"),
        E::Confirmation => text("admin-confirm-deletion"),
        E::Filters => text("admin-invalid-variant-filters"),
        E::Search => text("options-values-search-invalid"),
        E::Product(e) => product_error(e),
        E::Technical(e) => technical_error(e),
        E::Access(e) => access_error(e),
        E::Identity(e) => identity_error(e),
    }
}
pub(crate) fn tax_error(error: &talocart_commerce::taxes::TaxError) -> String {
    use talocart_commerce::taxes::TaxError as E;
    match error {
        E::Invalid(..) => text("admin-check-the-highlighted-fields"),
        E::Duplicate => text("admin-an-entry-with-this-name-already-exists"),
        E::InUse => {
            text("admin-this-entry-is-linked-to-other-resources-remove-or-edit-the-links-first")
        }
        E::InactiveRate => text("admin-an-active-class-must-use-an-active-tax-rate"),
        E::ActiveClasses => {
            text("admin-this-tax-rate-is-used-by-active-classes-deactivate-them-or-change-their")
        }
        E::DefaultInUse => {
            text("admin-this-is-the-default-class-choose-another-class-or-remove-the-default-bef")
        }
        E::InvalidDefault => text("admin-choose-an-active-default-class-with-an-active-tax-rate"),
        E::Limit => text("admin-the-limit-of-100-entries-for-this-section-has-been-reached"),
        E::CurrencyInUse => text("admin-currency-in-use"),
        E::CountryInUse => text("admin-country-in-use"),
        E::InactiveClass => text("admin-zone-active-class-required"),
        E::Stale => text("admin-fiscal-stale"),
        E::ActiveZones => text("admin-class-active-zones"),
        E::ProductPrices => {
            text("admin-price-mode-cannot-change-while-products-exist-explicit-price-conversion")
        }
        E::NotFound => text("admin-entry-not-found"),
        E::Overflow => {
            text("admin-the-calculation-exceeds-the-maximum-supported-amount-reduce-price-or-qua")
        }
        E::Technical(e) => technical_error(e),
        E::Access(e) => access_error(e),
        E::Identity(e) => identity_error(e),
    }
}
pub(crate) fn library_error(error: &talocart_platform::media::library::LibraryError) -> String {
    use talocart_platform::media::library::LibraryError as E;
    match error {
        E::FileInvalid => text("admin-invalid-document-upload"),
        E::Name => {
            text("admin-use-a-name-of-1-to-255-characters-ascii-letters-numbers-spaces-points-hy")
        }
        E::NotFound => text("admin-image-or-folder-not-found"),
        E::Collision => text("admin-a-file-or-folder-with-this-name-already-exists"),
        E::InUse => text("admin-the-resource-is-in-use-protected-or-contains-other-items"),
        E::Destination => {
            text("admin-you-cannot-move-a-folder-inside-itself-or-change-file-privacy")
        }
        E::Attachment(value) => value.clone(),
        E::Confirmation => text("admin-confirm-the-operation"),
        E::Identity(e) => identity_error(e),
        E::Media(e) => media_error(e),
    }
}
pub(crate) fn media_error(error: &talocart_platform::media::MediaError) -> String {
    use talocart_platform::media::MediaError as E;
    match error {
        E::Invalid => {
            text("admin-choose-a-valid-static-png-jpeg-or-webp-image-up-to-10-mib-and-8192-8192")
        }
        E::Missing => text("admin-image-unavailable"),
        E::Storage => text("admin-media-storage-temporarily-unavailable"),
        E::Busy => text("admin-image-processing-is-busy"),
    }
}
pub(crate) fn storefront_error(error: &talocart_platform::storefront::StorefrontError) -> String {
    use talocart_platform::storefront::StorefrontError as E;
    match error {
        E::Content(error) => crate::content::message(error),
        E::Invalid(..) => text("admin-check-the-highlighted-fields"),
        E::Technical(e) => technical_error(e),
        E::Access(e) => access_error(e),
        E::Identity(e) => identity_error(e),
    }
}

pub fn technical_error(error: &talocart_platform::technical::TechnicalError) -> String {
    use talocart_platform::technical::TechnicalError as E;
    text(match error {
        E::Busy => "admin-service-is-busy-retry-in-a-few-seconds",
        E::Storage => "admin-media-storage-temporarily-unavailable",
        _ => "admin-service-temporarily-unavailable",
    })
}
pub fn access_error(error: &talocart_platform::technical::AccessError) -> String {
    text(match error {
        talocart_platform::technical::AccessError::Forbidden => "admin-operation-not-allowed",
        talocart_platform::technical::AccessError::Unauthorized => {
            "admin-invalid-expired-or-already-used-credentials-or-token"
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use talocart_platform::{identity::IdentityError, media::MediaError};

    #[tokio::test]
    async fn english_diagnostics_preserve_localized_http_errors() {
        let identity = IdentityError::Forbidden;
        let media = MediaError::Missing;
        assert_eq!(identity.to_string(), "Operation not allowed");
        assert_eq!(media.to_string(), "Image unavailable");
        for (locale, expected) in [
            (
                super::super::Locale::ITALIAN,
                ["Operazione non consentita", "Immagine non disponibile"],
            ),
            (
                super::super::Locale::ENGLISH,
                ["Operation not allowed", "Image unavailable"],
            ),
        ] {
            super::super::scope(locale, async {
                assert_eq!(identity_error(&identity), expected[0]);
                assert_eq!(media_error(&media), expected[1]);
            })
            .await;
        }
    }
}
