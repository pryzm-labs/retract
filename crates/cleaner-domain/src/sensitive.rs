use std::collections::BTreeSet;

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::{ContentKind, SensitiveDataKind};

pub fn detect_sensitive_data(text: &str, content_kind: ContentKind) -> Vec<SensitiveDataKind> {
    let mut findings = BTreeSet::new();

    if content_kind == ContentKind::Location
        || has_coordinates(text)
        || LOCATION_LINK.is_match(text)
    {
        findings.insert(SensitiveDataKind::PreciseLocation);
    }
    if content_kind == ContentKind::Contact {
        findings.insert(SensitiveDataKind::ContactCard);
    }
    if EMAIL.is_match(text) {
        findings.insert(SensitiveDataKind::EmailAddress);
    }
    if has_phone_number(text) {
        findings.insert(SensitiveDataKind::PhoneNumber);
    }
    if POSTAL_ADDRESS.is_match(text) {
        findings.insert(SensitiveDataKind::PostalAddress);
    }
    if PERSONAL_IDENTIFIER.is_match(text) {
        findings.insert(SensitiveDataKind::PersonalIdentifier);
    }
    if IDENTITY.is_match(text) || SSN.is_match(text) {
        findings.insert(SensitiveDataKind::IdentityDocument);
    }
    if has_payment_card(text) || has_iban(text) {
        findings.insert(SensitiveDataKind::FinancialAccount);
    }
    if has_crypto_wallet(text) {
        findings.insert(SensitiveDataKind::CryptoWallet);
    }
    if SECRET.is_match(text) {
        findings.insert(SensitiveDataKind::CredentialOrSecret);
    }
    if has_ipv4(text) || IPV6.is_match(text) {
        findings.insert(SensitiveDataKind::NetworkAddress);
    }

    findings.into_iter().collect()
}

macro_rules! regex {
    ($name:ident, $pattern:literal) => {
        static $name: std::sync::LazyLock<Regex> =
            std::sync::LazyLock::new(|| Regex::new($pattern).expect("valid sensitive-data regex"));
    };
}

regex!(EMAIL, r"(?i)\b[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,63}\b");
regex!(PHONE, r"(?:\+?\d[\d\s().\-]{6,}\d)");
regex!(
    DATE_LIKE,
    r"^\s*(?:\d{4}[-/.]\d{1,2}[-/.]\d{1,2}|\d{1,2}[-/.]\d{1,2}[-/.]\d{4})\s*$"
);
regex!(
    POSTAL_ADDRESS,
    r"(?i)\b\d{1,6}\s+(?:[A-Z0-9.'\-]+\s+){0,5}(?:street|st|road|rd|avenue|ave|lane|ln|drive|dr|boulevard|blvd|way|court|ct|place|pl|terrace|trail|parkway|highway)\b\.?"
);
regex!(
    IDENTITY,
    r"(?i)\b(?:passport|national\s+id|identity\s+card|driver'?s?\s+licen[cs]e|tax\s+id|social\s+security|ssn)\b"
);
regex!(
    PERSONAL_IDENTIFIER,
    r"(?i)\b(?:date\s+of\s+birth|birth\s*date|dob|mother'?s\s+maiden\s+name|medical\s+record\s+(?:number|id)|patient\s+id|employee\s+id|student\s+id)\b"
);
regex!(SSN, r"\b\d{3}-\d{2}-\d{4}\b");
regex!(PAYMENT_CARD, r"\b(?:\d[ -]?){13,19}\b");
regex!(IBAN, r"(?i)\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]){11,30}\b");
regex!(ETHEREUM_ADDRESS, r"(?i)\b0x[0-9a-f]{40}\b");
regex!(
    BITCOIN_LEGACY_ADDRESS,
    r"\b[123mn][1-9A-HJ-NP-Za-km-z]{25,34}\b"
);
regex!(
    BITCOIN_SEGWIT_ADDRESS,
    r"(?i)\b(?:bc1|tb1)[ac-hj-np-z02-9]{6,87}\b"
);
regex!(SOLANA_ADDRESS, r"\b[1-9A-HJ-NP-Za-km-z]{32,44}\b");
regex!(
    SOLANA_CONTEXT,
    r"(?i)\b(?:solana|sol\s+(?:wallet|address)|wallet(?:\s+address)?)\b"
);
regex!(
    OTHER_CRYPTO_WALLET,
    r"(?i)\b(?:(?:ltc1|cosmos1)[ac-hj-np-z02-9]{11,71}|[LM][a-km-zA-HJ-NP-Z1-9]{25,34}|[DT][1-9A-HJ-NP-Za-km-z]{33}|[48][0-9AB][1-9A-HJ-NP-Za-km-z]{93}|r[1-9A-HJ-NP-Za-km-z]{24,34}|addr1[a-z0-9]{20,100}|G[A-Z2-7]{55}|(?:EQ|UQ)[A-Za-z0-9_-]{46})\b"
);
regex!(
    SECRET,
    r"(?i)\b(?:seed\s+phrase|recovery\s+phrase|private\s+key|api[_ -]?key|access[_ -]?token|auth(?:entication)?\s+token|password|passcode|one[- ]time\s+(?:code|password)|2fa\s+code)\b"
);
regex!(
    LOCATION_LINK,
    r"(?i)(?:geo:|maps\.google\.|goo\.gl/maps|maps\.apple\.|openstreetmap\.org)"
);
regex!(
    COORDINATES,
    r"(?x)(?P<lat>[+-]?(?:\d{1,2}(?:\.\d+)?|90(?:\.0+)?))\s*[,;]\s*(?P<lon>[+-]?(?:\d{1,3}(?:\.\d+)?|180(?:\.0+)?))"
);
regex!(IPV4, r"\b(?:\d{1,3}\.){3}\d{1,3}\b");
regex!(IPV6, r"(?i)\b(?:[0-9a-f]{1,4}:){2,7}[0-9a-f]{0,4}\b");

fn has_phone_number(text: &str) -> bool {
    PHONE.find_iter(text).any(|candidate| {
        let bounded = text[..candidate.start()]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_ascii_alphanumeric())
            && text[candidate.end()..]
                .chars()
                .next()
                .is_none_or(|character| !character.is_ascii_alphanumeric());
        let digits: Vec<_> = candidate
            .as_str()
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        bounded
            && !DATE_LIKE.is_match(candidate.as_str())
            && (8..=15).contains(&digits.len())
            && !digits.iter().all(|digit| *digit == digits[0])
    })
}

fn has_payment_card(text: &str) -> bool {
    PAYMENT_CARD.find_iter(text).any(|candidate| {
        let digits: Vec<u32> = candidate
            .as_str()
            .chars()
            .filter_map(|character| character.to_digit(10))
            .collect();
        (13..=19).contains(&digits.len()) && luhn_valid(&digits)
    })
}

fn luhn_valid(digits: &[u32]) -> bool {
    digits
        .iter()
        .rev()
        .enumerate()
        .map(|(index, digit)| {
            if index % 2 == 1 {
                let doubled = digit * 2;
                if doubled > 9 { doubled - 9 } else { doubled }
            } else {
                *digit
            }
        })
        .sum::<u32>()
        .is_multiple_of(10)
}

fn has_iban(text: &str) -> bool {
    IBAN.find_iter(text).any(|candidate| {
        let compact = candidate
            .as_str()
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        let rearranged = format!("{}{}", &compact[4..], &compact[..4]);
        let mut remainder = 0_u32;
        for character in rearranged.chars() {
            if let Some(digit) = character.to_digit(10) {
                remainder = (remainder * 10 + digit) % 97;
            } else if character.is_ascii_uppercase() {
                let value = character as u32 - 'A' as u32 + 10;
                remainder = (remainder * 100 + value) % 97;
            } else {
                return false;
            }
        }
        remainder == 1
    })
}

fn has_crypto_wallet(text: &str) -> bool {
    ETHEREUM_ADDRESS.is_match(text)
        || BITCOIN_LEGACY_ADDRESS
            .find_iter(text)
            .any(|candidate| is_valid_bitcoin_legacy_address(candidate.as_str()))
        || BITCOIN_SEGWIT_ADDRESS
            .find_iter(text)
            .any(|candidate| is_valid_bitcoin_segwit_address(candidate.as_str()))
        || SOLANA_CONTEXT.is_match(text)
            && SOLANA_ADDRESS
                .find_iter(text)
                .any(|candidate| is_valid_solana_address(candidate.as_str()))
        || OTHER_CRYPTO_WALLET.is_match(text)
}

fn is_valid_bitcoin_legacy_address(candidate: &str) -> bool {
    let Some(decoded) = decode_base58(candidate) else {
        return false;
    };
    if decoded.len() != 25 || !matches!(decoded[0], 0x00 | 0x05 | 0x6f | 0xc4) {
        return false;
    }

    let first_hash = Sha256::digest(&decoded[..21]);
    let second_hash = Sha256::digest(first_hash);
    decoded[21..] == second_hash[..4]
}

fn is_valid_bitcoin_segwit_address(candidate: &str) -> bool {
    if !(8..=90).contains(&candidate.len())
        || candidate
            .chars()
            .any(|character| character.is_ascii_lowercase())
            && candidate
                .chars()
                .any(|character| character.is_ascii_uppercase())
    {
        return false;
    }

    let normalized = candidate.to_ascii_lowercase();
    let Some(separator) = normalized.rfind('1') else {
        return false;
    };
    let (hrp, encoded) = normalized.split_at(separator);
    if !matches!(hrp, "bc" | "tb") {
        return false;
    }

    let data = encoded[1..]
        .bytes()
        .map(|character| {
            BECH32_CHARSET
                .iter()
                .position(|candidate| *candidate == character)
                .map(|position| position as u8)
        })
        .collect::<Option<Vec<_>>>();
    let Some(data) = data else {
        return false;
    };
    if data.len() < 7 {
        return false;
    }

    let checksum = bech32_polymod(
        hrp.bytes()
            .map(|character| character >> 5)
            .chain(std::iter::once(0))
            .chain(hrp.bytes().map(|character| character & 31))
            .chain(data.iter().copied()),
    );
    let witness_version = data[0];
    let checksum_matches_version = if witness_version == 0 {
        checksum == 1
    } else {
        checksum == 0x2bc8_30a3
    };
    if witness_version > 16 || !checksum_matches_version {
        return false;
    }

    let Some(program) = convert_5_bit_groups_to_bytes(&data[1..data.len() - 6]) else {
        return false;
    };
    (2..=40).contains(&program.len()) && (witness_version != 0 || matches!(program.len(), 20 | 32))
}

fn bech32_polymod(values: impl IntoIterator<Item = u8>) -> u32 {
    const GENERATORS: [u32; 5] = [
        0x3b6a_57b2,
        0x2650_8e6d,
        0x1ea1_19fa,
        0x3d42_33dd,
        0x2a14_62b3,
    ];
    values.into_iter().fold(1_u32, |checksum, value| {
        let high_bits = checksum >> 25;
        let mut next = ((checksum & 0x01ff_ffff) << 5) ^ u32::from(value);
        for (index, generator) in GENERATORS.iter().enumerate() {
            if (high_bits >> index) & 1 == 1 {
                next ^= generator;
            }
        }
        next
    })
}

fn convert_5_bit_groups_to_bytes(values: &[u8]) -> Option<Vec<u8>> {
    let mut accumulator = 0_u32;
    let mut bit_count = 0_u32;
    let mut decoded = Vec::new();
    for value in values {
        if *value > 31 {
            return None;
        }
        accumulator = (accumulator << 5) | u32::from(*value);
        bit_count += 5;
        while bit_count >= 8 {
            bit_count -= 8;
            decoded.push(((accumulator >> bit_count) & 0xff) as u8);
        }
    }
    if bit_count >= 5 || ((accumulator << (8 - bit_count)) & 0xff) != 0 {
        return None;
    }
    Some(decoded)
}

fn is_valid_solana_address(candidate: &str) -> bool {
    decode_base58(candidate).is_some_and(|decoded| decoded.len() == 32)
}

fn decode_base58(value: &str) -> Option<Vec<u8>> {
    let mut decoded_little_endian = Vec::<u8>::new();
    for character in value.bytes() {
        let digit = BASE58_ALPHABET
            .iter()
            .position(|candidate| *candidate == character)? as u32;
        let mut carry = digit;
        for byte in &mut decoded_little_endian {
            let expanded = u32::from(*byte) * 58 + carry;
            *byte = (expanded & 0xff) as u8;
            carry = expanded >> 8;
        }
        while carry > 0 {
            decoded_little_endian.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }

    decoded_little_endian.extend(std::iter::repeat_n(
        0,
        value
            .bytes()
            .take_while(|character| *character == b'1')
            .count(),
    ));
    decoded_little_endian.reverse();
    Some(decoded_little_endian)
}

const BASE58_ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const BECH32_CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

fn has_coordinates(text: &str) -> bool {
    COORDINATES.captures_iter(text).any(|captures| {
        let latitude = captures
            .name("lat")
            .and_then(|value| value.as_str().parse::<f64>().ok());
        let longitude = captures
            .name("lon")
            .and_then(|value| value.as_str().parse::<f64>().ok());
        matches!((latitude, longitude), (Some(lat), Some(lon)) if lat.abs() <= 90.0 && lon.abs() <= 180.0)
    })
}

fn has_ipv4(text: &str) -> bool {
    IPV4.find_iter(text).any(|candidate| {
        candidate
            .as_str()
            .split('.')
            .all(|octet| octet.parse::<u8>().is_ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_high_value_privacy_categories() {
        let text = "Email me at person@example.com from 17 Juniper Lane. Date of birth: 1990-01-02. Wallet: 0x52908400098527886E0F7030069857D2E4169EE7. My recovery phrase is offline.";
        let findings = detect_sensitive_data(text, ContentKind::Text);
        assert!(findings.contains(&SensitiveDataKind::EmailAddress));
        assert!(findings.contains(&SensitiveDataKind::PostalAddress));
        assert!(findings.contains(&SensitiveDataKind::PersonalIdentifier));
        assert!(findings.contains(&SensitiveDataKind::CryptoWallet));
        assert!(findings.contains(&SensitiveDataKind::CredentialOrSecret));
        assert!(!findings.contains(&SensitiveDataKind::PhoneNumber));
    }

    #[test]
    fn detects_ethereum_bitcoin_and_solana_wallet_formats() {
        let samples = [
            (
                "Ethereum",
                "Send ETH to 0xde709f2102306220921060314715629080e2fb77",
            ),
            (
                "Bitcoin P2PKH",
                "Bitcoin address: 1BoatSLRHtKNngkdXEeobR76b53LETtpyT",
            ),
            (
                "Bitcoin P2SH",
                "Bitcoin address: 3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy",
            ),
            (
                "Bitcoin SegWit",
                "Bitcoin address: BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4",
            ),
            (
                "Solana",
                "Solana wallet: So11111111111111111111111111111111111111112",
            ),
        ];

        for (format, sample) in samples {
            assert!(
                detect_sensitive_data(sample, ContentKind::Text)
                    .contains(&SensitiveDataKind::CryptoWallet),
                "failed to identify {format} address"
            );
        }
    }

    #[test]
    fn rejects_malformed_wallet_candidates_and_ambiguous_base58_ids() {
        let samples = [
            "Bitcoin address: 1BoatSLRHtKNngkdXEeobR76b53LETtpyQ",
            "Bitcoin address: bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t5",
            "Solana wallet: 1111111111111111111111111111111",
            "Opaque identifier 11111111111111111111111111111111",
            "Ethereum candidate 0xde709f2102306220921060314715629080e2fb7",
        ];

        for sample in samples {
            assert!(
                !detect_sensitive_data(sample, ContentKind::Text)
                    .contains(&SensitiveDataKind::CryptoWallet),
                "accepted malformed or ambiguous candidate: {sample}"
            );
        }
    }

    #[test]
    fn validates_structured_numbers_instead_of_matching_any_digits() {
        let findings = detect_sensitive_data(
            "Card 4242 4242 4242 4242, IP 192.168.1.10, coordinates 19.4326, -99.1332",
            ContentKind::Text,
        );
        assert!(findings.contains(&SensitiveDataKind::FinancialAccount));
        assert!(findings.contains(&SensitiveDataKind::NetworkAddress));
        assert!(findings.contains(&SensitiveDataKind::PreciseLocation));
        assert!(
            !detect_sensitive_data("build 1234567", ContentKind::Text)
                .contains(&SensitiveDataKind::PhoneNumber)
        );
    }

    #[test]
    fn identifies_native_contact_and_location_messages() {
        assert_eq!(
            detect_sensitive_data("Location", ContentKind::Location),
            vec![SensitiveDataKind::PreciseLocation]
        );
        assert_eq!(
            detect_sensitive_data("Contact · Alex", ContentKind::Contact),
            vec![SensitiveDataKind::ContactCard]
        );
    }
}
