//! Customer profile HTTP flows.
use super::*;

pub(super) async fn account(
    State(s): State<AppState>,

    Query(q): Query<AccountQuery>,
    customer: AuthenticatedCustomer,
    cart_context: CartContext,
) -> Response {
    let notice = if q.saved.as_deref() == Some("1") {
        "customer-saved"
    } else {
        ""
    };
    let v = View::profile(ProfilePage::Account, Input::default(), notice);
    page(&s, &customer.0, &cart_context, v, None)
        .await
        .unwrap_or_else(failure)
}

pub(super) async fn password_page(
    State(s): State<AppState>,
    customer: AuthenticatedCustomer,
    cart_context: CartContext,
) -> Response {
    page(
        &s,
        &customer.0,
        &cart_context,
        View::profile(ProfilePage::Password, Input::default(), ""),
        None,
    )
    .await
    .unwrap_or_else(failure)
}

pub(super) async fn email_page(
    State(s): State<AppState>,
    customer: AuthenticatedCustomer,
    cart_context: CartContext,
) -> Response {
    page(
        &s,
        &customer.0,
        &cart_context,
        View::profile(ProfilePage::Email, Input::default(), ""),
        None,
    )
    .await
    .unwrap_or_else(failure)
}

pub(super) async fn change_email(
    State(s): State<AppState>,
    customer: RequiredCustomer,
    cart_context: CartContext,
    Form(mut f): Form<Input>,
) -> Response {
    let result = async {
        let session = required(&customer)?;
        let account = s.customers().account(session).await?;
        f.profile.first_name = account.first_name;
        f.profile.last_name = account.last_name;
        f.profile.customer_type = account.customer_type;
        match s
            .customers()
            .update(
                session,
                &f.csrf,
                f.revision,
                &f.profile,
                f.current_password.clone(),
            )
            .await
        {
            Ok(true) => {
                let mut r = redirect("/account/login?changed=email");
                set_cookie(&mut r, "", s.secure_cookies, 0)?;
                forget_cart(&mut r, s.secure_cookies)?;
                Ok(r)
            }
            Ok(false) => Ok(redirect("/account")),
            Err(error) => {
                let v = View::profile(ProfilePage::Email, f, "");
                page(&s, session, &cart_context, v, Some(&error)).await
            }
        }
    }
    .await;
    result.unwrap_or_else(failure)
}

pub(super) async fn update(
    State(s): State<AppState>,
    customer: RequiredCustomer,
    cart_context: CartContext,
    Form(mut f): Form<Input>,
) -> Response {
    if f.action == "refresh" {
        let session = match required(&customer) {
            Ok(s) => s,
            Err(error) => return failure(error),
        };
        if let Err(error) = session.check_csrf(&f.csrf) {
            return failure(error.into());
        }
        let view = View::profile(ProfilePage::Account, f, "");
        return page(&s, session, &cart_context, view, None)
            .await
            .unwrap_or_else(failure);
    }
    let result = async {
        let session = required(&customer)?;
        f.profile.email = s.customers().account(session).await?.email;
        match s
            .customers()
            .update(
                session,
                &f.csrf,
                f.revision,
                &f.profile,
                f.current_password.clone(),
            )
            .await
        {
            Ok(_) => Ok(redirect("/account?saved=1")),
            Err(error) => {
                let v = View::profile(ProfilePage::Account, f, "");
                page(&s, session, &cart_context, v, Some(&error)).await
            }
        }
    }
    .await;
    result.unwrap_or_else(failure)
}

pub(super) async fn password(
    State(s): State<AppState>,
    customer: RequiredCustomer,
    cart_context: CartContext,
    Form(f): Form<Input>,
) -> Response {
    let result = async {
        let session = required(&customer)?;
        let result = if f.password != f.password_confirmation {
            Err(IdentityError::PasswordMismatch.into())
        } else {
            s.customers()
                .change_password(
                    session,
                    &f.csrf,
                    f.current_password.clone(),
                    f.password.clone(),
                )
                .await
        };
        match result {
            Ok(()) => {
                let mut r = redirect("/account/login?changed=password");
                set_cookie(&mut r, "", s.secure_cookies, 0)?;
                forget_cart(&mut r, s.secure_cookies)?;
                Ok(r)
            }
            Err(error) => {
                page(
                    &s,
                    session,
                    &cart_context,
                    View::profile(ProfilePage::Password, Input::default(), ""),
                    Some(&error),
                )
                .await
            }
        }
    }
    .await;
    result.unwrap_or_else(failure)
}
