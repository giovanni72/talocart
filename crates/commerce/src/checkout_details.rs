//! Customer/address draft owned by a guest cart, ready for a future immutable order snapshot.
use serde::Deserialize;
use talocart_platform::customer_fields::{self, Mode, Policy};
use talocart_platform::geography;
#[derive(Clone, serde::Serialize, Deserialize, sqlx::FromRow)]
#[serde(default)]
pub struct Details {
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub phone: String,
    pub recipient_first_name: String,
    pub recipient_last_name: String,
    pub shipping_company: String,
    pub shipping_country: String,
    pub shipping_street: String,
    pub shipping_number: String,
    pub shipping_extra: String,
    pub shipping_postal: String,
    pub shipping_city: String,
    pub shipping_region: String,
    pub delivery_notes: String,
    pub customer_type: String,
    pub billing_first_name: String,
    pub billing_last_name: String,
    pub billing_company: String,
    pub billing_country: String,
    pub billing_street: String,
    pub billing_number: String,
    pub billing_extra: String,
    pub billing_postal: String,
    pub billing_city: String,
    pub billing_region: String,
    pub vat_id: String,
    pub tax_id: String,
    pub invoice_channel: String,
    pub recipient_code: String,
    pub pec: String,
    #[serde(default, deserialize_with = "form_bool")]
    pub invoice_requested: bool,
    #[serde(default, deserialize_with = "form_bool")]
    pub billing_same: bool,
}
impl Default for Details {
    fn default() -> Self {
        Self {
            email: String::new(),
            first_name: String::new(),
            last_name: String::new(),
            phone: String::new(),
            recipient_first_name: String::new(),
            recipient_last_name: String::new(),
            shipping_company: String::new(),
            shipping_country: String::new(),
            shipping_street: String::new(),
            shipping_number: String::new(),
            shipping_extra: String::new(),
            shipping_postal: String::new(),
            shipping_city: String::new(),
            shipping_region: String::new(),
            delivery_notes: String::new(),
            customer_type: "private".into(),
            billing_first_name: String::new(),
            billing_last_name: String::new(),
            billing_company: String::new(),
            billing_country: String::new(),
            billing_street: String::new(),
            billing_number: String::new(),
            billing_extra: String::new(),
            billing_postal: String::new(),
            billing_city: String::new(),
            billing_region: String::new(),
            vat_id: String::new(),
            tax_id: String::new(),
            invoice_channel: "none".into(),
            recipient_code: String::new(),
            pec: String::new(),
            invoice_requested: false,
            billing_same: true,
        }
    }
}
impl Details {
    pub fn value(&self, field: &str) -> &str {
        match field {
            "email" => &self.email,
            "first_name" => &self.first_name,
            "last_name" => &self.last_name,
            "phone" => &self.phone,
            "recipient_first_name" => &self.recipient_first_name,
            "recipient_last_name" => &self.recipient_last_name,
            "shipping_company" => &self.shipping_company,
            "shipping_country" => &self.shipping_country,
            "shipping_street" => &self.shipping_street,
            "shipping_number" => &self.shipping_number,
            "shipping_extra" => &self.shipping_extra,
            "shipping_postal" => &self.shipping_postal,
            "shipping_city" => &self.shipping_city,
            "shipping_region" => &self.shipping_region,
            "delivery_notes" => &self.delivery_notes,
            "customer_type" => &self.customer_type,
            "billing_first_name" => &self.billing_first_name,
            "billing_last_name" => &self.billing_last_name,
            "billing_company" => &self.billing_company,
            "billing_country" => &self.billing_country,
            "billing_street" => &self.billing_street,
            "billing_number" => &self.billing_number,
            "billing_extra" => &self.billing_extra,
            "billing_postal" => &self.billing_postal,
            "billing_city" => &self.billing_city,
            "billing_region" => &self.billing_region,
            "vat_id" => &self.vat_id,
            "tax_id" => &self.tax_id,
            "invoice_channel" => &self.invoice_channel,
            "recipient_code" => &self.recipient_code,
            "pec" => &self.pec,
            _ => "",
        }
    }
    pub fn value_mut(&mut self, name: &str) -> Option<&mut String> {
        match name {
            "email" => Some(&mut self.email),
            "first_name" => Some(&mut self.first_name),
            "last_name" => Some(&mut self.last_name),
            "phone" => Some(&mut self.phone),
            "recipient_first_name" => Some(&mut self.recipient_first_name),
            "recipient_last_name" => Some(&mut self.recipient_last_name),
            "shipping_company" => Some(&mut self.shipping_company),
            "shipping_country" => Some(&mut self.shipping_country),
            "shipping_street" => Some(&mut self.shipping_street),
            "shipping_number" => Some(&mut self.shipping_number),
            "shipping_extra" => Some(&mut self.shipping_extra),
            "shipping_postal" => Some(&mut self.shipping_postal),
            "shipping_city" => Some(&mut self.shipping_city),
            "shipping_region" => Some(&mut self.shipping_region),
            "delivery_notes" => Some(&mut self.delivery_notes),
            "billing_first_name" => Some(&mut self.billing_first_name),
            "billing_last_name" => Some(&mut self.billing_last_name),
            "billing_company" => Some(&mut self.billing_company),
            "billing_country" => Some(&mut self.billing_country),
            "billing_street" => Some(&mut self.billing_street),
            "billing_number" => Some(&mut self.billing_number),
            "billing_extra" => Some(&mut self.billing_extra),
            "billing_postal" => Some(&mut self.billing_postal),
            "billing_city" => Some(&mut self.billing_city),
            "billing_region" => Some(&mut self.billing_region),
            "vat_id" => Some(&mut self.vat_id),
            "tax_id" => Some(&mut self.tax_id),
            "recipient_code" => Some(&mut self.recipient_code),
            "pec" => Some(&mut self.pec),
            _ => None,
        }
    }
    /// Required marker for the active invoice/country, matching server validation.
    pub fn field_required(&self, name: &str, policy: &Policy) -> bool {
        if policy.mode(name, &self.customer_type) != Mode::Required {
            return false;
        }
        let billing = customer_fields::field(name).is_some_and(|f| f.scope == "billing");
        if billing && !(self.invoice_requested || self.customer_type == "business") {
            return false;
        }
        let country = if billing && !self.billing_same {
            &self.billing_country
        } else {
            &self.shipping_country
        };
        match name {
            "shipping_region" | "billing_region" => {
                ["IT", "US", "CA", "AU"].contains(&country.as_str())
            }
            "shipping_postal" | "billing_postal" => !postal_valid(country, ""),
            "tax_id" | "recipient_code" | "pec" => country == "IT",
            _ => true,
        }
    }
    pub fn validate(self) -> Result<Self, Vec<&'static str>> {
        self.validate_with(&Policy::default())
    }
    /// Hidden submitted values are ignored. Persistence may retain their previous values,
    /// while the final order receives only data applicable under the current policy.
    pub fn normalize_with(mut self, policy: &Policy) -> Self {
        self.customer_type = self.customer_type.trim().to_owned();
        for f in customer_fields::FIELDS {
            if policy.mode(f.name, &self.customer_type) == Mode::Hidden
                && let Some(value) = self.value_mut(f.name)
            {
                value.clear();
            }
        }

        self.email = self.email.trim().to_owned();
        self.first_name = self.first_name.trim().to_owned();
        self.last_name = self.last_name.trim().to_owned();
        self.phone = self.phone.trim().to_owned();
        self.recipient_first_name = self.recipient_first_name.trim().to_owned();
        self.recipient_last_name = self.recipient_last_name.trim().to_owned();
        self.shipping_company = self.shipping_company.trim().to_owned();
        self.shipping_country = self.shipping_country.trim().to_owned();
        self.shipping_street = self.shipping_street.trim().to_owned();
        self.shipping_number = self.shipping_number.trim().to_owned();
        self.shipping_extra = self.shipping_extra.trim().to_owned();
        self.shipping_postal = self.shipping_postal.trim().to_owned();
        self.shipping_city = self.shipping_city.trim().to_owned();
        self.shipping_region = self.shipping_region.trim().to_owned();
        self.delivery_notes = self.delivery_notes.trim().to_owned();
        self.customer_type = self.customer_type.trim().to_owned();
        self.billing_first_name = self.billing_first_name.trim().to_owned();
        self.billing_last_name = self.billing_last_name.trim().to_owned();
        self.billing_company = self.billing_company.trim().to_owned();
        self.billing_country = self.billing_country.trim().to_owned();
        self.billing_street = self.billing_street.trim().to_owned();
        self.billing_number = self.billing_number.trim().to_owned();
        self.billing_extra = self.billing_extra.trim().to_owned();
        self.billing_postal = self.billing_postal.trim().to_owned();
        self.billing_city = self.billing_city.trim().to_owned();
        self.billing_region = self.billing_region.trim().to_owned();
        self.vat_id = self.vat_id.trim().to_owned();
        self.tax_id = self.tax_id.trim().to_owned();
        self.invoice_channel = self.invoice_channel.trim().to_owned();
        self.recipient_code = self.recipient_code.trim().to_owned();
        self.pec = self.pec.trim().to_owned();

        if self.recipient_first_name.is_empty()
            && policy.mode("recipient_first_name", &self.customer_type) != Mode::Hidden
        {
            self.recipient_first_name = self.first_name.clone();
        }
        if self.recipient_last_name.is_empty()
            && policy.mode("recipient_last_name", &self.customer_type) != Mode::Hidden
        {
            self.recipient_last_name = self.last_name.clone();
        }
        if self.billing_first_name.is_empty()
            && !policy.overridden("billing_first_name", &self.customer_type)
        {
            self.billing_first_name = self.first_name.clone();
        }
        if self.billing_last_name.is_empty()
            && !policy.overridden("billing_last_name", &self.customer_type)
        {
            self.billing_last_name = self.last_name.clone();
        }
        self.invoice_requested |= self.customer_type == "business";
        if self.billing_same {
            self.billing_country = self.shipping_country.clone();
            self.billing_street = self.shipping_street.clone();
            self.billing_number = self.shipping_number.clone();
            self.billing_extra = self.shipping_extra.clone();
            self.billing_postal = self.shipping_postal.clone();
            self.billing_city = self.shipping_city.clone();
            self.billing_region = self.shipping_region.clone();
        }
        if !self.invoice_requested {
            self.billing_first_name.clear();
            self.billing_last_name.clear();
            self.billing_company.clear();
            self.billing_country.clear();
            self.billing_street.clear();
            self.billing_number.clear();
            self.billing_extra.clear();
            self.billing_postal.clear();
            self.billing_city.clear();
            self.billing_region.clear();
            self.vat_id.clear();
            self.tax_id.clear();
            self.recipient_code.clear();
            self.pec.clear();
            self.invoice_channel = "none".into();
        } else {
            if policy.mode("billing_company", &self.customer_type) == Mode::Hidden {
                self.billing_company.clear();
            }
            if policy.mode("vat_id", &self.customer_type) == Mode::Hidden {
                self.vat_id.clear();
            }
            if self.billing_country == "IT" {
                let code = policy.mode("recipient_code", &self.customer_type) == Mode::Required
                    || !self.recipient_code.is_empty();
                let pec = policy.mode("pec", &self.customer_type) == Mode::Required
                    || !self.pec.is_empty();
                self.invoice_channel = match (code, pec) {
                    (true, true) => "both",
                    (true, false) => "code",
                    (false, true) => "pec",
                    (false, false) => "none",
                }
                .into();
            } else {
                self.invoice_channel = "none".into();
                self.recipient_code.clear();
                self.pec.clear();
            }
        }
        self.tax_id = self.tax_id.to_ascii_uppercase();
        self.vat_id = self.vat_id.to_ascii_uppercase();
        self.recipient_code = self.recipient_code.to_ascii_uppercase();
        if self.billing_country == "IT"
            && let Some(v) = self.vat_id.strip_prefix("IT")
        {
            self.vat_id = v.into();
        }
        self
    }
    pub fn validate_with(self, policy: &Policy) -> Result<Self, Vec<&'static str>> {
        let self_data = self.normalize_with(policy);
        self_data.check_with(policy)
    }
    pub(crate) fn check_with(self, policy: &Policy) -> Result<Self, Vec<&'static str>> {
        let mut errors = Vec::new();
        if self.email.chars().count() > 254 || self.email.chars().any(|c| c.is_control()) {
            errors.push("email");
        }
        if self.first_name.chars().count() > 100 || self.first_name.chars().any(|c| c.is_control())
        {
            errors.push("first_name");
        }
        if self.last_name.chars().count() > 100 || self.last_name.chars().any(|c| c.is_control()) {
            errors.push("last_name");
        }
        if self.phone.chars().count() > 40 || self.phone.chars().any(|c| c.is_control()) {
            errors.push("phone");
        }
        if self.recipient_first_name.chars().count() > 100
            || self.recipient_first_name.chars().any(|c| c.is_control())
        {
            errors.push("recipient_first_name");
        }
        if self.recipient_last_name.chars().count() > 100
            || self.recipient_last_name.chars().any(|c| c.is_control())
        {
            errors.push("recipient_last_name");
        }
        if self.shipping_company.chars().count() > 160
            || self.shipping_company.chars().any(|c| c.is_control())
        {
            errors.push("shipping_company");
        }
        if self.shipping_country.chars().count() > 2
            || self.shipping_country.chars().any(|c| c.is_control())
        {
            errors.push("shipping_country");
        }
        if self.shipping_street.chars().count() > 160
            || self.shipping_street.chars().any(|c| c.is_control())
        {
            errors.push("shipping_street");
        }
        if self.shipping_number.chars().count() > 20
            || self.shipping_number.chars().any(|c| c.is_control())
        {
            errors.push("shipping_number");
        }
        if self.shipping_extra.chars().count() > 160
            || self.shipping_extra.chars().any(|c| c.is_control())
        {
            errors.push("shipping_extra");
        }
        if self.shipping_postal.chars().count() > 20
            || self.shipping_postal.chars().any(|c| c.is_control())
        {
            errors.push("shipping_postal");
        }
        if self.shipping_city.chars().count() > 100
            || self.shipping_city.chars().any(|c| c.is_control())
        {
            errors.push("shipping_city");
        }
        if self.shipping_region.chars().count() > 100
            || self.shipping_region.chars().any(|c| c.is_control())
        {
            errors.push("shipping_region");
        }
        if self.delivery_notes.chars().count() > 1000
            || self
                .delivery_notes
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\r')
        {
            errors.push("delivery_notes");
        }
        if self.customer_type.chars().count() > 12
            || self.customer_type.chars().any(|c| c.is_control())
        {
            errors.push("customer_type");
        }
        if self.billing_first_name.chars().count() > 100
            || self.billing_first_name.chars().any(|c| c.is_control())
        {
            errors.push("billing_first_name");
        }
        if self.billing_last_name.chars().count() > 100
            || self.billing_last_name.chars().any(|c| c.is_control())
        {
            errors.push("billing_last_name");
        }
        if self.billing_company.chars().count() > 160
            || self.billing_company.chars().any(|c| c.is_control())
        {
            errors.push("billing_company");
        }
        if self.billing_country.chars().count() > 2
            || self.billing_country.chars().any(|c| c.is_control())
        {
            errors.push("billing_country");
        }
        if self.billing_street.chars().count() > 160
            || self.billing_street.chars().any(|c| c.is_control())
        {
            errors.push("billing_street");
        }
        if self.billing_number.chars().count() > 20
            || self.billing_number.chars().any(|c| c.is_control())
        {
            errors.push("billing_number");
        }
        if self.billing_extra.chars().count() > 160
            || self.billing_extra.chars().any(|c| c.is_control())
        {
            errors.push("billing_extra");
        }
        if self.billing_postal.chars().count() > 20
            || self.billing_postal.chars().any(|c| c.is_control())
        {
            errors.push("billing_postal");
        }
        if self.billing_city.chars().count() > 100
            || self.billing_city.chars().any(|c| c.is_control())
        {
            errors.push("billing_city");
        }
        if self.billing_region.chars().count() > 100
            || self.billing_region.chars().any(|c| c.is_control())
        {
            errors.push("billing_region");
        }
        if self.vat_id.chars().count() > 40 || self.vat_id.chars().any(|c| c.is_control()) {
            errors.push("vat_id");
        }
        if self.tax_id.chars().count() > 40 || self.tax_id.chars().any(|c| c.is_control()) {
            errors.push("tax_id");
        }
        if self.invoice_channel.chars().count() > 8
            || self.invoice_channel.chars().any(|c| c.is_control())
        {
            errors.push("invoice_channel");
        }
        if self.recipient_code.chars().count() > 7
            || self.recipient_code.chars().any(|c| c.is_control())
        {
            errors.push("recipient_code");
        }
        if self.pec.chars().count() > 254 || self.pec.chars().any(|c| c.is_control()) {
            errors.push("pec");
        }

        let kind = self.customer_type.as_str();
        if !customer_fields::valid_type(kind) {
            errors.push("customer_type");
        }
        for f in customer_fields::FIELDS {
            if policy.mode(f.name, kind) != Mode::Required
                || (f.scope == "billing" && !self.invoice_requested)
                || matches!(
                    f.name,
                    "shipping_postal"
                        | "shipping_region"
                        | "billing_postal"
                        | "billing_region"
                        | "tax_id"
                        | "recipient_code"
                        | "pec"
                )
            {
                continue;
            }
            if self.value(f.name).is_empty() {
                errors.push(f.name);
            }
        }
        if talocart_platform::mail::address(&self.email).is_err() {
            errors.push("email");
        }
        let digits = self.phone.bytes().filter(u8::is_ascii_digit).count();
        if !self.phone.is_empty()
            && (!(7..=15).contains(&digits)
                || !self
                    .phone
                    .chars()
                    .all(|c| c.is_ascii_digit() || "+()-. ".contains(c)))
        {
            errors.push("phone");
        }
        for prefix in ["shipping", "billing"] {
            if prefix == "billing" && !self.invoice_requested {
                continue;
            }
            let (country, postal, region, country_key, postal_key, region_key) =
                if prefix == "shipping" {
                    (
                        &self.shipping_country,
                        &self.shipping_postal,
                        &self.shipping_region,
                        "shipping_country",
                        "shipping_postal",
                        "shipping_region",
                    )
                } else {
                    (
                        &self.billing_country,
                        &self.billing_postal,
                        &self.billing_region,
                        "billing_country",
                        "billing_postal",
                        "billing_region",
                    )
                };
            if !geography::country_valid(country) {
                errors.push(country_key);
            }
            let pm = policy.mode(postal_key, kind);
            if pm != Mode::Hidden
                && (pm == Mode::Required || !postal.is_empty())
                && !postal_valid(country, postal)
            {
                errors.push(postal_key);
            }
            let rm = policy.mode(region_key, kind);
            if rm == Mode::Required
                && ["IT", "US", "CA", "AU"].contains(&country.as_str())
                && region.is_empty()
            {
                errors.push(region_key);
            }
            if rm != Mode::Hidden
                && !region.is_empty()
                && country == "IT"
                && !(region.len() == 2 && region.bytes().all(|b| b.is_ascii_alphabetic()))
            {
                errors.push(region_key);
            }
        }
        if self.invoice_requested {
            if !self.vat_id.is_empty()
                && (!self
                    .vat_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || " -./".contains(c))
                    || (self.billing_country == "IT" && !italian_vat(&self.vat_id)))
            {
                errors.push("vat_id");
            }
            if policy.mode("tax_id", kind) != Mode::Hidden
                && self.billing_country == "IT"
                && (!self.tax_id.is_empty() || policy.mode("tax_id", kind) == Mode::Required)
                && !italian_tax_id(&self.tax_id)
            {
                errors.push("tax_id");
            }
            if !["none", "code", "pec", "both"].contains(&self.invoice_channel.as_str()) {
                errors.push("invoice_channel");
            }
            if ["code", "both"].contains(&self.invoice_channel.as_str())
                && !(self.recipient_code.len() == 7
                    && self
                        .recipient_code
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric()))
            {
                errors.push("recipient_code");
            }
            if ["pec", "both"].contains(&self.invoice_channel.as_str())
                && talocart_platform::mail::address(&self.pec).is_err()
            {
                errors.push("pec");
            }
        }
        errors.sort_unstable();
        errors.dedup();
        if errors.is_empty() {
            Ok(self)
        } else {
            Err(errors)
        }
    }
}
// Conservative postal validation: exact formats for supported common destinations;
// other ISO countries accept bounded text, including countries without postal codes.
fn postal_valid(country: &str, postal: &str) -> bool {
    let n = match country {
        "IT" | "DE" | "FR" | "ES" => 5,
        "AT" | "CH" | "AU" => 4,
        _ => 0,
    };
    if n > 0 {
        return postal.len() == n && postal.bytes().all(|b| b.is_ascii_digit());
    }
    match country {
        "US" => {
            (postal.len() == 5 && postal.bytes().all(|b| b.is_ascii_digit()))
                || (postal.len() == 10
                    && postal.as_bytes()[5] == b'-'
                    && postal
                        .bytes()
                        .enumerate()
                        .all(|(i, b)| i == 5 || b.is_ascii_digit()))
        }
        "CA" => {
            let p = postal.replace(' ', "");
            p.len() == 6
                && p.bytes().enumerate().all(|(i, b)| {
                    if i % 2 == 0 {
                        b.is_ascii_alphabetic()
                    } else {
                        b.is_ascii_digit()
                    }
                })
        }
        "GB" => {
            !postal.is_empty()
                && postal
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b' ')
        }
        _ => true,
    }
}
fn italian_vat(v: &str) -> bool {
    if v.len() != 11 || !v.bytes().all(|b| b.is_ascii_digit()) || v == "00000000000" {
        return false;
    }
    let sum: u32 = v
        .bytes()
        .enumerate()
        .map(|(i, b)| {
            let n = u32::from(b - b'0');
            if i % 2 == 1 {
                let n = n * 2;
                if n > 9 { n - 9 } else { n }
            } else {
                n
            }
        })
        .sum();
    sum.is_multiple_of(10)
}
// Syntax/check digit only; no assertion of registration or correspondence to a name.
fn italian_tax_id(v: &str) -> bool {
    if v.len() == 11 {
        return italian_vat(v);
    }
    if v.len() != 16 || !v.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    let b = v.as_bytes();
    if !b[..6].iter().all(u8::is_ascii_uppercase)
        || !b"ABCDEHLMPRST".contains(&b[8])
        || !b[11].is_ascii_uppercase()
        || !b[15].is_ascii_uppercase()
    {
        return false;
    }
    if [6, 7, 9, 10, 12, 13, 14]
        .iter()
        .any(|i| !b[*i].is_ascii_digit() && !b"LMNPQRSTUV".contains(&b[*i]))
    {
        return false;
    }
    const ODD: [u8; 26] = [
        1, 0, 5, 7, 9, 13, 15, 17, 19, 21, 2, 4, 18, 20, 11, 3, 6, 8, 12, 14, 16, 10, 22, 25, 24,
        23,
    ];
    let sum: u32 = b[..15]
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let n = if c.is_ascii_digit() {
                c - b'0'
            } else {
                c - b'A'
            };
            u32::from(if i % 2 == 0 { ODD[usize::from(n)] } else { n })
        })
        .sum();
    sum % 26 == u32::from(b[15] - b'A')
}

pub(crate) fn form_bool<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        Bool(bool),
        Text(String),
    }
    match Value::deserialize(d)? {
        Value::Bool(v) => Ok(v),
        Value::Text(v) => v.parse().map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid() -> Details {
        Details {
            email: "buyer@example.test".into(),
            first_name: "Alex".into(),
            last_name: "Example".into(),
            phone: "+39 333 1234567".into(),
            shipping_country: "IT".into(),
            shipping_street: "Via Esempio".into(),
            shipping_number: "SNC".into(),
            shipping_postal: "00100".into(),
            shipping_city: "Roma".into(),
            shipping_region: "RM".into(),
            ..Default::default()
        }
    }
    fn policy(changes: &[(&str, &str)]) -> Policy {
        let mut choices = Policy::default().choices();
        for (key, mode) in changes {
            choices
                .iter_mut()
                .find(|(k, _)| k == key)
                .expect("Known field")
                .1 = mode.to_string();
        }
        Policy::from_choices(&choices).expect("Valid field policy")
    }
    #[test]
    fn required_markers_follow_invoice_and_country() {
        let mut d = valid();
        let p = Policy::default();
        assert!(d.field_required("shipping_region", &p));
        assert!(!d.field_required("tax_id", &p));
        d.shipping_country = "FR".into();
        assert!(!d.field_required("shipping_region", &p));
        assert!(d.field_required("shipping_postal", &p));
        d.shipping_country = "HK".into();
        assert!(!d.field_required("shipping_postal", &p));
        d.invoice_requested = true;
        d.billing_same = false;
        d.billing_country = "IT".into();
        assert!(d.field_required("tax_id", &p));
        assert!(d.field_required("billing_region", &p));
    }
    #[test]
    fn optional_invoice_channels_are_derived_from_entered_values() {
        let mut d = valid();
        d.customer_type = "business".into();
        d.billing_company = "Example".into();
        d.vat_id = "12345678903".into();
        d.recipient_code = "ABC1234".into();
        d.pec = "invoice@example.test".into();
        let saved = d.validate().expect("Valid optional channels");
        assert_eq!(saved.invoice_channel, "both");
        assert_eq!(saved.recipient_code, "ABC1234");
        assert_eq!(saved.pec, "invoice@example.test");
    }
    #[test]
    fn configured_hidden_optional_required_and_type_are_independent() {
        let p = policy(&[
            ("private.phone", "hidden"),
            ("private.first_name", "optional"),
            ("private.delivery_notes", "required"),
        ]);
        let mut d = valid();
        d.phone = "forged\nvalue".into();
        d.first_name.clear();
        d.recipient_first_name = "Recipient".into();
        assert!(
            d.clone()
                .validate_with(&p)
                .err()
                .expect("Missing notes")
                .contains(&"delivery_notes")
        );
        d.delivery_notes = "Reception".into();
        let d = d.validate_with(&p).expect("Configured details");
        assert!(d.phone.is_empty());
        assert!(d.first_name.is_empty());
        let mut d = valid();
        d.customer_type = "business".into();
        d.phone.clear();
        assert!(
            d.validate_with(&p)
                .err()
                .expect("Business phone still required")
                .contains(&"phone")
        );
    }
    #[test]
    fn billing_sdi_and_pec_can_both_be_required_but_only_when_applicable() {
        let p = policy(&[
            ("business.recipient_code", "required"),
            ("business.pec", "required"),
        ]);
        let mut d = valid();
        d.customer_type = "business".into();
        d.billing_company = "Example".into();
        d.vat_id = "12345678903".into();
        let errors = d
            .clone()
            .validate_with(&p)
            .err()
            .expect("Missing both channels");
        assert!(errors.contains(&"recipient_code") && errors.contains(&"pec"));
        d.recipient_code = "abc1234".into();
        d.pec = "invoice@example.test".into();
        let d = d.validate_with(&p).expect("Both channels");
        assert_eq!(d.invoice_channel, "both");
        assert_eq!(d.recipient_code, "ABC1234");
        assert!(valid().validate_with(&p).is_ok());
    }
    #[test]
    fn postal_optional_still_checks_format_and_hidden_inputs_are_ignored() {
        let p = policy(&[
            ("private.shipping_postal", "optional"),
            ("private.shipping_region", "hidden"),
        ]);
        let mut d = valid();
        d.shipping_postal.clear();
        d.shipping_region = "forged".into();
        assert!(d.clone().validate_with(&p).is_ok());
        d.shipping_postal = "wrong".into();
        assert!(
            d.validate_with(&p)
                .err()
                .expect("Invalid present postcode")
                .contains(&"shipping_postal")
        );
    }
    #[test]
    fn private_without_invoice_discards_unused_fiscal_data() {
        let mut d = valid();
        d.vat_id = "secret".into();
        d.tax_id = "secret".into();
        d.pec = "secret".into();
        let d = d.validate().unwrap();
        assert!(
            d.tax_id.is_empty()
                && d.vat_id.is_empty()
                && d.pec.is_empty()
                && d.billing_street.is_empty()
        );
        assert_eq!(d.recipient_first_name, "Alex");
    }
    #[test]
    fn private_invoice_requires_tax_code_and_keeps_separate_addressee() {
        let mut d = valid();
        d.invoice_requested = true;
        assert!(d.clone().validate().err().unwrap().contains(&"tax_id"));
        d.tax_id = "RSSMRA80A01H501U".into();
        d.recipient_first_name = "Receiver".into();
        let d = d.validate().unwrap();
        assert_eq!(d.billing_first_name, "Alex");
        assert_eq!(d.recipient_first_name, "Receiver");
        assert_eq!(d.billing_street, d.shipping_street);
    }
    #[test]
    fn business_invoice_cannot_be_bypassed_and_channels_follow_values() {
        let mut d = valid();
        d.customer_type = "business".into();
        assert!(d.clone().validate().err().unwrap().contains(&"vat_id"));
        d.billing_company = "Example".into();
        d.vat_id = "IT12345678903".into();
        d.invoice_channel = "pec".into();
        d.pec = "invalid".into();
        assert!(d.clone().validate().err().unwrap().contains(&"pec"));
        d.pec = "billing@example.test".into();
        d.recipient_code = "ABCDEFG".into();
        let d = d.validate().unwrap();
        assert!(d.invoice_requested);
        assert_eq!(d.vat_id, "12345678903");
        assert_eq!(d.recipient_code, "ABCDEFG");
        assert_eq!(d.invoice_channel, "both");
    }
    #[test]
    fn foreign_billing_is_independent_of_shipping_and_italian_delivery() {
        let mut d = valid();
        d.customer_type = "business".into();
        d.billing_company = "Example".into();
        d.vat_id = "FR123456".into();
        d.billing_same = false;
        d.billing_country = "FR".into();
        d.billing_street = "Rue Example".into();
        d.billing_number = "1".into();
        d.billing_city = "Paris".into();
        d.billing_postal = "75001".into();
        d.invoice_channel = "pec".into();
        let d = d.validate().unwrap();
        assert_eq!(d.shipping_country, "IT");
        assert_eq!(d.billing_country, "FR");
        assert_eq!(d.invoice_channel, "none");
    }
    #[test]
    fn italian_tax_codes_support_omocodia_and_reject_wrong_checksums() {
        assert!(italian_tax_id("RSSMRA80A01H501U"));
        assert!(italian_tax_id("RSSMRA80A01H50MM"));
        assert!(italian_tax_id("12345678903"));
        assert!(!italian_tax_id("RSSMRA80A01H501A"));
        assert!(!italian_tax_id("1234567890123456"));
    }
    #[test]
    fn addresses_phone_headers_and_country_formats_are_bounded() {
        let mut d = valid();
        d.email = "x@example.test\r\nBcc:other@example.test".into();
        d.phone = "123".into();
        d.shipping_postal = "1234".into();
        d.shipping_region = "Roma".into();
        let e = d.validate().err().unwrap();
        for f in ["email", "phone", "shipping_postal", "shipping_region"] {
            assert!(e.contains(&f));
        }
        assert!(postal_valid("HK", ""));
        assert!(postal_valid("CA", "K1A 0B1"));
        assert!(postal_valid("US", "12345-6789"));
        assert!(!postal_valid("US", "1234"));
        assert!(!italian_vat("12345678900"));
        assert!(!italian_vat("00000000000"));
    }
}
