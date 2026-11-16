use crate::i18n;

/// Shared presentation only: callers supply validated counts and contextual URLs.
/// Page sizes and database access remain the responsibility of each list's owner.
pub struct Pagination {
    pub label: String,
    pub aria_label: String,
    pub total: i64,
    pub first: i64,
    pub last: i64,
    pub page: i64,
    pub pages: i64,
    pub previous: String,
    pub next: String,
}
impl Pagination {
    pub fn products(
        total: i64,
        page: i64,
        size: u16,
        aria_label: &str,
        link: impl Fn(i64) -> String,
    ) -> Self {
        Self::new(
            total,
            page,
            size,
            &i18n::text("admin-products-2"),
            aria_label,
            link,
        )
    }
    pub fn new(
        total: i64,
        page: i64,
        size: u16,
        label: &str,
        aria_label: &str,
        link: impl Fn(i64) -> String,
    ) -> Self {
        // Defensive normalization keeps this presentation model safe for future lists.
        let total = total.max(0);
        let size = i64::from(size.max(1));
        let pages = if total == 0 {
            1
        } else {
            (total - 1) / size + 1
        };
        let page = page.clamp(1, pages);
        let offset = (page - 1) * size;
        Self {
            label: label.into(),
            aria_label: aria_label.into(),
            total,
            first: if total == 0 { 0 } else { offset + 1 },
            last: offset + size.min(total - offset),
            page,
            pages,
            previous: if page > 1 {
                link(page - 1)
            } else {
                String::new()
            },
            next: if page < pages {
                link(page + 1)
            } else {
                String::new()
            },
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_boundaries_and_empty_sets() {
        for (total, requested, size, expected) in [
            (168, 2, 21, (22, 42, 2, 8)),
            (43, 3, 21, (43, 43, 3, 3)),
            (42, 99, 21, (22, 42, 2, 2)),
            (0, 99, 21, (0, 0, 1, 1)),
            (
                i64::MAX,
                i64::MAX,
                84,
                (i64::MAX - 6, i64::MAX, i64::MAX / 84 + 1, i64::MAX / 84 + 1),
            ),
        ] {
            let p = Pagination::products(total, requested, size, "Pagine", |page| {
                format!("/catalog?brand=test&page={page}")
            });
            assert_eq!((p.first, p.last, p.page, p.pages), expected);
            assert_eq!(p.previous.is_empty(), p.page == 1);
            assert_eq!(p.next.is_empty(), p.page == p.pages);
            if !p.previous.is_empty() {
                assert!(p.previous.contains("brand=test"));
            }
        }
    }
}
