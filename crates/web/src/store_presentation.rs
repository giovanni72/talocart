//! Rendering-only request data. Authentication and cart capabilities live in extractors.
use std::{cell::RefCell, future::Future};
use talocart_commerce::cart::Guest;
#[derive(Default)]
pub(crate) struct Presentation {
    count: i64,
    csrf: String,
    revision: i64,
    customer: Option<i64>,
}
tokio::task_local! { static PRESENTATION: RefCell<Presentation>; }
impl Presentation {
    pub(crate) fn new(guest: Option<&Guest>, customer: Option<i64>) -> Self {
        Self {
            count: guest.map_or(0, |g| g.count),
            csrf: guest.map(|g| g.csrf.clone()).unwrap_or_default(),
            revision: guest.map_or(0, |g| g.revision),
            customer,
        }
    }
}
pub(crate) async fn scope<F: Future>(view: Presentation, future: F) -> F::Output {
    PRESENTATION.scope(RefCell::new(view), future).await
}
pub(crate) fn update_cart(guest: Option<&Guest>) {
    let _ = PRESENTATION.try_with(|view| {
        let mut view = view.borrow_mut();
        view.count = guest.map_or(0, |g| g.count);
        view.csrf = guest.map(|g| g.csrf.clone()).unwrap_or_default();
        view.revision = guest.map_or(0, |g| g.revision);
    });
}
pub(crate) fn cart_count() -> i64 {
    PRESENTATION.try_with(|v| v.borrow().count).unwrap_or(0)
}
pub(crate) fn cart_csrf() -> String {
    PRESENTATION
        .try_with(|v| v.borrow().csrf.clone())
        .unwrap_or_default()
}
pub(crate) fn cart_revision() -> i64 {
    PRESENTATION.try_with(|v| v.borrow().revision).unwrap_or(0)
}
pub(crate) fn customer_authenticated() -> bool {
    PRESENTATION
        .try_with(|v| v.borrow().customer.is_some())
        .unwrap_or(false)
}
/// Only the existing hidden HTTP field uses 0 for an absent reference; no authorization uses it.
pub(crate) fn customer_id_value() -> i64 {
    PRESENTATION
        .try_with(|v| v.borrow().customer.unwrap_or(0))
        .unwrap_or(0)
}
