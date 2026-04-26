# Workflow

How to develop, build, and ship changes to Uncloud.

## Git Workflow

Scope the workflow to the size of the change.

### Large features → feature branch + PR + manual test

New features, significant refactors, schema changes, multi-file behaviour changes:

1. **Create a feature branch**:
   ```bash
   git checkout -b feature/<short-description>
   ```
2. **Commit changes** to that branch.
3. **Push** to origin:
   ```bash
   git push -u origin feature/<short-description>
   ```
4. **Open a PR** with `gh`:
   ```bash
   gh pr create --fill --base main
   ```
5. **Stop — do not merge**. The user manually tests the branch, then merges via GitHub UI or `gh pr merge` once satisfied.

### Small fixes → direct to `main` (or the relevant open branch)

Bug fixes, doc updates, config tweaks, CI adjustments, small maintenance — commit directly on `main`. No branch, no PR. If a feature branch is already open and the fix belongs there, push to it directly.

### Amending an open PR

```bash
git checkout feature/<branch-name>
# make the fix
git add <files> && git commit -m "Fix: ..."
git push                              # updates the PR automatically
git checkout main
```

### Main working directory stays on `main`

The primary checkout must always be on `main`. For large features, work in isolated worktrees so the primary tree's `git status` / builds stay predictable.

> Remote: `https://github.com/decaychain/uncloud.git`.

---

## CI / GitHub Actions

Workflows in `.github/workflows/` are triggered manually or on release tags — never on every push/PR:

| Workflow | Auto trigger | Manual trigger |
|---|---|---|
| `ci.yml` | — | `workflow_dispatch` |
| `release-server.yml` | push tag `v*` | `workflow_dispatch` |
| `release-desktop.yml` | push tag `v*` | `workflow_dispatch` |
| `release-android.yml` | push tag `v*` | `workflow_dispatch` |

Run a workflow manually from the Actions tab or:
```bash
gh workflow run ci.yml                         # on default branch
gh workflow run ci.yml --ref feature/foo       # on a specific branch
```

This keeps the feedback loop tight locally and avoids long CI queues on every commit. Verify builds locally before pushing.

---

## Dev Workflow

```bash
# Backend
cargo run -p uncloud-server

# Server CLI subcommands (each maps to a one-off task; default is `serve`):
cargo run -p uncloud-server -- bootstrap-admin --username alice
cargo run -p uncloud-server -- dedupe-files --dry-run    # see Architecture → Storage → Constraint

# Frontend (Tailwind is rebuilt automatically by build.rs on cargo build,
# but for watch mode during active UI work run both):
cd crates/uncloud-web
npx tailwindcss -i input.css -o assets/tailwind.css --watch   # Terminal 1
dx serve                                                        # Terminal 2

# Desktop (requires webkit2gtk4.1-devel, libsoup3-devel on Fedora)
./build-desktop.sh   # dx build → copy to src-frontend → cargo build desktop
cargo run -p uncloud-desktop
```
