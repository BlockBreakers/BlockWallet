# Roadmap

Self-custody GTK4/libadwaita wallet for the Librem 5. MVP is **BTC + ETH L1 send/receive/lock**, packaged, then QA on the phone. Solana (native + SPL, Phase 6), EVM L2s/sidechains (Phase 7), and Litecoin (Phase 8) all landed ahead of the original post-MVP schedule. Phase 9 rebuilt the UI on libadwaita's own patterns. Phase 10 was a security review of the send path and key storage; Phase 11 collects the bugs that on-device testing found afterwards.

## Run

**Linux / PureOS** (from the repo root, after GTK 4 and libadwaita dev packages):

```sh
cargo run
```

Release binary:

```sh
cargo run --release
```

**Windows** (MSYS2 mingw64 + GNU rustc). Plain `cargo run` uses MSVC and fails looking for pkg-config:

```powershell
.\scripts\run-windows.ps1
```

If the binary is already built: `target/debug/block_wallet` (or `block_wallet.exe`).

Override wallet files with `BLOCKWALLET_HOME`. Flatpak: `flatpak run io.github.BlockBreakersHQ.BlockWallet`.

---

## MVP phases

### Phase 0 — Project — **done**

- gtk4 0.10 / libadwaita 0.8; `ethers` replaced with Alloy; BTC path is **BDK 1.x** (`bdk_wallet` 1.2), not a bitcoin 0.29 bump
- Tests moved from `src/Tests` to `#[cfg(test)]` and `tests/`
- XDG paths (`directories`), `tracing` logs without secrets, hardcoded `/Users/andy/...` removed
- README, GPL-3.0-or-later, `.desktop`, AppStream
- First-launch 1inch download / 3-second sleep removed
- CSS class selectors fixed (`.label-error`, etc.)

### Phase 1 — Wallet core and security — **done**

- Encrypted store schema v1: Argon2id + ChaCha20-Poly1305 JSON (`Config.dic`)
- One BIP39 mnemonic → BTC BIP84 `bc1q`/`tb1q` + ETH BIP44 `m/44'/60'/0'/0/0`
- Onboarding: create 12/24, confirm phrase, restore, set password
- Unlock decrypts; lock wipes keys in memory and increments sync epoch
- Wallets tab hides mnemonic/private key unless reveal + password
- Clipboard timeout for secrets (later also addresses)

### Phase 2 — Bitcoin — **done**

- BDK in-memory wallet; Esplora default (`blockstream.info`) or Electrum (`ssl://` / `tcp://`)
- Sync, confirmed + pending balance, receive address + QR (works offline)
- PSBT prepare/sign/broadcast, fee tiers, bech32 validation
- History from the wallet, not `blockchain.info/getreceivedbyaddress`
- Review → confirm send UI; settings persist `btc_network` and `btc_node`

Desktop GTK send path is implemented, and Librem 5 testnet send was exercised on hardware during the v0.1.0 checklist run.

### Phase 3 — Ethereum — **done**

- Alloy HTTP provider against user RPC (Infura optional; public default if empty)
- Native ETH transfer signed locally; ERC-20 `balanceOf` / `transfer` (not Etherscan `tokentx`)
- Bundled list: ETH, USDC, USDT, DAI, WBTC; Sepolia USDC; add-by-contract
- History via `eth_getLogs` Transfer events for ERC-20s. Native transfers emit no logs, so they come from an indexer: Etherscan v2 when a key is set, otherwise Blockscout, which needs none
- Wallet address is never overwritten
- Review → confirm send with fee and token symbol; `eth_network` + RPC persist

### Phase 4 — UI and settings — **done** (verified on a Librem 5 running PureOS 11 Crimson, Aug 2026)

- Named list → detail → send stacks (no widget-tree walking)
- Home/Assets from node balances, not 1inch quotes
- Optional CoinGecko fiat (off by default)
- Trade tab replaced with **Activity**
- Settings persist: auto-lock, units, fiat, nodes
- Offline / node-unreachable banners; receive still works
- Touch-sized rows and tab bar; content clamp for a docked window

### Phase 5 — Package and harden — **done** (checklist passed on hardware; not yet git-tagged)

- Flatpak manifest: network for RPC, no `--filesystem=home`
- Debian/PureOS sketch under `packaging/debian/`
- Panic/unwrap audit on wallet startup, QR, extra-account generation
- Mainnet send requires an extra checkbox; testnet/Sepolia does not
- Clipboard timeout on addresses and keys
- Settings: **Use test networks** (BTC testnet + ETH Sepolia)
- Checklist: [LIBREM5.md](LIBREM5.md)
- Images resolved from `/app/share/blockwallet` and `/usr/share/blockwallet` when installed
- Crate version **0.1.0**. The phone checklist has passed; the git tag is still outstanding

**MVP gate: met.** [LIBREM5.md](LIBREM5.md) passed in full on a Librem 5 running PureOS 11
(Crimson) in August 2026. `git tag v0.1.0` is the one step still outstanding, and the tree has
moved on since that run — see Phases 10 and 11.

### Phase 6 — Solana (native + SPL) — **done** (verified on a Librem 5 running PureOS 11 Crimson, Aug 2026)

- One BIP39 mnemonic also derives Solana via SLIP-0010 ed25519, hardened-only `m/44'/501'/0'/0'`
- Key material: `ed25519-dalek` + `slip10_ed25519`, addresses/keys in base58 (`bs58`) — no `solana-sdk` dependency
- Raw JSON-RPC over `reqwest` (`getBalance`, `getLatestBlockhash`, `getTokenAccountBalance`, `sendTransaction`, `getSignaturesForAddress`, `getTransaction`) — same hand-rolled-protocol style as `eth_chain.rs`'s ERC-20 ABI encoding
- Hand-built legacy `Message`/`Transaction` wire format (compact-u16 arrays, fee-payer-first account ordering) signed with a single signer
- Native SOL send/receive/lock, balance sync, and best-effort Activity history (native transfers + tracked SPL transfers, derived from `preBalances`/`postBalances` and `preTokenBalances`/`postTokenBalances` deltas)
- SPL Token Program transfers, with idempotent Associated Token Account creation (hand-rolled PDA derivation) when the recipient has none yet
- Bundled: native SOL + mainnet USDC-SPL; add any other SPL token by mint address
- Settings: SOL RPC node, mainnet/devnet toggle (folded into the existing **Use test networks** switch)
- Mainnet send requires the same "I understand this spends real…" checkbox as BTC/ETH; devnet does not
- Extra accounts increment the hardened account index (`m/44'/501'/n'/0'`), matching Phantom/Solflare
- Token registry keys became chain-namespaced (`"eth:USDC"`, `"sol:USDC"`, …) so same-symbol tokens on different chains (e.g. USDC as ERC-20 vs SPL) don't collide in the token map — `Token.chain` is now the dispatch key used across Views instead of bare symbol matching
- Not yet added: fiat pricing for SPL tokens beyond native SOL, an SPL history parser for tokens outside the configured registry, a bundled SOL icon asset
- Librem 5 device QA (create/restore, receive, devnet send of SOL and the bundled SPL token) passed on hardware in the v0.1.0 checklist run

### Phase 7 — EVM L2s/sidechains — **done** (verified on a Librem 5 running PureOS 11 Crimson, Aug 2026)

- Same ETH key/address (`m/44'/60'/0'/0/0`), same Alloy provider code — added as more `eth_network` values: Arbitrum One, Base, Optimism, Polygon PoS, BNB Smart Chain, Avalanche C-Chain (chain IDs and RPCs verified against each network's own docs)
- Native gas token generalized: `eth_chain::native_symbol(network)` (ETH / POL / BNB / AVAX) now drives balance, fee, and history display instead of a hardcoded `"ETH"`
- Bundled stablecoin per network (Circle-issued native USDC on Arbitrum/Base/Optimism/Polygon/Avalanche; Binance-Peg USDT, 18 decimals, on BSC — Circle doesn't issue native USDC there)
- Switching networks now clears the previous network's bundled tokens and each wallet's ERC-20 balance cache, so a stale contract address/amount from the prior network can't linger under a reused symbol key
- Safety-check fix: the mainnet spend-confirmation checkbox was gated on `network == "mainnet"`, which hid it (and left Confirm enabled) for *any* other network name, including a new L2 — now gated on `eth_chain::is_testnet`, correct for every real-value network
- Settings: network dropdown extended to all eight networks; "Use test networks" still only touches BTC testnet / ETH Sepolia / SOL devnet — L2s are a separate, explicit choice since they're real-value networks, not test networks
- Not yet added: bridging UI (deliberately out of scope, per the original P2 note), per-L2 fiat pricing beyond ETH/BTC/SOL, Etherscan-v2-history verified end-to-end for each new chain (URL is chain-id-parameterized and should work, not independently confirmed per chain)

### Phase 8 — Litecoin — **done** (verified on a Librem 5 running PureOS 11 Crimson, Aug 2026)

- Hand-rolled, not BDK-backed: `bitcoin`/`bdk_wallet` (this project's BTC dependency) has no Litecoin variant in its `Network` enum, and the only BDK-quality alternative — LitecoinDevKit's fork — isn't on crates.io (git-only, pinned-commit). Chose to hand-roll rather than take this project's first non-released dependency.
- Reuses `bitcoin`'s network-agnostic pieces directly: BIP32 derivation (`m/84'/2'/0'/0/0` mainnet, `m/84'/1'/0'/0/0` testnet — SLIP-44 coin type 2, testnet convention 1), `Transaction`/`TxIn`/`TxOut`/`Witness` wire types, BIP143 sighash, secp256k1 signing — none of these depend on the `Network` enum for byte-level correctness, only address/WIF *text encoding* do
- Hand-written where Litecoin actually differs: bech32 `ltc1…`/`tltc1…` addresses (new `bech32` crate) and WIF with Litecoin's version byte (`0xB0` mainnet / `0xEF` testnet — reuses `sha2`/`bs58`, already dependencies from the Solana work, no new crate needed there)
- Signing correctness verified against BIP143's published "Native P2WPKH" test vector: an independent ECDSA verification (not a hardcoded "expected sighash" string) confirms the sighash computation, and RFC6979 determinism confirms the exact published signature is reproduced
- Backend: `litecoinspace.org` (Litecoin Foundation's mempool.space-fork explorer, Esplora/electrs-ltc compatible) — hand-rolled HTTP JSON calls, same style as `eth_chain.rs`/`sol_chain.rs`, not the `bdk_esplora`/`esplora-client` crates (those are built around `bdk_wallet::Wallet`, which isn't used here)
- Single reused address per wallet (mirrors how `BitcoinWallet` already behaves in practice — `peek_address(...,0)` is deterministic, no cross-session rotation), simple largest-first coin selection, exact vsize via `Transaction::vsize()` rather than an approximation formula
- Native LTC send/receive/lock, balance sync, Activity history; mainnet send gated behind the same "I understand this spends real…" checkbox as BTC/ETH/SOL
- Not yet added: RBF, an Electrum-protocol backend option (Esplora-only, unlike BTC's Esplora-or-Electrum choice), any fungible/token concept on Litecoin

### Phase 9 — Visual design pass — **done** (verified on a Librem 5 running PureOS 11 Crimson, Aug 2026)

The screens worked but were built from bare `GtkBox`/`GtkLabel`/`GtkEntry` stacks, so the app
read as assembled rather than designed. This pass rebuilt the presentation layer on
libadwaita's own patterns without changing any wallet, signing or network behaviour.

- New `src/Views/ui.rs`: single source of truth for gutters, buttons, labels, chain
  identity and toasts, so spacing no longer drifts per screen
- `src/style.css` rewritten as a design system on libadwaita's named colours
  (`@card_bg_color`, `@accent_bg_color`, …), so light mode, dark mode and the user's accent
  colour all follow the system with no second stylesheet
- Home: portfolio hero card, chain-grouped asset rows with press feedback and fiat
  sublines; Assets grouped per chain; Activity rows split into direction badge, amount and
  monospace metadata instead of one run-on string
- Settings rebuilt as an `AdwPreferencesPage`: every control now has a title and a
  subtitle. Previously it was an unlabelled column of dropdowns and entries whose only
  clue was placeholder text that vanished on first keystroke
- Send screens share one chrome (`SendChrome`) so the four chains cannot drift apart, with
  labelled `AdwEntryRow`s and a review card whose network strip is green for testnet and
  amber for real value
- Wallets shows all four chains at once in titled groups instead of hiding every account
  behind a per-chain toggle button
- Onboarding and unlock: centred lockups, step headers, seed words as numbered chips,
  `AdwPasswordEntryRow` throughout
- Toasts (`AdwToastOverlay`) confirm copies, saves and broadcasts — these were previously
  silent, with no way to tell whether a tap had registered

Fixed along the way:

- **Tab bar and account icons were broken glyphs.** `"home"`, `"wallet"`, `"assets"` and
  `"BTC"`/`"ETH"` were passed as icon-theme names; none of them resolve. All icon names are
  now checked against the shipped Adwaita theme.
- **Missing token logos rendered GTK's broken-image icon.** LTC has no bundled PNG, and
  add-by-contract tokens never will. `ui::coin_mark` falls back to a chain-coloured monogram.
- **QR codes could not scan in dark mode.** A QR is dark modules on a light field; on a dark
  background it had no quiet zone. It now sits on a deliberately white `.qr-frame` in both themes.
- **Enter did not submit the unlock password.** `connect_activate` on an `AdwPasswordEntryRow`
  compiles but binds `GtkListBoxRow`'s own activate signal, not the text field's;
  `connect_entry_activated` is the correct one.
- **Same-symbol tokens on different chains looked identical.** Rows now carry a chain tag,
  suppressed where it would only repeat the coin's own name.

Dependency: `libadwaita` gains the `v1_2` feature, for `AdwEntryRow`/`AdwPasswordEntryRow`.
Deliberately not higher — the only API above that this design wanted was `AdwSwitchRow`
(1.4), and `ui::add_switch_row` hand-builds the same thing from `AdwActionRow` +
`GtkSwitch` (including `set_activatable_widget`, so the whole row stays a hit target).
That keeps Debian bookworm (libadwaita 1.2) buildable; `packaging/debian/control` carries
the matching `>= 1.2` floor.

Not done: a bundled LTC/BNB/AVAX icon asset (the monogram covers it), and an
`AdwNavigationView` migration for the in-tab stacks.

### Phase 10 — security review and hardening — **done**

A review of the send path, key storage and network handling, followed by the fixes. Nothing
here was speculative; each item was a defect that existed in shipped code.

Fund-safety defects in the send flow:

- A reviewed plan survived changes to its own inputs. Editing the recipient after tapping
  Review left Confirm wired to the previous plan, so it sent to the old address while the
  summary the user had read said otherwise. Every input now tears the review card down
- The real-value acknowledgement was never reset, so only the first send in a visit was gated
- Plans carried no account binding. On Ethereum a plan quoted for one account could be signed
  by another whose next nonce happened to match, silently debiting the wrong wallet. Every
  chain's `PreparedSend` now names its account and signing refuses any other key

Everything a node says is now bounded: fee rates are capped, a fee larger than the amount is
refused, ERC-20 approvals are for exactly the swap amount and only to the contract being
called, and HTTP has timeouts and a response-size ceiling it previously lacked entirely.

Storage: Argon2id raised to 64 MiB, KDF parameters from disk bounded before they drive an
allocation, owner-only file permissions, and atomic writes so an interrupted save cannot
corrupt the store. Keys are wiped on `Drop` rather than only where `lock_store` reaches.

Remote endpoints must use TLS unless they point at localhost, and amount parsing refuses
ambiguous separators rather than risking a tenfold misread in comma-decimal locales.

### Phase 11 — post-QA bug fixes — **done**

Found by hardware testing after v0.1.0 passed its checklist. Recorded because each one is a
gap the checklist did not cover at the time.

- **Bitcoin never worked over Esplora.** `bdk_esplora` was built with the bare `blocking`
  feature, which compiles minreq without TLS, so every https request failed instantly with
  `HttpsFeatureNotEnabled`. The balance loop reported that as "offline", making a build-time
  misconfiguration look like a network outage for the entire life of the project
- **Bitcoin then rate-limited itself.** A full scan every 30 seconds against Blockstream's
  700-per-hour limit meant it went offline within minutes. Now 180 seconds with a smaller gap
  limit, and a failed poll keeps the last confirmed balance rather than blanking it
- **Ethereum history was silently key-gated.** Native transfers need an indexer and the code
  only called one when an Etherscan key was set, so Activity stayed empty however much ETH
  arrived or was sent. Blockscout is now the keyless fallback
- **Two default RPC endpoints were dead** (`eth.llamarpc.com` 521, `polygon-rpc.com` 401),
  which had made those networks entirely non-functional
- **Auto-lock fired during use.** The idle timer watched pointer motion and key presses, which
  a touchscreen with no keyboard never produces. It also raised the window over whatever the
  user was doing when it fired
- **The clipboard wiped addresses copied elsewhere.** Copying an address armed a timer that
  cleared whatever was on the clipboard 30 seconds later, which is why pasting a recipient
  appeared broken. Only secrets auto-clear now, and only if still present
- **The swap screen hung the app.** Clearing the offers list walked `first_child` on an
  `AdwPreferencesGroup`, whose first child is its own internal box and cannot be removed that
  way, so the loop never terminated. It ran before the empty check, so it froze on exactly the
  case where no provider quoted — which is every testnet

All errors that previously vanished into `Err(_)` are now logged with their cause. That is
what made several of these diagnosable at all.

### Phase 12 — Monero — **done in code, not yet run on the phone** (September 2026)

The first chain where a node cannot answer "what does this address hold", which forced a
different shape from the other four:

- Keys from the shared phrase by Ledger's bridge (secp256k1 `m/44'/128'/0'/0/0`, keccak256,
  reduce), pinned by a test vector; import by private spend key, with a restore height
- Scanning on the device via monero-oxide's `monero-wallet`, over a transport implemented on
  the existing reqwest client so no second HTTP or TLS stack was added; five blocks in flight
  where a node allows it, one where it resets connections; node failover mid-scan
- Scan state in an encrypted per-account cache, keyed from the view key, saved every twenty
  blocks and resumed after a kill; reorgs detected by the remembered tip hash and by each
  block's link to the last
- A new wallet records its birthday and scans from the day before it; a restored phrase
  scans a month back unless **Settings → Monero → Restore height** says otherwise
- Spend detection by key image, so a spend from another wallet holding the same keys shows;
  ten-block lock honoured in the balance; largest-first coin selection; decoys through the
  library with the output distribution memoised per send; fee rate capped and the
  fee-versus-amount rule applied; inputs marked spent at broadcast; a send that never lands
  releases them after a day
- Stagenet as the test network. No public stagenet node offers TLS, so the built-in stagenet
  defaults are the one plaintext exception, documented in the README
- Not in swaps: no venue lists XMR

Proven so far, against live public nodes from a desktop: scanning to the tip on mainnet and
stagenet; recognising a real stagenet faucet payment and holding it locked for ten blocks;
spending it in a transaction this code built and signed, which was mined and reconciled from
both ends. Nothing has moved on mainnet. See `tests/xmr.rs` for the network-gated tests. Left out on purpose: subaddresses, mempool receipts, additionally timelocked
outputs, authenticated daemons, Monero's own 25-word seed format.

**Still owed:** `data/generated-sources.json` must be regenerated on a Linux host
(`scripts/flatpak-gen-sources.sh`) before an offline Flatpak build will succeed, and the
Librem 5 checklist has new Monero boxes to run.

---

## Extended set (after MVP)

L2s shipped ahead of schedule — see Phase 7 above. Solana shipped ahead of schedule — see Phase 6 above.

### P1 — still the core wallet

- Extra accounts from the same seed; rename; hide empty
- Address book + recent recipients
- Fiat source you choose, or hide prices
- BIP39 passphrase (25th word)
- Custom ERC-20s + hide spam
- BTC RBF / cancel-replace; ETH speed-up
- Export xpub (watch-only later)
- Tor SOCKS for Electrum/RPC
- Seed reveal + encrypted backup to microSD

### P2 — remaining items

L2s (see Phase 7) and Solana (see Phase 6) already shipped. Still open: Lightning (unless BTC-only users demand it first), BIP86 Taproot, WalletConnect v2, watch-only, and an in-app bridging UI for the L2s (deliberately not built yet — Phase 7 is send/receive only, no bridge).

### P3 — swap / DeFi, no KYC — **done**

Shipped as local-signed swaps across five venues, compared side by side: LI.FI and KyberSwap
for same-chain EVM, Jupiter for Solana, THORChain and Maya Protocol for cross-chain
BTC/LTC/ETH. All non-custodial, none requiring an API key, and `api.1inch.io/v5.0` was deleted
rather than restored.

Two aggregators per EVM pair and two vault-based networks per cross-chain pair, rather than one
of each, because a single venue is a single point of both pricing and availability failure.
That was borne out during this pass: THORChain's trading halt lifted, and every one of its
public gateways then turned out to be unreachable, while Maya answered normally.

Still open here:

- No end-to-end run against either cross-chain network. THORChain's public gateways are down
  (no DNS on the default host, a Cloudflare challenge on one alternative, an expired
  certificate on another) and Maya reports `trading is halted`, so the vault-payment path is
  unit-tested against captured responses but has never moved real coins.
- Streaming swaps are quoted with THORChain's defaults rather than tuned.
- No in-app tracking of a cross-chain swap after broadcast; the outbound leg has to be checked
  on a block explorer.
- No affiliate fee is taken on any route. The hooks exist on both aggregators and both
  cross-chain networks, and the question was raised but not settled.

### Token coverage and sync cost

The bundled list went from 33 entries to about 315: roughly 275 ERC-20s across the seven EVM
networks (every token the curated source lists on four or more of them, plus the chain-specific
ones already carried) and the top 40 SPL tokens by holder count.

Nothing was bundled on the strength of a list file. All 276 EVM candidates and all 40 mints
were verified on-chain first, in bulk, and the check found real problems: FLUX listed at 18
decimals against three contracts that report 8, Polygon's gas token renamed from MATIC to POL,
and two contracts whose symbol cannot be rendered (Arbitrum USDT0 uses a non-ASCII glyph and
Arbitrum's bridged MKR reports a stringified bytes32).

A list that long is only affordable because the per-token costs were removed first:

- **Multicall3** (`currencies::multicall`) collapses every `balanceOf` into one `eth_call`, so
  a sync costs the same with 275 tokens as with 4. Verified end to end against a live node: a
  batched read reproduced, to the wei, what four separate `eth_call`s returned for the same
  account.
- **`getTokenAccountsByOwner`** does the same for Solana, one call for every holding.
- **Batched `eth_getLogs`** covers all token contracts in two queries rather than two per token.
- The Ethereum and Solana poll interval moved from 20s to 60s.

Still open here:

- Token-2022 mints are excluded. Four of the top forty SPL tokens use it (CASH, PUMP, ANSEM,
  CATE), but `find_associated_token_address` and the transfer instruction both assume the
  classic program id, and Token-2022 uses its own as an ATA seed. Bundling one would show a
  balance that could not be spent.
- ~~The Assets screen renders every bundled token as a row.~~ It never did: the ERC-20 and SPL
  loops already skipped zero balances, so only the four native rows persisted when empty.
  **Settings -> Hide empty assets** now covers those too, and the searchable picker handles the
  swap screen where the list really is long.
- No periodic refresh of the bundled list. It is a snapshot, and tokens get renamed.

### P4 — hardware / platform

- USB-C hardware wallet
- Librem Key / OpenPGP unlock 2FA if the SKU has a reader
- Phosh receive notifications
- Docked sidebar vs handheld bottom bar

### P5 — maybe never

NFT gallery, fiat on-ramp, social recovery, built-in full node (3 GB / 32 GB).
