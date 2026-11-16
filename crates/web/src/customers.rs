//! Native customer forms and private, URL-localized pages. Admin credentials never participate.
use super::cart::CartContext;
use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
};
use cookie::{Cookie, SameSite, time::Duration};
use serde::Deserialize;
use talocart_commerce::{
    customer_addresses::{Address, AddressInput, CustomerAddresses},
    orders::{Order, Orders, Summary},
};
use talocart_platform::identity::{
    ANONYMOUS_SESSION_SECONDS, AUTHENTICATED_SESSION_SECONDS, CustomerAccount, CustomerAuthAction,
    CustomerError, CustomerProfile, CustomerSession, IdentityError,
};
type Result<T> = std::result::Result<T, CustomerError>;
mod context;
pub(crate) use context::{
    AuthenticatedCustomer, OptionalCustomer, RequiredCustomer, auth_link, context, public_path,
    required,
};
use context::{forget_cart, private, redirect, return_target, set_cookie};
mod view;
use view::{AuthAction, AuthPage, ProfilePage, View, page};
mod authentication;
use authentication::{
    confirm_email, confirm_page, confirmation_page, login, login_page, logout, register,
    register_page, resend_confirmation, reset, reset_page,
};
mod profile;
use profile::{account, change_email, email_page, password, password_page, update};
mod addresses;
use addresses::{
    addresses, create_address, delete_address, edit_address, new_address, update_address,
    use_address,
};
mod history;
use history::{history, order};

fn retry_for(error: &CustomerError, response: &mut Response) {
    match error {
        CustomerError::Identity(source_error) => {
            access::IdentityErrorResponse::retry_for(source_error, response)
        }
        CustomerError::Technical(talocart_platform::technical::TechnicalError::Busy) => {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        _ => {}
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthQuery {
    next: String,
    changed: String,
    token: String,
}
async fn head() -> Response {
    private(StatusCode::OK.into_response())
}

pub(crate) fn routes(prefix: &str) -> Router<AppState> {
    use axum::routing::post;
    Router::new()
        .route(&format!("{prefix}/account"), get(account).post(update))
        .route(
            &format!("{prefix}/account/login"),
            get(login_page).head(head).post(login),
        )
        .route(
            &format!("{prefix}/account/register"),
            get(register_page).head(head).post(register),
        )
        .route(
            &format!("{prefix}/account/confirm"),
            get(confirm_page).head(head).post(confirm_email),
        )
        .route(
            &format!("{prefix}/account/confirmation"),
            get(confirmation_page).head(head).post(resend_confirmation),
        )
        .route(
            &format!("{prefix}/account/reset"),
            get(reset_page).head(head).post(reset),
        )
        .route(
            &format!("{prefix}/account/password"),
            get(password_page).post(password),
        )
        .route(
            &format!("{prefix}/account/email"),
            get(email_page).post(change_email),
        )
        .route(&format!("{prefix}/account/logout"), post(logout))
        .route(
            &format!("{prefix}/account/addresses"),
            get(addresses).head(head),
        )
        .route(
            &format!("{prefix}/account/addresses/new"),
            get(new_address).post(create_address),
        )
        .route(
            &format!("{prefix}/account/addresses/{{id}}"),
            get(edit_address).post(update_address),
        )
        .route(
            &format!("{prefix}/account/addresses/{{id}}/delete"),
            post(delete_address),
        )
        .route(
            &format!("{prefix}/account/addresses/{{id}}/use"),
            post(use_address),
        )
        .route(&format!("{prefix}/account/orders"), get(history))
        .route(
            &format!("{prefix}/account/orders/{{reference}}"),
            get(order),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
}
fn status(error: &CustomerError) -> StatusCode {
    match error {
        CustomerError::ConfirmationInvalid => StatusCode::UNPROCESSABLE_ENTITY,
        CustomerError::Fields(_) => StatusCode::UNPROCESSABLE_ENTITY,
        CustomerError::Duplicate | CustomerError::Stale | CustomerError::AddressLimit => {
            StatusCode::CONFLICT
        }
        CustomerError::NotFound => StatusCode::NOT_FOUND,
        CustomerError::Technical(error) => crate::access::technical_status(error),
        CustomerError::Access(error) => crate::access::access_status(error),
        CustomerError::Identity(error) => access::IdentityErrorResponse::status_for(error),
    }
}
fn message(error: &CustomerError) -> String {
    i18n::text(match error {
        CustomerError::ConfirmationInvalid => "customer-confirmation-invalid",
        CustomerError::Fields(fields) if fields.contains(&"shipping_country") => {
            "co-country-unavailable"
        }
        CustomerError::Fields(fields) if fields.contains(&"confirm") => "customer-confirm-delete",
        CustomerError::Fields(_) => "customer-invalid",
        CustomerError::Duplicate => "customer-duplicate",
        CustomerError::NotFound => "customer-not-found",
        CustomerError::Stale => "customer-stale",
        CustomerError::AddressLimit => "customer-address-limit",
        CustomerError::Technical(error) => return crate::i18n::technical_error(error),
        CustomerError::Access(error) => return crate::i18n::access_error(error),
        CustomerError::Identity(error) => return i18n::identity_error(error),
    })
}
pub(super) fn failure(error: CustomerError) -> Response {
    let mut r = private((status(&error), message(&error)).into_response());
    retry_for(&error, &mut r);
    r
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Input {
    action: String,
    next: String,
    csrf: String,
    revision: i64,
    #[serde(flatten)]
    profile: CustomerProfile,
    password: String,
    password_confirmation: String,
    current_password: String,
    token: String,
    cart_csrf: String,
    cart_revision: i64,
    confirm: Option<String>,
}
#[derive(Deserialize, Default)]
struct AddressForm {
    csrf: String,
    #[serde(default)]
    revision: i64,
    #[serde(flatten)]
    data: AddressInput,
}

#[derive(Deserialize)]
struct AccountQuery {
    saved: Option<String>,
}

#[derive(Deserialize, Default)]
struct HistoryQuery {
    page: Option<i64>,
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    #[test]
    fn capacity_failure_retains_retry_header_and_private_cache() {
        let response = failure(CustomerError::from(IdentityError::Busy));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}
