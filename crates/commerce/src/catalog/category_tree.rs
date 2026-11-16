//! Administrative preorder pagination with ancestor context. One bounded snapshot
//! supplies rows, matching counts and parent paths, including inactive ancestors.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub struct CategoryTreeEntry {
    pub entry: Entry,
    /// Root is depth zero; the database permits at most 16 levels.
    pub depth: u8,
    pub parent_path: String,
    /// Ancestor shown for orientation, outside this page's matching results.
    pub context: bool,
}
#[derive(FromRow)]
struct SnapshotEntry {
    #[sqlx(flatten)]
    entry: Entry,
    matching: bool,
}
impl Catalog {
    pub async fn category_tree_page(
        &self,
        session: &Session,
        q: &str,
        active: Option<bool>,
        page: i64,
    ) -> Result<talocart_platform::pagination::Page<CategoryTreeEntry>> {
        self.identity
            .authorize(session, Permission::CategoriesRead)
            .await?;
        if q.chars().count() > 120
            || q.chars().any(char::is_control)
            || !(1..=1_000_000).contains(&page)
        {
            return Err(CatalogError::Invalid(vec![]));
        }
        let rows = sqlx::query_as::<_, SnapshotEntry>(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT id,name,slug,description,sort_order,active,parent_id,NULL::text AS logo_key,\
                ((strpos(lower(name),lower($1))>0 \
                        OR strpos(slug,lower($1))>0) \
                    AND ($2::boolean IS NULL \
                        OR active=$2)) AS matching \
                FROM [[categories]] \
                ORDER BY sort_order,lower(name),id \
                LIMIT $3",
            self.language,
        )))
        .bind(q.trim())
        .bind(active)
        .bind(CATEGORY_LIMIT + 1)
        .fetch_all(&self.pool)
        .await?;
        tree_page(rows, page)
    }
}
fn tree_page(
    rows: Vec<SnapshotEntry>,
    requested: i64,
) -> Result<talocart_platform::pagination::Page<CategoryTreeEntry>> {
    if rows.len() > CATEGORY_LIMIT as usize {
        return Err(CatalogError::Limit);
    }
    let mut parents = BTreeMap::new();
    let mut by_parent: BTreeMap<Option<i64>, Vec<SnapshotEntry>> = BTreeMap::new();
    let mut matches = BTreeSet::new();
    for row in rows {
        if row.matching {
            matches.insert(row.entry.id);
        }
        if parents.insert(row.entry.id, row.entry.parent_id).is_some() {
            return Err(CatalogError::Hierarchy);
        }
        by_parent.entry(row.entry.parent_id).or_default().push(row);
    }
    let mut ordered = Vec::new();
    append(&mut by_parent, None, 0, "", &mut ordered)?;
    if !by_parent.is_empty() {
        return Err(CatalogError::Hierarchy);
    }
    let total = matches.len() as i64;
    let page = requested.clamp(1, ((total - 1).max(0) / PAGE_SIZE as i64) + 1);
    let selected: BTreeSet<_> = ordered
        .iter()
        .filter(|row| matches.contains(&row.entry.id))
        .skip((page as usize - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .map(|row| row.entry.id)
        .collect();
    let mut visible = selected.clone();
    for id in &selected {
        let mut next = parents.get(id).copied().flatten();
        while let Some(parent) = next {
            visible.insert(parent);
            next = parents.get(&parent).copied().flatten();
        }
    }
    let entries = ordered
        .into_iter()
        .filter(|row| visible.contains(&row.entry.id))
        .map(|mut row| {
            row.context = !selected.contains(&row.entry.id);
            row
        })
        .collect();
    Ok(talocart_platform::pagination::Page {
        entries,
        total,
        page,
    })
}
fn append(
    by_parent: &mut BTreeMap<Option<i64>, Vec<SnapshotEntry>>,
    parent: Option<i64>,
    depth: u8,
    parent_path: &str,
    out: &mut Vec<CategoryTreeEntry>,
) -> Result<()> {
    for row in by_parent.remove(&parent).unwrap_or_default() {
        if depth >= 16 {
            return Err(CatalogError::Hierarchy);
        }
        let id = row.entry.id;
        let path = if parent_path.is_empty() {
            row.entry.name.clone()
        } else {
            format!("{parent_path} / {}", row.entry.name)
        };
        out.push(CategoryTreeEntry {
            entry: row.entry,
            depth,
            parent_path: parent_path.into(),
            context: false,
        });
        append(by_parent, Some(id), depth + 1, &path, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(id: i64, parent_id: Option<i64>, matching: bool) -> SnapshotEntry {
        SnapshotEntry {
            matching,
            entry: Entry {
                id,
                parent_id,
                name: format!("Node {id}"),
                slug: format!("node-{id}"),
                description: String::new(),
                sort_order: 0,
                active: true,
                logo_key: None,
            },
        }
    }
    #[test]
    fn preorder_keeps_siblings_and_includes_only_needed_ancestors() {
        // Input already follows SQL sibling order; parent IDs need not precede children.
        let page = tree_page(
            vec![
                row(2, Some(9), true),
                row(3, Some(2), true),
                row(8, None, false),
                row(9, None, false),
                row(10, Some(9), false),
            ],
            1,
        )
        .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(
            page.entries
                .iter()
                .map(|r| (r.entry.id, r.depth, r.context))
                .collect::<Vec<_>>(),
            vec![(9, 0, true), (2, 1, false), (3, 2, false)]
        );
        assert_eq!(page.entries[2].parent_path, "Node 9 / Node 2");
    }
    #[test]
    fn paging_counts_matches_without_orphaning_children_and_bounds_invalid_graphs() {
        let mut rows = vec![row(100, None, true)];
        rows.extend((1..=23).map(|id| row(id, Some(100), true)));
        let page = tree_page(rows, 999).unwrap();
        assert_eq!((page.total, page.page), (24, 2));
        assert_eq!(
            page.entries
                .iter()
                .map(|r| (r.entry.id, r.context))
                .collect::<Vec<_>>(),
            vec![
                (100, true),
                (20, false),
                (21, false),
                (22, false),
                (23, false)
            ]
        );
        assert_eq!(
            (
                tree_page(vec![], 2).unwrap().total,
                tree_page(vec![], 2).unwrap().page
            ),
            (0, 1)
        );
        for rows in [
            vec![row(1, Some(9), true)],
            vec![row(1, Some(2), true), row(2, Some(1), true)],
            vec![row(1, None, true), row(1, None, true)],
        ] {
            assert!(tree_page(rows, 1).is_err());
        }
        assert!(tree_page((1..=1001).map(|id| row(id, None, true)).collect(), 1).is_err());
        assert!(
            tree_page(
                (1..=17)
                    .map(|id| row(id, if id == 1 { None } else { Some(id - 1) }, true))
                    .collect(),
                1
            )
            .is_err()
        );
        assert!(
            tree_page(
                (1..=16)
                    .map(|id| row(id, if id == 1 { None } else { Some(id - 1) }, true))
                    .collect(),
                1
            )
            .is_ok()
        );
    }
}
