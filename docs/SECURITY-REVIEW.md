# Security review (self-review, phase 6)

> This is an internal review by the same developer who wrote the code. It does not replace an
> external audit; one is still recommended before mainnet.

## Scope

| | |
|---|---|
| Program | `programs/sniper-tax`. Reviewed: sha256 `222ee7691fe1b9ca344f0831b5b9ead3b863a59ba38c26c5384a105a92f09c3a`, the version on devnet (`EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b`) |
| Fixes | Version with sha256 `9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd` (10 November 2026). Fixes H1, H2, L1, L2, I1 and I2. Live on devnet since 10 November 2026 (slot 520838510), checked on-chain |
| Also covered | `crates/sniper-tax-math` (the program's pricing; `programs/sniper-tax-math` since 14 November) and the interaction with the Pipeline router (pinned devnet binary) |
| Method | Every instruction and constraint walked through from an attacker's point of view. Each suspicion was, where possible, confirmed or refuted with a test against the real router. In addition: property tests, the stress test (60,000 steps) and the mutation test |

## Summary

| ID | Severity | Finding | Evidence | Status |
|---|---|---|---|---|
| H1 | High | A token's fee authority can later turn the pool into a honeypot | `tests/findings.rs` | **Fixed**: the fee authority must be `None`, the fee at most 5% |
| H2 | High | A transfer hook can block sells (honeypot) | Pipeline's own test `restrictive_hook_blocks_selling…` and `ROUTER.md` | **Fixed** for the launch token; deliberately allowed on the quote mint |
| M1 | Medium | One wallet is both upgrade authority and config admin | Configuration on devnet | Open: operational |
| M2 | Medium | By design, the creator has advantages over buyers | Design (§3 of PLAN) | Open: make it transparent |
| L1 | Low | A wrong treasury or launch fee can make every launch fail | Code analysis | **Fixed** for the launch fee; the treasury requirement is documented |
| L2 | Low | The config admin can move to an unusable address in one step | Code analysis | **Fixed**: two-step transfer |
| L3 | Low | Display extensions can mislead after launch | Code analysis | Open |
| L4 | Low | On-chain time can be off by a few seconds | Design | Accepted |
| L5 | Low | A program as creator can offer tax-free buys | Design | Accepted, to be disclosed |
| L6 | Low | Depends on the upgradeable Pipeline router | Router upgrade of 9 November | Guarded by the pinned router |
| I1–I4 | Info | Cosmetics and small points | See below | I1 and I2 fixed; I3 and I4 deliberately left as they are |

No finding lets anyone take money out of a pool **without the cooperation of the creator or the administrator**. H1 and H2 are about protecting buyers from a malicious creator. That is exactly the promise of a launchpad.

## Findings

### H1: a token's fee authority can turn the pool into a honeypot

**What:** the mint policy (`policy.rs`) allows the `TransferFeeConfig` extension, but does not check who may change the fee. Token-2022 lets the `transfer_fee_config_authority` raise the fee up to 100%. The new fee takes effect two epochs later.

**Impact:** a creator launches with a 0% fee and raises the fee to 100% after launch. After that:
- every sell yields nothing: the vault receives 0 tokens, so the seller gets 0 SOL;
- with a minimum output, every sell fails;
- buyers receive 0 tokens.

As `withdraw_withheld_authority`, the creator can collect the withheld fees. The same works with a custom **quote mint** that has a fee authority. That is less likely, because buyers must hold that coin first.

**Evidence (before the fix):** `finding_h1_a_kept_fee_authority_can_trap_sellers` in `crates/sniper-tax-tests/tests/findings.rs` (commit `34d2f3d`). After the increase, a sell with a minimum failed, and a sell without a minimum took the tokens and paid nothing. That test has now been flipped: such a token is refused.

**Recommended fix**, in `policy::check_launch_authorities` and the quote check at `launch`:
- A launch token with `TransferFeeConfig` must have `transfer_fee_config_authority == None`. The fee is then fixed for good.
- Both the current and the future fee must stay below a maximum, for example 500 bps.
- For the quote mint: the same rule, or an allowlist (WSOL, USDC).

**Fix (the owner's choice: option a, for the quote mint too):** `policy::check_transfer_fee`, called in `launch` for both mints. A mint with `TransferFeeConfig` is refused if:
- the `transfer_fee_config_authority` is not `None` (`TransferFeeAuthorityPresent`);
- the fee in force now, or a fee scheduled for a later epoch, is above 500 bps (5%) (`TransferFeeAboveCap`).

An old fee that no longer applies stays in the mint and does not count. The check runs at `launch`, the last moment before trading starts: between `create_market` and `launch` the authority can still change or revoke the fee. After that, the fee is fixed for good.

Tests in `crates/sniper-tax-tests/tests/findings.rs`, each for both the launch token and the quote mint:
- `finding_h1_a_mint_with_a_fee_authority_is_refused`: refused with a fee authority. Once the authority is revoked the pool launches, and after that nobody can raise the fee;
- `finding_h1_a_transfer_fee_above_5_percent_is_refused`: 501 bps refused, 500 bps allowed;
- `finding_h1_the_fee_in_force_and_a_scheduled_fee_both_count`: 1% now with 100% scheduled is refused. 10% now with 1% scheduled is refused until the 1% applies, and launches after that.

### H2: a transfer hook can block sells

**What:** the policy allows `TransferHook`. The hook authority can replace the hook program, and a hook program can itself be upgradeable or malicious. A hook that refuses transfers into the vault allows buys but not sells. Pipeline describes this itself ("Restrictive transfer hooks… can be bought through Pipeline but not sold", `ROUTER.md`) and tests it (`restrictive_hook_blocks_selling_unless_the_fee_account_is_whitelisted`).

**Impact:** the same honeypot as H1, but possible right from launch.

**Recommended fix:** choose one of:
1. **Refuse `TransferHook` on the launch token.** This is the recommendation for a launchpad: buyers must always be able to sell. Allow hooks only on a quote mint from an allowlist.
2. Allow only hooks with hook authority `None` and a hook program without an upgrade authority. That can be checked through the ProgramData. A hook that is malicious from the start remains possible.
3. An allowlist of known hook programs in the config.

The hook tests (`transfer_hook.rs`) stay useful for the quote side or for option 3.

**Fix (the owner's choice: option 1, without an allowlist for the quote mint):** the extension policy now differs per side (`policy::MintRole`). `TransferHook` is refused on the launch token (`ForbiddenMintExtension`), already at `create_market`, and again at `launch`. The quote mint may keep a hook.

Tests:
- `finding_h2_a_hooked_launch_token_is_refused` in `findings.rs`;
- `a_hooked_quote_mint_works_for_every_transfer` in `transfer_hook.rs`: every transfer the program makes with a hook on the quote mint (`launch`, refund, the output of a sell, `claim_tax` and `sweep_protocol_fees`), with exact counter values.

**Residual risk, accepted by the owner:** a quote mint with a hook can create the same honeypot, since a hook that refuses transfers out of the quote vault blocks the proceeds of every sell. That takes a custom quote mint with a malicious or replaceable hook, and buyers must hold that coin first. The launch UI should therefore show a quote mint with a hook clearly, or offer only well-known quote mints (WSOL, USDC).

### M1: one wallet is both upgrade authority and config admin

**What:** the program's upgrade authority and the config admin are the same wallet. On devnet that is `CUiq3QQ…`, a wallet only for devnet since 10 November; before that it was `DEwdb…`.

**Impact:** whoever holds that key can:
- replace the program with code that drains every vault, including the locked liquidity and the creators' unclaimed tax;
- redirect the treasury.

For buyers and creators, this is the biggest trust risk.

**Recommended fix:**
- For mainnet: put the upgrade authority and the admin on a multisig (for example Squads), preferably with a timelock on upgrades.
- Freeze the program after an external audit, or publish an upgrade policy.
- Make verifiable builds (`solana-verify`), as Pipeline has done since 9 November. Then anyone can check that the binary matches the source.

### M2: by design, the creator has advantages

**What:** the creator buys tax-free, also during the window. They keep the supply that is not in the pool, and can set a permanent fee of up to 5%.

**Impact:** a creator can buy cheaply while others pay 99% tax, and dump afterwards.

**Recommended fix:** this is a design choice from the original idea ("excluding the creator wallet"). Make it visible in the launch UI and in the metadata: the creator's address, the share of the supply outside the pool, and `tax_end_bps`. Optionally, creator buys during the window can be limited or forbidden.

### L1: a wrong treasury or launch fee can make every launch fail

**What:** `launch` pays the launch fee with a system transfer to `config.treasury`, which must be a `SystemAccount`.

**Impact:** two situations make every `launch` fail until the admin fixes it:
- the admin sets the treasury to an account that is not a system account;
- the treasury is empty and the fee is below the rent-exempt minimum of an empty account (0.00065 SOL on devnet on 9 November; rent can differ per cluster).

**Recommended fix:** `update_config` allows only a fee of 0 or a fee above the rent-exempt minimum. Also document that the treasury must be a system account with a balance.

**Fix:** `update_config` and `initialize_config` refuse a launch fee between 0 and the rent-exempt minimum of an empty account (`LaunchFeeBelowRent`). The program reads the cluster's rent itself (`Rent::get`), so the minimum is right on every cluster. A fee of exactly the minimum also reaches an empty treasury.

Test: `finding_l1_a_launch_fee_below_rent_is_refused` in `findings.rs`. It refuses 1 lamport and the minimum minus 1; 0 and the minimum itself are accepted, the minimum also into an empty treasury, and `initialize_config` checks the same.

Still operational: the treasury must be a system account (a wallet, or a multisig's vault PDA). A treasury owned by another program makes `launch` fail until the admin moves it.

### L2: the config admin moves in one step

**What:** `update_config` sets a new admin directly. A typo, or an address that cannot sign (a PDA), locks the config for good. The devnet tool already checks this; the program does not.

**Recommended fix:** a two-step transfer: `propose_admin` followed by `accept_admin`, signed by the new admin.

**Fix:**
- `update_config` now only changes the treasury and the fees (arguments `ConfigUpdate`, without an admin).
- `propose_admin(new_admin)` (signed by the admin) sets `pending_admin`. A new proposal replaces the previous one; `Pubkey::default()` withdraws it.
- `accept_admin` (signed by the proposed admin) makes that address the admin and clears the proposal (`NotPendingAdmin` for anyone else).

`pending_admin` takes the first 32 of the config's 64 reserved bytes. The config on devnet (all reserved bytes 0) therefore reads as "no proposal" after the upgrade. A multisig, such as a Squads vault PDA, can sign `accept_admin` through a CPI.

Test: `finding_l2_the_admin_moves_only_when_the_successor_accepts` in `findings.rs`. Only the admin proposes. A proposal for an address nobody controls changes nothing, and a new proposal replaces it. Only the proposed admin can accept, not even the old admin. Until then, the old admin stays in charge. A withdrawn proposal cannot be accepted. After the transfer, the old admin is out and the proposal is used up.

### L3: display extensions can mislead after launch

**What:** `MetadataPointer`/`TokenMetadata` (name, symbol, image), `ScaledUiAmount` and `InterestBearingConfig` are allowed. Their authorities can change what wallets and terminals show after launch. They do not affect pricing, which works on raw amounts.

**Recommended fix:** require at `launch` that those authorities are `None`, or leave it to the launch UI to show them.

### L4: on-chain time can be off by a few seconds

The tax curve uses `Clock::unix_timestamp`, which leaders set within bounds. The curve can therefore shift by a few seconds. With a window of 90 s or more, that is acceptable.

### L5: a program as creator

If `InitParams.creator` is a PDA, that program can buy tax-free for others through the router. That is the creator's choice; disclose it in the UI.

### L6: depends on the upgradeable Pipeline router

The creator exemption relies on the router passing on the signing trader. The router can be upgraded by Pipeline. `scripts/fetch-pipeline-router.sh` pins the binary, and CI stops on a change. That happened on 9 November (slot 520510501). The source was unchanged, and every test passed against the new binary.

### I1–I4: info

- **I1:** `LaunchPool::apply` uses `MathOverflow` for "a sell with tax", which cannot happen. A dedicated error code is clearer.
  **Fixed:** a dedicated error code, `TaxedSell`. No test can reach it: the maths never taxes a sell (`creator_and_sells_pay_nothing` and the property tests).
- **I2:** the constraint `destination != quote_vault` in `ClaimTax` does the same as Anchor's own duplicate check.
  **Fixed:** the constraint is gone. `the_creator_claims_their_tax_once` shows that a claim to the vault itself still fails, with `ConstraintDuplicateMutableAccount`.
- **I3:** the filter `*reserve > 0` in `Pricing::settle` can never trigger. It is the only surviving mutant in the mutation test. Deliberately kept as a defence.
- **I4:** anyone can create a creator's pool early. That is harmless: it stays Pending and only the creator can launch it. The pool does record the fees in force at that moment. Those are capped, and the launch fee is always the lower of the two.

## Checked and found sound

| Attack or risk | Why it does not work | Test |
|---|---|---|
| Bypassing the router (and with it the fee and the tax) | `pipeline_swap` requires the router's market PDA as signer | `swaps_only_come_through_the_router` |
| Posing as the creator | The router sets the trader itself, and `trade.trader` must equal account 2 | `swaps_only_come_through_the_router`, `the_creator_trades_tax_free` |
| Unclaimed tax as free input | Input counts only above the reserve plus accrued tax, capped at `amount_sent` | `a_transfer_fee_quote_mint_never_spends_unclaimed_tax` (catches the mutant) |
| Avoiding tax by splitting a buy | The tax rounds up per buy | property test `splitting_a_buy_never_saves_tax` |
| Rounding in the trader's favour | Output rounds down, exact-out input rounds up, and k never falls | property tests and the stress test |
| Donations to move the price | Reserves move only with trades and `launch` | `donations_do_not_move_the_price` and the stress test |
| Buying the pool empty | Output always stays below the reserve | `the_pool_can_never_be_emptied` |
| Creating the vaults in advance (to block) | `init_if_needed` with full ATA constraints | `vaults_created_in_advance_do_not_block_a_pool` |
| Creating the creator's pool in advance | The address includes the creator and the schedule; only the creator launches | `creating_the_creators_pool_first_gains_nothing` |
| `register_existing_market` on another account | The `Account` discriminator is checked | `register_existing_market_rejects_accounts_that_are_not_pools` |
| Claiming someone else's tax, or sending the dev fee elsewhere | `has_one = creator`; the sweep goes only to the treasury's ATA | `the_creator_claims_their_tax_once`, `protocol_tax_is_swept_only_to_the_treasury` |
| Raising fees for existing pools | Recorded per pool, hard caps, the launch fee is the minimum | `the_config_is_capped_and_changes_reach_new_pools_only` |
| Dangerous mints (permanent delegate, freeze, mint) | Policy at creation and at launch | `a_permanent_delegate_mint_is_refused`, `launch_requires_revoked_mint_and_freeze_authorities` |
| A fee that can rise after launch, or is above 5% (H1) | Fee authority `None` and a fee of at most 5%, now and scheduled, on both mints | `findings.rs` |
| A hook on the launch token (H2) | Refused at creation and at launch | `finding_h2_a_hooked_launch_token_is_refused` |
| The config admin moving to a wrong address (L2) | Two-step transfer | `finding_l2_the_admin_moves_only_when_the_successor_accepts` |
| Hook accounts that don't get through | A custom `transfer_checked` helper | `transfer_hook.rs`, on the quote mint (catches the mutant) |
| Reentrancy through a hook | Solana does not let a program that is already on the stack be called again | — |
| Overflow | u128 intermediates, `checked_*`, `overflow-checks = true` | `a_supply_near_the_u64_limit_trades_without_overflow`, `quotes_never_panic` |

## Changes after the review

**14 November 2026: the revenue model B14** (sha256 `db635b29f127a8b249e3b4a83693482abec03cf171a4b3783da0c3cec818e24f`). The cap on the protocol's share of the tax rises from 20% to 30%, and the tax on buys in the launch block (the launch slot and the next) goes to the protocol in full. Reviewed as follows:

| Question | Answer | Evidence |
|---|---|---|
| Does a trader pay or receive anything different? | No. The rate and the amounts are computed before the split; only `creator_tax` and `protocol_fee` change | `a_launch_block_buy_gets_the_same_tokens_as_any_buy_at_that_rate`, `quotes_in_the_launch_block_match_the_swap` |
| Do quote and swap agree? | Yes: both read the same `Clock` (time and slot) through `LaunchPool::quote` | `quotes_in_the_launch_block_match_the_swap` |
| Do the books still add up? | `split_tax` with a share of 10000 gives the protocol exactly the tax, the creator 0 | Property tests `the_launch_block_tax_goes_to_the_protocol_in_full`, `tax_shares_add_up`; `assert_solvent` in the integration tests |
| Existing pools? | Their reserved bytes are zero, so `launch_slot = 0`, which never triggers the rule; their recorded share is unchanged | `a_pool_launched_before_the_launch_block_keeps_its_split`; the devnet tool decodes every devnet pool with the new layout |
| Can the admin or anyone move the launch block? | No. `launch_slot` is written once, by `launch`, from the `Clock` sysvar | `the_launch_records_its_slot` |
| Overflow at the end of the slot range? | `slot - launch_slot` is computed only when `slot >= launch_slot` | Unit test with `u64::MAX` |
| Incentives | The creator gets nothing from launch-block snipes, so a creator gains nothing by tipping off or bundling snipers. The protocol gains from them, but can't change when a pool launches or which slot a trade lands in. The creator's own buys stay tax-free, in every slot | — |
| Transparency (M2) | The rule and the 70/30 split are in the metadata, the README and `INTEGRATION.md`; each pool shows its `launch_slot` and recorded share | — |

## Recommended order

1. **Before mainnet, in the program: done** (10 November 2026, sha256 `9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd`):
   - H1: fee authority `None` and a fee cap of 5%, on both mints;
   - H2: hooks on the launch token refused;
   - L1 and L2, and the info points I1–I2.

   This changed the binary. The owner upgraded devnet to it on 10 November 2026 (slot 520838510); the steps are in `HANDOFF.md`.
2. **Before mainnet, operational:**
   - M1: a multisig, a timelock or freezing, and a verifiable build;
   - M2: transparency in the UI;
   - L3: show or require the display authorities;
   - the residual risk of H2: show hooks on the quote mint, or offer only well-known quote mints.
3. **After that:** an external audit of the final version.
