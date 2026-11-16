//! Customer history HTTP flows.
use super::*;

pub(super) async fn history(
    State(s): State<AppState>,

    Query(q): Query<HistoryQuery>,
    customer: AuthenticatedCustomer,
    cart_context: CartContext,
) -> Response {
    let v = View::History {
        page: q.page.unwrap_or(1),
    };
    page(&s, &customer.0, &cart_context, v, None)
        .await
        .unwrap_or_else(failure)
}

pub(super) async fn order(
    State(s): State<AppState>,

    Path(reference): Path<String>,
    customer: AuthenticatedCustomer,
    cart_context: CartContext,
) -> Response {
    let v = View::Order { reference };
    page(&s, &customer.0, &cart_context, v, None)
        .await
        .unwrap_or_else(failure)
}
