//! Compiled module inventory; the app explicitly composes optional services and workers.
use std::collections::HashSet;
use thiserror::Error;

/// Internal areas are not independently installable or switchable modules.
#[derive(Clone, Copy)]
pub struct Component {
    pub id: &'static str,
    pub dependencies: &'static [&'static str],
}

/// Presence in this inventory never enables a commercial method or provider.
/// There is deliberately no activation state or disabling operation for the base.
#[derive(Clone, Copy)]
pub struct Module {
    pub id: &'static str,
    /// Cargo package version, not the Git release tag.
    pub version: &'static str,
    pub components: &'static [Component],
}

pub const BASE: Module = Module {
    id: "base",
    version: env!("CARGO_PKG_VERSION"),
    components: &[
        Component {
            id: "platform",
            dependencies: &[],
        },
        Component {
            id: "identity",
            dependencies: &["platform"],
        },
        Component {
            id: "media",
            dependencies: &["platform", "identity"],
        },
        Component {
            id: "pricing",
            dependencies: &["platform", "identity"],
        },
        Component {
            id: "catalog",
            dependencies: &["platform", "identity"],
        },
        Component {
            id: "related-products",
            dependencies: &["catalog"],
        },
        Component {
            id: "inventory",
            dependencies: &["catalog"],
        },
        Component {
            id: "demo",
            dependencies: &["catalog", "media"],
        },
        Component {
            id: "localization",
            dependencies: &["platform"],
        },
        Component {
            id: "customers",
            dependencies: &["identity", "localization"],
        },
        Component {
            id: "cart",
            dependencies: &["platform", "catalog", "pricing", "customers"],
        },
        Component {
            id: "shipping",
            dependencies: &["pricing"],
        },
        Component {
            id: "payments",
            dependencies: &["pricing"],
        },
        Component {
            id: "coupons",
            dependencies: &["cart", "pricing"],
        },
        Component {
            id: "notifications",
            dependencies: &["platform", "localization"],
        },
        Component {
            id: "orders",
            dependencies: &[
                "cart",
                "inventory",
                "shipping",
                "payments",
                "coupons",
                "notifications",
            ],
        },
        Component {
            id: "checkout",
            dependencies: &["orders", "customers"],
        },
        Component {
            id: "fulfillment",
            dependencies: &["orders", "notifications"],
        },
        Component {
            id: "pages",
            dependencies: &["localization", "identity"],
        },
        Component {
            id: "operations",
            dependencies: &["identity", "orders", "catalog", "pages"],
        },
        Component {
            id: "banners",
            dependencies: &["media", "localization", "identity"],
        },
        Component {
            id: "presentation",
            dependencies: &["media", "localization"],
        },
        Component {
            id: "web",
            dependencies: &[
                "platform",
                "identity",
                "catalog",
                "pricing",
                "cart",
                "checkout",
                "fulfillment",
                "pages",
                "operations",
                "presentation",
                "banners",
            ],
        },
    ],
};
pub const MODULES: &[Module] = &[BASE];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InventoryError {
    #[error("Required base module missing")]
    MissingBase,
    #[error("Invalid or duplicate module identifier: {0}")]
    ModuleId(&'static str),
    #[error("Module has no components: {0}")]
    EmptyModule(&'static str),
    #[error("Invalid or duplicate component identifier: {0}")]
    ComponentId(&'static str),
    #[error("Component {component} requires an earlier component: {dependency}")]
    Dependency {
        component: &'static str,
        dependency: &'static str,
    },
}

/// Components are prerequisite areas, not a graph of every service call.
/// They must be globally unique and in dependency order across modules. This rejects missing references,
/// self-dependencies and cycles before the app opens a database connection.
/// The product base is mandatory even when no external provider is configured.
pub fn validate(modules: &[Module]) -> Result<(), InventoryError> {
    let mut module_ids = HashSet::new();
    let mut seen = HashSet::new();
    for module in modules {
        if module.id.is_empty() || !module_ids.insert(module.id) {
            return Err(InventoryError::ModuleId(module.id));
        }
        if module.components.is_empty() {
            return Err(InventoryError::EmptyModule(module.id));
        }
        for component in module.components {
            if component.id.is_empty() || seen.contains(component.id) {
                return Err(InventoryError::ComponentId(component.id));
            }
            for dependency in component.dependencies {
                if !seen.contains(dependency) {
                    return Err(InventoryError::Dependency {
                        component: component.id,
                        dependency,
                    });
                }
            }
            seen.insert(component.id);
        }
    }
    if !module_ids.contains("base") {
        return Err(InventoryError::MissingBase);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base(components: &'static [Component]) -> Module {
        Module { components, ..BASE }
    }
    #[test]
    fn compiled_base_and_previous_component_ids_are_preserved() {
        assert_eq!(validate(MODULES), Ok(()));
        assert_eq!(MODULES.iter().map(|m| m.id).collect::<Vec<_>>(), ["base"]);
        assert!(!BASE.components.iter().any(|c| c.id == "imports"));
        assert!(BASE.components.iter().any(|c| c.id == "demo"));
        assert_eq!(validate(&[BASE]), Ok(()));
        assert_eq!(
            validate(&[
                BASE,
                Module {
                    id: "duplicate-areas",
                    ..BASE
                }
            ]),
            Err(InventoryError::ComponentId("platform"))
        );
        assert!(matches!(
            validate(&[
                Module {
                    id: "sample-extension",
                    version: "1",
                    components: &[Component {
                        id: "sample",
                        dependencies: &["catalog"]
                    }]
                },
                BASE
            ]),
            Err(InventoryError::Dependency { .. })
        ));
        for id in ["platform", "identity", "catalog", "pricing", "cart", "web"] {
            assert!(
                BASE.components.iter().any(|c| c.id == id),
                "Previous area {id}"
            );
        }
    }
    #[test]
    fn base_cannot_be_omitted_or_empty_and_modules_cannot_be_duplicated() {
        assert_eq!(validate(&[]), Err(InventoryError::MissingBase));
        assert_eq!(
            validate(&[Module {
                id: "other",
                ..BASE
            }]),
            Err(InventoryError::MissingBase)
        );
        assert_eq!(
            validate(&[BASE, BASE]),
            Err(InventoryError::ModuleId("base"))
        );
        assert_eq!(
            validate(&[Module { id: "", ..BASE }]),
            Err(InventoryError::ModuleId(""))
        );
        assert_eq!(
            validate(&[base(&[])]),
            Err(InventoryError::EmptyModule("base"))
        );
    }
    #[test]
    fn component_ids_are_nonempty_and_unique() {
        assert_eq!(
            validate(&[base(&[Component {
                id: "",
                dependencies: &[]
            }])]),
            Err(InventoryError::ComponentId(""))
        );
        assert_eq!(
            validate(&[base(&[
                Component {
                    id: "a",
                    dependencies: &[]
                },
                Component {
                    id: "a",
                    dependencies: &[]
                }
            ])]),
            Err(InventoryError::ComponentId("a"))
        );
    }
    #[test]
    fn missing_self_cyclic_and_out_of_order_dependencies_are_rejected() {
        for components in [
            &[Component {
                id: "a",
                dependencies: &["missing"],
            }][..],
            &[Component {
                id: "a",
                dependencies: &["a"],
            }][..],
            &[
                Component {
                    id: "a",
                    dependencies: &["b"],
                },
                Component {
                    id: "b",
                    dependencies: &["a"],
                },
            ][..],
            &[
                Component {
                    id: "a",
                    dependencies: &["b"],
                },
                Component {
                    id: "b",
                    dependencies: &[],
                },
            ][..],
        ] {
            assert!(matches!(
                validate(&[base(components)]),
                Err(InventoryError::Dependency { .. })
            ));
        }
        assert_eq!(
            validate(&[base(&[
                Component {
                    id: "b",
                    dependencies: &[]
                },
                Component {
                    id: "a",
                    dependencies: &["b"]
                }
            ])]),
            Ok(())
        );
    }
}
