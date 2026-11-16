//! National formats from SWIFT IBAN Registry release 103 (September 2026).
//! Source: https://www.swift.com/swift-resource/9606/download
//! This is format/checksum validation, not proof of account existence or ownership.
//! Update the table against a new registry release and run the format tests together.
const FORMATS: &[(&[u8; 2], &[u8])] = &[
    (b"AD", b"nnnnnnnnnncccccccccccc"),
    (b"AE", b"nnnnnnnnnnnnnnnnnnnnn"),
    (b"AL", b"nnnnnnnnnncccccccccccccccc"),
    (b"AT", b"nnnnnnnnnnnnnnnnnn"),
    (b"AZ", b"nnaaaacccccccccccccccccccc"),
    (b"BA", b"nnnnnnnnnnnnnnnnnn"),
    (b"BE", b"nnnnnnnnnnnnnn"),
    (b"BG", b"nnaaaannnnnncccccccc"),
    (b"BH", b"nnaaaacccccccccccccc"),
    (b"BI", b"nnnnnnnnnnnnnnnnnnnnnnnnn"),
    (b"BR", b"nnccccccccnnnnnnnnnnnnnnncc"),
    (b"BY", b"nnccccnnnncccccccccccccccc"),
    (b"CH", b"nnnnnnncccccccccccc"),
    (b"CR", b"nnnnnnnnnnnnnnnnnnnn"),
    (b"CY", b"nnnnnnnnnncccccccccccccccc"),
    (b"CZ", b"nnnnnnnnnnnnnnnnnnnnnn"),
    (b"DE", b"nnnnnnnnnnnnnnnnnnnn"),
    (b"DJ", b"nnnnnnnnnnnnnnnnnnnnnnnnn"),
    (b"DK", b"nnnnnnnnnnnnnnnn"),
    (b"DO", b"nnccccnnnnnnnnnnnnnnnnnnnn"),
    (b"EE", b"nnnnnnnnnnnnnnnnnn"),
    (b"EG", b"nnnnnnnnnnnnnnnnnnnnnnnnnnn"),
    (b"ES", b"nnnnnnnnnnnnnnnnnnnnnn"),
    (b"FI", b"nnnnnnnnnnnnnnnn"),
    (b"FK", b"nnaannnnnnnnnnnn"),
    (b"FO", b"nnnnnnnnnnnnnnnn"),
    (b"FR", b"nnnnnnnnnnnncccccccccccnn"),
    (b"GB", b"nnaaaannnnnnnnnnnnnn"),
    (b"GE", b"nnaannnnnnnnnnnnnnnn"),
    (b"GI", b"nnaaaaccccccccccccccc"),
    (b"GL", b"nnnnnnnnnnnnnnnn"),
    (b"GR", b"nnnnnnnnncccccccccccccccc"),
    (b"GT", b"nncccccccccccccccccccccccc"),
    (b"HN", b"nnaaaannnnnnnnnnnnnnnnnnnn"),
    (b"HR", b"nnnnnnnnnnnnnnnnnnn"),
    (b"HU", b"nnnnnnnnnnnnnnnnnnnnnnnnnn"),
    (b"IE", b"nnaaaannnnnnnnnnnnnn"),
    (b"IL", b"nnnnnnnnnnnnnnnnnnnnn"),
    (b"IQ", b"nnaaaannnnnnnnnnnnnnn"),
    (b"IS", b"nnnnnnnnnnnnnnnnnnnnnnnn"),
    (b"IT", b"nnannnnnnnnnncccccccccccc"),
    (b"JO", b"nnaaaannnncccccccccccccccccc"),
    (b"KW", b"nnaaaacccccccccccccccccccccc"),
    (b"KZ", b"nnnnnccccccccccccc"),
    (b"LB", b"nnnnnncccccccccccccccccccc"),
    (b"LC", b"nnaaaacccccccccccccccccccccccc"),
    (b"LI", b"nnnnnnncccccccccccc"),
    (b"LT", b"nnnnnnnnnnnnnnnnnn"),
    (b"LU", b"nnnnnccccccccccccc"),
    (b"LV", b"nnaaaaccccccccccccc"),
    (b"LY", b"nnnnnnnnnnnnnnnnnnnnnnn"),
    (b"MC", b"nnnnnnnnnnnncccccccccccnn"),
    (b"MD", b"nncccccccccccccccccccc"),
    (b"ME", b"nnnnnnnnnnnnnnnnnnnn"),
    (b"MK", b"nnnnnccccccccccnn"),
    (b"MN", b"nnnnnnnnnnnnnnnnnn"),
    (b"MR", b"nnnnnnnnnnnnnnnnnnnnnnnnn"),
    (b"MT", b"nnaaaannnnncccccccccccccccccc"),
    (b"MU", b"nnaaaannnnnnnnnnnnnnnnnnnaaa"),
    (b"NI", b"nnaaaannnnnnnnnnnnnnnnnnnn"),
    (b"NL", b"nnaaaannnnnnnnnn"),
    (b"NO", b"nnnnnnnnnnnnn"),
    (b"OM", b"nnnnncccccccccccccccc"),
    (b"PK", b"nnaaaacccccccccccccccc"),
    (b"PL", b"nnnnnnnnnnnnnnnnnnnnnnnnnn"),
    (b"PS", b"nnaaaaccccccccccccccccccccc"),
    (b"PT", b"nnnnnnnnnnnnnnnnnnnnnnn"),
    (b"QA", b"nnaaaaccccccccccccccccccccc"),
    (b"RO", b"nnaaaacccccccccccccccc"),
    (b"RS", b"nnnnnnnnnnnnnnnnnnnn"),
    (b"RU", b"nnnnnnnnnnnnnnnnccccccccccccccc"),
    (b"SA", b"nnnncccccccccccccccccc"),
    (b"SC", b"nnaaaannnnnnnnnnnnnnnnnnnnaaa"),
    (b"SD", b"nnnnnnnnnnnnnnnn"),
    (b"SE", b"nnnnnnnnnnnnnnnnnnnnnn"),
    (b"SI", b"nnnnnnnnnnnnnnnnn"),
    (b"SK", b"nnnnnnnnnnnnnnnnnnnnnn"),
    (b"SM", b"nnannnnnnnnnncccccccccccc"),
    (b"SO", b"nnnnnnnnnnnnnnnnnnnnn"),
    (b"ST", b"nnnnnnnnnnnnnnnnnnnnnnn"),
    (b"SV", b"nnaaaannnnnnnnnnnnnnnnnnnn"),
    (b"TL", b"nnnnnnnnnnnnnnnnnnnnn"),
    (b"TN", b"nnnnnnnnnnnnnnnnnnnnnn"),
    (b"TR", b"nnnnnnnncccccccccccccccc"),
    (b"UA", b"nnnnnnnnccccccccccccccccccc"),
    (b"VA", b"nnnnnnnnnnnnnnnnnnnn"),
    (b"VG", b"nnaaaannnnnnnnnnnnnnnn"),
    (b"XK", b"nnnnnnnnnnnnnnnnnn"),
    (b"YE", b"nnaaaannnncccccccccccccccccc"),
];

pub(super) fn valid(s: &str) -> bool {
    let b = s.as_bytes();
    if !(15..=34).contains(&b.len()) {
        return false;
    }
    let Some((_, format)) = FORMATS.iter().find(|(country, _)| b.starts_with(*country)) else {
        return false;
    };
    if b.len() != format.len() + 2
        || !b[2..].iter().zip(*format).all(|(c, kind)| match kind {
            b'n' => c.is_ascii_digit(),
            b'a' => c.is_ascii_uppercase(),
            b'c' => c.is_ascii_alphanumeric(),
            _ => false,
        })
    {
        return false;
    }
    // Valid check digits are 02..98; modulo alone also admits aliases 00, 01, 99.
    let check = (b[2] - b'0') * 10 + b[3] - b'0';
    if !(2..=98).contains(&check) {
        return false;
    }
    let mut remainder = 0u32;
    for c in b[4..].iter().chain(&b[..4]) {
        remainder = if c.is_ascii_digit() {
            (remainder * 10 + u32::from(c - b'0')) % 97
        } else {
            (remainder * 100 + u32::from(c.to_ascii_uppercase() - b'A' + 10)) % 97
        };
    }
    remainder == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    // Electronic examples from the same SWIFT release, independent of the format table.
    #[test]
    fn official_examples_and_invalid_lengths() {
        let examples = [
            "AD1200012030200359100100",
            "AE070331234567890123456",
            "AL47212110090000000235698741",
            "AT611904300234573201",
            "AZ21NABZ00000000137010001944",
            "BA391290079401028494",
            "BE68539007547034",
            "BG80BNBG96611020345678",
            "BH67BMAG00001299123456",
            "BI4210000100010000332045181",
            "BR6699999A03000010009795493C1",
            "BY13NBRB3600900000002Z00AB00",
            "CH9300762011623852957",
            "CR05015202001026284066",
            "CY17002001280000001200527600",
            "CZ6508000000192000145399",
            "DE89370400440532013000",
            "DJ2100010000000154000100186",
            "DK5000400440116243",
            "DO28BAGR00000001212453611324",
            "EE382200221020145685",
            "EG380019000500000000263180002",
            "ES9121000418450200051332",
            "FI2112345600000785",
            "FK88SC123456789012",
            "FO6264600001631634",
            "FR1420041010050500013M02606",
            "GB29NWBK60161331926819",
            "GE29NB0000000101904917",
            "GI75NWBK000000007099453",
            "GL8964710001000206",
            "GR1601101250000000012300695",
            "GT82TRAJ01020000001210029690",
            "HN88CABF00000000000250005469",
            "HR1210010051863000160",
            "HU42117730161111101800000000",
            "IE29AIBK93115212345678",
            "IL620108000000099999999",
            "IQ98NBIQ850123456789012",
            "IS140159260076545510730339",
            "IT60X0542811101000000123456",
            "JO94CBJO0010000000000131000302",
            "KW81CBKU0000000000001234560101",
            "KZ86125KZT5004100100",
            "LB62099900000001001901229114",
            "LC55HEMM000100010012001200023015",
            "LI21088100002324013AA",
            "LT121000011101001000",
            "LU280019400644750000",
            "LV80BANK0000435195001",
            "LY83002048000020100120361",
            "MC5811222000010123456789030",
            "MD24AG000225100013104168",
            "ME25505000012345678951",
            "MK07250120000058984",
            "MN121234123456789123",
            "MR1300020001010000123456753",
            "MT84MALT011000012345MTLCAST001S",
            "MU17BOMM0101101030300200000MUR",
            "NI45BAPR00000013000003558124",
            "NL91ABNA0417164300",
            "NO9386011117947",
            "OM810180000001299123456",
            "PK36SCBL0000001123456702",
            "PL61109010140000071219812874",
            "PS92PALS000000000400123456702",
            "PT50000201231234567890154",
            "QA58DOHB00001234567890ABCDEFG",
            "RO49AAAA1B31007593840000",
            "RS35260005601001611379",
            "RU0304452522540817810538091310419",
            "SA0380000000608010167519",
            "SC18SSCB11010000000000001497USD",
            "SD2129010501234001",
            "SE4550000000058398257466",
            "SI56263300012039086",
            "SK3112000000198742637541",
            "SM86U0322509800000000270100",
            "SO211000001001000100141",
            "ST23000100010051845310146",
            "SV62CENR00000000000000700025",
            "TL380080012345678910157",
            "TN5910006035183598478831",
            "TR330006100519786457841326",
            "UA213223130000026007233566001",
            "VA59001123000012345678",
            "VG96VPVG0000012345678901",
            "XK051212012345678906",
            "YE15CBYE0001018861234567891234",
        ];
        assert_eq!(examples.len(), FORMATS.len());
        for example in examples {
            assert!(valid(example), "{example}");
            assert!(!valid(&example[..example.len() - 1]));
            assert!(!valid(&format!("{example}0")));
        }
    }
    #[test]
    fn malformed_inputs_never_panic() {
        for s in [
            "",
            "IT",
            "IT00",
            "😀😀😀😀",
            "IT60X05428é11010000000123456",
            "IT60X05428\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        ] {
            assert!(!valid(s));
        }
    }
}
