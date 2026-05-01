# Desktop auto-update

The desktop app ships native auto-update on Windows and Fedora via two
distinct channels:

| Platform | Channel | How users get updates |
|---|---|---|
| Windows | `tauri-plugin-updater` + GitHub Releases manifest | App checks `latest.json` ~20 s after launch. If newer, prompts the user; on Install it downloads the signed NSIS installer, runs it in place, and restarts. |
| Fedora | COPR repo | Users `dnf copr enable decaychain/uncloud && dnf install uncloud`. From there `dnf upgrade` (or `dnf-automatic`) keeps it current. |

The matching escape hatch on every release: the `.exe` and `.rpm` are
always uploaded to the GitHub Release as direct downloads.

## One-time setup

The release pipeline assumes three repository secrets and one tag-driven
workflow. Once configured it is hands-off — every `vX.Y.Z` push triggers
both publication paths in parallel.

### 1. Minisign signing keypair (Windows updater)

The Tauri updater plugin requires every update bundle to be signed with
a minisign key. Without this the running client refuses to install
updates — the public key embedded in the binary at compile time has to
verify the `.sig` file the server returns.

Generate the keypair locally:

```bash
cargo install tauri-cli --locked   # if not already installed
mkdir -p ~/.config/tauri-updater
cargo tauri signer generate -w ~/.config/tauri-updater/uncloud.key
```

`cargo tauri signer generate` prompts for a passphrase, then writes:

- `~/.config/tauri-updater/uncloud.key` — encrypted private key (keep
  safe, never commit, never share)
- `~/.config/tauri-updater/uncloud.key.pub` — public key (copy into
  `crates/uncloud-desktop/tauri.conf.json` under `plugins.updater.pubkey`)

Embed the public key:

```json
{
  "plugins": {
    "updater": {
      "endpoints": [
        "https://github.com/decaychain/uncloud/releases/latest/download/latest.json"
      ],
      "pubkey": "<paste contents of uncloud.key.pub here>"
    }
  }
}
```

Add the private key + passphrase to GitHub repo secrets:

| Secret | Contents |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | `cat ~/.config/tauri-updater/uncloud.key` (the entire file content) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | The passphrase entered above |

### 2. COPR publication (Fedora)

`packaging/fedora/uncloud.spec.in` is the spec template the release
workflow renders for each tag. The COPR project rebuilds it in a
network-enabled mock chroot for every configured Fedora version.

Manual steps (one-time):

1. Create a [Fedora Account System](https://accounts.fedoraproject.org/)
   account if you don't have one.
2. Log in to <https://copr.fedorainfracloud.org/> — your COPR account is
   provisioned automatically via FAS SSO.
3. Create a new project named `uncloud`. **Critical settings:**
   - Chroots: pick the current and previous stable Fedora versions
     (e.g. `fedora-41-x86_64`, `fedora-42-x86_64`, optionally
     `fedora-rawhide-x86_64`).
   - Build options → tick **"Enable internet access during build"**.
     Without this, cargo and npm cannot fetch dependencies from inside
     the chroot and the build fails at the `cargo install dioxus-cli`
     step.
   - Optional but recommended: enable **GPG signing** so users get a
     properly signed RPM repo.
4. Visit <https://copr.fedorainfracloud.org/api/> and copy the entire
   config block shown there.
5. Add it to GitHub repo secrets as `COPR_CONFIG`. The workflow writes
   it verbatim to `~/.config/copr` before invoking `copr-cli`.

End users then run:

```bash
sudo dnf copr enable decaychain/uncloud
sudo dnf install uncloud
```

For unattended updates: `sudo dnf install dnf-automatic && sudo systemctl
enable --now dnf-automatic.timer`.

## Cutting a release

```bash
git tag v0.2.0
git push origin v0.2.0
```

That triggers `release-desktop.yml` which fans out into four parallel
jobs:

1. **`build-linux`** — bumps the workspace version, builds DEB + RPM,
   uploads them to the GitHub Release as the escape-hatch download.
2. **`build-windows`** — bumps the workspace version, builds the signed
   NSIS installer, uploads `.exe` + `.exe.sig` to the release.
3. **`publish-manifest`** — assembles `latest.json` (Tauri updater
   manifest schema) from the Windows signature and uploads it to the
   release. The plugin's endpoint
   `https://github.com/decaychain/uncloud/releases/latest/download/latest.json`
   always resolves to the freshest manifest.
4. **`publish-fedora`** — generates a source tarball + SRPM from
   `packaging/fedora/uncloud.spec.in`, decodes `COPR_CONFIG` into
   `~/.config/copr`, and submits the SRPM to COPR via `copr-cli build
   --nowait`. COPR then rebuilds for each chroot and updates the user
   repo. (Build progress visible at
   <https://copr.fedorainfracloud.org/coprs/decaychain/uncloud/>.)

The version bump is driven entirely by the tag — `Cargo.toml`'s
`workspace.package.version` is rewritten in CI before any build runs, so
releases never accidentally ship as 0.1.0 just because the working tree
hasn't been bumped.

## Why NSIS (not MSI)

`tauri-plugin-updater`'s in-place installer flow on Windows hooks into
NSIS specifically — MSI's upgrade machinery is awkward to drive from a
running user-mode app. NSIS 3.x is actively maintained and used by OBS,
qBittorrent, and most of the Rust + Electron desktop ecosystem; the
scripting language is ugly but Tauri generates the `.nsi` for us. MSIX
would be the modern Microsoft path, but its sandbox restricts arbitrary
filesystem access — fatal for a sync app whose entire point is reading
user-chosen folders.

## Why no AppImage on Linux

Tauri's updater technically supports AppImage, but COPR + DNF gives
Fedora users a strictly better experience (system tray integration,
uninstall via package manager, automatic security updates, signed repo).
The escape-hatch RPM in the GitHub Release covers users who can't use
COPR.
