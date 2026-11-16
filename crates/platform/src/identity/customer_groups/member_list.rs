//! Bounded read-only customer table. SQL ordering is selected only from static values.
use super::*;
use crate::pagination::{AdminPageSize, Page};
use serde::Deserialize;

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct MemberFilters {
    pub name: String,
    pub email: String,
    pub group: String,
    pub kind: String,
    pub state: String,
    pub sort: String,
    pub dir: String,
    pub per_page: String,
}
impl MemberFilters {
    pub fn active(&self) -> bool {
        [
            &self.name,
            &self.email,
            &self.group,
            &self.kind,
            &self.state,
        ]
        .iter()
        .any(|s| !s.trim().is_empty())
    }
    pub fn page_size(&self) -> GroupResult<AdminPageSize> {
        AdminPageSize::parse(&self.per_page).map_err(|_| invalid())
    }
    fn validate(&self) -> GroupResult<()> {
        for value in [&self.name, &self.email] {
            if value.chars().count() > 120 || value.chars().any(char::is_control) {
                return Err(invalid());
            }
        }
        if !["", "private", "business"].contains(&self.kind.as_str())
            || !["", "active", "inactive"].contains(&self.state.as_str())
        {
            return Err(invalid());
        }
        self.group_id()?;
        self.page_size()?;
        MemberOrder::parse(&self.sort, &self.dir)?;
        Ok(())
    }
    fn group_id(&self) -> GroupResult<Option<i64>> {
        if self.group.is_empty() {
            return Ok(None);
        }
        self.group
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .map(Some)
            .ok_or_else(invalid)
    }
}
fn invalid() -> GroupError {
    GroupError::Invalid(vec!["filters"])
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MemberSort {
    Name,
    Email,
    Group,
    Kind,
    State,
}
impl MemberSort {
    pub fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Email => "email",
            Self::Group => "group",
            Self::Kind => "kind",
            Self::State => "state",
        }
    }
}
#[derive(Clone, Copy)]
pub struct MemberOrder {
    pub column: MemberSort,
    pub descending: bool,
}
impl Default for MemberOrder {
    fn default() -> Self {
        Self {
            column: MemberSort::Name,
            descending: false,
        }
    }
}
impl MemberOrder {
    pub fn parse(sort: &str, dir: &str) -> GroupResult<Self> {
        let column = match sort {
            "" | "name" => MemberSort::Name,
            "email" => MemberSort::Email,
            "group" => MemberSort::Group,
            "kind" => MemberSort::Kind,
            "state" => MemberSort::State,
            _ => return Err(invalid()),
        };
        let descending = match dir {
            "" | "asc" => false,
            "desc" => true,
            _ => return Err(invalid()),
        };
        Ok(Self { column, descending })
    }
    pub fn direction(self) -> &'static str {
        if self.descending { "desc" } else { "asc" }
    }
    fn sql(self, locale: &str) -> &'static str {
        match (self.column, self.descending, locale == "it") {
            (MemberSort::Name, false, _) => {
                "lower(coalesce(nullif(trim(first_name || ' ' || last_name),''),email)),id"
            }
            (MemberSort::Name, true, _) => {
                "lower(coalesce(nullif(trim(first_name || ' ' || last_name),''),email)) DESC,id DESC"
            }
            (MemberSort::Email, false, _) => "lower(email),id",
            (MemberSort::Email, true, _) => "lower(email) DESC,id DESC",
            (MemberSort::Group, false, true) => "lower(name_it),id",
            (MemberSort::Group, true, true) => "lower(name_it) DESC,id DESC",
            (MemberSort::Group, false, false) => "lower(name_en),id",
            (MemberSort::Group, true, false) => "lower(name_en) DESC,id DESC",
            (MemberSort::Kind, false, _) => "customer_type,id",
            (MemberSort::Kind, true, _) => "customer_type DESC,id DESC",
            (MemberSort::State, false, _) => "active,id",
            (MemberSort::State, true, _) => "active DESC,id DESC",
        }
    }
}
// Escape LIKE metacharacters: search terms are literal substrings, never SQL patterns.
fn pattern(value: &str) -> String {
    format!(
        "%{}%",
        value
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}
impl CustomerGroups {
    pub async fn filtered_members(
        &self,
        s: &Session,
        filters: &MemberFilters,
        page: i64,
        locale: &str,
    ) -> GroupResult<Page<Member>> {
        self.identity
            .authorize(s, Permission::CustomersRead)
            .await?;
        valid_page(page)?;
        filters.validate()?;
        let sql = format!(
            "{MEMBERS} WHERE (c.first_name || ' ' || c.last_name ILIKE $1 OR (c.first_name='' AND c.last_name='' AND c.email ILIKE $1)) AND c.email ILIKE $2 AND ($3::bigint IS NULL OR c.group_id=$3) AND ($4='' OR c.customer_type=$4) AND ($5::boolean IS NULL OR c.active=$5)"
        );
        let query = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(pattern(&filters.name))
            .bind(pattern(&filters.email))
            .bind(filters.group_id()?)
            .bind(&filters.kind)
            .bind(match filters.state.as_str() {
                "active" => Some(true),
                "inactive" => Some(false),
                _ => None,
            });
        Ok(crate::pagination::read(
            &self.pool,
            query,
            MemberOrder::parse(&filters.sort, &filters.dir)?.sql(locale),
            page,
            filters.page_size()?.value(),
        )
        .await?)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_and_order_are_bounded_and_allowlisted() {
        assert!(MemberFilters::default().validate().is_ok());
        for (field, value) in [
            ("sort", "id; DROP TABLE customers"),
            ("dir", "up"),
            ("per_page", "500"),
            ("group", "0"),
            ("group", "9223372036854775808"),
            ("kind", "admin"),
            ("state", "all"),
            ("name", "line\nbreak"),
            ("email", &"x".repeat(121)),
        ] {
            let mut f = MemberFilters::default();
            match field {
                "sort" => f.sort = value.into(),
                "dir" => f.dir = value.into(),
                "per_page" => f.per_page = value.into(),
                "group" => f.group = value.into(),
                "kind" => f.kind = value.into(),
                "state" => f.state = value.into(),
                "name" => f.name = value.into(),
                _ => f.email = value.into(),
            }
            assert!(f.validate().is_err(), "{field}");
        }
        assert_eq!(pattern(" %_\\ "), "%\\%\\_\\\\%");
        for sort in ["name", "email", "group", "kind", "state"] {
            for dir in ["asc", "desc"] {
                for locale in ["en", "it"] {
                    assert!(
                        MemberOrder::parse(sort, dir)
                            .expect("allowed sort")
                            .sql(locale)
                            .contains("id")
                    );
                }
            }
        }
    }
}
