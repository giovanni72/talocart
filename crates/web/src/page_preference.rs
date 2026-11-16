//! Browser page-size preferences shared by administrative lists and the catalogue.
use super::*;
use cookie::{Cookie, SameSite, time::Duration};
use talocart_platform::pagination::{AdminPageSize, InvalidPageSize};

#[derive(Clone, Copy)]
pub(crate) enum AdminList {
    Products,
    Customers,
    Orders,
}
impl AdminList {
    fn name(self) -> &'static str {
        match self {
            Self::Products => "talocart_admin_product_page_size",
            Self::Customers => "talocart_admin_customer_page_size",
            Self::Orders => "talocart_admin_order_page_size",
        }
    }
    fn parse(self, raw: &str) -> std::result::Result<AdminPageSize, InvalidPageSize> {
        match self {
            Self::Products => AdminPageSize::parse_products(raw),
            _ => AdminPageSize::parse(raw),
        }
    }
    pub(crate) fn resolve(
        self,
        raw: &str,
        headers: &HeaderMap,
    ) -> std::result::Result<AdminPageSize, InvalidPageSize> {
        if !raw.is_empty() {
            return self.parse(raw);
        }
        Ok(cookie_value(headers, self.name())
            .and_then(|v| self.parse(&v).ok())
            .unwrap_or_default())
    }
    pub(crate) fn remember(
        self,
        response: Response,
        raw: &str,
        size: AdminPageSize,
        secure: bool,
    ) -> std::result::Result<Response, talocart_platform::identity::IdentityError> {
        remember_cookie(response, raw, size.value(), secure, self.name(), "/admin")
    }
}

pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        let Ok(value) = header.to_str() else { continue };
        for cookie in Cookie::split_parse(value).flatten() {
            if cookie.name() == name {
                if found.is_some() {
                    return None;
                }
                found = Some(cookie.value().to_owned());
            }
        }
    }
    found
}

pub(crate) fn remember_cookie(
    mut response: Response,
    raw: &str,
    size: u16,
    secure: bool,
    name: &'static str,
    path: &'static str,
) -> std::result::Result<Response, talocart_platform::identity::IdentityError> {
    if !raw.is_empty() {
        let cookie = Cookie::build((name, size.to_string()))
            .path(path)
            .http_only(true)
            .same_site(SameSite::Lax)
            .secure(secure)
            .max_age(Duration::days(180))
            .build();
        let value = HeaderValue::from_str(&cookie.to_string())
            .map_err(|_| talocart_platform::identity::IdentityError::Unavailable)?;
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn administrative_preferences_are_independent_bounded_and_secure() {
        for list in [AdminList::Products, AdminList::Customers, AdminList::Orders] {
            let mut headers = HeaderMap::new();
            headers.insert(header::COOKIE, HeaderValue::from_static("talocart_admin_product_page_size=500; talocart_admin_customer_page_size=50; talocart_admin_order_page_size=100; talocart_catalog_page_size=84"));
            let expected = match list {
                AdminList::Products => 500,
                AdminList::Customers => 50,
                AdminList::Orders => 100,
            };
            assert_eq!(
                list.resolve("", &headers)
                    .expect("valid preference")
                    .value(),
                expected
            );
            assert_eq!(
                list.resolve("10", &headers).expect("explicit size").value(),
                10
            );
            assert!(list.resolve("invalid", &headers).is_err());
            for secure in [true, false] {
                let response = list
                    .remember(
                        StatusCode::OK.into_response(),
                        "50",
                        AdminPageSize::parse("50").expect("valid size"),
                        secure,
                    )
                    .expect("cookie header");
                let cookie = response.headers()[header::SET_COOKIE]
                    .to_str()
                    .expect("ASCII");
                assert!(cookie.starts_with(&format!("{}=50;", list.name())));
                assert!(
                    cookie.contains("Path=/admin")
                        && cookie.contains("HttpOnly")
                        && cookie.contains("SameSite=Lax")
                        && cookie.contains("Max-Age=15552000")
                );
                assert_eq!(cookie.contains("Secure"), secure);
            }
            assert!(
                !list
                    .remember(
                        StatusCode::OK.into_response(),
                        "",
                        AdminPageSize::default(),
                        false
                    )
                    .expect("unchanged response")
                    .headers()
                    .contains_key(header::SET_COOKIE)
            );
            for value in ["bad".into(), format!("50; {}=100", list.name())] {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::COOKIE,
                    HeaderValue::from_str(&format!("{}={value}", list.name())).expect("header"),
                );
                assert_eq!(list.resolve("", &headers).expect("fallback").value(), 20);
            }
            let mut headers = HeaderMap::new();
            headers.append(
                header::COOKIE,
                HeaderValue::from_str(&format!("{}=50", list.name())).expect("header"),
            );
            headers.append(
                header::COOKIE,
                HeaderValue::from_str(&format!("{}=100", list.name())).expect("header"),
            );
            assert_eq!(
                list.resolve("", &headers)
                    .expect("ambiguous cookie ignored")
                    .value(),
                20
            );
        }
        assert!(
            AdminList::Customers
                .resolve("500", &HeaderMap::new())
                .is_err()
        );
        assert!(AdminList::Orders.resolve("500", &HeaderMap::new()).is_err());
    }
}
