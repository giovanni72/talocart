use super::*;
use crate::page_preference::{AdminList, cookie_value, remember_cookie};

const NAME: &str = "talocart_catalog_page_size";

// Explicit URL values retain their validation errors. Untrusted preferences fall
// back to the default, including ambiguous duplicate cookies.
pub(super) fn resolve(raw: &str, headers: &HeaderMap) -> Result<CatalogPageSize> {
    if !raw.is_empty() {
        return Ok(CatalogPageSize::parse(raw)?);
    }
    Ok(cookie_value(headers, NAME)
        .and_then(|value| CatalogPageSize::parse(&value).ok())
        .unwrap_or_default())
}

pub(super) fn resolve_admin(raw: &str, headers: &HeaderMap) -> Result<AdminPageSize> {
    AdminList::Products
        .resolve(raw, headers)
        .map_err(|_| ProductError::Invalid(vec![]).into())
}

pub(super) fn remember(
    response: Response,
    raw: &str,
    size: CatalogPageSize,
    secure: bool,
) -> Result<Response> {
    Ok(remember_cookie(
        response,
        raw,
        size.value(),
        secure,
        NAME,
        "/",
    )?)
}

pub(super) fn remember_admin(
    response: Response,
    raw: &str,
    size: AdminPageSize,
    secure: bool,
) -> Result<Response> {
    Ok(AdminList::Products.remember(response, raw, size, secure)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_cookie_is_scoped_and_respects_https() {
        for secure in [false, true] {
            let response = remember_admin(
                StatusCode::OK.into_response(),
                "500",
                AdminPageSize::parse_products("500").expect("allowed admin size"),
                secure,
            )
            .map_err(|_| "invalid cookie")
            .expect("valid admin cookie");
            let cookie = response.headers()[header::SET_COOKIE]
                .to_str()
                .expect("ASCII cookie");
            assert!(cookie.starts_with("talocart_admin_product_page_size=500;"));
            assert!(cookie.contains("Path=/admin"));
            assert_eq!(cookie.contains("Secure"), secure);
        }
        assert!(
            !remember_admin(
                StatusCode::OK.into_response(),
                "",
                AdminPageSize::default(),
                false
            )
            .map_err(|_| "invalid cookie")
            .expect("response without preference update")
            .headers()
            .contains_key(header::SET_COOKIE)
        );
    }

    #[test]
    fn preference_cookie_security_and_url_precedence() {
        for secure in [false, true] {
            let response = remember(
                StatusCode::OK.into_response(),
                "63",
                CatalogPageSize::parse("63")
                    .map_err(|_| "invalid preference")
                    .expect("valid preference"),
                secure,
            )
            .map_err(|_| "invalid preference")
            .expect("valid preference");
            let cookie = response.headers()[header::SET_COOKIE]
                .to_str()
                .map_err(|_| "invalid preference")
                .expect("valid preference");
            assert_eq!(cookie.contains("Secure"), secure);
            assert!(cookie.contains("Max-Age=15552000"));
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=value; talocart_catalog_page_size=84"),
        );
        assert_eq!(
            resolve("", &headers)
                .map_err(|_| "invalid preference")
                .expect("valid preference")
                .value(),
            84
        );
        assert_eq!(
            resolve("21", &headers)
                .map_err(|_| "invalid preference")
                .expect("valid preference")
                .value(),
            21
        );
        assert!(resolve("100", &headers).is_err());
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("talocart_catalog_page_size=42"),
        );
        assert_eq!(
            resolve("", &headers)
                .map_err(|_| "invalid preference")
                .expect("valid preference")
                .value(),
            21
        );
        assert!(
            !remember(
                StatusCode::OK.into_response(),
                "",
                CatalogPageSize::default(),
                false
            )
            .map_err(|_| "invalid preference")
            .expect("valid preference")
            .headers()
            .contains_key(header::SET_COOKIE)
        );
    }
}
