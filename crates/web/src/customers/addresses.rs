//! Customer addresses HTTP flows.
use super::*;

pub(super) async fn addresses(
    State(s): State<AppState>,
    customer: AuthenticatedCustomer,
    cart_context: CartContext,
) -> Response {
    if let Err(error) = cart::establish(&s, &cart_context, Some(&customer.0)).await {
        return failure(error.into());
    }
    page(&s, &customer.0, &cart_context, View::Addresses, None)
        .await
        .unwrap_or_else(failure)
}

pub(super) async fn new_address(
    State(s): State<AppState>,
    customer: AuthenticatedCustomer,
    cart_context: CartContext,
) -> Response {
    page(
        &s,
        &customer.0,
        &cart_context,
        View::address(None, 0, AddressInput::default()),
        None,
    )
    .await
    .unwrap_or_else(failure)
}

pub(super) async fn edit_address(
    State(s): State<AppState>,

    Path(id): Path<i64>,
    customer: AuthenticatedCustomer,
    cart_context: CartContext,
) -> Response {
    let result = async {
        let a = CustomerAddresses::new(s.pool.clone())
            .get(&customer.0, id)
            .await?;
        let v = View::address(Some(id), a.revision, a.input());
        page(&s, &customer.0, &cart_context, v, None).await
    }
    .await;
    result.unwrap_or_else(failure)
}

pub(super) async fn save_address(
    s: AppState,
    f: AddressForm,
    id: Option<i64>,
    customer: RequiredCustomer,
    cart_context: &CartContext,
) -> Response {
    let result = async {
        let target = id.map(|id| (id, f.revision));
        match CustomerAddresses::new(s.pool.clone())
            .save(required(&customer)?, &f.csrf, target, &f.data)
            .await
        {
            Ok(_) => Ok(redirect("/account/addresses")),
            Err(error) => {
                let v = View::address(id, f.revision, f.data);
                page(&s, required(&customer)?, cart_context, v, Some(&error)).await
            }
        }
    }
    .await;
    result.unwrap_or_else(failure)
}

pub(super) async fn create_address(
    State(s): State<AppState>,
    customer: RequiredCustomer,
    cart_context: CartContext,
    Form(f): Form<AddressForm>,
) -> Response {
    save_address(s, f, None, customer, &cart_context).await
}

pub(super) async fn update_address(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    customer: RequiredCustomer,
    cart_context: CartContext,
    Form(f): Form<AddressForm>,
) -> Response {
    save_address(s, f, Some(id), customer, &cart_context).await
}

pub(super) async fn delete_address(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    customer: RequiredCustomer,
    Form(f): Form<Input>,
) -> Response {
    let result = async {
        if f.confirm.as_deref() != Some("yes") {
            return Err(CustomerError::Fields(vec!["confirm"]));
        }
        CustomerAddresses::new(s.pool.clone())
            .delete(required(&customer)?, &f.csrf, id, f.revision)
            .await?;
        Ok(redirect("/account/addresses"))
    }
    .await;
    result.unwrap_or_else(failure)
}

pub(super) async fn use_address(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    customer: RequiredCustomer,
    cart_context: CartContext,
    Form(f): Form<Input>,
) -> Response {
    let result = async {
        let guest = cart_context.guest().map_err(|_| IdentityError::Forbidden)?;
        let result = CustomerAddresses::new(s.pool.clone())
            .use_in_checkout(
                required(&customer)?,
                &f.csrf,
                id,
                (&guest, &f.cart_csrf, f.cart_revision),
            )
            .await;
        match result {
            Ok(()) => Ok(redirect("/checkout")),
            Err(error) => {
                page(
                    &s,
                    required(&customer)?,
                    &cart_context,
                    View::Addresses,
                    Some(&error),
                )
                .await
            }
        }
    }
    .await;
    result.unwrap_or_else(failure)
}
