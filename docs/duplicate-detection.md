# Duplicate Detection — Design Document

**Status**: design + initial implementation. Captures the model so the
implementation phases can land incrementally without re-deriving it.

## Problem

Personal cloud installations accumulate duplicates — the same photo backed up
twice, an entire folder mirrored under `Backup/`, files copied between
projects. A naive duplicate list ("here are 12,847 hash collisions") is
unactionable on real libraries: a single mirrored 2,000-photo folder produces
2,000 entries that are all the same story.

The feature needs to **detect duplicates and present them in actionable
chunks**, with the chunk size matching the relationship: collapse "two
folders are mirrors of each other" into one card, not two thousand.

## Detection model

Every `File` already carries `checksum_sha256` (populated on upload). That's
the duplicate key — exact-content matching, no fuzzy comparison.

A **duplicate set** is a group of two or more live files that share a
checksum. Excluded from detection:

- Soft-deleted files (`deleted_at != null`).
- Files with `size_bytes == 0` (the empty-file hash `e3b0c4...` would be a
  meaningless megacluster).
- Hidden Uncloud internals (anything under `.uncloud/`, e.g. version archives,
  thumbnails, trash).

## Folder-relationship classification

Once duplicate sets are computed, the algorithm walks the (folder × folder)
matrix induced by the sets and classifies each pair of folders that
participates in any duplicate.

For folders A and B and the set of file checksums each contains (excluding
the items already excluded above):

- **Equal** — `hashes(A) == hashes(B)` and both are non-empty. A and B are
  mirrors.
- **Subset** — `hashes(B) ⊂ hashes(A)`, strictly. Every file in B has a hash
  twin in A but A has more.
- **Partial** — overlap is non-empty but neither contains the other.
- **Unrelated** — empty intersection.

Equal pairs are clustered into connected components: A == B == C produces a
single 3-folder cluster, not three pairwise rows.

Subsets do **not** cluster. B ⊂ A and C ⊂ A says nothing about B and C, so
each subset is rendered as its own pair.

Partial pairs are **not** surfaced as their own bucket in v1 — the duplicates
in those folders fall into the **stray** bucket as individual hash sets. Too
much noise for too little signal in the partial case (a folder that's "60 %
the same" usually doesn't have an obvious cleanup decision).

## Three buckets, in priority order

The UI presents results in three sections, each empty if there's nothing to
show:

### 1. Mirror clusters

> *"247 files (12.3 GB) are duplicated across 3 folders"*
> `Photos/2023`, `Backup/Photos/2023`, `Old/2023`
> [Keep ▾ `Photos/2023`] [Delete the others]

One card per connected component of the equal-folders graph. The card lists
every member folder, the file count and total size, and a single "keep one,
delete the rest" affordance. Default keep-pick is the shallowest path that
doesn't match a backup-style segment (see [Smart picker](#smart-picker)
below); user can override per card.

Resolving a mirror cluster removes the duplicate-set rows it explains from
the stray bucket.

### 2. Subset folders

> *"`Photos/2023-favourites` is already inside `Photos/2023`"*
> 38 files · 412 MB
> [Delete subset]

One card per (subset, superset) pair. Default action is "delete subset"; user
can flip via a smaller "delete superset's matching files instead" affordance
on the card if they prefer.

A subset card likewise removes its files from the stray bucket once
resolved.

### 3. Stray duplicates

Hash sets that aren't explained by a mirror cluster or subset relationship.
Rendered as classic per-set cards: one card per hash, each row a file with
its full path, with checkbox selection and a "delete selected" action.
Sorted by total wasted bytes descending so the biggest space wins are at the
top.

This bucket should be small once (1) and (2) are resolved — it's the
honest residual.

## Smart picker

When the UI suggests "keep this one, delete the rest" for a mirror cluster,
the default is computed as:

1. Strip candidates whose path contains a backup-suggestive segment:
   `Backup`, `Backups`, `Archive`, `Archives`, `Old`, `Trash`, anything
   prefixed with `.uncloud/`. If candidates remain, restrict to those.
2. Among remaining candidates, prefer the **shallowest path** (fewest
   segments).
3. Tie-break by `created_at` — older wins (more "original").

The user always sees the dropdown and can override.

## Server-side surface

### Endpoint

`GET /api/duplicates` (and `/api/v1/duplicates`)

Returns the full report for the current user. No pagination in v1 — duplicate
volumes in personal-cloud workloads are bounded and the report is only as
heavy as the duplicate count, not the total file count.

```json
{
  "scanned_at": "2026-05-09T12:34:56Z",
  "total_duplicate_files": 2451,
  "total_wasted_bytes": 13287436820,
  "mirror_clusters": [
    {
      "id": "mc-1",
      "folders": [
        { "id": "...", "path": "Photos/2023", "file_count": 247 },
        { "id": "...", "path": "Backup/Photos/2023", "file_count": 247 },
        { "id": "...", "path": "Old/2023", "file_count": 247 }
      ],
      "file_count": 247,
      "total_bytes": 13189378048,
      "suggested_keep_folder_id": "..."
    }
  ],
  "subsets": [
    {
      "id": "ss-1",
      "subset": { "id": "...", "path": "Photos/2023-favourites", "file_count": 38 },
      "superset": { "id": "...", "path": "Photos/2023", "file_count": 247 },
      "file_count": 38,
      "total_bytes": 412312345
    }
  ],
  "stray_sets": [
    {
      "id": "ss-h-abc123",
      "checksum": "abc123...",
      "size_bytes": 1048576,
      "files": [
        { "id": "...", "path": "Documents/report.pdf",          "created_at": "..." },
        { "id": "...", "path": "Inbox/2024-01/report.pdf",      "created_at": "..." }
      ]
    }
  ]
}
```

### Algorithm

1. Aggregation pipeline on `files` filtered by
   `{ owner_id, deleted_at: null, size_bytes: { $gt: 0 }, storage_path: { $not: /^[^/]+\/.uncloud\// } }`,
   `$group: { _id: "$checksum_sha256", files: { $push: { ... } } }`,
   `$match: { "files.1": { $exists: true } }`.
2. For each duplicate set, build a `(folder_id → file_count)` map and a
   `(folder_id → set(checksums))` registry.
3. Folder-relationship classification: for every unordered pair of folders
   that both appear in any duplicate set, compare their checksum sets to
   classify as Equal / Subset / Partial.
4. Equal pairs → union-find → mirror clusters. Verify each cluster member
   has identical hash sets (transitive closure of binary equals can give a
   false positive when the algorithm runs over a partial overlap; the
   verification step is cheap and rules it out).
5. Subset pairs → list, with `subset` and `superset` decided by hash-count.
   Skip pairs already explained by a mirror cluster (a folder inside a
   mirror cluster is not also a subset of its mirrors — it's an equal).
6. Compute the residual: subtract the files explained by mirror clusters
   and subsets from the original duplicate-set list; what remains is the
   stray bucket.
7. Run the smart picker over each mirror cluster to compute
   `suggested_keep_folder_id`.

Performance: O(P) where P is the number of folder pairs that share at least
one duplicate hash. P is bounded by `(folders_with_duplicates)²`, which is
sparse in practice — a Mongo sweep on a sub-million-file database completes
in a few hundred milliseconds.

### Index

Add a partial index on `files.checksum_sha256` to accelerate the aggregation:

```js
db.files.createIndex(
  { owner_id: 1, checksum_sha256: 1 },
  { partialFilterExpression: { deleted_at: null, size_bytes: { $gt: 0 } } }
)
```

The aggregation runs per user, so the leading `owner_id` is load-bearing.

### Caching

v1 computes on demand on each `GET /api/duplicates` call. Mongo query is
cheap for typical libraries; the algorithm overhead is small. If a future
profiling pass shows this is slow on large data sets, cache the report in a
small `duplicate_reports` collection keyed by `owner_id`, invalidated on
file create / delete / move / restore. Not in v1.

## Deletion path

The cleanup actions ("Delete the others", "Delete subset", "Delete selected
stray") all reuse the existing `DELETE /api/files/{id}` handler — same
soft-delete-to-trash semantics, same audit trail, same recoverability. No
new deletion code.

The frontend issues these in parallel with a small concurrency cap (≤ 8 at
once) and a progress indicator. Once a card's deletions all complete, it
disappears and the totals at the top recompute from a fresh
`GET /api/duplicates` call.

## Frontend

### Route + sidebar

New route `/duplicates`. Sidebar entry under **Maintenance** (a new section
that will host duplicates, future cleanup tools, and possibly a future
"orphan blob scanner"). For now, "Maintenance" is just one entry.

### Page layout

```
┌─ Duplicates ──────────────────────────────────────────────────────┐
│  Found 2,451 duplicate files across 3 mirror folders, 1 subset,   │
│  and 12 stray sets — 12.4 GB recoverable. [Rescan]                │
│                                                                   │
│  ## Mirror folders                                                │
│  [card] [card]                                                    │
│                                                                   │
│  ## Subsets                                                       │
│  [card]                                                           │
│                                                                   │
│  ## Stray duplicates                                              │
│  [card] [card] [card] ...                                         │
└───────────────────────────────────────────────────────────────────┘
```

Each section is collapsible (defaults expanded) and shows zero-state copy
when empty. The page is navigable directly via the URL (no extra
permissioning required beyond the default authenticated session).

### Components

- `components/duplicates/page.rs` — page shell + section orchestration.
- `components/duplicates/mirror_card.rs` — one mirror cluster with the keep-
  picker dropdown and bulk delete button.
- `components/duplicates/subset_card.rs` — one subset pair with the
  delete-subset / flip action.
- `components/duplicates/stray_card.rs` — one hash set, file checkboxes,
  delete-selected.
- `hooks/use_duplicates.rs` — `GET /api/duplicates`, `delete_files(ids)` (a
  thin parallel-delete wrapper).

### State

`Signal<DuplicateReport>` at the page level. After a successful resolve, do
**not** locally edit the signal — refetch the report. Cleaner state, and
avoids drift if the server's classification changes after the deletions
(e.g., a previously-stray pair becomes empty).

## Out of scope (v1)

- Partial-overlap folders surfaced as their own bucket (too noisy).
- Fuzzy / perceptual matching (e.g. "same photo at different resolution",
  "same audio file re-encoded"). Hash-only.
- Cross-user duplicate detection. The aggregation is always
  `owner_id`-scoped — the goal is helping a user clean up their own files,
  not de-duplicate the whole installation.
- Server-side dedup at the storage layer (different problem; a content-
  addressed backend or a backup-side dedup is a separate design).
- Scheduled scanning. Manual page load only. Caching + background refresh
  is a follow-up if the manual model gets sluggish.

## Risks

- **False positives in the mirror classification when the algorithm
  rounds.** Two folders that happen to have the exact same set of duplicate
  hashes but otherwise differ would be classified as equal. The
  verification step (compare full hash sets, not just counts) rules this
  out — equal must mean exactly equal.
- **Smart-picker default deleting the wrong thing.** Mitigated by always
  showing the dropdown with all candidates and never auto-applying — the
  user clicks "Delete the others" knowing what they kept.
- **A user could mass-delete via this page much faster than via the file
  browser**, so the deletion goes through soft-delete-to-trash like every
  other delete and is recoverable for `versioning.trash_retention_days`.
