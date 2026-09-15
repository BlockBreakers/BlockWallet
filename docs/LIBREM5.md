# Librem 5 test checklist

A **per-release** gate, not a one-off. Reset the boxes for each version under test and work
through it again on real hardware; completed runs are recorded at the bottom.

Run it on a real Librem 5 (Phosh, aarch64, ~3 GB RAM). Prefer **Bitcoin testnet**, **Ethereum
Sepolia**, **Solana devnet**, **Litecoin testnet**, and **Monero stagenet**. Do not use mainnet funds.

Install with Flatpak (`docs/packaging.md`) or a `.deb`. Building natively on the phone takes
hours, and Crimson's own `rustc` is too old for this crate; see the packaging notes.

**Currently under test:** the untagged tree after v0.1.0 — the swap feature, the security
hardening, and the fixes that followed the v0.1.0 run. The crate version is still `0.1.0`;
the next version number has not been decided.

## 1. Install

- [ ] App appears in the app grid as **Block Wallet**
- [ ] Icon and `.desktop` `X-Purism-FormFactor=Mobile` are present
- [ ] First launch opens at about 360×720, not a desktop-only layout
- [ ] Touch targets (Unlock, tab bar, Send/Receive) are usable with a finger
- [ ] Tab bar shows five real icons (Home / Wallets / Assets / Swap / Activity), not missing-image glyphs
- [ ] Switch Phosh to dark mode: cards, banners and text all follow it, and the receive QR keeps a white frame that still scans
- [ ] **Settings → Display → Appearance**: Light and Dark apply immediately, and "Follow system" tracks a Phosh theme change while the app is open
- [ ] The chosen appearance survives a restart, including on the unlock screen itself
- [ ] Change the system accent colour: buttons and the balance card follow it

## 1b. Assets and refresh cadence

The bundled token list is now around forty entries and the Ethereum and Solana polls were
slowed from twenty seconds to sixty to pay for it. Both are worth eyeballing on the device,
since a long list is a layout problem and a slow poll is a "did that work?" problem.

- [ ] Home on an empty wallet lists all four chains; the hero subtitle reads "4 assets across
      4 chains"
- [ ] Once one chain has a balance, Home lists only that one, and the subtitle reads
      "1 asset across 4 chains" (singular, not "1 assets")
- [ ] With a balance held, turn the radios off: no row vanishes, because an unreachable node
      reports zero without meaning it
- [ ] Immediately after unlock, Home does not drop and re-add rows as each chain reports in
- [ ] Assets lists the four native assets plus any token with a non-zero balance, each with a
      symbol, a name and a balance, and scrolls cleanly at 360 px
- [ ] Settings -> **Hide empty assets** on: empty assets disappear, and a "Show N empty assets"
      button appears at the bottom of the list
- [ ] Tapping that button reveals them for this visit only; leaving Assets and coming back
      hides them again, and the Settings switch is still on
- [ ] With every asset empty and the switch on, the screen explains that rather than going blank
- [ ] Turn the radios off with the switch on: nothing disappears, because an unreachable node
      reports zero and must never be treated as a confirmed zero
- [ ] Immediately after unlock, assets do not flicker in and out as balances arrive (a syncing
      row is not hideable either)
- [ ] Avalanche: bridged assets show their real symbols (WETH.e, LINK.e, WBTC.e) rather than
      implying the native token
- [ ] Polygon: the native balance reads POL, not MATIC, while the bridged MATIC ERC-20 on
      mainnet, BSC and Arbitrum still reads MATIC
- [ ] No token row shows a placeholder or a broken icon
- [ ] Switching the ETH network replaces the token list rather than appending to it, and no
      stale balance from the previous network is left showing
- [ ] After receiving, the balance updates within about a minute without the app being restarted
- [ ] Leave the app open on Assets for ten minutes: balances keep updating and nothing starts
      reading "offline" (the slower cadence exists to stop the wallet rate-limiting itself)

## 2. Onboard

- [ ] Create 12-word wallet: write down phrase, confirm, set password
- [ ] Force-quit and reopen: unlock screen, not a second create flow
- [ ] Restore on a throwaway profile with a known test phrase; BTC address is BIP84 `tb1q`/`bc1q`, ETH is `0x…`, SOL is a base58 address, LTC is `ltc1q…`/`tltc1q…`, and XMR is a 95-character address starting `4` (mainnet) or `5` (stagenet)
- [ ] Lock wipes keys from the UI; Wallets does not show the mnemonic until Reveal + password

## 3. Receive (radios on and off)

- [ ] Settings → **Use test networks** → Save
- [ ] Bitcoin receive: address + QR visible
- [ ] Ethereum receive: address + QR visible
- [ ] Solana receive: address + QR visible
- [ ] Litecoin receive: address + QR visible
- [ ] Monero receive: address + QR visible, and the balance row reads "Syncing…" with a percentage on a fresh account rather than "offline"
- [ ] Enable airplane mode / kill switches: receive address and QR still show
- [ ] Banner says the node is unreachable / receive still works
- [ ] Copy an address, then copy something else in another app: the second value is still on the clipboard a minute later (addresses must not auto-clear)
- [ ] Reveal the recovery phrase and copy it: the clipboard clears within 30 seconds, but only if nothing else was copied in the meantime
- [ ] Paste into a Send recipient field works
- [ ] Each receive QR is large enough to scan from another phone at arm's length

## 4. Send (testnet)

- [ ] Fund the testnet BTC address from a faucet; wait for a confirmation
- [ ] Send a small amount: Review → summary shows **testnet** → Confirm and broadcast
- [ ] The sent transaction appears in **Activity** with no Etherscan or other API key configured
- [ ] Mainnet send requires the “I understand this spends real bitcoin” checkbox (spot-check by turning test networks off; do not broadcast)
- [ ] Tap Review, then change the recipient or amount: the review card disappears rather than staying armed with the old plan
- [ ] Send twice in one visit to the screen: the acknowledgement checkbox is unticked again for the second send
- [ ] ETH Sepolia: send a tiny amount of ETH the same way
- [ ] ETH recipient accepts an **ENS name**; the review card shows the resolved `0x…` address, not the name
- [ ] Optional: send a Sepolia ERC-20 if you have a test token
- [ ] SOL devnet: airdrop with `solana airdrop`, send a tiny amount back; Review → summary shows **devnet**
- [ ] Optional: send the bundled devnet USDC-SPL (or a token added by mint address) to confirm the associated-token-account creation path works
- [ ] Settings → switch the ETH network dropdown to an L2 (Arbitrum/Base/Optimism/Polygon/BSC/Avalanche): balance row shows the correct native symbol (POL/BNB/AVAX where applicable, ETH otherwise), and the "I understand this spends real value" checkbox is visible and gates Confirm on send
- [ ] LTC testnet: fund from a faucet, send a small amount back; Review → summary shows **testnet**; mainnet send requires the "I understand this spends real litecoin" checkbox (spot-check by turning test networks off; do not broadcast)
- [ ] XMR stagenet: fund from https://stagenet-faucet.xmr-tw.org; the payment shows as **pending** for ten blocks, then moves into the balance; send a small amount back once unlocked; Review → summary shows **stagenet** and the input count; mainnet send requires the "I understand this spends real monero" checkbox (spot-check by turning test networks off; do not broadcast)
- [ ] XMR: force-quit the app mid-scan and reopen; the scan resumes from where it was rather than restarting (the percentage does not fall back to 0)
- [ ] XMR: Settings → Monero → Restore height set to a block before the faucet payment, Save; the account rescans from there and finds the payment again exactly once
- [ ] XMR: memory stays flat during a scan of a few hundred blocks (watch resident size; a public node answering with oversized blocks must not grow it without bound)

## 5. Swap

Be clear about what is testable where. On testnets every provider correctly declines: neither
aggregator has Sepolia liquidity, Jupiter has none on devnet, and both cross-chain networks
refuse outright. The testnet rows therefore verify the *refusal* path, which is where the
v0.1.0 UI freeze lived. Real quoting needs mainnet.

Cross-chain is currently unquotable at all, for two separate reasons worth telling apart when
reading a failure on the device: THORChain has resumed trading but none of its public gateways
answer (no DNS on the default host, a Cloudflare challenge on one alternative, an expired
certificate on another), while Maya answers and reports its own trading halt. So the expected
mainnet result today is that the two EVM aggregators quote and the two vault venues decline
with different messages.

- [ ] Swap tab opens; From and To are searchable rows, not dropdowns
- [ ] Tapping one opens a search dialog listing every bundled token for the selected network
      (about 275 on Ethereum mainnet, 40 on Solana), scrolling smoothly at 360 px
- [ ] Typing filters the list; a symbol ("usdc") and a chain name ("solana") both narrow it
- [ ] The currently selected token is ticked when the dialog opens, and Cancel leaves it alone
- [ ] Choosing a token closes the dialog, updates the row, and clears any shown offer
- [ ] On testnet, **Find the best rate** returns promptly and does **not** hang the app
- [ ] It reports that no provider could route the swap, and lists why each declined, naming all
      five venues rather than silently dropping the ones that failed
- [ ] Changing the amount or either asset clears any shown offer
- [ ] Mainnet, small amount, ETH-family pair: LI.FI and KyberSwap both quote, and offers are
      ordered best-output-first
- [ ] Mainnet, BTC to ETH: THORChain reports a gateway failure and Maya reports a halt; neither
      shows as simply "unavailable"
- [ ] A vault-based offer is labelled as vault-held; an aggregator offer is labelled as settling
      in one transaction
- [ ] The review card shows the minimum received, and Confirm is gated behind the acknowledgement
- [ ] Optional (mainnet, real value): execute a small same-chain swap and confirm it lands

## 6. Lock and settings

- [ ] Header **Lock** returns to Unlock
- [ ] Wrong password stays locked
- [ ] Auto-lock fires after idle
- [ ] Auto-lock does **not** fire while the app is in use by touch alone (tap around for longer than the timeout)
- [ ] When auto-lock fires while another app is in front, Block Wallet does **not** raise itself
- [ ] Change BTC Electrum/Esplora URL, ETH RPC, SOL RPC, LTC Esplora URL, and XMR daemon URL; Save; a toast confirms and balances refresh or show offline honestly
- [ ] A plaintext `http://` endpoint is rejected unless it points at localhost
- [ ] Every Settings row has a readable title and subtitle at 360 px wide; nothing is clipped or requires horizontal scrolling
- [ ] Header shows the LIVE / TEST NETWORKS chip and it matches the network actually in use
- [ ] Fiat prices stay optional (CoinGecko); send/receive work with prices off

## 7. Kill switch / privacy

- [ ] With WWAN/Wi-Fi off, **Home** does not panic and shows the unreachable-node banner
- [ ] With WWAN/Wi-Fi off, **Activity** does not panic (bring it to the foreground during the outage)
- [ ] Balances show the last known figure marked offline rather than blanking to nothing
- [ ] No mnemonic or private key in `~/.local/state/blockwallet/blockwallet.log` (or the Flatpak equivalent under `~/.var/app/io.github.BlockBreakersHQ.BlockWallet/`)

## 8. Package

- [ ] aarch64 Flatpak **or** `.deb` installs and launches
- [ ] Uninstall does not leave keys outside XDG / Flatpak app data

---

## Completed runs

### v0.1.0 — 25 to 26 August 2026, PureOS 11 (Crimson)

All 41 boxes of the then-current checklist passed on a Librem 5 running Crimson
(`/etc/debian_version` 12.0, kernel 6.12.0-1-librem5), installed as an aarch64 Flatpak and
driven over SSH with the device attached by USB. `grim` was used for screen capture, which
works on Crimson's compositor where it did not on Byzantium.

Two caveats from that run, recorded rather than smoothed over:

- **Activity was never displayed while offline.** The app was proven to survive 75+ seconds
  fully offline (`wlan0` down, no route) with zero panics, and Home was captured rendering its
  unreachable-node banner correctly. But Activity itself was never brought to the foreground
  during the outage. It reads the same history `Arc` that the balance path writes and makes no
  network calls of its own, so a panic there but not in Home is unlikely — that is reasoning,
  not observation. It is now its own row in section 7 so the next run covers it explicitly.
- **Swaps were not covered at all.** The checklist predated the feature, which is why a UI
  freeze on the swap screen reached a human tester instead of a test. Section 5 exists now.

Bugs found *after* that run finished, none of which the checklist as it then stood would have
caught: the swap-screen freeze, Bitcoin rate-limiting itself into a permanent "offline",
Ethereum history silently requiring an API key, auto-lock firing during touch-only use, the
clipboard wiping addresses copied in other apps, and two dead default RPC endpoints. Rows in
sections 3 through 7 were added to cover each of them.
