# Packaging

Application ID: `io.github.BlockBreakersHQ.BlockWallet`  
Binary: `block_wallet`  
Keys live in XDG data (`~/.local/share/blockwallet/` on a normal install, or the Flatpak app data dir). The sandbox does **not** need `--filesystem=home`.

## Preflight: will it run on this phone?

The UI needs **GTK 4.6+** and **libadwaita 1.2+**. Flatpak supplies both from the runtime,
so this check only matters for a `.deb` / native install. On the phone:

```sh
pkg-config --modversion gtk4 libadwaita-1
```

PureOS **Byzantium** is Debian bullseye-based and predates libadwaita, so a native build
there cannot work. Use Flatpak.

PureOS **Crimson** satisfies the GTK side. Measured on a Librem 5 running Crimson
(`/etc/debian_version` 12.0, so bookworm-based rather than trixie):

| | Crimson ships | Needed |
| --- | --- | --- |
| GTK4 | 4.8.3 | 4.6+ |
| libadwaita | 1.2.2 | 1.2+ |
| rustc | **1.63** | **1.85** |

The Rust version is the blocker, not the GTK stack. A native or `.deb` build on Crimson needs
a rustup toolchain; `apt install rustc cargo` is 22 minor versions short.

Note also that libadwaita is exactly 1.2.2, i.e. the floor. The `v1_2` feature pin in
`Cargo.toml` is what keeps the native path buildable there, and raising it would break this
device.

## Build cost

~580 crates, including the vendored C in `secp256k1-sys` and `openssl-sys`. On the phone
itself (4 slow cores, 3 GB RAM) that is hours, and can OOM. Prefer building on a faster
aarch64 machine and copying the result over. If you must build on device, add swap and
use `cargo build -j2` to keep peak memory down.

`bdk_electrum` is deliberately pinned to the `ring` rustls provider rather than the
default `aws-lc-rs`, which would otherwise require `cmake` and build a large C library.

## Testing without risking a real wallet

`BLOCKWALLET_HOME` relocates the whole profile, so a test run cannot touch an existing
store:

```sh
BLOCKWALLET_HOME=~/blockwallet-test RUST_LOG=debug block_wallet
```

Force dark mode to check contrast (the logo, banners and QR frame all have dark-theme
handling):

```sh
ADW_DEBUG_COLOR_SCHEME=prefer-dark block_wallet
```

## Validating the packaging metadata

The app id appears in six files, and Flathub rejects mismatches with unhelpful errors. Run
the checks before submitting, and again after adding screenshots and tagging:

```sh
./scripts/validate-packaging.sh
```

It runs `appstreamcli validate --strict`, `desktop-file-validate`, a cross-file
consistency check, and confirms `data/generated-sources.json` still matches `Cargo.lock`.

On Windows this needs a Linux environment; WSL is enough, and the metadata checks do not
require flatpak itself:

```sh
sudo apt install -y appstream desktop-file-utils python3
```

## Flatpak (primary)

Runtime: **GNOME 50** (current stable; 48 went EOL on 2026-03-24, so anything below 49 is
rejected). Runtime branches are supported for about a year and EOL on the next stable
release, so re-check this before each submission. Network is required at *runtime* so
Electrum/Esplora and the Ethereum and Solana RPCs work; there is no `--filesystem=home`.

The manifests do not use the `llvm` SDK extension. It only bought faster linking via
`-fuse-ld=lld`, and pinning an extension to a specific LLVM version is one more thing that
breaks when the runtime moves.

There are two manifests:

| Manifest | Source | Network at build | Use for |
| --- | --- | --- | --- |
| `data/io.github.BlockBreakersHQ.BlockWallet.Devel.json` | working tree | yes | local iteration |
| `data/io.github.BlockBreakersHQ.BlockWallet.json` | pinned git tag | **no** | releases and stores |

### Local build from the working tree

```sh
flatpak-builder --user --install --force-clean build-dir data/io.github.BlockBreakersHQ.BlockWallet.Devel.json
flatpak run io.github.BlockBreakersHQ.BlockWallet.Devel
```

The devel app ID is separate, so it installs alongside a released copy without
overwriting it — and, importantly, without sharing its wallet data directory.

### Release build

Stores build with no network access, so every crate must be declared up front with a
checksum. `data/generated-sources.json` holds that list and is committed. **Regenerate it
whenever `Cargo.lock` changes**, or the offline build will fail on a missing or mismatched
crate:

```sh
./scripts/flatpak-gen-sources.sh      # rewrites data/generated-sources.json
```

Then:

```sh
flatpak-builder --arch=aarch64 --user --install --force-clean build-dir data/io.github.BlockBreakersHQ.BlockWallet.json
```

The manifest pins a git tag, so the tag must exist and be pushed first.

Expect a long compile. Build on a faster aarch64 host rather than the phone.

## Installing via a Flatpak repo you host

This is the route that does not depend on any store accepting the app. Export the build
into an OSTree repo, sign it, and publish the repo over https.

```sh
# 1. Build into a repo rather than installing directly
flatpak-builder --arch=aarch64 --repo=repo --gpg-sign=$KEYID --force-clean \
    build-dir data/io.github.BlockBreakersHQ.BlockWallet.json

# 2. Generate summary metadata (do this after every build)
flatpak build-update-repo repo --gpg-sign=$KEYID

# 3. Export the public key users will verify against
gpg --export $KEYID > repo/blockwallet.gpg
```

Publish `repo/` at a stable https URL (GitHub Pages works), then users run:

```sh
flatpak remote-add --if-not-exists blockwallet https://example.org/repo/blockwallet.flatpakrepo
flatpak install blockwallet io.github.BlockBreakersHQ.BlockWallet
```

`blockwallet.flatpakrepo` is a small INI file served next to the repo:

```ini
[Flatpak Repo]
Title=Block Wallet
Url=https://example.org/repo/
Homepage=https://github.com/BlockBreakersHQ/BlockWallet
Description=Self-custody crypto wallet for Linux phones
GPGKey=<base64 of repo/blockwallet.gpg, single line>
```

Sign the repo. An unsigned repo means anyone who can tamper with the transport or the
host can ship a wallet binary that steals recovery phrases.

### Single-file install

For handing a build to a tester without standing up a repo:

```sh
flatpak build-bundle repo blockwallet.flatpak io.github.BlockBreakersHQ.BlockWallet --arch=aarch64
# on the phone:
flatpak install --user ./blockwallet.flatpak
```

A bundle is unsigned and carries no update path, so use it for QA only.

## Release checklist

In order. Items marked **device** need a real Librem 5; the rest can be done anywhere.

1. **device** — Work through [LIBREM5.md](LIBREM5.md) on hardware. This is the existing
   hard gate and everything below depends on it.
2. **device** — Capture the three store screenshots into `data/screenshots/`
   (see the README there). Test networks and a throwaway profile only; never publish a
   screenshot showing a real address or balance.
3. Push the screenshots to the default branch, so the raw URLs in the metainfo resolve.
   Flathub rejects a submission whose screenshots 404.
4. Validate the metadata (must exit 0):
   ```sh
   ./scripts/validate-packaging.sh
   ```
5. Confirm the runtime branch is still supported (see above), and regenerate
   `data/generated-sources.json` if `Cargo.lock` moved.
6. Tag and push:
   ```sh
   git tag v0.1.0 && git push origin v0.1.0
   ```
   The release manifest pins this tag, so it must exist before any store build.
7. Build the release manifest for aarch64 on a fast host and install the result on the
   phone. This is the first real proof the offline vendored build works.
8. Pick a distribution route:
   - **Your own repo** - sign and publish, no third party involved. See above.
   - **Flathub** - fork `flathub/flathub`, add a branch containing
     `io.github.BlockBreakersHQ.BlockWallet.json` and `generated-sources.json` at the repo
     root, and open a PR against the `new-pr` branch. Expect review feedback.
   - **PureOS Store** - see below.

## Stores

Both stores want a Flatpak, both take submissions by merge request, and both build from a
manifest plus `generated-sources.json`. They are still two separate submissions, and the
PureOS one needs its own manifest.

### Flathub

Reaches Librem 5 owners, who can add Flathub themselves, and every other distro with flatpak
installed. Builds x86_64 and aarch64.

Process (verified against Flathub's docs, August 2026):

1. Fork `flathub/flathub` with **"Copy the master branch only" unticked**.
2. `git clone --branch=new-pr …`, then branch from `new-pr`.
3. Put `io.github.BlockBreakersHQ.BlockWallet.json` and `generated-sources.json` at the
   **repository root**. Nothing else: source code and build artefacts are prohibited.
4. Open the PR against the **`new-pr`** branch, never `master`. Title: `Add io.github.…`.
5. After merge you are invited to a new repo under the Flathub org. The invite needs 2FA and
   **expires after a week**.

Requirements worth checking before submitting, each of which has bitten this project:

- The licence must be installed to `$FLATPAK_DEST/share/licenses/$FLATPAK_ID`. All three
  manifests do this; none of them did until it was caught during submission prep.
- App ID must be a domain you control, or a code-hosting ID of at least four components.
  `io.github.BlockBreakersHQ.BlockWallet` maps to `github.com/BlockBreakersHQ/BlockWallet`.
- At least one screenshot at an https URL that actually resolves on the default branch.
- No network during the build. The release manifest complies and this is proven, not assumed.
- The runtime must be the latest available at submission time.

Two lint errors are **expected locally** and resolved by Flathub's own pipeline:
`appstream-external-screenshot-url` and `appstream-screenshots-not-mirrored-in-ostree`. Both
concern mirroring screenshots to `dl.flathub.org`, which only their builders do.

### PureOS Store

Documented at <https://storage.puri.sm/pureos-policy/publish.html>:

1. Fork <https://source.puri.sm/flatpak-apps/submission>.
2. Branch from the `submission` branch, named after the app ID.
3. Commit the flatpak manifest and anything else it needs.
4. Open a merge request against the `submission` branch.
5. After CI passes, Purism's App Curation Team tags it for inclusion, a repo is created, and
   it builds and distributes automatically.

**Being on Flathub does not put you here**, and the manifest is not reusable as-is. PureOS
requires its own runtime, `sm.puri.Platform` and `sm.puri.Sdk`.

`data/io.github.BlockBreakersHQ.BlockWallet.PureOS.json` is that variant, ready except for the
blocker below. It pins the same tag and commit as the Flathub manifest, swaps the runtime, and
installs the extra icon sizes. Rename it to `io.github.BlockBreakersHQ.BlockWallet.json` when
submitting: PureOS names the app repo after the app ID, and the Flathub linter fails a manifest
whose filename does not match the ID.

#### Blocked: the PureOS runtime is too old to build this

Measured against the live `pureos` remote
(`flatpak remote-add pureos https://store.puri.sm/repo/stable/pureos.flatpakrepo --user`):

| Ref | Latest branch | Last built |
| --- | --- | --- |
| `sm.puri.Platform` (aarch64) | 44, and `master` | **2023-03-28** |
| `sm.puri.Sdk` (aarch64) | 44, and `master` | 2023-03-28 |
| `org.freedesktop.Sdk.Extension.rust-stable` | **22.08** | 2023-05-02 |

`sm.puri.Sdk` 44 is built on freedesktop 22.08, so the only Rust toolchain available to it is
that 22.08 extension, which is stable Rust as of May 2023: approximately 1.69. This crate
declares `rust-version = "1.85"` in `Cargo.toml`, and the dependency tree (alloy 1.x, bdk 1.x)
requires it. There is roughly a two-year gap, not a marginal one.

Note that `master` is no newer than 44; both were built the same day. There is no newer branch
to target.

Three ways out, none of them quick:

- **Wait for Purism to refresh the runtime.** Nothing to do here but watch the remote. Given it
  has not moved since March 2023, this may not happen.
- **Bundle a Rust toolchain as a manifest module**, downloading a `rustup` tarball as a source
  rather than using the SDK extension. Technically possible and sometimes accepted, but it is a
  large addition, has to be vendored for an offline build, and curated stores tend to look hard
  at an app shipping its own compiler.
- **Lower the MSRV below 1.69.** Not realistic. It would mean replacing alloy and bdk.

Everything else PureOS asks for is already satisfied: GPL-3.0-or-later, CC0-1.0 metadata
licence, OARS rating, a description, screenshots, `<display_length compare="ge">360</display_length>`,
and now icons at 64x64 and 128x128 alongside the existing 256x256.

`generated-sources.json` carries over unchanged; the Rust dependency set does not care which
runtime it builds against.

## App ID

`io.github.BlockBreakersHQ.BlockWallet`.

Flathub requires an ID under a domain you control, or a code-hosting ID with at least four
components. This uses the GitHub organisation the project already publishes from, so it
needs no domain ownership proof.

The app ID decides the Flatpak data directory, `~/.var/app/<app-id>/`, which is where the
encrypted wallet store lives. **Changing it after release relocates every user's wallet**,
which is why it was settled before tagging v0.1.0. Do not change it again without a
migration path.

The devel manifest uses `…BlockWallet.Devel`, giving it a separate data directory so a
test build cannot touch a real wallet. `main.rs` reads `FLATPAK_ID` at startup so the GTK
application ID follows whichever variant is running; hardcoding it would leave the devel
build's window tagged with the release ID and showing the wrong icon in Phosh.

## Debian / PureOS sketch

Files under `packaging/debian/` are a starting point, not a complete source package.

Note the toolchain: `rustc` and `cargo` are deliberately **not** in the apt line below, because
on Crimson they are 1.63 against this crate's 1.85 floor (see Preflight). Install a toolchain
from [rustup](https://rustup.rs) instead. The GTK and libadwaita dev packages are fine.

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libssl-dev
# rustup, not apt, for the compiler:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

cargo build --release
sudo install -Dm755 target/release/block_wallet /usr/bin/block_wallet
sudo install -Dm644 data/io.github.BlockBreakersHQ.BlockWallet.desktop /usr/share/applications/io.github.BlockBreakersHQ.BlockWallet.desktop
sudo install -Dm644 data/io.github.BlockBreakersHQ.BlockWallet.metainfo.xml /usr/share/metainfo/io.github.BlockBreakersHQ.BlockWallet.metainfo.xml
sudo install -Dm644 data/icons/hicolor/256x256/apps/io.github.BlockBreakersHQ.BlockWallet.png /usr/share/icons/hicolor/256x256/apps/io.github.BlockBreakersHQ.BlockWallet.png
sudo mkdir -p /usr/share/blockwallet
sudo cp -a Images /usr/share/blockwallet/Images
```

To turn this into a `.deb`, copy `packaging/debian/` to a `debian/` directory in a source tree and run `dpkg-buildpackage -us -uc`.

Installed assets are loaded from `/usr/share/blockwallet/Images` or `/app/share/blockwallet/Images`. Wallet files stay under XDG.

## Version

Current crate version is `0.1.0`. After device QA:

```sh
git tag v0.1.0
git push origin v0.1.0
```
