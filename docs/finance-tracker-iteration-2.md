# Finance Tracker — Iteration 2 Plan

**Context**: iteration 1 (`finance-tracker-foundation`) shipped the bare slice — accounts, categories, transactions with legs, and a CSV importer hard-coded for Sparkasse CAMT V8. This document captures the iteration-2 scope agreed in conversation: turn the importer into a real workflow, add reconciliation, and add rules.

The original [finance-tracker-design.md](finance-tracker-design.md) already specs import profiles, batches, and rules. This plan re-states only the parts that change in iteration 2 and adds three things that were not in the original design: auto-create-account-from-CSV, sourcing imports from an Uncloud file, and reconciliation via balance snapshots.

## 1. User-editable import schemas

Replace the `ImportProfile` trait + hard-coded structs in `crates/uncloud-server/src/finance_import/` with a database-stored document that the parser reads at runtime.

**Model** (`ImportSchema`):

```
ImportSchema:
  id, owner_id, name, account_id (default target, optional — see §3)
  delimiter (default ',')
  encoding   (default 'utf-8')
  decimal_separator (',' | '.')
  skip_header_rows
  date_column, date_format
  amount_column, amount_sign_convention (positive_credit | positive_debit)
  description_columns: Vec<usize>  -- joined with ' / '
  currency_column (or fixed_currency)
  bank_ref_column (optional)
  iban_column (optional — drives auto-account-create)
  raw_category_column (optional)
  is_builtin: bool       -- true for seeded templates; can be cloned but not edited
  created_at, updated_at
```

**Parser**: one generic `parse(bytes, &ImportSchema) -> Vec<Result<ParsedRow, ParseError>>` that walks columns by index using the schema. The current Sparkasse module becomes the *seed* of one `ImportSchema` row (cloned per user on first import, marked `is_builtin: true`).

`parse_amount_minor` strips any currency symbol (ASCII `$`, Latin-1 `¢£¤¥`, and the whole Unicode Currency Symbols block incl. `€`) and all whitespace (incl. non-breaking spaces used as grouping) from the amount cell before applying the schema's decimal separator, so exports that embed the symbol in the value column (e.g. Revolut `-12,99€`, `$1,234.56`) parse correctly. Row-level parse failures are non-fatal — junk/preamble rows in multi-section statements are simply skipped and counted as errors.

**API**:
- `GET/POST /finance/import-schemas`, `PUT/DELETE /finance/import-schemas/{id}`
- `POST /finance/import-schemas/{id}/clone` — server-side clone (builtin → owned editable copy)

**UI**: list + edit form; "Clone" button on each row.

## 2. ImportRun log with undo / re-run

Replace the fire-and-forget `import_csv` route with a `ImportRun` model:

```
ImportRun:
  id, owner_id, account_id, schema_id
  source: { kind: 'upload' | 'uncloud_file', filename, size_bytes, sha256,
            uncloud_file_id (when kind=uncloud_file) }
  status: 'pending' | 'applied' | 'reverted'
  created_transaction_ids: Vec<ObjectId>
  summary: { created, skipped_duplicate, errored }
  errors: Vec<{row: usize, message: String}>
  started_at, completed_at
```

**Flow**:
- `POST /finance/imports` with multipart CSV *or* `{ uncloud_file_id }` + `schema_id` (+ optional `account_id`; see §3). Returns the diff preview (no DB writes yet) — preview includes new/match/orphaned rows per [finance-tracker-design.md §"Re-import as diff preview"](finance-tracker-design.md).
- `POST /finance/imports/{id}/apply` — commits, status `applied`, writes the txs and the `created_transaction_ids` list.
- `POST /finance/imports/{id}/revert` — deletes the txs (preserving user-edited categories? — design decision: **delete unconditionally**; the user explicitly asked for "undo this import"). Status `reverted`.
- `POST /finance/imports/{id}/rerun` with `{ schema_id }` — convenience: creates a fresh `ImportRun` reusing the same source, optionally with a different schema.

**Uncloud-file sourcing**: when `kind=uncloud_file`, the server reads the bytes via the existing storage backend. The CSV that was imported is then versioned and backed up by Uncloud automatically. Re-running an old import means re-fetching the file by ID — if the user has deleted it from Uncloud, the run is no longer re-runnable (return 410 Gone).

**UI**: new "Imports" tab in the Finance section showing the table — date, schema, source, status, counts — with Revert / Re-run actions per row.

## 3. Auto-create account from CSV

If a schema sets `iban_column`, on import the server:
1. Reads the IBAN from the first row (most banks repeat it on every row; we sample row 0 only — if it varies, error out and tell the user the schema is wrong).
2. Looks up an account by `{ owner_id, iban }` (new field on `FinanceAccount`; nullable).
3. If found, uses it as the import target.
4. If not, creates a new `FinanceAccount` with name = "{Bank} {last-4 of IBAN}" (or just the IBAN when the bank is unknown), currency from the CSV's currency column (fall back to EUR for IBANs starting with DE), `account_type = "checking"`.

The import-preview API returns `auto_created_account: Option<AccountResponse>` so the UI can show "Will create account: …" and let the user override the target before applying.

This is the only feature in this iteration that adds a field to `FinanceAccount` (`iban: Option<String>`).

## 4. Reconciliation via balance snapshots

```
BalanceSnapshot:
  id, owner_id, account_id, on_date, actual_balance_minor, note (optional)
  adjustment_transaction_id  -- FK to the auto-generated tx
  created_at
```

**Flow** (`POST /finance/accounts/{id}/reconcile`):
1. Request: `{ on_date, actual_balance_minor, note? }`.
2. Server computes balance at end of `on_date` from existing transactions.
3. Returns `{ computed, actual, delta }` as a *preview* (no writes).
4. `POST /finance/accounts/{id}/reconcile/apply` with `{ on_date, actual_balance_minor, note? }` creates the snapshot AND an adjustment transaction:
   - `amount_minor = delta`
   - `date = on_date`
   - `description = "Reconciliation: <note>"` (or "Reconciliation" if no note)
   - `legs = [{ amount_minor: delta, category_id: <Reconciliation category>, category_source: 'user' }]`
   - A new top-level field `source_snapshot_id: Option<ObjectId>` on `FinanceTransaction` distinguishes it from imported/manual transactions.

**Late-import policy**: snapshots stay fixed. If later imports change the computed balance for a previously-reconciled date, the adjustment transaction stays unchanged (it represents what was true at the time of reconciliation). The UI shows a banner on the account: "1 reconciliation drifted by EUR 4.20 — recompute?". A "Recompute" action regenerates the adjustment transaction to match the snapshot against the now-complete history.

**Reconciliation category**: seeded per-user on first reconcile attempt — a category named "Reconciliation" with a neutral colour. User can rename later (the FK by id is stable).

**Transaction-list display**: rows with `source_snapshot_id != None` get a distinct badge ("Reconciliation") and can't be edited directly — only via re-reconciliation.

## 5. Categorization rules

```
Rule:
  id, owner_id, name
  pattern, pattern_kind: 'substring' | 'regex' | 'starts_with' | 'wildcard'
  case_insensitive: bool (default true)
  category_id
  priority: i32 (lower = applied first)
  enabled: bool (default true)
  created_at, updated_at
```

**Application**:
- **On import**: each parsed row runs through the user's enabled rules ordered by `priority`. First match assigns `legs[0].category_id`, `category_source = 'rule'`, `rule_id = <matched>`. No match → `category_source = 'unset'`, no category.
- **Re-run** (`POST /finance/rules/apply`): scans all the user's transactions; updates legs where `category_source ∈ {unset, rule}`. Never touches `category_source = 'user'`.

**API**:
- `GET/POST /finance/rules`, `PUT/DELETE /finance/rules/{id}`
- `POST /finance/rules/apply` — full re-run, returns `{ updated_legs: count, unmatched: count }`
- `POST /finance/rules/test` with `{ pattern, pattern_kind, case_insensitive }` — returns sample of last 50 user transactions that would match (no writes); lets users iterate on a pattern before saving.

**UI**: list + edit form; "Test" action shows live preview matches.

## Order of work

1. Plan doc (this file)
2. **User-editable import schemas** — biggest blast radius; touches model, routes, parser, UI. Everything else depends on `schema_id` being a real FK.
3. **ImportRun log** — depends on schemas existing as DB rows.
4. **Auto-create account** — small follow-up to schemas + ImportRun.
5. **Reconciliation snapshots** — independent of the import work; can interleave or wait.
6. **Categorization rules** — independent; can run in parallel with reconciliation.

Each step lands as its own commit on `finance-tracker-foundation` and gets a brief mention in [Features.md](Features.md) on completion.

## Open questions

- **Multi-currency in one CSV**: not supported; schema picks a single currency (column or fixed). Sparkasse CSVs are single-currency, so this is fine for v1.
- **CSV header autodetection**: deferred. User picks column indices manually. A "detect" button that proposes a schema from the file's header row is a nice-to-have for a later iteration.
- **Rule complexity**: starting at substring/starts_with/wildcard/regex. No AND/OR composition, no amount-based conditions, no date-based conditions. Add later if these turn out to be insufficient.

## Out of scope (still)

Everything marked deferred in the original design ([finance-tracker-design.md §"Deferred / Open"](finance-tracker-design.md)) remains deferred: budgets, FX conversion, recurring detection, merchant resolution, multi-user, mobile-first.
