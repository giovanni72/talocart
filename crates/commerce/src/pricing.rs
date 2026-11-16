//! Two-decimal price contract: non-negative cents, basis points, exact rational tax arithmetic.
use thiserror::Error;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PriceError {
    #[error("Importo o quantità non validi")]
    Invalid,
    #[error("Importo troppo grande per il calcolo")]
    Overflow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceMode {
    Inclusive,
    Exclusive,
}
impl PriceMode {
    pub fn key(self) -> &'static str {
        match self {
            Self::Inclusive => "inclusive",
            Self::Exclusive => "exclusive",
        }
    }
    pub fn parse(value: &str) -> Result<Self, PriceError> {
        match value {
            "inclusive" => Ok(Self::Inclusive),
            "exclusive" => Ok(Self::Exclusive),
            _ => Err(PriceError::Invalid),
        }
    }
}
/// Hundredths of the selected store currency. Signed storage is used for PostgreSQL BIGINT compatibility; negatives are rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Money(i64);
impl Money {
    pub fn from_cents(cents: i64) -> Result<Self, PriceError> {
        if cents < 0 {
            Err(PriceError::Invalid)
        } else {
            Ok(Self(cents))
        }
    }
    pub fn parse(value: &str) -> Result<Self, PriceError> {
        Self::from_cents(parse_hundredths(value)?)
    }
    pub fn cents(self) -> i64 {
        self.0
    }
    pub fn display(self) -> String {
        format!("{},{:02}", self.0 / 100, self.0 % 100)
    }
}
/// 100 basis points = 1%. This initial single-rate model supports 0..=100%.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate(i32);
impl Rate {
    pub fn from_basis_points(value: i32) -> Result<Self, PriceError> {
        if (0..=10000).contains(&value) {
            Ok(Self(value))
        } else {
            Err(PriceError::Invalid)
        }
    }
    pub fn parse(value: &str) -> Result<Self, PriceError> {
        let v = parse_hundredths(value)?;
        Self::from_basis_points(i32::try_from(v).map_err(|_| PriceError::Invalid)?)
    }
    pub fn basis_points(self) -> i32 {
        self.0
    }
    pub fn display(self) -> String {
        format!("{},{:02}", self.0 / 100, self.0 % 100)
    }
}
fn parse_hundredths(value: &str) -> Result<i64, PriceError> {
    let value = value.trim();
    let mut parts = value.split(['.', ',']);
    let whole = parts.next().ok_or(PriceError::Invalid)?;
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return Err(PriceError::Invalid);
    }
    let fraction = match parts.next() {
        None => 0,
        Some(part)
            if !part.is_empty() && part.len() <= 2 && part.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let v = part.parse::<i64>().map_err(|_| PriceError::Invalid)?;
            if part.len() == 1 { v * 10 } else { v }
        }
        _ => return Err(PriceError::Invalid),
    };
    if parts.next().is_some() {
        return Err(PriceError::Invalid);
    }
    whole
        .parse::<i64>()
        .map_err(|_| PriceError::Overflow)?
        .checked_mul(100)
        .and_then(|v| v.checked_add(fraction))
        .ok_or(PriceError::Overflow)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineTotals {
    net: Money,
    tax: Money,
    gross: Money,
}
impl LineTotals {
    pub fn net(self) -> Money {
        self.net
    }
    pub fn tax(self) -> Money {
        self.tax
    }
    pub fn gross(self) -> Money {
        self.gross
    }
}
fn rounded(numerator: i128, denominator: i128) -> Result<i64, PriceError> {
    let value = numerator
        .checked_add(denominator / 2)
        .and_then(|v| v.checked_div(denominator))
        .ok_or(PriceError::Overflow)?;
    i64::try_from(value).map_err(|_| PriceError::Overflow)
}
/// Multiply the unit price first, round once per line (half-up), then derive the remainder.
/// Included prices keep the entered gross amount; excluded prices keep the entered net amount.
pub fn line(
    unit: Money,
    quantity: u32,
    rate: Rate,
    mode: PriceMode,
) -> Result<LineTotals, PriceError> {
    if quantity == 0 {
        return Err(PriceError::Invalid);
    }
    let entered = unit
        .0
        .checked_mul(i64::from(quantity))
        .ok_or(PriceError::Overflow)?;
    let (net, tax, gross) = match mode {
        PriceMode::Exclusive => {
            let numerator = i128::from(entered)
                .checked_mul(i128::from(rate.0))
                .ok_or(PriceError::Overflow)?;
            let tax = rounded(numerator, 10000)?;
            (
                entered,
                tax,
                entered.checked_add(tax).ok_or(PriceError::Overflow)?,
            )
        }
        PriceMode::Inclusive => {
            let numerator = i128::from(entered)
                .checked_mul(10000)
                .ok_or(PriceError::Overflow)?;
            let net = rounded(numerator, 10000 + i128::from(rate.0))?;
            (
                net,
                entered.checked_sub(net).ok_or(PriceError::Overflow)?,
                entered,
            )
        }
    };
    Ok(LineTotals {
        net: Money(net),
        tax: Money(tax),
        gross: Money(gross),
    })
}
/// Resolve the final unit price in cents, rounding the resulting price half-up.
/// All later line and tax calculations use this exact displayed unit price.
pub fn discounted_unit(unit: Money, discount: Rate) -> Result<Money, PriceError> {
    Money::from_cents(rounded(
        i128::from(unit.0) * i128::from(10000 - discount.0),
        10000,
    )?)
}
/// Totals sum rounded lines; never recompute tax on the aggregate or round unit taxes first.
pub fn total(lines: &[LineTotals]) -> Result<LineTotals, PriceError> {
    let mut result = LineTotals {
        net: Money(0),
        tax: Money(0),
        gross: Money(0),
    };
    for line in lines {
        result.net.0 = result
            .net
            .0
            .checked_add(line.net.0)
            .ok_or(PriceError::Overflow)?;
        result.tax.0 = result
            .tax
            .0
            .checked_add(line.tax.0)
            .ok_or(PriceError::Overflow)?;
        result.gross.0 = result
            .gross
            .0
            .checked_add(line.gross.0)
            .ok_or(PriceError::Overflow)?;
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quantity_price_is_a_final_unit_price_and_keeps_line_totals_consistent() {
        for (cents, percent, expected) in [
            (1000, "10", 900),
            (999, "10", 899),
            (1, "50", 1),
            (999, "50", 500),
            (1, "0", 1),
            (i64::MAX, "100", 0),
        ] {
            let unit = discounted_unit(
                Money::from_cents(cents).unwrap(),
                Rate::parse(percent).unwrap(),
            )
            .unwrap();
            assert_eq!(unit.cents(), expected);
            for mode in [PriceMode::Inclusive, PriceMode::Exclusive] {
                let totals = line(unit, 3, Rate::parse("22").unwrap(), mode).unwrap();
                assert_eq!(
                    if mode == PriceMode::Exclusive {
                        totals.net().cents()
                    } else {
                        totals.gross().cents()
                    },
                    expected * 3
                );
            }
        }
        assert_eq!(
            line(
                Money::from_cents(i64::MAX).unwrap(),
                2,
                Rate::parse("0").unwrap(),
                PriceMode::Inclusive
            ),
            Err(PriceError::Overflow)
        );
    }
    #[test]
    fn decimal_input_is_exact_bounded_and_unambiguous() {
        for (input, cents) in [
            ("0", 0),
            ("12,34", 1234),
            (" 12.3 ", 1230),
            ("001.02", 102),
            ("92233720368547758.07", i64::MAX),
        ] {
            assert_eq!(Money::parse(input).unwrap().cents(), cents);
        }
        for input in [
            "", "-1", "+1", "1e2", "NaN", ".5", "1.", "1,", "1.001", "1.000,00", "1 000", "１２",
        ] {
            assert!(Money::parse(input).is_err(), "{input}");
        }
        assert_eq!(
            Money::parse("92233720368547758.08"),
            Err(PriceError::Overflow)
        );
        for input in ["0", "7,50", "100.00"] {
            assert!(Rate::parse(input).is_ok());
        }
        for input in ["100.01", "-1", "1.001", "1e1"] {
            assert!(Rate::parse(input).is_err());
        }
        assert_eq!(Money::parse("12.3").unwrap().display(), "12,30");
        assert_eq!(Rate::parse("7.5").unwrap().display(), "7,50");
    }
    #[test]
    fn line_rounding_included_excluded_and_sum_contract() {
        let r = Rate::parse("50").unwrap();
        let unit = Money::from_cents(1).unwrap();
        let excluded = line(unit, 3, r, PriceMode::Exclusive).unwrap();
        assert_eq!(
            (
                excluded.net().cents(),
                excluded.tax().cents(),
                excluded.gross().cents()
            ),
            (3, 2, 5)
        );
        let per_unit = line(unit, 1, r, PriceMode::Exclusive).unwrap();
        assert_eq!(total(&[per_unit; 3]).unwrap().gross().cents(), 6);
        let included = line(Money::from_cents(5).unwrap(), 1, r, PriceMode::Inclusive).unwrap();
        assert_eq!(
            (
                included.net().cents(),
                included.tax().cents(),
                included.gross().cents()
            ),
            (3, 2, 5)
        );
        let sum = total(&[included, excluded]).unwrap();
        assert_eq!(
            (sum.net().cents(), sum.tax().cents(), sum.gross().cents()),
            (6, 4, 10)
        );
        assert_eq!(total(&[]).unwrap().gross().cents(), 0);
    }
    #[test]
    fn rounding_ties_zero_rates_and_reconciliation() {
        for cents in 0..1000 {
            for bp in [0, 1, 750, 3333, 5000, 10000] {
                for mode in [PriceMode::Inclusive, PriceMode::Exclusive] {
                    let result = line(
                        Money::from_cents(cents).unwrap(),
                        3,
                        Rate::from_basis_points(bp).unwrap(),
                        mode,
                    )
                    .unwrap();
                    assert_eq!(
                        result.net().cents() + result.tax().cents(),
                        result.gross().cents()
                    );
                    if bp == 0 {
                        assert_eq!(result.tax().cents(), 0);
                    }
                    assert_eq!(
                        if mode == PriceMode::Inclusive {
                            result.gross().cents()
                        } else {
                            result.net().cents()
                        },
                        cents * 3
                    );
                }
            }
        }
        assert_eq!(
            line(
                Money::from_cents(1).unwrap(),
                1,
                Rate::parse("100").unwrap(),
                PriceMode::Inclusive
            )
            .unwrap()
            .net()
            .cents(),
            1
        );
    }
    #[test]
    fn overflow_and_invalid_quantity_are_errors() {
        let max = Money::from_cents(i64::MAX).unwrap();
        let zero = Rate::parse("0").unwrap();
        assert_eq!(
            line(max, 2, zero, PriceMode::Inclusive),
            Err(PriceError::Overflow)
        );
        assert_eq!(
            line(max, 1, Rate::parse("100").unwrap(), PriceMode::Exclusive),
            Err(PriceError::Overflow)
        );
        assert_eq!(
            line(max, 0, zero, PriceMode::Exclusive),
            Err(PriceError::Invalid)
        );
        let huge = line(max, 1, zero, PriceMode::Inclusive).unwrap();
        assert_eq!(total(&[huge, huge]), Err(PriceError::Overflow));
        assert!(Money::from_cents(-1).is_err());
    }
}
