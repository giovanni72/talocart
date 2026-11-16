use super::*;
use axum::{extract::Form, response::Redirect};
use serde::Deserialize;
use talocart_platform::identity::IdentityError;
use talocart_platform::updates::{Action, Blocker, Candidate, Notice, Phase, Stage};

#[derive(Template)]
#[template(path = "updates.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    status: talocart_platform::updates::Snapshot,
    label: &'static str,
    available: bool,
    can_check: bool,
    rows: Vec<PlanRow>,
    stage_label: Option<&'static str>,
    notice_label: Option<&'static str>,
    plan_title: &'static str,
    from_label: &'static str,
    to_label: &'static str,
}
struct PlanRow {
    id: String,
    current: String,
    target: String,
    action: &'static str,
    activation: Option<&'static str>,
    blocker: Option<&'static str>,
}
fn action_label(action: Action, completed: bool) -> &'static str {
    if completed {
        return match action {
            Action::Keep => "updates-kept",
            Action::Update => "updates-changed",
            Action::Blocked => "updates-blocked-action",
        };
    }
    match action {
        Action::Keep => "updates-keep",
        Action::Update => "updates-change",
        Action::Blocked => "updates-blocked-action",
    }
}
fn blocker_label(blocker: Blocker) -> &'static str {
    match blocker {
        Blocker::Incomplete => "updates-blocker-incomplete",
        Blocker::Incompatible => "updates-blocker-incompatible",
        Blocker::Publisher => "updates-blocker-publisher",
        Blocker::Version => "updates-blocker-version",
        Blocker::Schema => "updates-blocker-schema",
        Blocker::Conflict => "updates-blocker-conflict",
    }
}
pub async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, access::IdentityErrorResponse> {
    let status = state.updates.read(&state.identity, &session).await?;
    let available = status.can_request();
    let can_check = status.can_check();
    let label = if !status.configured {
        "updates-disabled"
    } else if status.recovery_required {
        "updates-recovery"
    } else if status.queued {
        "updates-queued"
    } else {
        match status.phase {
            None => "updates-waiting",
            Some(Phase::Available) => "updates-available",
            Some(Phase::Current) => "updates-current",
            Some(Phase::Installing) => "updates-installing",
            Some(Phase::Succeeded) => "updates-succeeded",
            Some(Phase::Failed) => "updates-failed",
            Some(Phase::CheckFailed) => "updates-check-failed",
            Some(Phase::Blocked) => "updates-blocked",
        }
    };
    let completed = status.phase == Some(Phase::Succeeded);
    let mut rows = Vec::new();
    if let Some(plan) = &status.plan {
        rows.push(PlanRow {
            id: crate::i18n::text("updates-base"),
            current: plan.base_from.clone(),
            target: plan.base_to.clone(),
            action: action_label(plan.base_action, completed),
            activation: None,
            blocker: None,
        });
        rows.extend(plan.modules.iter().map(|m| PlanRow {
            id: m.id.clone(),
            current: m.current.clone(),
            target: m.to.clone(),
            action: action_label(m.action, completed),
            activation: Some(if m.enabled {
                "updates-stays-enabled"
            } else {
                "updates-stays-disabled"
            }),
            blocker: m.blocker.map(blocker_label),
        }));
    }
    let stage_label = status.stage.map(|stage| match stage {
        Stage::Prepared | Stage::UpdaterReady => "updates-stage-preparing",
        Stage::Quiescing | Stage::Fenced => "updates-stage-quiescing",
        Stage::BackedUp => "updates-stage-backup",
        Stage::Applying => "updates-stage-applying",
        Stage::Verifying => "updates-stage-verifying",
        Stage::Resuming | Stage::Succeeded => "updates-stage-resuming",
    });
    let notice_label = status.notice.map(|notice| match notice {
        Notice::PlanChanged => "updates-plan-changed",
        Notice::PreparationFailed => "updates-preparation-failed",
        Notice::EngineFailed => "updates-engine-failed",
        Notice::ControlBusy => "updates-control-busy",
        Notice::ControlConflict => "updates-control-conflict",
        Notice::ControlConfiguration => "updates-control-configuration",
        Notice::ControlUnavailable => "updates-control-unavailable",
    });
    let shell = AdminShell::load(&state, &session).await?;
    let mut response = render(Page {
        store_name: &shell.store_name,
        logo: shell.logo,
        csrf: shell.csrf,
        section: "updates",
        theme: shell.theme,
        nav: shell.nav,
        account: shell.account,
        status,
        label,
        available,
        can_check,
        rows,
        stage_label,
        notice_label,
        plan_title: if completed {
            "updates-plan-completed"
        } else {
            "updates-plan-title"
        },
        from_label: if completed {
            "updates-before"
        } else {
            "updates-from"
        },
        to_label: if completed {
            "updates-from"
        } else {
            "updates-to"
        },
    });
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}
#[derive(Deserialize)]
pub struct Install {
    csrf: String,
    version: String,
    commit: String,
    digest: String,
    confirm: Option<String>,
}
pub async fn install(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<Install>,
) -> Result<Response, access::IdentityErrorResponse> {
    session.check_csrf(&form.csrf)?;
    if form.confirm.as_deref() != Some("yes") {
        return Err(IdentityError::Invalid.into());
    }
    state
        .updates
        .request(
            &state.identity,
            &session,
            Candidate {
                version: form.version,
                commit: form.commit,
                digest: form.digest,
            },
        )
        .await?;
    Ok(Redirect::to("/admin/updates").into_response())
}
#[derive(Deserialize)]
pub struct Check {
    csrf: String,
}
pub async fn check(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<Check>,
) -> Result<Response, access::IdentityErrorResponse> {
    session.check_csrf(&form.csrf)?;
    state
        .updates
        .request_check(&state.identity, &session)
        .await?;
    Ok(Redirect::to("/admin/updates").into_response())
}
