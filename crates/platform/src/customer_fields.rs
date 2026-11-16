//! Shared field policy: identity owns account data, commerce owns checkout data.
//! The finite registry is the only source of accepted field identifiers.
use crate::identity::{IdentityError, Permission, Session};
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Hidden,
    Optional,
    Required,
}
impl Mode {
    pub fn key(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Optional => "optional",
            Self::Required => "required",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hidden" => Some(Self::Hidden),
            "optional" => Some(Self::Optional),
            "required" => Some(Self::Required),
            _ => None,
        }
    }
}
#[derive(Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub scope: &'static str,
    pub label: &'static str,
    pub private: Mode,
    pub business: Mode,
    pub locked: bool,
}
use Mode::{Hidden as H, Optional as O, Required as R};
pub const FIELDS: &[Field] = &[
    Field {
        name: "email",
        scope: "customer",
        label: "co-field-email",
        private: R,
        business: R,
        locked: true,
    },
    Field {
        name: "first_name",
        scope: "customer",
        label: "co-field-first-name",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "last_name",
        scope: "customer",
        label: "co-field-last-name",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "phone",
        scope: "shipping",
        label: "co-field-phone",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "recipient_first_name",
        scope: "shipping",
        label: "co-field-recipient-first-name",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "recipient_last_name",
        scope: "shipping",
        label: "co-field-recipient-last-name",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "shipping_company",
        scope: "shipping",
        label: "co-field-company",
        private: O,
        business: O,
        locked: false,
    },
    Field {
        name: "shipping_country",
        scope: "shipping",
        label: "co-field-country",
        private: R,
        business: R,
        locked: true,
    },
    Field {
        name: "shipping_street",
        scope: "shipping",
        label: "co-field-street",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "shipping_number",
        scope: "shipping",
        label: "co-field-number",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "shipping_extra",
        scope: "shipping",
        label: "co-field-extra",
        private: O,
        business: O,
        locked: false,
    },
    Field {
        name: "shipping_postal",
        scope: "shipping",
        label: "co-field-postal",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "shipping_city",
        scope: "shipping",
        label: "co-field-city",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "shipping_region",
        scope: "shipping",
        label: "co-field-region",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "delivery_notes",
        scope: "shipping",
        label: "co-field-delivery-notes",
        private: O,
        business: O,
        locked: false,
    },
    Field {
        name: "billing_first_name",
        scope: "billing",
        label: "co-field-billing-first-name",
        private: O,
        business: H,
        locked: false,
    },
    Field {
        name: "billing_last_name",
        scope: "billing",
        label: "co-field-billing-last-name",
        private: O,
        business: H,
        locked: false,
    },
    Field {
        name: "billing_company",
        scope: "billing",
        label: "co-field-billing-company",
        private: H,
        business: R,
        locked: false,
    },
    Field {
        name: "billing_country",
        scope: "billing",
        label: "co-field-country",
        private: R,
        business: R,
        locked: true,
    },
    Field {
        name: "billing_street",
        scope: "billing",
        label: "co-field-street",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "billing_number",
        scope: "billing",
        label: "co-field-number",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "billing_extra",
        scope: "billing",
        label: "co-field-extra",
        private: O,
        business: O,
        locked: false,
    },
    Field {
        name: "billing_postal",
        scope: "billing",
        label: "co-field-postal",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "billing_city",
        scope: "billing",
        label: "co-field-city",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "billing_region",
        scope: "billing",
        label: "co-field-region",
        private: R,
        business: R,
        locked: false,
    },
    Field {
        name: "vat_id",
        scope: "billing",
        label: "co-field-vat-id",
        private: H,
        business: R,
        locked: false,
    },
    Field {
        name: "tax_id",
        scope: "billing",
        label: "co-field-tax-id",
        private: R,
        business: O,
        locked: false,
    },
    Field {
        name: "recipient_code",
        scope: "billing",
        label: "co-field-recipient-code",
        private: H,
        business: O,
        locked: false,
    },
    Field {
        name: "pec",
        scope: "billing",
        label: "co-field-pec",
        private: H,
        business: O,
        locked: false,
    },
];
pub fn field(name: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.name == name)
}
pub fn valid_type(kind: &str) -> bool {
    matches!(kind, "private" | "business")
}
#[derive(Clone, Default)]
pub struct Policy {
    pub revision: i64,
    overrides: BTreeMap<String, Mode>,
}
impl Policy {
    /// Administrative master data exposes hidden public fields for deliberate editing.
    /// Public requiredness and format rules remain in force; this is not persisted.
    pub fn for_administration(&self) -> Self {
        let mut result = self.clone();
        for field in FIELDS {
            for kind in ["private", "business"] {
                if self.mode(field.name, kind) == Mode::Hidden {
                    result
                        .overrides
                        .insert(format!("{kind}.{}", field.name), Mode::Optional);
                }
            }
        }
        result
    }
    pub fn mode(&self, name: &str, kind: &str) -> Mode {
        self.overrides
            .get(&format!("{kind}.{name}"))
            .copied()
            .unwrap_or_else(|| {
                field(name)
                    .map(|f| {
                        if kind == "business" {
                            f.business
                        } else {
                            f.private
                        }
                    })
                    .unwrap_or(O)
            })
    }
    pub fn overridden(&self, name: &str, kind: &str) -> bool {
        self.overrides.contains_key(&format!("{kind}.{name}"))
    }
    /// Only a complete, duplicate-free registry is accepted by the settings boundary.
    pub fn from_choices(choices: &[(String, String)]) -> Result<Self, Error> {
        let mut policy = Self::default();
        let mut seen = std::collections::BTreeSet::new();
        for (key, value) in choices {
            let (kind, name) = key.split_once('.').ok_or(Error::Invalid)?;
            let f = field(name).ok_or(Error::Invalid)?;
            let mode = Mode::parse(value).ok_or(Error::Invalid)?;
            if !valid_type(kind) || !seen.insert(key) || (f.locked && mode != R) {
                return Err(Error::Invalid);
            }
            let default = if kind == "business" {
                f.business
            } else {
                f.private
            };
            if mode != default {
                policy.overrides.insert(key.clone(), mode);
            }
        }
        if seen.len() != FIELDS.len() * 2 {
            return Err(Error::Invalid);
        }
        Ok(policy)
    }
    pub fn choices(&self) -> Vec<(String, String)> {
        FIELDS
            .iter()
            .flat_map(|f| {
                ["private", "business"].map(|kind| {
                    (
                        format!("{kind}.{}", f.name),
                        self.mode(f.name, kind).key().into(),
                    )
                })
            })
            .collect()
    }
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Technical(#[from] crate::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] crate::technical::AccessError),
    #[error(transparent)]
    Identity(IdentityError),
    #[error("Invalid field policy")]
    Invalid,
    #[error("Field policy changed")]
    Conflict,
}
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(crate::technical::DatabaseFailure::capture(e, module_path!()).into())
    }
}
pub async fn load(pool: &PgPool) -> Result<Policy, IdentityError> {
    let mut tx = pool.begin().await?;
    let policy = load_in(&mut tx).await?;
    tx.commit().await?;
    Ok(policy)
}
/// Hold the policy row SHARE lock through the mutation/confirmation it governs.
/// Writers authorize identity before taking this row; callers lock account/cart first.
pub async fn load_in(tx: &mut Transaction<'_, Postgres>) -> Result<Policy, IdentityError> {
    let (revision, json): (i64, serde_json::Value) = sqlx::query_as(
        "SELECT revision,overrides FROM customer_field_policy WHERE singleton FOR SHARE",
    )
    .fetch_one(&mut **tx)
    .await?;
    let overrides = serde_json::from_value(json).map_err(|_| IdentityError::Invalid)?;
    Ok(Policy {
        revision,
        overrides,
    })
}
pub async fn save(
    pool: &PgPool,
    session: &Session,
    csrf: &str,
    revision: i64,
    choices: &[(String, String)],
) -> Result<(), Error> {
    let policy = Policy::from_choices(choices)?;
    let mut tx = pool.begin().await?;
    crate::identity::Identity::authorize_transaction(
        &mut tx,
        session,
        csrf,
        Permission::SettingsWrite,
    )
    .await?;
    let changed = sqlx::query("UPDATE customer_field_policy SET overrides=$1,revision=revision+1 WHERE singleton AND revision=$2")
        .bind(serde_json::to_value(&policy.overrides).map_err(|_| Error::Invalid)?).bind(revision).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(Error::Conflict);
    }
    tx.commit().await?;
    Ok(())
}
impl From<IdentityError> for Error {
    fn from(error: IdentityError) -> Self {
        if let Some(cause) = error.technical() {
            Self::Technical(cause)
        } else if let Some(cause) = error.access() {
            Self::Access(cause)
        } else {
            Self::Identity(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn administrative_policy_exposes_hidden_values_without_changing_public_policy() {
        let public = Policy::default();
        let admin = public.for_administration();
        assert_eq!(public.mode("billing_company", "private"), Mode::Hidden);
        assert_eq!(admin.mode("billing_company", "private"), Mode::Optional);
        assert_eq!(admin.mode("billing_company", "business"), Mode::Required);
        assert_eq!(public.choices(), Policy::default().choices());
    }
    #[test]
    fn finite_complete_policy_and_protected_fields() {
        let p = Policy::default();
        let choices = p.choices();
        assert!(Policy::from_choices(&choices).is_ok());
        assert!(Policy::from_choices(&choices[1..]).is_err());
        let mut bad = choices.clone();
        bad[0].1 = "hidden".into();
        assert!(Policy::from_choices(&bad).is_err());
        let mut bad = choices.clone();
        bad[0].0 = "private.unknown".into();
        assert!(Policy::from_choices(&bad).is_err());
        let mut bad = choices.clone();
        bad[1] = bad[0].clone();
        assert!(Policy::from_choices(&bad).is_err());
        let mut custom = choices;
        custom
            .iter_mut()
            .find(|(k, _)| k == "business.pec")
            .expect("PEC registry")
            .1 = "required".into();
        let p = Policy::from_choices(&custom).expect("Valid override");
        assert_eq!(p.mode("pec", "business"), R);
        assert_eq!(p.mode("pec", "private"), H);
    }
}
