//! Built-in Sparkasse CAMT V8 import schema template.
//!
//! Seeded into the user's `finance_import_schemas` collection on first
//! access (see [`crate::routes::finance::ensure_builtin_schemas`]). The
//! user can clone it to get an editable copy; the original stays
//! immutable so re-running an old `ImportRun` always finds the same
//! schema.
//!
//! CSV layout (17 columns):
//! `Auftragskonto;Buchungstag;Valutadatum;Buchungstext;Verwendungszweck;
//! Glaeubiger ID;Mandatsreferenz;Kundenreferenz (End-to-End);
//! Sammlerreferenz;Lastschrift Ursprungsbetrag;
//! Auslagenersatz Ruecklastschrift;Beguenstigter/Zahlungspflichtiger;
//! Kontonummer/IBAN;BIC;Betrag;Waehrung;Info`

use chrono::Utc;
use mongodb::bson::oid::ObjectId;

use crate::models::{
    AmountSignConvention, CurrencySource, DecimalSeparator, ImportSchema,
};

pub const BUILTIN_ID: &str = "sparkasse_camt_v8";
pub const SCHEMA_NAME: &str = "Sparkasse CAMT V8";

/// Returns a fresh `ImportSchema` for the Sparkasse CAMT V8 layout,
/// owned by the given user.
pub fn seed_for(owner_id: ObjectId) -> ImportSchema {
    let now = Utc::now();
    ImportSchema {
        id: ObjectId::new(),
        owner_id,
        name: SCHEMA_NAME.to_string(),
        delimiter: ";".to_string(),
        encoding: "windows-1252".to_string(),
        decimal_separator: DecimalSeparator::Comma,
        skip_header_rows: 0,
        has_headers: true,
        date_column: 1,
        date_format: "DD.MM.YY".to_string(),
        amount_column: 14,
        amount_sign_convention: AmountSignConvention::PositiveCredit,
        // Beguenstigter + Verwendungszweck.
        description_columns: vec![11, 4],
        currency_source: CurrencySource::Column,
        currency_column: Some(15),
        fixed_currency: None,
        bank_ref_column: None,
        iban_column: Some(12),
        raw_category_column: Some(3),
        is_builtin: true,
        builtin_id: Some(BUILTIN_ID.to_string()),
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finance_import::{parse_csv, parse_amount_minor};
    use crate::models::DecimalSeparator;

    fn fixture_csv() -> &'static [u8] {
        b"Auftragskonto;Buchungstag;Valutadatum;Buchungstext;Verwendungszweck;Glaeubiger ID;Mandatsreferenz;Kundenreferenz (End-to-End);Sammlerreferenz;Lastschrift Ursprungsbetrag;Auslagenersatz Ruecklastschrift;Beguenstigter/Zahlungspflichtiger;Kontonummer/IBAN;BIC;Betrag;Waehrung;Info\n\
DE12345;05.03.26;05.03.26;LASTSCHRIFT;Spotify monthly;;;;;;;Spotify AB;SE12345;SPKSDEFF;-9,99;EUR;Umsatz gebucht\n\
DE12345;06.03.26;06.03.26;GUTSCHRIFT;Salary March;;;;;;;Acme GmbH;DE98765;ACMEDEFF;3.500,00;EUR;Umsatz gebucht\n"
    }

    #[test]
    fn parses_fixture_with_seeded_schema() {
        let schema = seed_for(ObjectId::new());
        let rows = parse_csv(fixture_csv(), &schema).unwrap();
        assert_eq!(rows.len(), 2);

        let first = rows[0].as_ref().expect("row 1 ok");
        assert_eq!(first.amount_minor, -999);
        assert_eq!(first.currency, "EUR");
        assert_eq!(first.description, "Spotify AB / Spotify monthly");
        assert_eq!(first.raw_bank_category.as_deref(), Some("LASTSCHRIFT"));
        assert_eq!(first.date.format("%Y-%m-%d").to_string(), "2026-03-05");
        assert_eq!(first.iban.as_deref(), Some("SE12345"));
        assert_eq!(first.source_ref.len(), 16);

        let second = rows[1].as_ref().expect("row 2 ok");
        assert_eq!(second.amount_minor, 350000);
        assert_eq!(second.currency, "EUR");
        assert_ne!(first.source_ref, second.source_ref);
    }

    #[test]
    fn german_amount_positive() {
        assert_eq!(parse_amount_minor("1.234,56", DecimalSeparator::Comma), Some(123456));
        assert_eq!(parse_amount_minor("0,00", DecimalSeparator::Comma), Some(0));
    }

    #[test]
    fn german_amount_negative() {
        assert_eq!(parse_amount_minor("-1.234,56", DecimalSeparator::Comma), Some(-123456));
    }

    #[test]
    fn us_amount_positive() {
        assert_eq!(parse_amount_minor("1,234.56", DecimalSeparator::Dot), Some(123456));
        assert_eq!(parse_amount_minor("0.00", DecimalSeparator::Dot), Some(0));
    }
}
