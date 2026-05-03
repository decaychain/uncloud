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

## Fedora/COPR build notes

Every entry below corresponds to a real failure we hit getting the COPR
pipeline green. Read this before touching `packaging/fedora/uncloud.spec.in`
or the `publish-fedora` job in `release-desktop.yml`.

### Pre-release version mapping

RPM's `Version:` field doesn't allow `-` (it's the NVR separator) or
`~` (we tried — it tilde-expanded inside our shell substitutions). The
workflow splits a tag like `v0.1.0-alpha.32` into:

```
Version: 0.1.0
Release: 0.alpha.32%{?dist}
```

`Release: 0.alpha.*` sorts lower than `Release: 1` so a future stable
`0.1.0-1` correctly supersedes any pre-release. Stable tags map to
`Version: X.Y.Z` + `Release: 1%{?dist}` directly.

### Source tarball

Built via `cp -r` + `rm` + `tar` rather than `git ls-files | tar`. The
fedora:latest container variants we tried complained "fatal: not a git
repository" because the workspace mount sat across a filesystem
boundary git refused to traverse. Plain `cp` doesn't care.

### Required BuildRequires

| Package | Why |
|---|---|
| `rust`, `cargo` | Native build of the desktop binary |
| `rust-std-static-wasm32-unknown-unknown` | Without this, `dx build` invokes `rustup target add` to install the wasm target. Fedora ships Rust as system packages, not via rustup, so the install attempt fails with `No such file or directory`. |
| `lld` | The native binary is linked with LLD instead of Fedora's default BFD. See "DT_NEEDED ordering" below. |
| `pkgconfig(openssl)` | dx pulls `openssl-sys`. Without `openssl-devel` the build fails at "Could not find openssl via pkg-config". |
| `pkgconfig(webkit2gtk-4.1)`, `gtk+-3.0`, `libsoup-3.0`, `ayatana-appindicator3-0.1` | Tauri runtime deps |
| `nodejs`, `npm` | dx invokes npm during the frontend build |
| `patchelf`, `rsvg-pixbuf-loader` | Fedora-standard build helpers |

The COPR project must have **"Enable internet access during build"**
ticked. Without it cargo and npm can't fetch dependencies inside the
mock chroot.

### dioxus-cli version pin

`cargo install dioxus-cli` without a `--version` pulls the latest from
crates.io (currently 0.7.7), which refuses to build a project pinned to
dioxus 0.7.5 with `dx and dioxus versions are incompatible!`. The spec
pins `--version 0.7.5` to match the workspace's lockfile. The same pin
lives in `Dockerfile.server`.

### RUSTFLAGS handling

Fedora's RPM build env injects `-Clink-arg=-specs=/usr/lib/rpm/redhat/redhat-package-notes`
into RUSTFLAGS. The spec file is a GCC spec format that the **native**
GCC linker understands but **lld doesn't** (`unknown argument: -specs=`).

The wasm `dx build` step links via lld unconditionally (rustc's
default for wasm32). The spec wraps it in a subshell that
`unset RUSTFLAGS CARGO_BUILD_RUSTFLAGS CARGO_ENCODED_RUSTFLAGS` so lld
never sees the spec flag. The native `cargo build` keeps RUSTFLAGS but
appends `-Clink-arg=-fuse-ld=lld` to switch the linker — see next
section.

### DT_NEEDED ordering — why we link with LLD

The COPR-built RPM crashed on Fedora 43 + NVIDIA + Wayland inside
`libnvidia-eglcore.so` calling into libdbus, with the assertion
`dbus message changed byte order since iterator was created`. The
GitHub-built RPM (which works on the same machine) has a different
DT_NEEDED ordering. `libgio-2.0.so.0` was 5th in the COPR binary's
NEEDED list and 9th in GitHub's. Library constructors run in DT_NEEDED
traversal order, so libgio (which bundles GIO + gdbus integration) ran
earlier under COPR — leaving shared dbus state that NVIDIA's libdbus
client interpreted as the wrong byte order.

Two factors put libgio earlier under BFD:

* BFD adds every `-l` flag as DT_NEEDED. LLD has `--as-needed` enabled
  by default and trims unused deps, which removes spurious entries
  (`libpango-1.0`, `libcairo-gobject`) and changes traversal order.
* BFD and LLD walk the dep graph differently.

Linking with LLD via `-Clink-arg=-fuse-ld=lld` produces a binary
structurally identical to the GitHub one (same DT_NEEDED list, same
order) and the crash goes away. `lld` 13+ supports
`--package-metadata` so the redhat-package-notes spec still works at
link time.

### Linux runtime env vars (`crates/uncloud-desktop/src/main.rs`)

Three Linux-only knobs are set in `main()` before any GTK/WebKit code
runs. Don't drop them without understanding what each closes:

* `WEBKIT_DISABLE_COMPOSITING_MODE=1` — long-standing, dodges Wayland
  protocol errors on some driver/mesa stacks.
* `NO_AT_BRIDGE=1` — skips GTK's AT-SPI accessibility bridge. Tauri
  renders inside WebKit, so the bridge wasn't carrying useful
  screen-reader content; meanwhile, on Fedora 43 it consistently
  triggered the libdbus init race that segfaulted the process.
* **`init_dbus_threads()` (`dbus_threads_init_default()` via dlopen)** —
  populates libdbus's global mutex pointers up-front. libdbus
  initializes those lazily on first use; if two consumers (atk-bridge,
  NVIDIA EGL, etc.) race in concurrently, one finds the mutex still
  NULL and segfaults inside `pthread_mutex_lock`. Calling
  `dbus_threads_init_default()` synchronously before any threading
  starts closes the window.

### Per-installation fallback key

`UNCLOUD_DESKTOP_FALLBACK_KEY` is a GitHub Actions secret used to
embed an AES-256 key for the credential-fallback feature. COPR mock
chroots can't see GHA secrets, so the build.rs no longer panics when
the env var is missing — it embeds an all-zeros sentinel and a
`FALLBACK_KEY_PROVIDED = false` flag. At runtime, `secret_store.rs`:

1. If `<secrets_dir>/fallback.key` exists, use it.
2. Else if `FALLBACK_KEY_PROVIDED` (GitHub builds), seed the file with
   the embedded key — so existing GitHub-RPM installs decrypt their
   old credentials transparently on upgrade.
3. Else (COPR builds, source builds), generate fresh 32 random bytes
   and persist.

The file is mode 0600 on Unix. Sits alongside the encrypted blobs.

### Why we ship the RPM in two places

`release-desktop.yml`'s `build-linux` job uploads the RPM directly to
the GitHub Release as an escape hatch. The COPR build is the primary
auto-update channel; the GH Release RPM is the fallback for users who
can't or won't enable COPR (or when COPR is slow — builds can sit in
the queue 15-30 min).

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
