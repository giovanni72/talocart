//! Integration scenarios grouped for proportional compilation; PostgreSQL stays serial.
include!("identity/shared.rs");
use talocart_platform::identity::digest;

#[path = "identity/accessory_taxes.rs"]
mod accessory_taxes;

#[path = "identity/cart.rs"]
mod cart;

#[path = "identity/checkout_countries.rs"]
mod checkout_countries;

#[path = "identity/checkout_details.rs"]
mod checkout_details;

#[path = "identity/checkout_methods.rs"]
mod checkout_methods;

#[path = "identity/currency_zones.rs"]
mod currency_zones;

#[path = "identity/default_taxes.rs"]
mod default_taxes;

#[path = "identity/email.rs"]
mod email;

#[path = "identity/order_statuses.rs"]
mod order_statuses;

#[path = "identity/orders.rs"]
mod orders;

#[path = "identity/payments.rs"]
mod payments;

#[path = "identity/shipping.rs"]
mod shipping;

#[path = "identity/taxes.rs"]
mod taxes;

#[path = "identity/theme_presentation.rs"]
mod theme_presentation;

#[path = "identity/unified_settings.rs"]
mod unified_settings;
