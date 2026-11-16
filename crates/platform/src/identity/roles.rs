//! Configurable permissions for implemented areas; the owner bypass is immutable policy.
use super::*;

pub const MAX_ROLES: i64 = 100;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    AdminRead,
    OperationsRead,
    SettingsRead,
    SettingsWrite,
    UsersRead,
    UsersWrite,
    RolesRead,
    RolesWrite,
    CategoriesRead,
    CategoriesWrite,
    BrandsRead,
    BrandsWrite,
    TaxesRead,
    TaxesWrite,
    ProductsRead,
    ProductsWrite,
    MediaRead,
    MediaWrite,
    PagesRead,
    PagesWrite,
    CustomersRead,
    CustomersWrite,
    OrdersRead,
    OrdersWrite,
}
impl Permission {
    pub fn parse(value: &str) -> Result<Self> {
        PERMISSIONS
            .into_iter()
            .find(|permission| permission.key() == value)
            .ok_or(IdentityError::Invalid)
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::AdminRead => "overview.read",
            Self::OperationsRead => "operations.read",
            Self::SettingsRead => "settings.read",
            Self::SettingsWrite => "settings.write",
            Self::UsersRead => "users.read",
            Self::UsersWrite => "users.write",
            Self::RolesRead => "roles.read",
            Self::RolesWrite => "roles.write",
            Self::CategoriesRead => "categories.read",
            Self::CategoriesWrite => "categories.write",
            Self::BrandsRead => "brands.read",
            Self::BrandsWrite => "brands.write",
            Self::TaxesRead => "taxes.read",
            Self::TaxesWrite => "taxes.write",
            Self::ProductsRead => "products.read",
            Self::ProductsWrite => "products.write",
            Self::MediaRead => "media.read",
            Self::MediaWrite => "media.write",
            Self::PagesRead => "pages.read",
            Self::PagesWrite => "pages.write",
            Self::CustomersRead => "customers.read",
            Self::CustomersWrite => "customers.write",
            Self::OrdersRead => "orders.read",
            Self::OrdersWrite => "orders.write",
        }
    }
}
pub const PERMISSIONS: [Permission; 24] = [
    Permission::AdminRead,
    Permission::OperationsRead,
    Permission::SettingsRead,
    Permission::SettingsWrite,
    Permission::UsersRead,
    Permission::UsersWrite,
    Permission::RolesRead,
    Permission::RolesWrite,
    Permission::CategoriesRead,
    Permission::CategoriesWrite,
    Permission::BrandsRead,
    Permission::BrandsWrite,
    Permission::TaxesRead,
    Permission::TaxesWrite,
    Permission::ProductsRead,
    Permission::ProductsWrite,
    Permission::MediaRead,
    Permission::MediaWrite,
    Permission::PagesRead,
    Permission::PagesWrite,
    Permission::CustomersRead,
    Permission::CustomersWrite,
    Permission::OrdersRead,
    Permission::OrdersWrite,
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Owner,
    Viewer,
    Custom(String),
}
impl Role {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "owner" => Ok(Self::Owner),
            "viewer" => Ok(Self::Viewer),
            _ if value.len() == 21
                && value.starts_with("role_")
                && value[5..]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
            {
                Ok(Self::Custom(value.to_owned()))
            }
            _ => Err(IdentityError::Invalid),
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::Owner => "owner",
            Self::Viewer => "viewer",
            Self::Custom(key) => key,
        }
    }
}
#[derive(FromRow, Clone)]
pub struct RoleDefinition {
    pub key: String,
    pub name: String,
    pub permissions: Vec<String>,
}
impl RoleDefinition {
    pub fn protected(&self) -> bool {
        self.key == "owner"
    }
    pub fn grantable_by(&self, actor: &Session) -> bool {
        actor.is_owner()
            || (!self.protected()
                && self
                    .permissions
                    .iter()
                    .all(|p| actor.permissions.contains(p)))
    }
    pub fn allows(&self, permission: Permission) -> bool {
        self.protected() || self.permissions.iter().any(|p| p == permission.key())
    }
}
fn role_name(value: &str) -> Result<String> {
    store_name(value)
}
/// Write permissions always include the matching read permission; unknown values cannot enter SQL.
fn normalized_permissions(permissions: &[Permission]) -> Vec<String> {
    let mut result = permissions
        .iter()
        .map(|p| p.key().to_owned())
        .collect::<Vec<_>>();
    for (write, read) in [
        (Permission::SettingsWrite, Permission::SettingsRead),
        (Permission::UsersWrite, Permission::UsersRead),
        (Permission::RolesWrite, Permission::RolesRead),
        (Permission::CategoriesWrite, Permission::CategoriesRead),
        (Permission::BrandsWrite, Permission::BrandsRead),
        (Permission::TaxesWrite, Permission::TaxesRead),
        (Permission::ProductsWrite, Permission::ProductsRead),
        (Permission::MediaWrite, Permission::MediaRead),
        (Permission::OrdersWrite, Permission::OrdersRead),
        (Permission::CustomersWrite, Permission::CustomersRead),
        (Permission::PagesWrite, Permission::PagesRead),
    ] {
        if permissions.contains(&write) {
            result.push(read.key().to_owned());
        }
    }
    result.sort();
    result.dedup();
    result
}
impl Identity {
    pub(super) async fn role_definition(&self, key: &str) -> Result<RoleDefinition> {
        sqlx::query_as("SELECT key,name,ARRAY(SELECT permission FROM admin_role_permissions p WHERE p.role_key=r.key ORDER BY permission) AS permissions FROM admin_roles r WHERE key=$1")
            .bind(key).fetch_optional(&self.pool).await?.ok_or(IdentityError::RoleNotFound)
    }
    pub(super) async fn role_locked(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        key: &str,
    ) -> Result<RoleDefinition> {
        sqlx::query_as("SELECT key,name,ARRAY(SELECT permission FROM admin_role_permissions p WHERE p.role_key=r.key ORDER BY permission) AS permissions FROM admin_roles r WHERE key=$1")
            .bind(key).fetch_optional(&mut **tx).await?.ok_or(IdentityError::RoleNotFound)
    }
    /// Delegates may grant/manage only permissions they already possess, and never the owner role.
    pub(super) fn ensure_grant(actor: &Session, role: &RoleDefinition) -> Result<()> {
        if role.grantable_by(actor) {
            Ok(())
        } else {
            Err(IdentityError::Forbidden)
        }
    }

    pub async fn roles_counted(
        &self,
        session: &Session,
        page: i64,
    ) -> Result<crate::pagination::Page<RoleDefinition>> {
        self.authorize(session, Permission::RolesRead).await?;
        if !(1..=1_000_000).contains(&page) {
            return Err(IdentityError::Invalid);
        }
        Ok(crate::pagination::read(&self.pool, sqlx::query("SELECT key,name,ARRAY(SELECT permission FROM admin_role_permissions p WHERE p.role_key=r.key ORDER BY permission) AS permissions FROM admin_roles r"), "(key='owner') DESC,lower(name),key", page, 20).await?)
    }

    pub async fn roles(&self, session: &Session) -> Result<Vec<RoleDefinition>> {
        self.authorize(session, Permission::RolesRead).await?;
        self.role_catalog().await
    }
    pub async fn role(&self, session: &Session, key: &str) -> Result<RoleDefinition> {
        self.authorize(session, Permission::RolesRead).await?;
        self.role_definition(key).await
    }
    async fn role_catalog(&self) -> Result<Vec<RoleDefinition>> {
        Ok(sqlx::query_as("SELECT key,name,ARRAY(SELECT permission FROM admin_role_permissions p WHERE p.role_key=r.key ORDER BY permission) AS permissions FROM admin_roles r ORDER BY (key='owner') DESC,lower(name),key LIMIT $1")
            .bind(MAX_ROLES).fetch_all(&self.pool).await?)
    }
    pub async fn assignable_roles(&self, session: &Session) -> Result<Vec<RoleDefinition>> {
        self.authorize(session, Permission::UsersRead).await?;
        // Session was loaded by the request; write services recheck grants under the common lock.
        Ok(self
            .role_catalog()
            .await?
            .into_iter()
            .filter(|r| Self::ensure_grant(session, r).is_ok())
            .collect())
    }
    pub async fn create_role(
        &self,
        session: &Session,
        csrf: &str,
        name: &str,
        permissions: &[Permission],
    ) -> Result<String> {
        session.check_csrf(csrf)?;
        session.require(Permission::RolesWrite)?;
        let name = role_name(name)?;
        let key = format!("role_{}", &token()?[..16]);
        let definition = RoleDefinition {
            key: key.clone(),
            name,
            permissions: normalized_permissions(permissions),
        };
        let mut tx = self.pool.begin().await?;
        Self::lock_changes(&mut tx).await?;
        let actor = Self::authorize_locked(&mut tx, session, Permission::RolesWrite).await?;
        Self::ensure_grant(&actor, &definition)?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_roles")
            .fetch_one(&mut *tx)
            .await?;
        if count >= MAX_ROLES {
            return Err(IdentityError::RoleLimit);
        }
        let inserted =
            sqlx::query("INSERT INTO admin_roles(key,name) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                .bind(&key)
                .bind(&definition.name)
                .execute(&mut *tx)
                .await?;
        if inserted.rows_affected() != 1 {
            return Err(IdentityError::DuplicateRole);
        }
        Self::write_permissions(&mut tx, &definition).await?;
        tx.commit().await?;
        Ok(key)
    }
    async fn write_permissions(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        role: &RoleDefinition,
    ) -> Result<()> {
        sqlx::query("DELETE FROM admin_role_permissions WHERE role_key=$1")
            .bind(&role.key)
            .execute(&mut **tx)
            .await?;
        sqlx::query(
            "INSERT INTO admin_role_permissions(role_key,permission) SELECT $1,unnest($2::text[])",
        )
        .bind(&role.key)
        .bind(&role.permissions)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
    pub async fn update_role(
        &self,
        session: &Session,
        csrf: &str,
        key: &str,
        name: &str,
        permissions: &[Permission],
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        session.require(Permission::RolesWrite)?;
        if key == "owner" {
            return Err(IdentityError::ProtectedRole);
        }
        let name = role_name(name)?;
        let mut tx = self.pool.begin().await?;
        Self::lock_changes(&mut tx).await?;
        let actor = Self::authorize_locked(&mut tx, session, Permission::RolesWrite).await?;
        let current = Self::role_locked(&mut tx, key).await?;
        Self::ensure_grant(&actor, &current)?;
        let next = RoleDefinition {
            key: key.to_owned(),
            name,
            permissions: normalized_permissions(permissions),
        };
        Self::ensure_grant(&actor, &next)?;
        let changed = current.permissions != next.permissions;
        let result = sqlx::query("UPDATE admin_roles SET name=$1 WHERE key=$2")
            .bind(&next.name)
            .bind(key)
            .execute(&mut *tx)
            .await;
        match result {
            Ok(_) => {}
            Err(e)
                if e.as_database_error()
                    .is_some_and(|e| e.is_unique_violation()) =>
            {
                return Err(IdentityError::DuplicateRole);
            }
            Err(e) => return Err(e.into()),
        }
        if changed {
            Self::write_permissions(&mut tx, &next).await?;
            sqlx::query("DELETE FROM admin_sessions s USING administrators a WHERE s.administrator_id=a.id AND a.role=$1")
                .bind(key).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn delete_role(&self, session: &Session, csrf: &str, key: &str) -> Result<()> {
        session.check_csrf(csrf)?;
        session.require(Permission::RolesWrite)?;
        if key == "owner" {
            return Err(IdentityError::ProtectedRole);
        }
        let mut tx = self.pool.begin().await?;
        Self::lock_changes(&mut tx).await?;
        let actor = Self::authorize_locked(&mut tx, session, Permission::RolesWrite).await?;
        let role = Self::role_locked(&mut tx, key).await?;
        Self::ensure_grant(&actor, &role)?;
        let assigned: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators WHERE role=$1)")
                .bind(key)
                .fetch_one(&mut *tx)
                .await?;
        if assigned {
            return Err(IdentityError::RoleInUse);
        }
        sqlx::query("DELETE FROM admin_roles WHERE key=$1")
            .bind(key)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn role_keys_and_write_permissions_obey_contract() {
        assert!(Role::parse("root").is_err());
        assert!(Role::parse("role_0123456789abcdef").is_ok());
        assert_eq!(
            normalized_permissions(&[Permission::UsersWrite, Permission::UsersWrite]),
            vec!["users.read", "users.write"]
        );
    }
}
