//! Customer authentication HTTP flows.
use super::*;

pub(super) async fn auth_page(
    state: AppState,
    kind: AuthPage,
    query: AuthQuery,
    mut customer: OptionalCustomer,
    cart_context: &CartContext,
) -> Response {
    if customer.authenticated().is_some() {
        return redirect(&return_target(&query.next));
    }
    let mut raw = None;
    if customer.0.is_none() {
        match state.customers().anonymous().await {
            Ok((token, s)) => {
                customer.0 = Some(s);
                raw = Some(token);
            }
            Err(error) => return failure(error),
        }
    }
    let mut input = Input {
        next: return_target(&query.next),
        ..Default::default()
    };
    if kind == AuthPage::Confirm && query.token.len() <= 64 {
        input.token = query.token;
    }
    let notice = match query.changed.as_str() {
        "email" => "customer-email-changed",
        "password" => "customer-password-changed",
        "confirmed" => "customer-email-confirmed",
        "pending" => "customer-confirmation-pending",
        "resent" => "customer-confirmation-resent",
        _ => "",
    };
    let session = match customer.session() {
        Ok(s) => s,
        Err(error) => return failure(error),
    };
    let mut r = page(
        &state,
        session,
        cart_context,
        View::authentication(kind, input, notice),
        None,
    )
    .await
    .unwrap_or_else(failure);
    if let Some(raw) = raw
        && let Err(error) = set_cookie(
            &mut r,
            &raw,
            state.secure_cookies,
            ANONYMOUS_SESSION_SECONDS,
        )
    {
        return failure(error);
    }
    r
}

pub(super) async fn login_page(
    State(s): State<AppState>,
    Query(q): Query<AuthQuery>,
    customer: OptionalCustomer,
    cart_context: CartContext,
) -> Response {
    auth_page(s, AuthPage::Login, q, customer, &cart_context).await
}

pub(super) async fn register_page(
    State(s): State<AppState>,
    Query(q): Query<AuthQuery>,
    customer: OptionalCustomer,
    cart_context: CartContext,
) -> Response {
    auth_page(s, AuthPage::Register, q, customer, &cart_context).await
}

pub(super) async fn confirm_page(
    State(s): State<AppState>,
    Query(q): Query<AuthQuery>,
    customer: OptionalCustomer,
    cart_context: CartContext,
) -> Response {
    auth_page(s, AuthPage::Confirm, q, customer, &cart_context).await
}

pub(super) async fn confirmation_page(
    State(s): State<AppState>,
    Query(q): Query<AuthQuery>,
    customer: OptionalCustomer,
    cart_context: CartContext,
) -> Response {
    auth_page(s, AuthPage::Confirmation, q, customer, &cart_context).await
}

pub(super) async fn confirm_email(
    State(state): State<AppState>,
    customer: OptionalCustomer,
    cart_context: CartContext,
    Form(input): Form<Input>,
) -> Response {
    let s = match customer.session() {
        Ok(s) => s,
        Err(error) => return failure(error),
    };
    match state
        .customers()
        .confirm_email(s, &input.csrf, &input.token)
        .await
    {
        Ok(()) => redirect("/account/login?changed=confirmed"),
        Err(error) => {
            let v = View::authentication(AuthPage::Confirm, input, "");
            page(&state, s, &cart_context, v, Some(&error))
                .await
                .unwrap_or_else(failure)
        }
    }
}

pub(super) async fn resend_confirmation(
    State(state): State<AppState>,
    customer: OptionalCustomer,
    cart_context: CartContext,
    Form(input): Form<Input>,
) -> Response {
    let s = match customer.session() {
        Ok(s) => s,
        Err(error) => return failure(error),
    };
    let path = store_language::path("/account/confirm");
    let delivery = talocart_platform::identity::customer_registration::RegistrationEmail {
        origin: &state.public_origin,
        path: &path,
        locale: i18n::locale(),
    };
    match state
        .customers()
        .resend_confirmation(s, &input.csrf, &input.profile.email, &delivery)
        .await
    {
        Ok(()) => redirect("/account/confirmation?changed=resent"),
        Err(error) => {
            let v = View::authentication(AuthPage::Confirmation, input, "");
            page(&state, s, &cart_context, v, Some(&error))
                .await
                .unwrap_or_else(failure)
        }
    }
}

pub(super) async fn reset_page(
    State(s): State<AppState>,
    Query(q): Query<AuthQuery>,
    customer: OptionalCustomer,
    cart_context: CartContext,
) -> Response {
    auth_page(s, AuthPage::Reset, q, customer, &cart_context).await
}

pub(super) async fn auth_submit(
    state: AppState,
    input: Input,
    action: AuthAction,
    customer: OptionalCustomer,
    cart_context: &CartContext,
) -> Response {
    let kind = action.page();
    let s = match customer.session() {
        Ok(s) => s,
        Err(error) => return failure(error),
    };
    if kind == AuthPage::Register && input.action == "refresh" {
        if let Err(error) = s.check_csrf(&input.csrf) {
            return failure(error.into());
        }
        let view = View::authentication(AuthPage::Register, input, "");
        return page(&state, s, cart_context, view, None)
            .await
            .unwrap_or_else(failure);
    }
    let result = if kind == AuthPage::Register && input.password != input.password_confirmation {
        Err(IdentityError::PasswordMismatch.into())
    } else if kind == AuthPage::Register {
        let path = store_language::path("/account/confirm");
        let delivery = talocart_platform::identity::customer_registration::RegistrationEmail {
            origin: &state.public_origin,
            path: &path,
            locale: i18n::locale(),
        };
        state
            .customers()
            .register(
                s,
                &input.csrf,
                &input.profile,
                input.password.clone(),
                Some(&delivery),
            )
            .await
    } else {
        state
            .customers()
            .login(s, &input.csrf, &input.profile.email, input.password.clone())
            .await
            .map(Some)
    };
    match result {
        Ok(None) => redirect("/account/confirmation?changed=pending"),
        Ok(Some(raw)) => {
            let target = if cart::after_login(&state, &raw, cart_context).await.is_ok() {
                return_target(&input.next)
            } else {
                "/cart".into()
            };
            let mut r = redirect(&target);
            if let Err(error) = set_cookie(
                &mut r,
                &raw,
                state.secure_cookies,
                AUTHENTICATED_SESSION_SECONDS,
            ) {
                return failure(error);
            }
            r
        }
        Err(error) => {
            let v = View::authentication(kind, input, "");
            page(&state, s, cart_context, v, Some(&error))
                .await
                .unwrap_or_else(failure)
        }
    }
}

pub(super) async fn register(
    State(s): State<AppState>,
    customer: OptionalCustomer,
    cart_context: CartContext,
    Form(f): Form<Input>,
) -> Response {
    auth_submit(s, f, AuthAction::Register, customer, &cart_context).await
}

pub(super) async fn login(
    State(s): State<AppState>,
    customer: OptionalCustomer,
    cart_context: CartContext,
    Form(f): Form<Input>,
) -> Response {
    auth_submit(s, f, AuthAction::Login, customer, &cart_context).await
}

pub(super) async fn logout(
    State(s): State<AppState>,
    customer: RequiredCustomer,
    Form(f): Form<Input>,
) -> Response {
    let result = async {
        s.customers().logout(required(&customer)?, &f.csrf).await?;
        let mut r = redirect("/account/login");
        set_cookie(&mut r, "", s.secure_cookies, 0)?;
        forget_cart(&mut r, s.secure_cookies)?;
        Ok::<_, CustomerError>(r)
    }
    .await;
    result.unwrap_or_else(failure)
}

pub(super) async fn reset(
    State(s): State<AppState>,
    customer: OptionalCustomer,
    cart_context: CartContext,
    Form(f): Form<Input>,
) -> Response {
    let result = async {
        let session = customer.session()?;
        let result = if f.password != f.password_confirmation {
            Err(IdentityError::PasswordMismatch.into())
        } else {
            s.customers()
                .reset(session, &f.csrf, &f.token, f.password)
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
                    View::authentication(AuthPage::Reset, Input::default(), ""),
                    Some(&error),
                )
                .await
            }
        }
    }
    .await;
    result.unwrap_or_else(failure)
}
