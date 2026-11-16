pub struct SearchFilters {
    pub category: String,
    pub brand: String,
    pub available: bool,
    pub per_page: u16,
}
impl Default for SearchFilters {
    fn default() -> Self {
        Self {
            category: String::new(),
            brand: String::new(),
            available: false,
            per_page: 21,
        }
    }
}
use std::collections::BTreeMap;

use talocart_commerce::NavigationCategory;

#[derive(Default)]
pub struct CategoryMenu {
    pub pages: Vec<PageLink>,
    pub groups: Vec<CategoryGroup>,
    pub all_current: bool,
    pub brands: Vec<BrandLink>,
    pub brands_current: bool,
    pub more_brands: bool,
}
pub struct BrandLink {
    pub name: String,
    pub href: String,
    pub current: bool,
    pub logo_url: String,
    pub menu_logo_url: String,
    /// Decorative monogram shown in place of a missing logo; empty when the
    /// name has no alphanumeric character.
    pub initial: String,
}
impl BrandLink {
    pub fn new(brand: talocart_commerce::NavigationBrand, selected: &str) -> Self {
        Self {
            initial: brand
                .name
                .chars()
                .find(|c| c.is_alphanumeric())
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default(),
            name: brand.name,
            href: super::store_language::path(format!(
                "/brands/{}",
                super::catalog::encoded(&brand.slug)
            )),
            current: brand.slug == selected,
            menu_logo_url: brand
                .logo_id
                .map(|id| format!("/media/image/{id}/thumbnail"))
                .unwrap_or_default(),
            logo_url: brand
                .logo_id
                .map(|id| format!("/media/image/{id}/card"))
                .unwrap_or_default(),
        }
    }
}
pub struct CategoryGroup {
    pub root: CategoryLink,
    pub columns: Vec<CategoryColumn>,
    pub selected: bool,
}
pub struct CategoryColumn {
    pub root: CategoryLink,
    pub children: Vec<CategoryLink>,
}
pub struct CategoryLink {
    pub name: String,
    pub href: String,
    pub current: bool,
    pub ancestors: String,
}
impl CategoryMenu {
    pub fn has_footer_pages(&self) -> bool {
        self.pages.iter().any(|p| p.footer_menu)
    }
    pub fn has_header_pages(&self) -> bool {
        self.pages.iter().any(|p| p.header_menu)
    }
    pub async fn load(
        state: &super::AppState,
        selected: &str,
        all_current: bool,
        selected_brand: &str,
        brands_current: bool,
    ) -> Result<Self, talocart_platform::identity::IdentityError> {
        let localized_state = state.localized();
        let state = &localized_state;
        let categories = state.catalog.public_navigation().await?;
        Self::new(categories, selected, all_current)
            .load_store_links(state, selected_brand, brands_current)
            .await
    }
    pub async fn load_store_links(
        mut self,
        state: &super::AppState,
        selected: &str,
        current: bool,
    ) -> Result<Self, talocart_platform::identity::IdentityError> {
        let localized_state = state.localized();
        let state = &localized_state;
        let brands =
            state
                .catalog
                .public_brand_navigation()
                .await
                .map_err(|error| match error {
                    talocart_commerce::CatalogError::Technical(error) => error.into(),
                    talocart_commerce::CatalogError::Access(error) => error.into(),
                    talocart_commerce::CatalogError::Identity(error) => error,
                    _ => talocart_platform::identity::IdentityError::Unavailable,
                })?;
        self.more_brands = brands.len() > talocart_commerce::PUBLIC_BRAND_PAGE_SIZE;
        self.brands = brands
            .into_iter()
            .take(talocart_commerce::PUBLIC_BRAND_PAGE_SIZE)
            .map(|brand| BrandLink::new(brand, selected))
            .collect();
        self.pages =
            talocart_commerce::pages::Pages::new(state.pool.clone(), state.identity.clone())
                .navigation(super::store_language::content_language())
                .await
                .map_err(super::pages::identity_error)?
                .into_iter()
                .map(|p| {
                    let href = super::store_language::path(format!("/pages/{}", p.slug));
                    PageLink {
                        current: super::store_language::request_target().split('?').next()
                            == Some(href.as_str()),
                        href,
                        name: p.name,
                        header_menu: p.header_menu,
                        footer_menu: p.footer_menu,
                    }
                })
                .collect();
        self.brands_current = current;
        Ok(self)
    }

    /// The commerce service supplies a bounded, acyclic tree in sibling order.
    /// Second-level categories own columns; their descendants stay in preorder.
    /// Ancestor labels retain access to levels deeper than the third without nesting panels.
    pub fn new(categories: Vec<NavigationCategory>, selected: &str, all_current: bool) -> Self {
        let mut by_parent: BTreeMap<Option<i64>, Vec<NavigationCategory>> = BTreeMap::new();
        for category in categories {
            by_parent
                .entry(category.parent_id)
                .or_default()
                .push(category);
        }
        let groups = by_parent
            .remove(&None)
            .unwrap_or_default()
            .into_iter()
            .map(|root| {
                let columns: Vec<_> = by_parent
                    .remove(&Some(root.id))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|child| {
                        let mut children = Vec::new();
                        descendants(&mut by_parent, child.id, "", selected, &mut children);
                        CategoryColumn {
                            root: link(child, selected, ""),
                            children,
                        }
                    })
                    .collect();
                let root = link(root, selected, "");
                CategoryGroup {
                    selected: root.current
                        || columns.iter().any(|column| {
                            column.root.current || column.children.iter().any(|child| child.current)
                        }),
                    root,
                    columns,
                }
            })
            .collect();
        Self {
            groups,
            all_current,
            ..Self::default()
        }
    }
}
fn link(category: NavigationCategory, selected: &str, ancestors: &str) -> CategoryLink {
    CategoryLink {
        current: category.slug == selected,
        href: super::store_language::path(format!(
            "/catalog/{}",
            super::catalog::encoded(&category.slug)
        )),
        name: category.name,
        ancestors: ancestors.into(),
    }
}
fn descendants(
    by_parent: &mut BTreeMap<Option<i64>, Vec<NavigationCategory>>,
    parent: i64,
    ancestors: &str,
    selected: &str,
    links: &mut Vec<CategoryLink>,
) {
    for child in by_parent.remove(&Some(parent)).unwrap_or_default() {
        let id = child.id;
        let path = if ancestors.is_empty() {
            child.name.clone()
        } else {
            format!("{ancestors} / {}", child.name)
        };
        links.push(link(child, selected, ancestors));
        descendants(by_parent, id, &path, selected, links);
    }
}

pub struct PageLink {
    pub name: String,
    pub href: String,
    pub current: bool,
    pub header_menu: bool,
    pub footer_menu: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn category(id: i64, parent_id: Option<i64>, name: &str) -> NavigationCategory {
        NavigationCategory {
            id,
            parent_id,
            name: name.into(),
            slug: format!("category-{id}"),
        }
    }
    #[test]
    fn brand_links_have_a_distinct_route_and_current_state() {
        let selected = BrandLink::new(
            talocart_commerce::NavigationBrand {
                name: "Nome & marchio".into(),
                slug: "marchio-1".into(),
                logo_id: Some(42),
            },
            "marchio-1",
        );
        assert_eq!(selected.href, "/brands/marchio-1");
        assert!(selected.current);
        assert_eq!(selected.logo_url, "/media/image/42/card");
        let other = BrandLink::new(
            talocart_commerce::NavigationBrand {
                name: "Altro".into(),
                slug: "marchio-2".into(),
                logo_id: None,
            },
            "marchio-1",
        );
        assert!(!other.current);
        assert!(other.logo_url.is_empty());
        assert_eq!(other.initial, "A");
        let symbols = BrandLink::new(
            talocart_commerce::NavigationBrand {
                name: "& ébène".into(),
                slug: "marchio-3".into(),
                logo_id: None,
            },
            "",
        );
        assert_eq!(symbols.initial, "É");
        let empty = BrandLink::new(
            talocart_commerce::NavigationBrand {
                name: "+ &".into(),
                slug: "marchio-4".into(),
                logo_id: None,
            },
            "",
        );
        assert!(empty.initial.is_empty());
        assert!(CategoryMenu::default().brands.is_empty());
    }
    #[test]
    fn empty_menu_and_preorder_selection_preserve_sibling_order() {
        assert!(CategoryMenu::new(vec![], "", true).all_current);
        let menu = CategoryMenu::new(
            vec![
                category(3, Some(1), "First child"),
                category(5, Some(3), "Grandchild"),
                category(1, None, "Root"),
                category(4, Some(1), "Second child"),
                category(2, None, "Other root"),
            ],
            "category-5",
            false,
        );
        assert_eq!(menu.groups.len(), 2);
        let group = &menu.groups[0];
        assert_eq!(group.root.name, "Root");
        assert!(group.selected);
        assert!(!group.root.current);
        assert_eq!(
            group
                .columns
                .iter()
                .map(|column| column.root.name.as_str())
                .collect::<Vec<_>>(),
            ["First child", "Second child"]
        );
        assert!(group.columns[1].children.is_empty());
        assert_eq!(group.columns[0].children[0].name, "Grandchild");
        assert!(group.columns[0].children[0].ancestors.is_empty());
        assert_eq!(group.columns[0].children[0].href, "/catalog/category-5");
        assert!(group.columns[0].children[0].current);
        assert!(!menu.groups[1].selected);
    }

    #[test]
    fn columns_keep_deeper_paths_and_select_each_level_without_orphans() {
        for selected in ["category-1", "category-3", "category-5", "category-6"] {
            let menu = CategoryMenu::new(
                vec![
                    category(6, Some(5), "Fourth"),
                    category(5, Some(3), "Third"),
                    category(3, Some(1), "Second"),
                    category(1, None, "Root"),
                    category(7, Some(99), "Orphan"),
                    category(2, None, "Leaf root"),
                ],
                selected,
                false,
            );
            assert!(menu.groups[0].selected);
            assert!(!menu.groups[1].selected);
            assert!(menu.groups[1].columns.is_empty());
            let columns = &menu.groups[0].columns;
            assert_eq!(columns.len(), 1);
            assert_eq!(columns[0].children.len(), 2);
            assert_eq!(columns[0].children[1].ancestors, "Third");
            assert_eq!(columns[0].children[1].href, "/catalog/category-6");
        }
    }
}
