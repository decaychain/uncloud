# Finance Tracker — Design Document

**Status**: design only, not implemented. Captures the design discussion from a brainstorming session so the model can be picked up later without re-deriving it.

## Overview

A built-in personal finance tracker for Uncloud, scoped at "log + categorize + monthly summary" rather than a full accounting tool. Primary workflow: once a month, import a CSV from each bank, categorize new transactions, clear outstanding pending settlements (informal debts owed to or from people).

Explicitly **out of scope** for the first version: budgets, double-entry semantics, FX conversion, stocks / investment tracking, automated bank API integration (Plaid-style), recurring transaction detection, merchant inference.

## Core Concepts

### Accounts

- Each account is **single-currency**. Multi-currency wallets like Wise / Revolut are tracked as N separate accounts (one per currency held), which matches how those services work internally anyway.
- Account types: free-form string (e.g. "checking", "savings", "credit card", "cash"). Used only for display/grouping; no behavioural difference.
- Net-worth view shows **per-currency totals**, not a converted single number. Output is "EUR 1234, USD 567", never "≈ EUR 1745".

### Transactions (with legs)

Even though most transactions are single-category, the schema is **transaction-with-legs**:

```
Transaction:
  id, account_id, source_ref (stable hash from import — UNIQUE per account)
  date, amount, currency, description, raw_bank_category
  notes, tags
  ... + provenance fields (see Import below)

TransactionLeg:
  id, transaction_id, amount, category_id, note
  -- legs sum to transaction.amount; default = single leg covering full amount
```

A simple transaction has exactly one leg (auto-created on import). A split transaction has 2+ legs. The UI hides legs for single-leg rows so the common case stays simple. Cost of carrying the legs table from day one is small; cost of retrofitting it later is a data migration.

### Categories

- Two-level hierarchy: parent category → subcategory (e.g., Medicine → Dentist).
- Categories belong to the user. No cross-user sharing.
- Each category has name + parent_id + display colour/icon.

### Rules

A **rule** is a substring, wildcard, or regex pattern on transaction `description` that maps to a category. Applied:

1. **On import**: each new transaction runs through the rule list; first match wins; assigns the leg's category with `category_source = 'rule'` and records `rule_id` for provenance.
2. **On demand**: "Apply current rules" button re-runs rules against existing transactions. Only touches legs whose `category_source` is `'unset'` or `'rule'` — user-set categories are never overwritten.

Rules stay deliberately simple. No ML, no fuzzy matching, no merchant resolution; manual triage handles the long tail. Real-world coverage from simple pattern rules + manual is ~95% with very little upkeep.

### Pending settlements (separate module)

Informal debts — money you've spent on someone else's behalf, money someone has lent you, group-dinner-style "you'll pay me back later" promises — are a separate model from transactions, not a special transaction type. The label **pending settlement** captures the state: an obligation that exists in your records and is awaiting actual money movement to clear.

```
PendingSettlement:
  id, owner_id, counterparty (free-text name), amount, currency
  direction: owed_to_me | owed_by_me
  category_id (reuses transaction categories — "beer", "garage repair", etc.)
  description, notes, opened_at, next_payment_at (optional reminder date),
  source_transaction_id (optional FK to the original real transaction that
    created the obligation)
  status: open | settled | forgiven
  closed_at

SettlementEntry (own collection, FK settlement_id):
  id, owner_id, settlement_id, kind: payment | forgiveness | charge
  counterparty (optional override — e.g. settlement is "Friends", entries are
    "Bob", "Gary")
  amount, date, linked_transaction_id (optional FK to a real transaction once
    the money actually moves), note, created_at
```

The list view groups by counterparty and currency: "Alice owes you EUR 30, USD 15"; "Bob owes you EUR 10"; "You owe Carol EUR 12". A settlement contains one or more entries; `payment` and `forgiveness` reduce the outstanding amount, `charge` increases it (a new obligation kept under the same settlement — "you also owe me 50 for the door"; a charge may land on a settled settlement and reopens it). Outstanding = amount + charges − payments − forgiveness; entries are the source of truth, totals are aggregated from the entries collection at read time. Entries may link to a normal bank transaction, but usually won't — many small repayments happen in cash or outside the imported bank accounts. Group settlements are represented by a broad settlement counterparty ("Friends") plus per-entry counterparties ("Bob paid EUR 20", "Gary paid EUR 30"). `forgiveness` entries cover the case where you write off all or part of the obligation without money moving — small amounts, lost contact, etc. `next_payment_at` records a promised repayment date ("Bob pays before Friday") and is surfaced in the UI with an overdue highlight.

The web UI is a two-pane master/detail view on desktop (settlement list left, entries + totals right); on mobile the panes are separate screens backed by the `/finance/settlements` and `/finance/settlements/:id` routes.

## Import Workflow — The Critical Part

The biggest pain point in existing tools (Lunch Money, YNAB, Money Manager EX, etc.) is **re-importing a CSV after discovering an import-rule mistake destroys manual categorization work**. Designing around this is the main reason for the schema choices below.

### Stable source identity

Every imported row gets a deterministic `source_ref` hash, computed from the source side:

- If the bank exports a stable transaction reference (most do): `source_ref = sha256(account_id || bank_ref)`.
- Otherwise: `source_ref = sha256(account_id || date || amount || currency || description)`.

`source_ref` is **unique per account**. Re-import becomes UPSERT keyed on `source_ref`, never INSERT. Same CSV imported twice = no duplicates by construction.

### Imported vs. user-applied fields

Each transaction has two layers:

| Layer | Fields | Re-import behaviour |
|-------|--------|---------------------|
| **Imported** (owned by import) | `date`, `amount`, `currency`, `description`, `raw_bank_category` | Overwritten on re-import |
| **User-applied** (owned by user) | leg.`category_id`, `notes`, `tags`, manual splits | Never touched by re-import |

Each leg also carries provenance:

- `category_source`: `'user'` | `'rule'` | `'unset'`
- `rule_id` (when `category_source = 'rule'`)

User-set categories always win and never get auto-overwritten. Re-running rules only touches `unset` and `rule`-sourced legs.

### Re-import as diff preview

Re-importing a CSV (or the same CSV after fixing the import profile) does **not** apply changes immediately. It produces a diff:

```
Re-importing checking.csv with profile "Chase EUR" — preview:
  • 124 rows match existing transactions, no changes (skipped)
  • 12 rows match existing transactions, only imported fields would change
    (your categories preserved). Show details ▾
  •  3 rows are new (would be inserted)
  •  1 existing transaction would be orphaned: bank no longer reports it.
    [Delete] [Keep]
[Apply]  [Cancel]
```

User confirms or cancels. Manual edits survive by design — the worst that can happen on a re-import is a few transactions get their `description` or `date` rewritten while their category and notes stay put.

### Import profiles

Each bank's CSV format is captured once as an **import profile**:

```
ImportProfile:
  id, owner_id, name (e.g. "Revolut EUR"), account_id (default target)
  date_format, date_column
  amount_column, amount_sign_convention (positive=credit | positive=debit)
  description_column(s) — concatenated with separator
  currency_column (or fixed currency)
  bank_ref_column (optional)
  skip_header_rows
```

Saved per user; selected on each import. Re-keying the same mapping every month is the second-fastest way to make people stop using a finance app, after losing their categorization work.

### Import batches

Each import creates an `ImportBatch` row recording the source filename, profile used, timestamp, raw CSV (or its hash, for verification), and the resulting diff. Lets the user browse history and undo a recent batch if needed without losing edits made before that batch.

## Data Model Summary

```
Account            owner_id, name, type, currency, opening_balance
Category           owner_id, parent_id (nullable), name, colour
Rule               owner_id, name, pattern, pattern_kind (substring|starts_with|wildcard|regex), category_id, priority
Transaction        owner_id, account_id, source_ref UNIQUE(account_id, source_ref),
                   date, amount, currency, description, raw_bank_category,
                   notes, tags
TransactionLeg     transaction_id, amount, category_id NULLABLE, category_source, rule_id, note
ImportProfile      owner_id, name, account_id, ...mapping fields...
ImportBatch       owner_id, account_id, profile_id, source_filename, source_hash, imported_at, summary
PendingSettlement  owner_id, counterparty, direction, amount, currency, category_id,
                   description, notes, opened_at, next_payment_at,
                   source_transaction_id, status, closed_at
SettlementEntry    own collection: owner_id, settlement_id, kind, counterparty,
                   amount, date, linked_transaction_id, note, created_at
```

## API Sketch

All under `/api/finance/...`:

- `GET/POST /accounts`, `PUT/DELETE /accounts/{id}`
- `GET /accounts/{id}/balance` — running balance per currency
- `GET/POST /categories`, `PUT/DELETE /categories/{id}`
- `GET/POST /rules`, `PUT/DELETE /rules/{id}`, `POST /rules/apply` — re-run rules on existing legs
- `GET /transactions` — paginated, filterable by account/category/date/uncategorized
- `PUT /transactions/{id}` — edit user-applied fields only; imported fields immutable via this route
- `POST /transactions/{id}/legs` — split a transaction
- `GET/POST /import-profiles`, `PUT/DELETE /import-profiles/{id}`
- `POST /imports/preview` — multipart CSV upload + profile_id, returns diff (no DB writes)
- `POST /imports/apply` — confirms a previewed diff, returns ImportBatch
- `GET /import-batches`, `DELETE /import-batches/{id}` — undo a batch (preserving manual edits to the affected transactions where possible)
- `GET/POST /settlements`, `GET/PUT/DELETE /settlements/{id}` — the list omits entries; the single-settlement GET returns them (delete cascades to entries)
- `POST /settlements/{id}/entries`, `DELETE /settlements/{id}/entries/{entry_id}` — partial payments/forgiveness/charges; status derives from the remaining balance and both return the full detail response

## Frontend Structure

Sidebar section `Finance`, with sub-views:

- **Overview**: per-currency balance totals, last month spend by category (top 5), uncategorized count badge.
- **Transactions**: filterable list, with prominent **Review queue** at the top showing uncategorized + new-merchant transactions for the active month. The review queue is the killer-feature UI; the rest is just lists.
- **Accounts**: balance per account, click into per-account transaction list.
- **Categories**: tree view + edit.
- **Rules**: list + edit + "Apply rules" button.
- **Import**: pick profile, upload CSV, see diff preview, confirm.
- **Pending settlements**: open / settled / forgiven, grouped by counterparty and direction (owed to you vs. owed by you).

## Deferred / Open

- **Recurring transaction detection** ("Netflix monthly") — defer until 6+ months of data shows whether substring rules already cover it.
- **Merchant resolution / cleaning** — defer; manual descriptions + rules are fine.
- **Multi-user sharing** — household finance is a real use case but a different problem (split transactions across two users, shared categories, settlement). Not in v1.
- **Budgets** — explicitly out of v1 per user request; revisit if it ever feels missing.
- **FX conversion** — explicitly out. Net-worth stays per-currency.
- **Mobile-friendly review-queue UI** — should be designed for desktop first; mobile triage on a phone-sized screen is a known hard UX problem and not worth optimising for in v1.

## Risks

- **Import correctness across banks**: realistically, the user will need 2-3 import profiles before they're comfortable. The diff-preview / re-import design exists to make profile iteration safe.
- **Adoption decay**: if the user imports once, doesn't open the app for 6 months, and forgets the workflow, the tool dies. Worth keeping the monthly cadence in mind when designing nudges (e.g. an "uncategorized older than 30 days" badge in the main Uncloud sidebar).
- **Self-hosted single-user finance is a tiny market**: this is fine — the user is the user — but means there's no community to spot bugs. Test data integrity carefully before relying on it for taxes / settlements.
