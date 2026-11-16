//! Compiled ISO country and two-decimal currency allowlists. No runtime external lookup.

/// Supported ISO 4217 currencies; all use hundredths. No exchange-rate conversion.
pub const CURRENCIES: &[&str] = &[
    "EUR", "USD", "GBP", "CHF", "CAD", "AUD", "NZD", "SEK", "NOK", "DKK", "PLN", "CZK", "RON",
];
/// ISO 3166-1 alpha-2 nations/territories; names are localized by the web catalogs.
pub const COUNTRIES: &[&str] = &[
    "AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AX", "AZ",
    "BA", "BB", "BD", "BE", "BF", "BG", "BH", "BI", "BJ", "BL", "BM", "BN", "BO", "BQ", "BR", "BS",
    "BT", "BV", "BW", "BY", "BZ", "CA", "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN",
    "CO", "CR", "CU", "CV", "CW", "CX", "CY", "CZ", "DE", "DJ", "DK", "DM", "DO", "DZ", "EC", "EE",
    "EG", "EH", "ER", "ES", "ET", "FI", "FJ", "FK", "FM", "FO", "FR", "GA", "GB", "GD", "GE", "GF",
    "GG", "GH", "GI", "GL", "GM", "GN", "GP", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK", "HM",
    "HN", "HR", "HT", "HU", "ID", "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE", "JM",
    "JO", "JP", "KE", "KG", "KH", "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB", "LC",
    "LI", "LK", "LR", "LS", "LT", "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MF", "MG", "MH", "MK",
    "ML", "MM", "MN", "MO", "MP", "MQ", "MR", "MS", "MT", "MU", "MV", "MW", "MX", "MY", "MZ", "NA",
    "NC", "NE", "NF", "NG", "NI", "NL", "NO", "NP", "NR", "NU", "NZ", "OM", "PA", "PE", "PF", "PG",
    "PH", "PK", "PL", "PM", "PN", "PR", "PS", "PT", "PW", "PY", "QA", "RE", "RO", "RS", "RU", "RW",
    "SA", "SB", "SC", "SD", "SE", "SG", "SH", "SI", "SJ", "SK", "SL", "SM", "SN", "SO", "SR", "SS",
    "ST", "SV", "SX", "SY", "SZ", "TC", "TD", "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN", "TO",
    "TR", "TT", "TV", "TW", "TZ", "UA", "UG", "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG", "VI",
    "VN", "VU", "WF", "WS", "YE", "YT", "ZA", "ZM", "ZW",
];
pub fn currency_valid(code: &str) -> bool {
    CURRENCIES.contains(&code)
}
pub fn country_valid(code: &str) -> bool {
    COUNTRIES.contains(&code)
}
pub fn currency_label(code: &str) -> &str {
    if code == "EUR" { "€" } else { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compiled_allowlists_are_unique_and_reject_unknown_codes() {
        assert_eq!(COUNTRIES.len(), 249);
        for list in [COUNTRIES, CURRENCIES] {
            let unique: std::collections::BTreeSet<_> = list.iter().collect();
            assert_eq!(unique.len(), list.len());
        }
        assert!(country_valid("IT"));
        assert!(!country_valid("ZZ"));
        assert!(!country_valid("it"));
        assert!(currency_valid("USD"));
        assert!(!currency_valid("JPY"));
        assert!(!currency_valid("usd"));
        assert_eq!(currency_label("EUR"), "€");
        assert_eq!(currency_label("USD"), "USD");
    }
}
