//! Indexed sources retained independently of commercial references. Keys never change on moves.
use super::*;
use crate::identity::{Identity, IdentityError, Permission, Session};
use sqlx::{FromRow, PgPool};

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error(
        "Usa un nome da 1 a 255 caratteri: senza caratteri di controllo, separatori o doppio punto"
    )]
    Name,
    #[error("Scegli un file non vuoto fino a 10 MiB")]
    FileInvalid,
    #[error("File o cartella non trovata")]
    NotFound,
    #[error("Esiste già un file o una cartella con questo nome")]
    Collision,
    #[error("La risorsa è utilizzata, protetta o contiene altri elementi")]
    InUse,
    #[error("Non puoi spostare una cartella al suo interno o cambiare la riservatezza del file")]
    Destination,
    #[error("{0}")]
    Attachment(String),
    #[error("Conferma l’operazione")]
    Confirmation,
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Media(#[from] MediaError),
}
impl From<sqlx::Error> for LibraryError {
    fn from(error: sqlx::Error) -> Self {
        if error
            .as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            Self::Collision
        } else if error
            .as_database_error()
            .is_some_and(|e| e.is_foreign_key_violation())
        {
            Self::InUse
        } else {
            let _ = crate::technical::DatabaseFailure::capture(error, module_path!());
            Self::Media(MediaError::Storage)
        }
    }
}
pub type Result<T> = std::result::Result<T, LibraryError>;
#[derive(Clone, FromRow)]
pub struct Folder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub path: String,
    pub owner_id: Option<i64>,
    pub protected: bool,
}
/// A bounded branch of the folder navigation; file queries remain independent.
#[derive(FromRow)]
pub struct FolderTreeNode {
    pub id: i64,
    pub name: String,
    pub has_children: bool,
}
pub struct FolderBranch {
    pub parent: Option<i64>,
    pub entries: crate::pagination::Page<FolderTreeNode>,
}
#[derive(FromRow)]
struct FolderBranchRow {
    parent: Option<i64>,
    total: i64,
    page: i64,
    id: i64,
    name: String,
    has_children: bool,
}
#[derive(FromRow)]
pub struct Asset {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub folder_id: i64,
    pub path: String,
    pub width: i32,
    pub height: i32,
    pub byte_size: i64,
    pub content_type: String,
    pub owner_id: Option<i64>,
    pub trashed: bool,
    pub uses: i64,
    pub cache_pending: bool,
}
impl Asset {
    pub fn extension_label(&self) -> String {
        extension(&self.name)
            .chars()
            .take(12)
            .collect::<String>()
            .to_uppercase()
    }
    pub fn is_image(&self) -> bool {
        self.width > 0 && self.height > 0
    }
    pub fn display_path(&self) -> &str {
        if self.path.is_empty() {
            "/"
        } else {
            &self.path
        }
    }
    pub fn preview(&self) -> String {
        format!("/admin/media/{}/preview", self.id)
    }
    pub fn weight(&self) -> String {
        format!(
            "{} KiB",
            self.byte_size / 1024 + i64::from(self.byte_size % 1024 != 0)
        )
    }
}
#[derive(FromRow)]
pub struct Usage {
    pub kind: String,
    pub resource_id: i64,
    pub label: String,
    pub href: String,
}
#[derive(Default, Clone)]
pub struct Filter {
    pub folder: Option<i64>,
    pub query: String,
    pub format: String,
    pub usage: String,
    pub order: String,
    pub page: i64,
    pub trash: bool,
    pub personal: bool,
    pub images_only: bool,
}
#[derive(Clone)]
pub struct MediaLibrary {
    pool: PgPool,
    identity: Identity,
    store: MediaStore,
}
const ASSET_SELECT: &str = "SELECT m.id,m.key,l.name,l.folder_id,f.path,m.width,m.height,m.byte_size,m.content_type,l.owner_id,l.trashed_at IS NOT NULL AS trashed,(SELECT count(*) FROM media_usage u WHERE u.key=m.key OR u.key IN(SELECT object_key FROM media_transforms WHERE source_key=m.key)) AS uses,coalesce((SELECT status IN ('pending','running') FROM media_cache_jobs j WHERE j.source_key=m.key),false) AS cache_pending FROM media_library l JOIN media_objects m ON m.key=l.key JOIN media_folders f ON f.id=l.folder_id";
// Count direct and derived references once for the whole list. DISTINCT preserves
// the old OR/IN semantics when several transforms share an object; self transforms
// must not count a direct reference twice. Single-asset reads keep ASSET_SELECT.
const ASSET_LIST_SELECT: &str = "WITH usages AS MATERIALIZED (SELECT key,count(*)::bigint n FROM media_usage GROUP BY key), usage_totals AS MATERIALIZED (SELECT key,sum(n)::bigint n FROM (SELECT key,n FROM usages UNION ALL SELECT t.source_key,u.n FROM (SELECT DISTINCT source_key,object_key FROM media_transforms WHERE source_key<>object_key) t JOIN usages u ON u.key=t.object_key) all_usage GROUP BY key) SELECT m.id,m.key,l.name,l.folder_id,f.path,m.width,m.height,m.byte_size,m.content_type,l.owner_id,l.trashed_at IS NOT NULL AS trashed,coalesce(uc.n,0)::bigint AS uses,coalesce((SELECT status IN ('pending','running') FROM media_cache_jobs j WHERE j.source_key=m.key),false) AS cache_pending FROM media_library l JOIN media_objects m ON m.key=l.key JOIN media_folders f ON f.id=l.folder_id LEFT JOIN usage_totals uc ON uc.key=m.key";
/// Physical segment policy also applies to acquisition; no paths, symlinks or hidden names.
pub fn segment(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 255
        || value.trim() != value
        || value.contains("..")
        || value.starts_with('.')
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        Err(LibraryError::Name)
    } else {
        Ok(value)
    }
}
impl MediaLibrary {
    pub fn new(pool: PgPool, identity: Identity, store: MediaStore) -> Self {
        Self {
            pool,
            identity,
            store,
        }
    }
    pub fn store(&self) -> &MediaStore {
        &self.store
    }
    async fn access(&self, s: &Session, personal: bool) -> Result<()> {
        if personal {
            self.identity.account(s).await?;
        } else {
            self.identity.authorize(s, Permission::MediaRead).await?;
        }
        Ok(())
    }
    async fn write(
        &self,
        s: &Session,
        csrf: &str,
        personal: bool,
    ) -> Result<Transaction<'_, Postgres>> {
        s.check_csrf(csrf)?;
        let mut tx = self.pool.begin().await?;
        MediaStore::exclusive_lock(&mut tx).await?;
        if personal {
            Identity::authorize_account_transaction(&mut tx, s, csrf).await?;
        } else {
            Identity::authorize_transaction(&mut tx, s, csrf, Permission::MediaWrite).await?;
        }
        Ok(tx)
    }
    pub async fn destination(&self, s: &Session, path: &str, personal: bool) -> Result<Folder> {
        self.access(s, personal).await?;
        let path = path
            .strip_prefix('/')
            .filter(|_| path == "/")
            .unwrap_or(path);
        sqlx::query_as(
            "SELECT * FROM media_folders WHERE path=$1 AND owner_id IS NOT DISTINCT FROM $2",
        )
        .bind(path)
        .bind(if personal { s.administrator_id } else { None })
        .fetch_optional(&self.pool)
        .await?
        .ok_or(LibraryError::Destination)
    }
    pub async fn folders_counted(
        &self,
        s: &Session,
        parent: Option<i64>,
        page: i64,
        personal: bool,
    ) -> Result<crate::pagination::Page<Folder>> {
        self.access(s, personal).await?;
        if let Some(id) = parent {
            self.folder(s, id, personal).await?;
        }
        page_offset(page, 100)?;
        Ok(crate::pagination::read(&self.pool, sqlx::query("SELECT * FROM media_folders WHERE parent_id IS NOT DISTINCT FROM $1 AND owner_id IS NOT DISTINCT FROM $2")
            .bind(parent).bind(if personal { s.administrator_id } else { None }), "lower(name),id", page.max(1),100).await?)
    }

    /// Root siblings and the open ancestry, with physical visibility checked before paging.
    /// Each branch is capped at 100 entries. Ancestor pages contain the active child;
    /// only the selected folder's children use the requested pagination number.
    pub async fn folder_tree(
        &self,
        s: &Session,
        selected: Option<i64>,
        page: i64,
        personal: bool,
    ) -> Result<Vec<FolderBranch>> {
        self.access(s, personal).await?;
        page_offset(page, 100)?;
        let owner = if personal { s.administrator_id } else { None };
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        // The visited IDs also bound malformed cyclic hierarchies. Paths are <=1024
        // bytes, so 1024 ancestors exceed every valid filesystem path's depth.
        let ancestors: Vec<Folder> = sqlx::query_as(
            "WITH RECURSIVE chain AS (
                SELECT f.*, ARRAY[f.id] visited FROM media_folders f
                WHERE f.id=$1 AND f.owner_id IS NOT DISTINCT FROM $2
                UNION ALL
                SELECT f.*, c.visited || f.id FROM media_folders f JOIN chain c ON f.id=c.parent_id
                WHERE f.owner_id IS NOT DISTINCT FROM $2 AND NOT f.id=ANY(c.visited)
                    AND cardinality(c.visited)<1024
             ) SELECT * FROM chain ORDER BY cardinality(visited) DESC",
        )
        .bind(selected)
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        if selected.is_some() && ancestors.is_empty() {
            return Err(LibraryError::NotFound);
        }
        let ancestor_ids: Vec<i64> = ancestors.iter().map(|f| f.id).collect();
        let candidates: Vec<(i64, String)> = sqlx::query_as(
            "WITH siblings AS (SELECT id FROM media_folders WHERE owner_id IS NOT DISTINCT FROM $2
                AND (parent_id IS NULL OR parent_id=ANY($1)))
             SELECT id,path FROM media_folders WHERE owner_id IS NOT DISTINCT FROM $2
                AND (id=ANY($1) OR id IN(SELECT id FROM siblings) OR parent_id IN(SELECT id FROM siblings))",
        ).bind(&ancestor_ids).bind(owner).fetch_all(&mut *tx).await?;
        let mut visible = Vec::new();
        for (id, path) in candidates {
            if self.store.library_directory_exists(&path).await? {
                visible.push(id);
            }
        }
        if ancestors
            .iter()
            .any(|f| !f.path.is_empty() && !visible.contains(&f.id))
        {
            return Err(LibraryError::NotFound);
        }
        let mut parents = vec![None];
        parents.extend(ancestors.iter().map(|f| Some(f.id)));
        let mut children: Vec<Option<i64>> = ancestors.iter().map(|f| Some(f.id)).collect();
        children.push(None);
        let rows: Vec<FolderBranchRow> = sqlx::query_as(
            "WITH folders AS (
                SELECT id,parent_id,CASE WHEN path='library' THEN 'library' ELSE name END name
                FROM media_folders WHERE id=ANY($6) AND owner_id IS NOT DISTINCT FROM $3
             ), branches AS (
                SELECT * FROM unnest($1::bigint[], $2::bigint[]) b(parent,child)
             ), counts AS (
                SELECT b.parent, count(f.id) total,
                    count(f.id) FILTER(WHERE (lower(f.name),f.id)<(lower(c.name),c.id))/100+1 containing_page
                FROM branches b LEFT JOIN folders c ON c.id=b.child
                LEFT JOIN folders f ON f.parent_id IS NOT DISTINCT FROM b.parent
                GROUP BY b.parent
             ), pages AS (
                SELECT parent,total,CASE WHEN parent IS NOT DISTINCT FROM $4
                    THEN least(greatest($5::bigint,1),greatest((total+99)/100,1))
                    ELSE containing_page END page FROM counts
             ) SELECT p.parent,p.total,p.page,coalesce(f.id,0) id,coalesce(f.name,'') name,
                EXISTS(SELECT 1 FROM folders c WHERE c.parent_id=f.id) has_children
             FROM pages p LEFT JOIN LATERAL (
                SELECT id,name FROM folders f
                WHERE f.parent_id IS NOT DISTINCT FROM p.parent
                ORDER BY lower(name),id LIMIT 100 OFFSET (p.page-1)*100
             ) f ON true ORDER BY p.parent NULLS FIRST,lower(f.name),f.id")
            .bind(parents).bind(children).bind(owner).bind(selected).bind(page.max(1)).bind(visible)
            .fetch_all(&mut *tx).await?;
        let mut branches: Vec<FolderBranch> = Vec::new();
        for row in rows {
            if branches
                .last()
                .is_none_or(|branch| branch.parent != row.parent)
            {
                branches.push(FolderBranch {
                    parent: row.parent,
                    entries: crate::pagination::Page {
                        entries: Vec::new(),
                        total: row.total,
                        page: row.page,
                    },
                });
            }
            if row.id != 0
                && let Some(branch) = branches.last_mut()
            {
                branch.entries.entries.push(FolderTreeNode {
                    id: row.id,
                    name: row.name,
                    has_children: row.has_children,
                });
            }
        }
        tx.commit().await?;
        Ok(branches)
    }

    pub async fn folders(
        &self,
        s: &Session,
        parent: Option<i64>,
        page: i64,
        personal: bool,
    ) -> Result<Vec<Folder>> {
        self.access(s, personal).await?;
        let owner = if personal { s.administrator_id } else { None };
        Ok(sqlx::query_as("SELECT * FROM media_folders WHERE parent_id IS NOT DISTINCT FROM $1 AND owner_id IS NOT DISTINCT FROM $2 ORDER BY lower(name),id LIMIT 101 OFFSET $3")
           .bind(parent).bind(owner).bind(page_offset(page,100)?).fetch_all(&self.pool).await?)
    }
    pub async fn folder(&self, s: &Session, id: i64, personal: bool) -> Result<Folder> {
        self.access(s, personal).await?;
        let row: Folder = sqlx::query_as("SELECT * FROM media_folders WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(LibraryError::NotFound)?;
        if (personal || row.owner_id.is_some()) && row.owner_id != s.administrator_id {
            return Err(LibraryError::NotFound);
        }
        Ok(row)
    }
    pub async fn assets_counted(
        &self,
        s: &Session,
        filter: &Filter,
    ) -> Result<crate::pagination::Page<Asset>> {
        self.access(s, filter.personal).await?;
        if filter.query.len() > 160
            || !matches!(
                filter.format.as_str(),
                "" | "png" | "jpeg" | "webp" | "pdf" | "other"
            )
            || !matches!(filter.usage.as_str(), "" | "used" | "unused")
        {
            return Err(LibraryError::Name);
        }
        if let Some(id) = filter.folder {
            self.folder(s, id, filter.personal).await?;
        }
        let sort = match filter.order.as_str() {
            "" | "name" => "lower(l.name),m.id",
            "newest" => "m.id DESC",
            "size" => "m.byte_size DESC,m.id",
            "dimensions" => "m.width::bigint*m.height DESC,m.id",
            _ => return Err(LibraryError::Name),
        };
        let sql = format!(
            "{ASSET_LIST_SELECT} WHERE (($1::bigint IS NULL AND l.owner_id IS NULL) OR l.owner_id=$1) AND ($2::bigint IS NULL OR l.folder_id=$2) AND ($3='' OR strpos(lower(l.name),lower($3))>0) AND ($4='' OR m.content_type='image/'||$4 OR $4='pdf' AND m.content_type='application/pdf' OR $4='other' AND m.width=0 AND m.content_type<>'application/pdf') AND (l.trashed_at IS NOT NULL)=$5 AND ($6='' OR (coalesce(uc.n,0)>0)=($6='used')) AND (NOT $7 OR m.width>0)"
        );
        page_offset(filter.page, 40)?;
        Ok(crate::pagination::read(
            &self.pool,
            // Optional filters otherwise let PostgreSQL reuse a generic plan with
            // severely underestimated shared rows and quadratic usage-total joins.
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .persistent(false)
                .bind(if filter.personal {
                    s.administrator_id
                } else {
                    None
                })
                .bind(filter.folder)
                .bind(&filter.query)
                .bind(&filter.format)
                .bind(filter.trash)
                .bind(&filter.usage)
                .bind(filter.images_only),
            sort,
            filter.page.max(1),
            40,
        )
        .await?)
    }
    pub async fn assets(&self, s: &Session, filter: &Filter) -> Result<Vec<Asset>> {
        self.access(s, filter.personal).await?;
        if filter.query.len() > 160
            || !matches!(
                filter.format.as_str(),
                "" | "png" | "jpeg" | "webp" | "pdf" | "other"
            )
            || !matches!(filter.usage.as_str(), "" | "used" | "unused")
        {
            return Err(LibraryError::Name);
        }
        if let Some(id) = filter.folder {
            self.folder(s, id, filter.personal).await?;
        }
        let sort = match filter.order.as_str() {
            "" | "name" => "lower(l.name),m.id",
            "newest" => "m.id DESC",
            "size" => "m.byte_size DESC,m.id",
            "dimensions" => "m.width::bigint*m.height DESC,m.id",
            _ => return Err(LibraryError::Name),
        };
        let sql = format!(
            "{ASSET_LIST_SELECT} WHERE (($1::bigint IS NULL AND l.owner_id IS NULL) OR l.owner_id=$1) AND ($2::bigint IS NULL OR l.folder_id=$2) AND ($3='' OR strpos(lower(l.name),lower($3))>0) AND ($4='' OR m.content_type='image/'||$4 OR $4='pdf' AND m.content_type='application/pdf' OR $4='other' AND m.width=0 AND m.content_type<>'application/pdf') AND (l.trashed_at IS NOT NULL)=$5 AND ($6='' OR (coalesce(uc.n,0)>0)=($6='used')) AND (NOT $7 OR m.width>0) ORDER BY {sort} LIMIT 41 OFFSET $8"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .persistent(false)
            .bind(if filter.personal {
                s.administrator_id
            } else {
                None
            })
            .bind(filter.folder)
            .bind(&filter.query)
            .bind(&filter.format)
            .bind(filter.trash)
            .bind(&filter.usage)
            .bind(filter.images_only)
            .bind(page_offset(filter.page, 40)?)
            .fetch_all(&self.pool)
            .await?)
    }
    pub async fn asset(&self, s: &Session, id: i64, personal: bool) -> Result<Asset> {
        self.access(s, personal).await?;
        let sql = format!("{ASSET_SELECT} WHERE m.id=$1 AND l.owner_id IS NOT DISTINCT FROM $2");
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .bind(if personal { s.administrator_id } else { None })
            .fetch_optional(&self.pool)
            .await?
            .ok_or(LibraryError::NotFound)
    }
    pub async fn usages(&self, s: &Session, id: i64, personal: bool) -> Result<Vec<Usage>> {
        self.usages_page(s, id, personal, 1).await
    }
    pub async fn usages_counted(
        &self,
        s: &Session,
        id: i64,
        personal: bool,
        page: i64,
    ) -> Result<crate::pagination::Page<Usage>> {
        let asset = self.asset(s, id, personal).await?;
        page_offset(page, 100)?;
        let mut result: crate::pagination::Page<Usage> = crate::pagination::read(&self.pool, sqlx::query("SELECT kind,resource_id,label,href FROM media_usage WHERE key=$1 OR key IN(SELECT object_key FROM media_transforms WHERE source_key=$1)").bind(asset.key), "kind,resource_id", page.max(1),100).await?;
        // A media reader must not see commercial labels without that area's read permission.
        for row in &mut result.entries {
            let allowed = match row.kind.as_str() {
                "Prodotto" | "Variante" | "Galleria" => s.allows(Permission::ProductsRead),
                "Marchio" => s.allows(Permission::BrandsRead),
                "Banner" => s.allows(Permission::SettingsRead),
                "Avatar" => Some(row.resource_id) == s.administrator_id,
                _ => s.allows(Permission::SettingsRead),
            };
            if !allowed {
                row.label = "Risorsa collegata".into();
                row.href.clear();
            }
        }
        Ok(result)
    }
    pub async fn usages_page(
        &self,
        s: &Session,
        id: i64,
        personal: bool,
        page: i64,
    ) -> Result<Vec<Usage>> {
        let asset = self.asset(s, id, personal).await?;
        let mut rows:Vec<Usage>=sqlx::query_as("SELECT kind,resource_id,label,href FROM media_usage WHERE key=$1 OR key IN(SELECT object_key FROM media_transforms WHERE source_key=$1) ORDER BY kind,resource_id LIMIT 101 OFFSET $2").bind(asset.key).bind(page_offset(page,100)?).fetch_all(&self.pool).await?;
        // A media reader must not see commercial labels without that area's read permission.
        for row in &mut rows {
            let allowed = match row.kind.as_str() {
                "Prodotto" | "Variante" | "Galleria" => s.allows(Permission::ProductsRead),
                "Marchio" => s.allows(Permission::BrandsRead),
                "Banner" => s.allows(Permission::SettingsRead),
                "Avatar" => Some(row.resource_id) == s.administrator_id,
                _ => s.allows(Permission::SettingsRead),
            };
            if !allowed {
                row.label = "Risorsa collegata".into();
                row.href.clear();
            }
        }
        Ok(rows)
    }
    pub async fn create_folder(
        &self,
        s: &Session,
        csrf: &str,
        parent: Option<i64>,
        name: &str,
        personal: bool,
    ) -> Result<i64> {
        segment(name)?;
        let mut tx = self.write(s, csrf, personal).await?;
        let owner = if personal { s.administrator_id } else { None };
        let parent = match parent {
            Some(id) => id,
            None => root_folder(&mut tx, owner).await?,
        };
        let base = folder_tx(&mut tx, parent, owner).await?.path;
        // The shared physical root is represented by an internal row, not a tree node.
        let parent = if base.is_empty() { None } else { Some(parent) };
        let path = new_folder_path(&base, name)?;
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM media_folders WHERE lower(path)=lower($1))",
        )
        .bind(&path)
        .fetch_one(&mut *tx)
        .await?
        {
            return Err(LibraryError::Collision);
        }
        self.store.ensure_directory(&path).await?;
        let id=sqlx::query_scalar("INSERT INTO media_folders(parent_id,name,path,owner_id) VALUES($1,$2,$3,$4) RETURNING id").bind(parent).bind(name).bind(path).bind(owner).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn upload(
        &self,
        s: &Session,
        csrf: &str,
        folder: Option<i64>,
        raw: Vec<u8>,
        filename: &str,
        personal: bool,
    ) -> Result<i64> {
        self.access(s, personal).await?;
        if !personal {
            self.identity.authorize(s, Permission::MediaWrite).await?;
        }
        s.check_csrf(csrf)?;
        let document = !matches!(
            extension(filename).to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp"
        );
        let prepared = self
            .store
            .prepare_library_source(raw, filename)
            .await
            .map_err(|error| {
                if document && matches!(error, MediaError::Invalid) {
                    LibraryError::FileInvalid
                } else {
                    LibraryError::Media(error)
                }
            })?;
        let name = upload_name(filename)?;
        let mut tx = self.write(s, csrf, personal).await?;
        let owner = if personal { s.administrator_id } else { None };
        let folder = match folder {
            Some(id) => folder_tx(&mut tx, id, owner).await?,
            None => {
                let id = root_folder(&mut tx, owner).await?;
                folder_tx(&mut tx, id, owner).await?
            }
        };
        if folder.path.len() + 1 + name.len() > 1024 {
            return Err(LibraryError::Destination);
        }
        // An immutable staging key is independent of the user's physical folder and filename.
        let token = crate::identity::token()?;
        let stored = self.store.put_library_source(prepared, &token).await?;
        let id = self
            .publish_library_entry(&mut tx, stored, &folder, name, owner)
            .await?;
        tx.commit().await?;
        Ok(id)
    }
    async fn publish_library_entry(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        stored: StoredMedia,
        folder: &Folder,
        name: String,
        owner: Option<i64>,
    ) -> Result<i64> {
        let personal = owner.is_some();
        stored.register_internal(tx, !personal).await?;
        self.store.ensure_directory(&folder.path).await?;
        let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
        let mut physical_name = None;
        for number in 1..=10000 {
            let candidate = if number == 1 {
                name.clone()
            } else if ext.is_empty() {
                format!("{stem}-{number}")
            } else {
                format!("{stem}-{number}.{ext}")
            };
            if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM media_library WHERE folder_id=$1 AND lower(name)=lower($2))").bind(folder.id).bind(&candidate).fetch_one(&mut **tx).await?{continue;}
            let target = super::files::join_path(&folder.path, &candidate);
            if target.len() > 1024 {
                return Err(LibraryError::Destination);
            }
            match self.store.link_file(&stored.key, &target).await {
                Ok(()) => {
                    sqlx::query("UPDATE media_locations SET path=$1 WHERE key=$2")
                        .bind(&target)
                        .bind(&stored.key)
                        .execute(&mut **tx)
                        .await?;
                    sqlx::query("INSERT INTO media_retired_files(path) VALUES($1)")
                        .bind(&stored.key)
                        .execute(&mut **tx)
                        .await?;
                    physical_name = Some(candidate);
                    break;
                }
                Err(MediaError::Invalid) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        let physical_name = physical_name.ok_or(LibraryError::Collision)?;
        sqlx::query("INSERT INTO media_library(key,folder_id,name,owner_id) VALUES($1,$2,$3,$4)")
            .bind(&stored.key)
            .bind(folder.id)
            .bind(physical_name)
            .bind(owner)
            .execute(&mut **tx)
            .await?;
        jobs::enqueue(tx, Some(&stored.key), true, jobs::Enqueue::New).await?;
        let id = sqlx::query_scalar("SELECT id FROM media_objects WHERE key=$1")
            .bind(&stored.key)
            .fetch_one(&mut **tx)
            .await?;
        Ok(id)
    }
    pub async fn move_asset(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        destination: &str,
        name: &str,
        personal: bool,
    ) -> Result<()> {
        segment(name)?;
        let destination = if destination == "/" { "" } else { destination };
        let mut tx = self.write(s, csrf, personal).await?;
        let (key,old,owner,trash):(String,String,Option<i64>,bool)=sqlx::query_as("SELECT l.key,p.path,l.owner_id,l.trashed_at IS NOT NULL FROM media_library l JOIN media_objects m ON m.key=l.key JOIN media_locations p ON p.key=l.key WHERE m.id=$1 FOR UPDATE OF l,p").bind(id).fetch_optional(&mut *tx).await?.ok_or(LibraryError::NotFound)?;
        check_owner(owner, s, personal)?;
        if trash {
            return Err(LibraryError::InUse);
        }
        let folder: Folder = sqlx::query_as(
            "SELECT * FROM media_folders WHERE path=$1 AND owner_id IS NOT DISTINCT FROM $2",
        )
        .bind(destination)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(LibraryError::Destination)?;
        if extension(&old) != extension(name) {
            return Err(LibraryError::Name);
        }
        if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM media_library WHERE folder_id=$1 AND lower(name)=lower($2) AND key<>$3)").bind(folder.id).bind(name).bind(&key).fetch_one(&mut *tx).await?{return Err(LibraryError::Collision);}
        let target = super::files::join_path(&folder.path, name);
        if old != target {
            self.relocate(&mut tx, &key, &old, &target).await?;
        }
        sqlx::query("UPDATE media_library SET folder_id=$1,name=$2 WHERE key=$3")
            .bind(folder.id)
            .bind(name)
            .bind(key)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn relocate(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        key: &str,
        old: &str,
        target: &str,
    ) -> Result<()> {
        self.store
            .link_file(old, target)
            .await
            .map_err(|e| match e {
                MediaError::Invalid => LibraryError::Collision,
                _ => e.into(),
            })?;
        sqlx::query("UPDATE media_locations SET path=$1 WHERE key=$2")
            .bind(target)
            .bind(key)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO media_retired_files(path) VALUES($1) ON CONFLICT(path) DO UPDATE SET retired_at=now()").bind(old).execute(&mut **tx).await?;
        Ok(())
    }
    pub async fn move_folder(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        destination: Option<i64>,
        name: &str,
        personal: bool,
    ) -> Result<()> {
        segment(name)?;
        let owner = if personal { s.administrator_id } else { None };
        let mut tx = self.write(s, csrf, personal).await?;
        let folder = folder_tx(&mut tx, id, owner).await?;
        if folder.protected {
            return Err(LibraryError::InUse);
        }
        let destination = match destination {
            Some(id) => id,
            None => root_folder(&mut tx, owner).await?,
        };
        let base = folder_tx(&mut tx, destination, owner).await?.path;
        let destination = if base.is_empty() {
            None
        } else {
            Some(destination)
        };
        if base == folder.path || base.starts_with(&format!("{}/", folder.path)) {
            return Err(LibraryError::Destination);
        }
        let new = new_folder_path(&base, name)?;
        if new == folder.path {
            return Ok(());
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM media_folders WHERE lower(path)=lower($1))",
        )
        .bind(&new)
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            return Err(LibraryError::Collision);
        }
        self.store.ensure_directory(&new).await?;
        // Materialize empty descendants too. Keep old aliases for the normal file
        // retirement grace period, but record them atomically with the path updates.
        let directories: Vec<String> = sqlx::query_scalar(
            "SELECT path FROM media_folders WHERE path=$1 OR left(path,length($1)+1)=$1||'/'",
        )
        .bind(&folder.path)
        .fetch_all(&mut *tx)
        .await?;
        for directory in &directories {
            let target = format!("{new}{}", &directory[folder.path.len()..]);
            if target.len() > 1024 {
                return Err(LibraryError::Destination);
            }
            self.store.ensure_directory(&target).await?;
            sqlx::query("INSERT INTO media_retired_directories(path) VALUES($1) ON CONFLICT(path) DO UPDATE SET retired_at=now()")
                .bind(directory).execute(&mut *tx).await?;
        }
        let prefix = format!("{}/", folder.path);
        let locations: Vec<(String, String)> = sqlx::query_as(
            "SELECT key,path FROM media_locations WHERE left(path,length($1))=$1 ORDER BY key",
        )
        .bind(&prefix)
        .fetch_all(&mut *tx)
        .await?;
        for (key, old) in locations {
            let target = format!("{new}/{}", &old[prefix.len()..]);
            self.relocate(&mut tx, &key, &old, &target).await?;
        }
        sqlx::query("UPDATE media_folders SET path=$1||substring(path from $3) WHERE path=$2 OR left(path,length($2)+1)=$2||'/'").bind(&new).bind(&folder.path).bind((folder.path.chars().count()+1) as i32).execute(&mut *tx).await?;
        sqlx::query("UPDATE media_folders SET parent_id=$1,name=$2 WHERE id=$3")
            .bind(destination)
            .bind(name)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn delete_folder(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        personal: bool,
        confirm: bool,
    ) -> Result<()> {
        if !confirm {
            return Err(LibraryError::Confirmation);
        }
        let owner = if personal { s.administrator_id } else { None };
        let mut tx = self.write(s, csrf, personal).await?;
        let folder = folder_tx(&mut tx, id, owner).await?;
        if folder.protected {
            return Err(LibraryError::InUse);
        }
        let occupied:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_folders WHERE parent_id=$1) OR EXISTS(SELECT 1 FROM media_library WHERE folder_id=$1)").bind(id).fetch_one(&mut *tx).await?;
        if occupied {
            return Err(LibraryError::InUse);
        }
        // Old hardlinks can still be awaiting retirement; never remove a nonempty directory.
        let path = self.store.relative_path(&folder.path)?;
        self.store.check_parents(&path).await?;
        match tokio::fs::remove_dir(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                return Err(LibraryError::InUse);
            }
            Err(e) => return Err(MediaError::from(e).into()),
        }
        sqlx::query("DELETE FROM media_folders WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn trash(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        restore: bool,
        personal: bool,
        confirm: bool,
    ) -> Result<()> {
        if !restore && !confirm {
            return Err(LibraryError::Confirmation);
        }
        let mut tx = self.write(s, csrf, personal).await?;
        let (key,owner):(String,Option<i64>)=sqlx::query_as("SELECT l.key,l.owner_id FROM media_library l JOIN media_objects m ON m.key=l.key WHERE m.id=$1 FOR UPDATE OF l").bind(id).fetch_optional(&mut *tx).await?.ok_or(LibraryError::NotFound)?;
        check_owner(owner, s, personal)?;
        let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_usage WHERE key=$1 OR key IN(SELECT object_key FROM media_transforms WHERE source_key=$1))").bind(&key).fetch_one(&mut *tx).await?;
        if used && !restore {
            return Err(LibraryError::InUse);
        }
        sqlx::query("UPDATE media_library SET trashed_at=CASE WHEN $1 THEN NULL ELSE coalesce(trashed_at,now()) END WHERE key=$2").bind(restore).bind(&key).execute(&mut *tx).await?;
        if restore {
            jobs::enqueue(&mut tx, Some(&key), true, jobs::Enqueue::Repair).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn copy(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        destination: i64,
        personal: bool,
    ) -> Result<i64> {
        let asset = self.asset(s, id, personal).await?;
        self.copy_named(s, csrf, id, destination, &asset.name, personal)
            .await
    }
    pub async fn copy_named(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        destination: i64,
        name: &str,
        personal: bool,
    ) -> Result<i64> {
        let asset = self.asset(s, id, personal).await?;
        if asset.trashed {
            return Err(LibraryError::InUse);
        }
        check_owner(asset.owner_id, s, personal)?;
        segment(name)?;
        if !extension(&asset.name).eq_ignore_ascii_case(extension(name)) {
            return Err(LibraryError::Name);
        }
        let mut source = self.store.open_registered(&self.pool, &asset.key).await?;
        let mut tx = self.write(s, csrf, personal).await?;
        let owner = if personal { s.administrator_id } else { None };
        // Revalidate the immutable source under the publication lock after opening its handle.
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_library WHERE key=$1 AND owner_id IS NOT DISTINCT FROM $2 AND trashed_at IS NULL)").bind(&asset.key).bind(owner).fetch_one(&mut *tx).await?;
        if !active {
            return Err(LibraryError::InUse);
        }
        let folder = folder_tx(&mut tx, destination, owner).await?;
        let token = crate::identity::token()?;
        let key = format!("staging/{token}/source.png");
        self.store
            .ensure_directory(&format!("staging/{token}"))
            .await?;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut target = options
            .open(self.store.root().join(&key))
            .await
            .map_err(MediaError::from)?;
        let bytes = tokio::io::copy(&mut source, &mut target)
            .await
            .map_err(MediaError::from)?;
        target.sync_all().await.map_err(MediaError::from)?;
        let content_type = match asset.content_type.as_str() {
            "image/png" => "image/png",
            "image/jpeg" => "image/jpeg",
            "image/webp" => "image/webp",
            _ => super::files::document_type(&asset.name),
        };
        let stored = StoredMedia {
            key,
            bytes: i64::try_from(bytes).map_err(|_| MediaError::Invalid)?,
            width: asset.width,
            height: asset.height,
            content_type,
            variants: Vec::new(),
        };
        let id = self
            .publish_library_entry(&mut tx, stored, &folder, name.into(), owner)
            .await?;
        tx.commit().await?;
        Ok(id)
    }
    /// Explicit, confirmed purge has no minimum trash age; live business references stay protected.
    pub async fn purge(
        &self,
        s: &Session,
        csrf: &str,
        personal: bool,
        confirm: bool,
    ) -> Result<u64> {
        if !confirm {
            return Err(LibraryError::Confirmation);
        }
        let mut tx = self.write(s, csrf, personal).await?;
        let owner = if personal { s.administrator_id } else { None };
        let rows:Vec<String>=sqlx::query_scalar("DELETE FROM media_library WHERE owner_id IS NOT DISTINCT FROM $1 AND trashed_at IS NOT NULL AND key NOT IN(SELECT key FROM media_business_roots) AND NOT EXISTS(SELECT 1 FROM media_transforms t JOIN media_business_roots r ON r.key=t.object_key WHERE t.source_key=media_library.key) RETURNING key").bind(owner).fetch_all(&mut *tx).await?;
        for key in &rows {
            // Keep discovery from re-adopting the original before physical cleanup runs.
            // Explicit purge already bypasses the grace period for these objects and paths.
            sqlx::query("INSERT INTO media_retired_files(path,retired_at) SELECT path,now()-interval '24 hours 1 second' FROM media_locations WHERE key=$1 ON CONFLICT(path) DO UPDATE SET retired_at=least(media_retired_files.retired_at,excluded.retired_at)").bind(key).execute(&mut *tx).await?;
            sqlx::query("UPDATE media_objects SET unused_since=now()-interval '24 hours 1 second' WHERE key=$1 OR key IN(SELECT object_key FROM media_variants WHERE source_key=$1)").bind(key).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(rows.len() as u64)
    }
}
fn page_offset(page: i64, size: i64) -> Result<i64> {
    let page = page.max(1);
    if page > 1_000_000 {
        Err(LibraryError::Name)
    } else {
        Ok((page - 1) * size)
    }
}
fn extension(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, e)| e).unwrap_or("")
}
fn upload_name(filename: &str) -> Result<String> {
    let base = filename
        .rsplit(['/', '\\'])
        .next()
        .ok_or(LibraryError::Name)?;
    let (_, ext) = base.rsplit_once('.').ok_or(LibraryError::Name)?;
    let ext = ext.to_ascii_lowercase();
    if ext.is_empty() || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(LibraryError::Name);
    }
    let normalized = MediaStore::filename(base);
    let stem = normalized.trim_end_matches(".png");
    Ok(format!("{stem}.{ext}"))
}
fn check_owner(owner: Option<i64>, s: &Session, personal: bool) -> Result<()> {
    let expected = if personal { s.administrator_id } else { None };
    if owner != expected {
        Err(LibraryError::NotFound)
    } else {
        Ok(())
    }
}

async fn folder_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    owner: Option<i64>,
) -> Result<Folder> {
    sqlx::query_as(
        "SELECT * FROM media_folders WHERE id=$1 AND owner_id IS NOT DISTINCT FROM $2 FOR UPDATE",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(LibraryError::Destination)
}

async fn root_folder(tx: &mut Transaction<'_, Postgres>, owner: Option<i64>) -> Result<i64> {
    let path = owner
        .map(|id| format!("private/account-{id}"))
        .unwrap_or_default();
    sqlx::query("INSERT INTO media_folders(name,path,owner_id,protected) VALUES($1,$2,$3,true) ON CONFLICT(path) DO NOTHING").bind(if owner.is_some(){"Le mie immagini"}else{"Media"}).bind(&path).bind(owner).execute(&mut **tx).await?;
    Ok(sqlx::query_scalar(
        "SELECT id FROM media_folders WHERE path=$1 AND owner_id IS NOT DISTINCT FROM $2",
    )
    .bind(path)
    .bind(owner)
    .fetch_one(&mut **tx)
    .await?)
}

fn new_folder_path(base: &str, name: &str) -> Result<String> {
    // Root namespaces owned by storage/importers cannot become user folders.
    if base.is_empty()
        && matches!(
            name.to_ascii_lowercase().as_str(),
            "cache" | "staging" | "private" | "objects" | "brand" | "library"
        )
    {
        return Err(LibraryError::Destination);
    }
    let path = super::files::join_path(base, name);
    if path.len() > 1024 || path.split('/').count() > 16 {
        return Err(LibraryError::Destination);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_uploads_are_bounded() {
        for bad in [
            "../a", ".hidden", "/etc", "x/y", "a\\b", "a\n", " a", "a..b",
        ] {
            assert!(segment(bad).is_err());
        }
        assert!(segment("Foto estate-2026").is_ok());
        assert_eq!(
            upload_name("C:\\fakepath\\Foto estate.JPG").unwrap(),
            "Foto-estate.jpg"
        );
        assert_eq!(upload_name("a.pdf").unwrap(), "a.pdf");
        assert_eq!(upload_name("a.svg").unwrap(), "a.svg");
        assert!(segment("Documenti è 2026").is_ok());
        assert!(page_offset(i64::MAX, 40).is_err());
    }
}
