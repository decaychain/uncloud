//! Matching engine for user-defined categorization rules.
//!
//! Compiles a list of `FinanceRule` rows into a sorted set of
//! `CompiledRule`s. Each `matches()` call returns the index of the
//! first rule whose pattern hits the description; we use indexes so
//! the caller can pluck the corresponding rule id out of the original
//! `Vec<FinanceRule>` without re-borrowing.

use regex::Regex;

use crate::models::{FinanceRule, RulePatternKind};

#[derive(Debug)]
pub struct CompiledRule {
    pub original_index: usize,
    kind: CompiledKind,
}

#[derive(Debug)]
enum CompiledKind {
    Substring {
        needle: String,
        case_insensitive: bool,
    },
    StartsWith {
        needle: String,
        case_insensitive: bool,
    },
    Wildcard(Regex),
    Regex(Regex),
}

pub struct RuleEngine {
    rules: Vec<CompiledRule>,
}

impl RuleEngine {
    /// Build an engine from the user's rules. Disabled rules are
    /// dropped; remaining rules sort by `(priority asc, _id asc)` so
    /// matching is deterministic. Regex rules that fail to compile are
    /// skipped (errors are reported separately via `compile_errors`).
    pub fn build(rules: &[FinanceRule]) -> (Self, Vec<CompileError>) {
        let mut indexed: Vec<(usize, &FinanceRule)> = rules
            .iter()
            .enumerate()
            .filter(|(_, r)| r.enabled)
            .collect();
        indexed.sort_by(|a, b| {
            a.1.priority
                .cmp(&b.1.priority)
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        let mut compiled = Vec::with_capacity(indexed.len());
        let mut errors = Vec::new();
        for (idx, rule) in indexed {
            match compile_rule(rule) {
                Ok(c) => compiled.push(CompiledRule {
                    original_index: idx,
                    kind: c,
                }),
                Err(e) => errors.push(CompileError {
                    rule_id: rule.id.to_hex(),
                    message: e,
                }),
            }
        }
        (Self { rules: compiled }, errors)
    }

    /// Returns the index into the *original* `&[FinanceRule]` slice of
    /// the first rule that matches `description`. None on no match.
    pub fn match_first(&self, description: &str) -> Option<usize> {
        for r in &self.rules {
            if r.kind.matches(description) {
                return Some(r.original_index);
            }
        }
        None
    }
}

impl CompiledKind {
    fn matches(&self, description: &str) -> bool {
        match self {
            CompiledKind::Substring {
                needle,
                case_insensitive,
            } => {
                if *case_insensitive {
                    description.to_lowercase().contains(&needle.to_lowercase())
                } else {
                    description.contains(needle.as_str())
                }
            }
            CompiledKind::StartsWith {
                needle,
                case_insensitive,
            } => {
                if *case_insensitive {
                    description
                        .to_lowercase()
                        .starts_with(&needle.to_lowercase())
                } else {
                    description.starts_with(needle.as_str())
                }
            }
            CompiledKind::Wildcard(re) | CompiledKind::Regex(re) => re.is_match(description),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompileError {
    pub rule_id: String,
    pub message: String,
}

fn compile_rule(rule: &FinanceRule) -> Result<CompiledKind, String> {
    let pattern = rule.pattern.trim();
    if pattern.is_empty() {
        return Err("Empty pattern".into());
    }
    match rule.pattern_kind {
        RulePatternKind::Substring => Ok(CompiledKind::Substring {
            needle: pattern.to_string(),
            case_insensitive: rule.case_insensitive,
        }),
        RulePatternKind::StartsWith => Ok(CompiledKind::StartsWith {
            needle: pattern.to_string(),
            case_insensitive: rule.case_insensitive,
        }),
        RulePatternKind::Wildcard => {
            compile_wildcard(pattern, rule.case_insensitive).map(CompiledKind::Wildcard)
        }
        RulePatternKind::Regex => {
            let wrapped = if rule.case_insensitive {
                format!("(?i){pattern}")
            } else {
                pattern.to_string()
            };
            Regex::new(&wrapped)
                .map(CompiledKind::Regex)
                .map_err(|e| e.to_string())
        }
    }
}

fn compile_wildcard(pattern: &str, case_insensitive: bool) -> Result<Regex, String> {
    let mut regex = String::new();
    if case_insensitive {
        regex.push_str("(?i)");
    }
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            _ => regex.push_str(&regex::escape(&ch.to_string())),
        }
    }
    Regex::new(&regex).map_err(|e| e.to_string())
}

/// Build a single-rule engine for the `POST /rules/test` endpoint —
/// the caller hasn't persisted anything yet, so we synthesize a
/// minimum rule out of the request body. Returns `Err(msg)` if the
/// regex doesn't compile.
pub fn compile_pattern(
    pattern: &str,
    kind: RulePatternKind,
    case_insensitive: bool,
) -> Result<SingleMatcher, String> {
    use mongodb::bson::oid::ObjectId;
    let rule = FinanceRule {
        id: ObjectId::new(),
        owner_id: ObjectId::new(),
        name: String::new(),
        pattern: pattern.to_string(),
        pattern_kind: kind,
        case_insensitive,
        category_id: ObjectId::new(),
        priority: 0,
        enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let kind = compile_rule(&rule)?;
    Ok(SingleMatcher { kind })
}

pub struct SingleMatcher {
    kind: CompiledKind,
}

impl SingleMatcher {
    pub fn matches(&self, description: &str) -> bool {
        self.kind.matches(description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use mongodb::bson::oid::ObjectId;

    fn rule(name: &str, pattern: &str, kind: RulePatternKind, priority: i32) -> FinanceRule {
        FinanceRule {
            id: ObjectId::new(),
            owner_id: ObjectId::new(),
            name: name.into(),
            pattern: pattern.into(),
            pattern_kind: kind,
            case_insensitive: true,
            category_id: ObjectId::new(),
            priority,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn substring_case_insensitive() {
        let rules = vec![rule("groceries", "lidl", RulePatternKind::Substring, 0)];
        let (engine, errs) = RuleEngine::build(&rules);
        assert!(errs.is_empty());
        assert_eq!(engine.match_first("Card payment LIDL Berlin"), Some(0));
        assert_eq!(engine.match_first("Spotify monthly"), None);
    }

    #[test]
    fn priority_first_match_wins() {
        let rules = vec![
            rule("transport", "uber", RulePatternKind::Substring, 10),
            rule("eats", "uber", RulePatternKind::Substring, 5), // wins on priority
        ];
        let (engine, _) = RuleEngine::build(&rules);
        // Original index 1 (Uber Eats) has lower priority, so it should win.
        assert_eq!(engine.match_first("UBER EATS amsterdam"), Some(1));
    }

    #[test]
    fn disabled_rules_skipped() {
        let mut r = rule("groceries", "lidl", RulePatternKind::Substring, 0);
        r.enabled = false;
        let (engine, _) = RuleEngine::build(&[r]);
        assert_eq!(engine.match_first("LIDL"), None);
    }

    #[test]
    fn regex_with_anchor() {
        let rules = vec![rule("salary", r"^SALARY\b", RulePatternKind::Regex, 0)];
        let (engine, errs) = RuleEngine::build(&rules);
        assert!(errs.is_empty());
        assert_eq!(engine.match_first("salary october"), Some(0));
        assert_eq!(engine.match_first("My salary slip"), None);
    }

    #[test]
    fn bad_regex_reports_error() {
        let rules = vec![rule("broken", "(unterminated", RulePatternKind::Regex, 0)];
        let (engine, errs) = RuleEngine::build(&rules);
        assert!(engine.match_first("anything").is_none());
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn starts_with() {
        let rules = vec![rule("rent", "Miete", RulePatternKind::StartsWith, 0)];
        let (engine, _) = RuleEngine::build(&rules);
        assert_eq!(engine.match_first("MIETE October 2026"), Some(0));
        assert_eq!(engine.match_first("Monthly miete"), None);
    }

    #[test]
    fn wildcard_matches_terms_in_order() {
        let rules = vec![rule(
            "amazon via paypal",
            "Paypal*Amazon*",
            RulePatternKind::Wildcard,
            0,
        )];
        let (engine, errs) = RuleEngine::build(&rules);
        assert!(errs.is_empty());
        assert_eq!(
            engine.match_first("POS PAYPAL Europe payment Amazon Marketplace"),
            Some(0)
        );
        assert_eq!(engine.match_first("Amazon Marketplace via PayPal"), None);
    }

    #[test]
    fn wildcard_question_mark_matches_one_character() {
        let rules = vec![rule("shop", "SHOP-??-Berlin", RulePatternKind::Wildcard, 0)];
        let (engine, errs) = RuleEngine::build(&rules);
        assert!(errs.is_empty());
        assert_eq!(engine.match_first("Card SHOP-42-Berlin"), Some(0));
        assert_eq!(engine.match_first("Card SHOP-123-Berlin"), None);
    }
}
