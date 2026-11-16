//! Shared accessible state for sortable native table headers.
pub(crate) struct SortState {
    pub aria: &'static str,
    pub indicator: &'static str,
    pub action: String,
    pub next_descending: bool,
}
impl SortState {
    pub fn new(active: bool, descending: bool, label: String) -> Self {
        let next_descending = active && !descending;
        let mut args = fluent_bundle::FluentArgs::new();
        args.set("label", label);
        Self {
            aria: match (active, descending) {
                (false, _) => "none",
                (true, true) => "descending",
                (true, false) => "ascending",
            },
            indicator: match (active, descending) {
                (false, _) => "↕",
                (true, true) => "↓",
                (true, false) => "↑",
            },
            action: crate::i18n::with_args(
                if next_descending {
                    "admin-sort-descending"
                } else {
                    "admin-sort-ascending"
                },
                &args,
            ),
            next_descending,
        }
    }
}
