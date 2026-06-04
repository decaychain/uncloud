//! CSV-import pipeline for the finance tracker.
//!
//! Each user has a set of `ImportSchema` rows (seeded with builtin
//! templates on first use). `parse_csv` takes the raw bytes plus a schema
//! and emits `ParsedRow`s. The route handler then UPSERTs those into
//! `finance_transactions` keyed by `(account_id, source_ref)`.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use encoding_rs::{Encoding, UTF_8};
use sha2::{Digest, Sha256};

use crate::models::{AmountSignConvention, CurrencySource, DecimalSeparator, ImportSchema};

pub mod sparkasse_camt_v8;

#[derive(Debug, Clone)]
pub struct ParsedRow {
    pub date: DateTime<Utc>,
    pub amount_minor: i64,
    pub currency: String,
    pub description: String,
    pub raw_bank_category: Option<String>,
    /// Unique-per-account dedup key.
    pub source_ref: String,
    /// IBAN/account-number from the row (only populated when the schema
    /// sets `iban_column`). Used by the auto-account-create flow.
    pub iban: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ParseError {
    /// The whole input is malformed (wrong delimiter, can't decode, etc.)
    /// — abort the import.
    Fatal(String),
    /// One row was bad; skip it and keep going.
    Row { line: u32, message: String },
}

/// Parse raw CSV bytes per the provided schema. Returns one entry per
/// data row (skipping the header row(s)). Each entry is `Ok(ParsedRow)`
/// or `Err(ParseError::Row {...})` for per-row failures.
pub fn parse_csv(
    bytes: &[u8],
    schema: &ImportSchema,
) -> Result<Vec<Result<ParsedRow, ParseError>>, ParseError> {
    let encoding = resolve_encoding(&schema.encoding)?;
    let (decoded, _enc, had_errors) = encoding.decode(bytes);
    if had_errors {
        return Err(ParseError::Fatal(format!(
            "Failed to decode CSV as {}",
            schema.encoding
        )));
    }
    let text = decoded.into_owned();

    let delimiter_byte = resolve_delimiter(&schema.delimiter)?;

    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter_byte)
        // We handle skip_header_rows manually below so the schema is
        // explicit about how many lines to skip rather than relying on
        // the csv crate's `has_headers` toggle alone.
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());

    let skip_rows = schema.skip_header_rows as usize + if schema.has_headers { 1 } else { 0 };

    let mut out = Vec::new();
    for (idx, rec) in reader.records().enumerate() {
        if idx < skip_rows {
            continue;
        }
        // CSV line numbers are 1-based (header counts as line 1).
        let line = (idx as u32) + 1;
        match rec {
            Ok(rec) => match parse_record(&rec, line, schema) {
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

fn resolve_encoding(name: &str) -> Result<&'static Encoding, ParseError> {
    if name.eq_ignore_ascii_case("utf-8") || name.eq_ignore_ascii_case("utf8") {
        return Ok(UTF_8);
    }
    Encoding::for_label(name.as_bytes())
        .ok_or_else(|| ParseError::Fatal(format!("Unknown encoding `{name}` in schema")))
}

fn resolve_delimiter(s: &str) -> Result<u8, ParseError> {
    let bytes = s.as_bytes();
    match bytes.len() {
        1 => Ok(bytes[0]),
        _ => Err(ParseError::Fatal(format!(
            "Delimiter must be a single byte, got `{s}`"
        ))),
    }
}

fn parse_record(
    rec: &csv::StringRecord,
    line: u32,
    schema: &ImportSchema,
) -> Result<ParsedRow, ParseError> {
    let date_str = field(rec, schema.date_column as usize, line, "date")?;
    let date = parse_date(date_str.trim(), &schema.date_format).ok_or_else(|| ParseError::Row {
        line,
        message: format!(
            "Invalid date `{date_str}`, expected format `{}`",
            schema.date_format
        ),
    })?;

    let amount_str = field(rec, schema.amount_column as usize, line, "amount")?;
    let amount_minor =
        parse_amount_minor(amount_str.trim(), schema.decimal_separator).ok_or_else(|| {
            ParseError::Row {
                line,
                message: format!("Invalid amount `{amount_str}`"),
            }
        })?;
    let amount_minor = match schema.amount_sign_convention {
        AmountSignConvention::PositiveCredit => amount_minor,
        AmountSignConvention::PositiveDebit => -amount_minor,
    };

    let currency = match schema.currency_source {
        CurrencySource::Column => {
            let col = schema.currency_column.ok_or_else(|| {
                ParseError::Fatal(
                    "Schema uses currency_source=column but currency_column is unset".into(),
                )
            })?;
            let raw = field(rec, col as usize, line, "currency")?.trim();
            validate_currency(raw, line)?
        }
        CurrencySource::Fixed => {
            let fixed = schema.fixed_currency.as_deref().ok_or_else(|| {
                ParseError::Fatal(
                    "Schema uses currency_source=fixed but fixed_currency is unset".into(),
                )
            })?;
            validate_currency(fixed.trim(), line)?
        }
    };

    let description = build_description(rec, &schema.description_columns);

    let raw_bank_category = schema
        .raw_category_column
        .and_then(|c| rec.get(c as usize))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let iban = schema
        .iban_column
        .and_then(|c| rec.get(c as usize))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let source_ref = match schema.bank_ref_column {
        Some(c) => {
            let raw = rec.get(c as usize).map(str::trim).filter(|s| !s.is_empty());
            match raw {
                Some(r) => stable_hash(&[r.as_bytes()]),
                // Fall back to row hash when the bank-ref cell is empty
                // — typical for in-house transfers without a reference.
                None => row_hash(rec),
            }
        }
        None => row_hash(rec),
    };

    Ok(ParsedRow {
        date,
        amount_minor,
        currency,
        description,
        raw_bank_category,
        source_ref,
        iban,
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

fn validate_currency(raw: &str, line: u32) -> Result<String, ParseError> {
    if raw.len() != 3 || !raw.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(ParseError::Row {
            line,
            message: format!("Invalid currency `{raw}`, expected 3-letter ISO 4217 code"),
        });
    }
    Ok(raw.to_ascii_uppercase())
}

fn build_description(rec: &csv::StringRecord, cols: &[u32]) -> String {
    let parts: Vec<&str> = cols
        .iter()
        .filter_map(|&c| rec.get(c as usize))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        "(no description)".to_string()
    } else {
        parts.join(" / ")
    }
}

/// Parse an amount string with the given decimal separator. Returns
/// `i64` minor units (cents); rejects inputs without exactly two
/// decimal places. Sign comes from a leading `-` or `+`.
pub fn parse_amount_minor(s: &str, sep: DecimalSeparator) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    let (sign, rest) = match s.as_bytes()[0] {
        b'-' => (-1i64, &s[1..]),
        b'+' => (1, &s[1..]),
        _ => (1, s),
    };
    // `decimal` is the char that separates units from cents; `thousands`
    // is the grouping char and gets stripped before parsing.
    let (decimal, thousands) = match sep {
        DecimalSeparator::Dot => ('.', ','),
        DecimalSeparator::Comma => (',', '.'),
    };
    // `thousands` is the *opposite* separator and gets stripped out.
    let mut parts = rest.splitn(2, decimal);
    let int_part = parts.next()?.replace(thousands, "");
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

/// Parse a date string with one of the supported format directives.
/// Today we accept `DD.MM.YY`, `DD.MM.YYYY`, `DD/MM/YYYY`, `YYYY-MM-DD`,
/// `YYYY-MM-DD HH:MM:SS`, and `MM/DD/YYYY`. The schema's `date_format` is one
/// of those literal strings; chrono format directives are *not* used directly
/// so users can pick a format without learning strftime syntax.
pub fn parse_date(input: &str, format: &str) -> Option<DateTime<Utc>> {
    let nd = match format {
        "DD.MM.YY" | "DD.MM.YYYY" => parse_dotted(input),
        "DD/MM/YYYY" => parse_slashed_dmy(input),
        "MM/DD/YYYY" => parse_slashed_mdy(input),
        "YYYY-MM-DD" => parse_iso_date_or_datetime(input),
        "YYYY-MM-DD HH:MM:SS" => parse_iso_datetime(input),
        // Fall through to chrono if the user picks something unusual.
        other => NaiveDate::parse_from_str(input, other).ok(),
    }?;
    let dt = nd.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&dt))
}

/// Parse `DD.MM.YY` or `DD.MM.YYYY`. 2-digit years pivot at 70.
pub fn parse_dotted(s: &str) -> Option<NaiveDate> {
    parse_with_separator(s, '.')
}

fn parse_slashed_dmy(s: &str) -> Option<NaiveDate> {
    parse_with_separator(s, '/')
}

fn parse_slashed_mdy(s: &str) -> Option<NaiveDate> {
    let mut parts = s.split('/');
    let month = parts.next()?;
    let day = parts.next()?;
    let year_raw = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (year, day, month) = pivot_year(year_raw, day, month)?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn parse_iso_date_or_datetime(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .or_else(|| parse_iso_datetime(s))
}

fn parse_iso_datetime(s: &str) -> Option<NaiveDate> {
    const FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
    ];
    FORMATS.iter().find_map(|format| {
        NaiveDateTime::parse_from_str(s, format)
            .ok()
            .map(|dt| dt.date())
    })
}

fn parse_with_separator(s: &str, sep: char) -> Option<NaiveDate> {
    let mut parts = s.split(sep);
    let day = parts.next()?;
    let month = parts.next()?;
    let year_raw = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (year, day, month) = pivot_year(year_raw, day, month)?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn pivot_year(year_raw: &str, day: &str, month: &str) -> Option<(i32, u32, u32)> {
    let year: i32 = year_raw.parse().ok()?;
    let year = match year_raw.len() {
        2 => {
            if year < 70 {
                2000 + year
            } else {
                1900 + year
            }
        }
        4 => year,
        _ => return None,
    };
    let day: u32 = day.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    Some((year, day, month))
}

/// SHA-256 (first 16 hex chars) of the row's fields joined with `\u{1f}`
/// after trimming each. Stable regardless of CSV quoting.
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
    hex::encode(&hasher.finalize()[..8])
}

fn stable_hash(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    let mut first = true;
    for p in parts {
        if !first {
            hasher.update([0x1f]);
        }
        hasher.update(p);
        first = false;
    }
    hex::encode(&hasher.finalize()[..8])
}

#[cfg(test)]
mod tests {
    use super::parse_date;

    #[test]
    fn parse_date_accepts_iso_datetime_and_ignores_time() {
        let parsed =
            parse_date("2026-05-24 13:45:59", "YYYY-MM-DD HH:MM:SS").expect("date should parse");

        assert_eq!(parsed.format("%Y-%m-%d").to_string(), "2026-05-24");
        assert_eq!(parsed.format("%H:%M:%S").to_string(), "00:00:00");
    }

    #[test]
    fn parse_iso_date_format_tolerates_datetime_column() {
        let parsed = parse_date("2026-05-24 13:45:59", "YYYY-MM-DD").expect("date should parse");

        assert_eq!(parsed.format("%Y-%m-%d").to_string(), "2026-05-24");
    }
}
