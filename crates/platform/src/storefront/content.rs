use super::*;

impl Storefront {
    pub const CONTENT: crate::content::Definition = crate::content::Definition {
        table: "storefront_settings",
        name_column: "heading",
        description_column: Some("introduction"),
        name_limit: 120,
        description_limit: 500,
        seo: true,
        read: Permission::SettingsRead,
        write: Permission::SettingsWrite,
    };
    pub async fn translations(
        &self,
        identity: &Identity,
        session: &Session,
    ) -> Result<Vec<crate::content::Translation>, crate::content::ContentError> {
        Self::CONTENT.load(&self.pool, identity, session, 1).await
    }
    pub async fn save_translations(
        &self,
        session: &Session,
        csrf: &str,
        primary: &str,
        rows: &[crate::content::Translation],
    ) -> Result<(), crate::content::ContentError> {
        Self::CONTENT
            .save(&self.pool, session, csrf, 1, primary, rows)
            .await
    }
    pub async fn localize(
        &self,
        settings: &mut Settings,
        language: crate::content::Language,
    ) -> Result<(), IdentityError> {
        Self::read_localize(&self.pool, settings, language).await
    }
    async fn read_localize<'e, E>(
        executor: E,
        settings: &mut Settings,
        language: crate::content::Language,
    ) -> Result<(), IdentityError>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        let mut values: (String, String, String, String) =
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT {},{},{},{} FROM storefront_settings s {} WHERE s.id=1",
                language.expression("name", "s.heading"),
                language.expression("description", "s.introduction"),
                language.expression("seo_title", "s.seo_title"),
                language.expression("seo_description", "s.seo_description"),
                language.joins("storefront_settings", "s.id")
            )))
            .fetch_one(executor)
            .await?;
        std::mem::swap(&mut settings.heading, &mut values.0);
        std::mem::swap(&mut settings.introduction, &mut values.1);
        std::mem::swap(&mut settings.seo_title, &mut values.2);
        std::mem::swap(&mut settings.seo_description, &mut values.3);
        Ok(())
    }
    /// Reads through the caller transaction without acquiring another pool connection.
    pub async fn localize_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        settings: &mut Settings,
        language: crate::content::Language,
    ) -> Result<(), IdentityError> {
        Self::read_localize(&mut **tx, settings, language).await
    }
}
