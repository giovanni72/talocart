//! Optional, presentation-only host context on an authenticated native-module request.
use super::*;
use serde::{Deserialize, Serialize};

pub(super) const HEADER: &str = "x-talocart-navigation";
const LIMIT: usize = 28 * 1024;
// Wire v1 bit positions are immutable; they do not depend on enum discriminants.
const PERMISSIONS: [&str; 24] = [
    "overview.read",
    "operations.read",
    "settings.read",
    "settings.write",
    "users.read",
    "users.write",
    "roles.read",
    "roles.write",
    "categories.read",
    "categories.write",
    "brands.read",
    "brands.write",
    "taxes.read",
    "taxes.write",
    "products.read",
    "products.write",
    "media.read",
    "media.write",
    "pages.read",
    "pages.write",
    "customers.read",
    "customers.write",
    "orders.read",
    "orders.write",
];
#[derive(Serialize, Deserialize)]
struct Menu {
    id: String,
    href: String,
    title: String,
    permissions: u32,
}
#[derive(Serialize, Deserialize)]
struct Context {
    version: u8,
    locale: String,
    updates: bool,
    error_logs: bool,
    modules: Vec<Menu>,
}
struct RequestContext {
    path: String,
    host: Option<Context>,
}
tokio::task_local! { static CURRENT: RequestContext; }

pub(super) fn encode(state: &AppState, session: &Session) -> Option<HeaderValue> {
    let nav = state.admin_navigation(session);
    let modules = state
        .packages
        .as_ref()?
        .inventory()
        .ok()?
        .into_iter()
        .filter(|p| p.enabled && p.available())
        .filter(|p| {
            p.manifest
                .permissions
                .iter()
                .filter_map(|p| Permission::parse(p).ok())
                .any(|p| session.allows(p))
        })
        .map(|p| Menu {
            id: p.manifest.id,
            href: p.manifest.menu,
            title: p
                .manifest
                .title
                .get(i18n::locale())
                .trim()
                .chars()
                .take(128)
                .collect(),
            permissions: PERMISSIONS.iter().enumerate().fold(0, |mask, (bit, key)| {
                if p.manifest.permissions.iter().any(|p| p == key) {
                    mask | (1 << bit)
                } else {
                    mask
                }
            }),
        })
        .collect();
    let context = Context {
        version: 1,
        locale: i18n::locale().into(),
        updates: nav.updates,
        error_logs: nav.error_logs,
        modules,
    };
    let bytes = serde_json::to_vec(&context).ok()?;
    if bytes.len() > LIMIT {
        return None;
    }
    HeaderValue::from_bytes(&bytes).ok()
}
fn decode(value: &HeaderValue) -> Option<Context> {
    if value.as_bytes().len() > LIMIT {
        return None;
    }
    let context: Context = serde_json::from_slice(value.as_bytes()).ok()?;
    if context.version != 1
        || !["en", "it"].contains(&context.locale.as_str())
        || context.modules.len() > 32
    {
        return None;
    }
    let mut ids = std::collections::HashSet::new();
    for menu in &context.modules {
        let namespace = format!("/admin/extensions/{}", menu.id);
        if menu.id.is_empty()
            || menu.id.len() > 48
            || !menu
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !ids.insert(&menu.id)
            || !talocart_platform::packages::matches_path(&namespace, &menu.href)
            || menu.href.len() > 160
            || menu.href.contains("..")
            || !menu
                .href
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b))
            || menu.title.trim().is_empty()
            || menu.title.len() > 512
            || menu.title.chars().any(char::is_control)
            || menu.permissions == 0
            || menu.permissions >> PERMISSIONS.len() != 0
        {
            return None;
        }
    }
    Some(context)
}
pub(super) async fn context(
    State(state): State<AppState>,
    request: axum::http::Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path().to_owned();
    // The standalone router is behind the module token gate. Base never trusts client context.
    let host = if state.is_standalone_module() && path.starts_with("/admin/extensions/") {
        request.headers().get(HEADER).and_then(decode)
    } else {
        None
    };
    CURRENT
        .scope(RequestContext { path, host }, next.run(request))
        .await
}
pub(super) fn apply(nav: &mut AdminNavigation, session: &Session) {
    let _ = CURRENT.try_with(|request| {
        if let Some(host) = &request.host {
            if host.locale != i18n::locale() {
                return;
            }
            nav.updates = host.updates && session.is_owner();
            nav.error_logs = host.error_logs && session.is_owner();
            nav.extensions = host
                .modules
                .iter()
                .filter(|menu| {
                    PERMISSIONS.iter().enumerate().any(|(bit, key)| {
                        menu.permissions & (1 << bit) != 0
                            && Permission::parse(key).is_ok_and(|p| session.allows(p))
                    })
                })
                .map(|menu| extensions::AdminLink {
                    section: menu.id.clone(),
                    href: menu.href.clone(),
                    title: menu.title.clone(),
                    icon: "download".into(),
                })
                .collect();
        }
    });
}
pub(super) fn active(href: &str, fallback: bool) -> bool {
    CURRENT
        .try_with(|r| {
            if r.path.starts_with("/admin/extensions/") {
                // All pages owned by the same native package share its menu entry.
                let namespace = href.split('/').take(4).collect::<Vec<_>>().join("/");
                talocart_platform::packages::matches_path(&namespace, &r.path)
            } else {
                fallback
            }
        })
        .unwrap_or(fallback)
}
pub(super) fn module_page() -> bool {
    CURRENT
        .try_with(|r| r.path.starts_with("/admin/extensions/"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn host_context_never_replaces_current_permissions_or_escapes_its_request() {
        let session = Session {
            token_hash: String::new(),
            csrf: String::new(),
            administrator_id: Some(1),
            role: Some("viewer".into()),
            permissions: vec!["overview.read".into()],
            display_name: String::new(),
            email: None,
            has_avatar: false,
            avatar_version: 0,
            locale: "en".into(),
        };
        let context = RequestContext {
            path: "/admin/extensions/visible/settings".into(),
            host: Some(Context {
                version: 1,
                locale: "en".into(),
                updates: true,
                error_logs: true,
                modules: vec![
                    Menu {
                        id: "visible".into(),
                        href: "/admin/extensions/visible".into(),
                        title: "Visible".into(),
                        permissions: 1,
                    },
                    Menu {
                        id: "revoked".into(),
                        href: "/admin/extensions/revoked".into(),
                        title: "Revoked".into(),
                        permissions: 8,
                    },
                ],
            }),
        };
        i18n::scope(
            talocart_platform::locale::Locale::parse("en").unwrap(),
            CURRENT.scope(context, async {
                let mut nav = AdminNavigation::new(&session);
                apply(&mut nav, &session);
                assert!(!nav.updates && !nav.error_logs);
                assert_eq!(nav.extensions.len(), 1);
                assert_eq!(nav.extensions[0].section, "visible");
                assert!(nav.extensions[0].is_active("modules"));
                assert!(nav.module_page());
            }),
        )
        .await;
        assert!(!module_page());
        let mut nav = AdminNavigation::new(&session);
        apply(&mut nav, &session);
        assert!(
            nav.extensions.is_empty(),
            "Older hosts have no request context"
        );
        CURRENT
            .scope(
                RequestContext {
                    path: "/admin/extensions/fixture".into(),
                    host: None,
                },
                async {
                    assert!(
                        !nav.module_page(),
                        "Fallback shells retain the ordinary Modules link"
                    );
                },
            )
            .await;
    }

    #[test]
    fn context_is_bounded_and_confined_to_native_namespaces() {
        let mut value = serde_json::json!({"version":1,"locale":"it","updates":true,"error_logs":true,
            "modules":[{"id":"fixture","href":"/admin/extensions/fixture/settings","title":"Qualità","permissions":1}]});
        let header = |v: &serde_json::Value| {
            HeaderValue::from_bytes(&serde_json::to_vec(v).unwrap()).unwrap()
        };
        assert_eq!(decode(&header(&value)).unwrap().modules[0].title, "Qualità");
        value["modules"][0]["href"] = "/admin/extensions/fixture/settings.html".into();
        assert!(
            decode(&header(&value)).is_some(),
            "Signed namespace paths may contain dots"
        );
        for path in [
            "https://example.test",
            "/admin/users",
            "/admin/extensions/fixture/../users",
            "/admin/extensions/fixture?spoof",
        ] {
            value["modules"][0]["href"] = path.into();
            assert!(decode(&header(&value)).is_none());
        }
        assert!(decode(&HeaderValue::from_str(&"x".repeat(LIMIT + 1)).unwrap()).is_none());
    }
}
