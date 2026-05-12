//! Sparkasse CAMT V8 CSV format.
//!
//! Encoding: CP1252 (Windows-1252).
//! Delimiter: `;`, quoted with `"`.
//! Date format: `DD.MM.YY` (booking + value date).
//! Amount format: German decimal — thousands `.`, decimal `,`, e.g.
//! `-1.234,56`. Sign on the amount itself; no separate Soll/Haben column
//! in the V8 layout.
//!
//! `source_ref` strategy: SHA-256 (first 16 hex chars) of the normalised
//! raw CSV row, scoped per-account via the partial unique index in
//! `db.rs`. The wide hash basis means columns we don't extract today
//! (Glaeubiger-ID, Mandatsreferenz, BIC, etc.) still contribute to dedup,
//! and the source_ref stays stable across parser changes.

use super::{ImportProfile, ParsedRow, ParseError, ProfileInfo};
use chrono::{NaiveDate, TimeZone, Utc};
use encoding_rs::WINDOWS_1252;
use sha2::{Digest, Sha256};

pub struct SparkasseCamtV8;

const PROFILE_ID: &str = "sparkasse_camt_v8";
const PROFILE_NAME: &str = "Sparkasse CAMT V8";

// Column indices in the Sparkasse CAMT V8 export. Order is stable enough
// to hard-code; we still verify the header on parse to fail fast if the
// user uploads a different format by mistake.
const COL_BUCHUNGSTAG: usize = 1;
const COL_BUCHUNGSTEXT: usize = 3;
const COL_VERWENDUNGSZWECK: usize = 4;
const COL_BEGUENSTIGTER: usize = 11;
const COL_BETRAG: usize = 14;
const COL_WAEHRUNG: usize = 15;
const EXPECTED_COL_COUNT: usize = 17;

impl ImportProfile for SparkasseCamtV8 {
    fn info(&self) -> ProfileInfo {
        ProfileInfo {
            id: PROFILE_ID,
            name: PROFILE_NAME,
        }
    }

    fn parse(&self, bytes: &[u8]) -> Result<Vec<Result<ParsedRow, ParseError>>, ParseError> {
        let (decoded, _enc, had_errors) = WINDOWS_1252.decode(bytes);
        if had_errors {
            // CP1252 is a single-byte encoding with no invalid sequences,
            // so this should never fire — but guard anyway.
            return Err(ParseError::Fatal(
                "Failed to decode CSV as Windows-1252".into(),
            ));
        }
        let text = decoded.into_owned();

        let mut reader = csv::ReaderBuilder::new()
            .delimiter(b';')
            .has_headers(true)
            .flexible(true)
            .from_reader(text.as_bytes());

        let headers = reader
            .headers()
            .map_err(|e| ParseError::Fatal(format!("Failed to read header row: {e}")))?
            .clone();
        if headers.len() < EXPECTED_COL_COUNT {
            return Err(ParseError::Fatal(format!(
                "Expected at least {EXPECTED_COL_COUNT} columns, got {} \
                 — this does not look like a Sparkasse CAMT V8 export",
                headers.len()
            )));
        }

        let mut out = Vec::new();
        for (idx, rec) in reader.records().enumerate() {
            // CSV line number = idx + 2 (header is line 1, records start at 2).
            let line = (idx as u32) + 2;
            match rec {
                Ok(rec) => match parse_record(&rec, line) {
                    Ok(row) => out.push(Ok(row)),
                    Err(e) => out.push(Err(e)),
                },
                Err(e) => out.push(Err(ParseError::Row {
                    line,
                    message: format!("CSV parse error: {e}"),
                })),
            }
        }
        Ok(out)
    }
}

fn parse_record(rec: &csv::StringRecord, line: u32) -> Result<ParsedRow, ParseError> {
    let date_str = field(rec, COL_BUCHUNGSTAG, line, "Buchungstag")?;
    let date = parse_german_date(date_str).ok_or_else(|| ParseError::Row {
        line,
        message: format!("Invalid Buchungstag `{date_str}`, expected DD.MM.YY"),
    })?;

    let amount_str = field(rec, COL_BETRAG, line, "Betrag")?;
    let amount_minor = parse_german_amount_minor(amount_str).ok_or_else(|| ParseError::Row {
        line,
        message: format!("Invalid Betrag `{amount_str}`, expected German decimal e.g. -1.234,56"),
    })?;

    let currency_raw = field(rec, COL_WAEHRUNG, line, "Waehrung")?.trim();
    if currency_raw.len() != 3 || !currency_raw.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(ParseError::Row {
            line,
            message: format!("Invalid Waehrung `{currency_raw}`, expected 3-letter ISO 4217 code"),
        });
    }
    let currency = currency_raw.to_ascii_uppercase();

    let verwendungszweck = rec.get(COL_VERWENDUNGSZWECK).unwrap_or("").trim();
    let beguenstigter = rec.get(COL_BEGUENSTIGTER).unwrap_or("").trim();
    let description = match (beguenstigter.is_empty(), verwendungszweck.is_empty()) {
        (true, true) => "(no description)".to_string(),
        (true, false) => verwendungszweck.to_string(),
        (false, true) => beguenstigter.to_string(),
        (false, false) => format!("{beguenstigter} — {verwendungszweck}"),
    };

    let raw_bank_category = rec
        .get(COL_BUCHUNGSTEXT)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let source_ref = row_hash(rec);

    Ok(ParsedRow {
        date,
        amount_minor,
        currency,
        description,
        raw_bank_category,
        source_ref,
    })
}

fn field<'a>(
    rec: &'a csv::StringRecord,
    idx: usize,
    line: u32,
    name: &str,
) -> Result<&'a str, ParseError> {
    rec.get(idx).ok_or_else(|| ParseError::Row {
        line,
        message: format!("Missing column {name} (index {idx})"),
    })
}

/// Parse `DD.MM.YY` or `DD.MM.YYYY` into a UTC midnight timestamp.
/// 2-digit years pivot at 70: 00–69 → 2000–2069, 70–99 → 1970–1999.
/// chrono's `%y` parses 2-digit years literally (00 → year 0), so we
/// pre-expand the year ourselves rather than relying on the directive.
pub(crate) fn parse_german_date(s: &str) -> Option<chrono::DateTime<Utc>> {
    let s = s.trim();
    let mut parts = s.split('.');
    let day = parts.next()?;
    let month = parts.next()?;
    let year_raw = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let year: i32 = year_raw.parse().ok()?;
    let year = match year_raw.len() {
        2 => if year < 70 { 2000 + year } else { 1900 + year },
        4 => year,
        _ => return None,
    };
    let day: u32 = day.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    let nd = NaiveDate::from_ymd_opt(year, month, day)?;
    let dt = nd.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&dt))
}

/// Parse a German-formatted amount like `-1.234,56` or `0,00` into i64
/// minor units (cents). Assumes exactly two decimal places; rejects
/// inputs with more or fewer.
pub(crate) fn parse_german_amount_minor(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (sign, rest) = match s.as_bytes()[0] {
        b'-' => (-1i64, &s[1..]),
        b'+' => (1, &s[1..]),
        _ => (1, s),
    };
    let mut parts = rest.splitn(2, ',');
    let int_part = parts.next()?.replace('.', "");
    // Sparkasse always writes the comma + two decimals; reject inputs
    // missing either, so we don't silently treat "1" as €1.00.
    let frac_part = parts.next()?;
    if frac_part.len() != 2 {
        return None;
    }
    if !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let int: i64 = int_part.parse().ok()?;
    let frac: i64 = frac_part.parse().ok()?;
    Some(sign * (int * 100 + frac))
}

/// SHA-256 (first 16 hex chars) of the row's fields joined with `\u{1f}`
/// after trimming each. Joining via a control character that can't appear
/// in CSV fields means the hash is stable regardless of original quoting
/// and column widths, while still differing if any field changes.
fn row_hash(rec: &csv::StringRecord) -> String {
    let mut hasher = Sha256::new();
    let mut first = true;
    for field in rec.iter() {
        if !first {
            hasher.update([0x1f]);
        }
        hasher.update(field.trim().as_bytes());
        first = false;
    }
    let digest = hasher.finalize();
    hex::encode(&digest[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn german_amount_positive() {
        assert_eq!(parse_german_amount_minor("1.234,56"), Some(123456));
        assert_eq!(parse_german_amount_minor("0,00"), Some(0));
        assert_eq!(parse_german_amount_minor("10,00"), Some(1000));
    }

    #[test]
    fn german_amount_negative() {
        assert_eq!(parse_german_amount_minor("-1.234,56"), Some(-123456));
        assert_eq!(parse_german_amount_minor("-0,01"), Some(-1));
    }

    #[test]
    fn german_amount_rejects_wrong_decimal_count() {
        assert_eq!(parse_german_amount_minor("1,5"), None);
        assert_eq!(parse_german_amount_minor("1,500"), None);
        assert_eq!(parse_german_amount_minor("1"), None);
    }

    #[test]
    fn german_amount_rejects_garbage() {
        assert_eq!(parse_german_amount_minor(""), None);
        assert_eq!(parse_german_amount_minor("abc"), None);
        assert_eq!(parse_german_amount_minor("1.2.3,45"), Some(12345));
    }

    #[test]
    fn german_date_2digit() {
        let d = parse_german_date("05.03.26").unwrap();
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2026-03-05");
    }

    #[test]
    fn german_date_4digit() {
        let d = parse_german_date("05.03.2026").unwrap();
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2026-03-05");
    }

    #[test]
    fn german_date_rejects_iso() {
        assert!(parse_german_date("2026-03-05").is_none());
    }

    #[test]
    fn row_hash_stable_across_calls() {
        let mut rec = csv::StringRecord::new();
        rec.push_field("DE12345");
        rec.push_field("05.03.26");
        rec.push_field("-1.234,56");
        let a = row_hash(&rec);
        let b = row_hash(&rec);
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn row_hash_differs_on_any_field_change() {
        let mut a = csv::StringRecord::new();
        a.push_field("DE12345");
        a.push_field("05.03.26");
        let mut b = csv::StringRecord::new();
        b.push_field("DE12345");
        b.push_field("06.03.26");
        assert_ne!(row_hash(&a), row_hash(&b));
    }

    #[test]
    fn row_hash_ignores_trailing_whitespace() {
        let mut a = csv::StringRecord::new();
        a.push_field("DE12345  ");
        a.push_field(" 05.03.26 ");
        let mut b = csv::StringRecord::new();
        b.push_field("DE12345");
        b.push_field("05.03.26");
        assert_eq!(row_hash(&a), row_hash(&b));
    }

    fn fixture_csv() -> &'static [u8] {
        // Minimal Sparkasse CAMT V8 fixture: header + 2 records.
        // The 17 columns in order:
        // Auftragskonto;Buchungstag;Valutadatum;Buchungstext;Verwendungszweck;
        // Glaeubiger ID;Mandatsreferenz;Kundenreferenz (End-to-End);Sammlerreferenz;
        // Lastschrift Ursprungsbetrag;Auslagenersatz Ruecklastschrift;
        // Beguenstigter/Zahlungspflichtiger;Kontonummer/IBAN;BIC;Betrag;Waehrung;Info
        b"Auftragskonto;Buchungstag;Valutadatum;Buchungstext;Verwendungszweck;Glaeubiger ID;Mandatsreferenz;Kundenreferenz (End-to-End);Sammlerreferenz;Lastschrift Ursprungsbetrag;Auslagenersatz Ruecklastschrift;Beguenstigter/Zahlungspflichtiger;Kontonummer/IBAN;BIC;Betrag;Waehrung;Info\n\
DE12345;05.03.26;05.03.26;LASTSCHRIFT;Spotify monthly;;;;;;;Spotify AB;SE12345;SPKSDEFF;-9,99;EUR;Umsatz gebucht\n\
DE12345;06.03.26;06.03.26;GUTSCHRIFT;Salary March;;;;;;;Acme GmbH;DE98765;ACMEDEFF;3.500,00;EUR;Umsatz gebucht\n"
    }

    #[test]
    fn parses_fixture_csv() {
        let profile = SparkasseCamtV8;
        let rows = profile.parse(fixture_csv()).unwrap();
        assert_eq!(rows.len(), 2);

        let first = rows[0].as_ref().expect("row 1 ok");
        assert_eq!(first.amount_minor, -999);
        assert_eq!(first.currency, "EUR");
        assert_eq!(first.description, "Spotify AB — Spotify monthly");
        assert_eq!(first.raw_bank_category.as_deref(), Some("LASTSCHRIFT"));
        assert_eq!(first.date.format("%Y-%m-%d").to_string(), "2026-03-05");
        assert_eq!(first.source_ref.len(), 16);

        let second = rows[1].as_ref().expect("row 2 ok");
        assert_eq!(second.amount_minor, 350000);
        assert_eq!(second.currency, "EUR");
        assert_ne!(first.source_ref, second.source_ref);
    }

    #[test]
    fn rejects_non_sparkasse_header() {
        let profile = SparkasseCamtV8;
        let result = profile.parse(b"a;b;c\n1;2;3\n");
        match result {
            Err(ParseError::Fatal(msg)) => assert!(msg.contains("does not look like")),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }
}
