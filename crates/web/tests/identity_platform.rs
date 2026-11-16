//! Integration scenarios grouped for proportional compilation; PostgreSQL stays serial.
include!("identity/shared.rs");
use talocart_platform::identity::digest;

#[path = "identity/account.rs"]
mod account;

#[path = "identity/administration.rs"]
mod administration;

#[path = "identity/authentication.rs"]
mod authentication;

#[path = "identity/banners.rs"]
mod banners;

#[path = "identity/base_modules.rs"]
mod base_modules;

#[path = "identity/customer_admin.rs"]
mod customer_admin;

#[path = "identity/customer_fields.rs"]
mod customer_fields;

#[path = "identity/customer_groups.rs"]
mod customer_groups;

#[path = "identity/customer_registration.rs"]
mod customer_registration;

#[path = "identity/installation_languages.rs"]
mod installation_languages;

#[path = "identity/localization.rs"]
mod localization;

#[path = "identity/operations.rs"]
mod operations;

#[path = "identity/packages.rs"]
mod packages;

#[path = "identity/pages.rs"]
mod pages;

#[path = "identity/performance.rs"]
mod performance;

#[path = "identity/regional_settings.rs"]
mod regional_settings;

#[path = "identity/settings.rs"]
mod settings;

#[path = "identity/store_language_policy.rs"]
mod store_language_policy;

#[path = "identity/store_localization.rs"]
mod store_localization;

#[path = "identity/storefront.rs"]
mod storefront;

#[path = "identity/ui.rs"]
mod ui;

#[path = "identity/updates.rs"]
mod updates;
