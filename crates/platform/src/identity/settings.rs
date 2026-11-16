//! General store data. A settings form replaces all fields atomically.
use super::*;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsField {
    Name,
    LegalName,
    ContactEmail,
    Phone,
    AddressLine1,
    AddressLine2,
    PostalCode,
    City,
    Province,
    Country,
}
impl SettingsField {
    pub const ALL: [Self; 10] = [
        Self::Name,
        Self::LegalName,
        Self::ContactEmail,
        Self::Phone,
        Self::AddressLine1,
        Self::AddressLine2,
        Self::PostalCode,
        Self::City,
        Self::Province,
        Self::Country,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::LegalName => "legal_name",
            Self::ContactEmail => "contact_email",
            Self::Phone => "phone",
            Self::AddressLine1 => "address_line1",
            Self::AddressLine2 => "address_line2",
            Self::PostalCode => "postal_code",
            Self::City => "city",
            Self::Province => "province",
            Self::Country => "country",
        }
    }
    pub fn limit(self) -> usize {
        match self {
            Self::LegalName | Self::AddressLine1 | Self::AddressLine2 => 160,
            Self::ContactEmail => 254,
            Self::Phone => 40,
            Self::PostalCode => 20,
            _ => 80,
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, Deserialize, FromRow)]
#[serde(default)]
pub struct SettingsInput {
    pub name: String,
    pub legal_name: String,
    pub contact_email: String,
    pub phone: String,
    pub address_line1: String,
    pub address_line2: String,
    pub postal_code: String,
    pub city: String,
    pub province: String,
    pub country: String,
}
impl SettingsInput {
    pub fn value(&self, field: SettingsField) -> &str {
        match field {
            SettingsField::Name => &self.name,
            SettingsField::LegalName => &self.legal_name,
            SettingsField::ContactEmail => &self.contact_email,
            SettingsField::Phone => &self.phone,
            SettingsField::AddressLine1 => &self.address_line1,
            SettingsField::AddressLine2 => &self.address_line2,
            SettingsField::PostalCode => &self.postal_code,
            SettingsField::City => &self.city,
            SettingsField::Province => &self.province,
            SettingsField::Country => &self.country,
        }
    }
    fn normalized(&self) -> Result<Self> {
        let invalid: Vec<_> = SettingsField::ALL
            .into_iter()
            .filter(|field| {
                let value = self.value(*field).trim();
                (matches!(field, SettingsField::Name) && value.is_empty())
                    || value.chars().count() > field.limit()
                    || value.chars().any(char::is_control)
                    || (matches!(field, SettingsField::ContactEmail)
                        && !value.is_empty()
                        && email(value).is_err())
            })
            .collect();
        if !invalid.is_empty() {
            return Err(IdentityError::InvalidSettings(invalid));
        }
        Ok(Self {
            name: self.name.trim().to_owned(),
            legal_name: self.legal_name.trim().to_owned(),
            contact_email: self.contact_email.trim().to_ascii_lowercase(),
            phone: self.phone.trim().to_owned(),
            address_line1: self.address_line1.trim().to_owned(),
            address_line2: self.address_line2.trim().to_owned(),
            postal_code: self.postal_code.trim().to_owned(),
            city: self.city.trim().to_owned(),
            province: self.province.trim().to_owned(),
            country: self.country.trim().to_owned(),
        })
    }
}
#[derive(FromRow)]
pub struct Settings {
    #[sqlx(flatten)]
    pub values: SettingsInput,
    pub currency: String,
    pub locale: String,
}
impl Identity {
    pub async fn settings(&self) -> Result<Option<Settings>> {
        Self::read_settings(&self.pool).await
    }
    async fn read_settings<'e, E>(executor: E) -> Result<Option<Settings>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        Ok(sqlx::query_as("SELECT name,legal_name,contact_email,phone,address_line1,address_line2,postal_code,city,province,country,currency,locale FROM store_settings WHERE id=1")
            .fetch_optional(executor).await?)
    }
    /// Reads through the caller transaction without acquiring another pool connection.
    pub async fn settings_in(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<Option<Settings>> {
        Self::read_settings(&mut **tx).await
    }
    /// Replaces general data; currency and locale are managed by their dedicated settings forms. Revoked sessions cannot save.
    pub async fn save_settings(
        &self,
        session: &Session,
        csrf: &str,
        input: &SettingsInput,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.save_settings_in(&mut tx, session, csrf, input).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Reuses the caller's transaction; does not commit it.
    pub async fn save_settings_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        session: &Session,
        csrf: &str,
        input: &SettingsInput,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        session.require(Permission::SettingsWrite)?;
        let input = input.normalized()?;
        Self::lock_changes(tx).await?;
        Self::authorize_locked(tx, session, Permission::SettingsWrite).await?;
        let result = sqlx::query("UPDATE store_settings SET name=$1,legal_name=$2,contact_email=$3,phone=$4,address_line1=$5,address_line2=$6,postal_code=$7,city=$8,province=$9,country=$10 WHERE id=1")
            .bind(input.name).bind(input.legal_name).bind(input.contact_email).bind(input.phone)
            .bind(input.address_line1).bind(input.address_line2).bind(input.postal_code).bind(input.city)
            .bind(input.province).bind(input.country).execute(&mut **tx).await?;
        if result.rows_affected() != 1 {
            return Err(IdentityError::Unavailable);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn general_settings_validate_every_field_and_normalize_optional_data() {
        let input = SettingsInput {
            name: "  Negozio  ".into(),
            contact_email: " CONTATTI@EXAMPLE.TEST ".into(),
            legal_name: "  Società é  ".into(),
            phone: " +39 02 1234 ext. 5 ".into(),
            ..Default::default()
        };
        let clean = input.normalized().expect("Dati sintetici validi");
        assert_eq!(clean.name, "Negozio");
        assert_eq!(clean.contact_email, "contatti@example.test");
        assert_eq!(clean.legal_name, "Società é");
        assert_eq!(clean.phone, "+39 02 1234 ext. 5");
        assert!(
            SettingsInput {
                name: "Shop".into(),
                ..Default::default()
            }
            .normalized()
            .is_ok()
        );
        for (field, limit) in [
            (SettingsField::Name, 80),
            (SettingsField::LegalName, 160),
            (SettingsField::ContactEmail, 254),
            (SettingsField::Phone, 40),
            (SettingsField::AddressLine1, 160),
            (SettingsField::AddressLine2, 160),
            (SettingsField::PostalCode, 20),
            (SettingsField::City, 80),
            (SettingsField::Province, 80),
            (SettingsField::Country, 80),
        ] {
            assert_eq!(field.limit(), limit);
            // Check the real input contract, including Unicode length, without duplicating validation.
            for (value, valid) in [
                ("é".repeat(limit), field != SettingsField::ContactEmail),
                ("a".repeat(limit + 1), false),
                ("a\0b".into(), false),
            ] {
                let mut map = std::collections::BTreeMap::from([("name", "Shop".to_owned())]);
                map.insert(field.key(), value);
                let input = SettingsInput::deserialize(serde::de::value::MapDeserializer::<
                    _,
                    serde::de::value::Error,
                >::new(map.into_iter()))
                .unwrap();
                assert_eq!(input.normalized().is_ok(), valid, "{}", field.key());
            }
        }
        for value in [
            "not-an-email",
            "a@localhost",
            "a@.example.test",
            "a@b.test\r\nBcc:x@y.test",
        ] {
            let input = SettingsInput {
                name: "Shop".into(),
                contact_email: value.into(),
                ..Default::default()
            };
            assert!(
                matches!(input.normalized(), Err(IdentityError::InvalidSettings(fields)) if fields == vec![SettingsField::ContactEmail])
            );
        }
        let longest_email = SettingsInput {
            name: "Shop".into(),
            contact_email: format!("{}@example.test", "a".repeat(241)),
            ..Default::default()
        };
        assert!(longest_email.normalized().is_ok());
        let too_long_email = SettingsInput {
            contact_email: format!("a{}", longest_email.contact_email),
            ..longest_email
        };
        assert!(too_long_email.normalized().is_err());
        let invalid = SettingsInput {
            name: " ".into(),
            phone: "x".repeat(41),
            ..Default::default()
        };
        assert!(
            matches!(invalid.normalized(), Err(IdentityError::InvalidSettings(fields)) if fields == vec![SettingsField::Name, SettingsField::Phone])
        );
    }
}
