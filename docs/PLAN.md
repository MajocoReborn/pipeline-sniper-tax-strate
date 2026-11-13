# Pipeline Sniper Tax: design plan

> Status: **built and on devnet;** this is the design, with progress in §10. Based on Pipeline commit `a2d5bfb`
> (`MetalLegBob/pipeline`, "Router verification and security audit, with the hardened router").

## 1. Goal

An Anchor program for Pipeline that protects new tokens from snipers:

- A buy right after launch pays **99% tax**.
- That tax **falls to 0%** over a configurable window, for example **90 seconds (1.5 min)** or **600 seconds (10 min)**.
- The **creator's wallet is exempt**.
- Full support for **SPL Token and Token-2022** through `anchor_spl::token_interface`, with strict account validation.

## 2. Why a Pipeline *strategy* (and not a Token-2022 hook or transfer fee)

| Option | Does it work? | Reason |
|---|---|---|
| Token-2022 **TransferFee** extension | No | A new fee only takes effect 2 epochs later (a few days). A fall from 99% to 0% within minutes is therefore impossible. The fee also applies to *every* transfer, including ordinary wallet transfers. |
| Token-2022 **TransferHook** | No | A hook can only approve or refuse a transfer, not withhold tax. It also runs on every transfer. |
| "Wrapper" strategy around another AMM | No | That becomes router → wrapper → AMM → token program → hook, 5 levels. Solana allows 4, so it fails with hooked tokens. |
| **Own strategy: a launch pool with a built-in sniper tax** | **Yes** | The strategy sets the price itself and reduces the effective input by the tax. Every integration that uses Pipeline (bots, terminals, charts) sees and quotes the pool automatically. |

Pipeline facts the design relies on (checked in the code):

1. The router's `swap` requires `trader: Signer` and passes **exactly that key** as account index 2 and as `TradeRequest.trader` (`programs/pipeline/src/lib.rs`, lines 477, 493 and 666). The market PDA signs the call. If the strategy checks the market signer, `trader` can therefore be trusted. The creator exemption rests on this.
2. The creator's signature **never** reaches `pipeline_init_market`. That is why there is a separate `launch` instruction that the creator signs (see §4).
3. Strategy Guide rule 10 says input stays in the input vault until the end of the swap. The tax therefore physically stays in the quote vault and is tracked separately (`accrued_tax`).
4. Strategy Guide rule 2 prescribes `credited_input(vault − reserve, amount_sent)`. We must **also subtract the accrued tax** (see §8, attack A1).

## 3. Mechanism

### 3.1 Lifecycle of a pool

```
create_market (router, permissionless)      launch (creator signs)             claim_tax (creator)
 ┌──────────────────────────┐   ┌────────────────────────────────┐   ┌──────────────────┐
 │ pool = Pending           │──▶│ deposit token + quote          │──▶│ tax → creator    │
 │ vaults created           │   │ launch_ts = Clock.unix_ts      │   │ (any number of   │
 │ params fixed in PDA seeds│   │ pool = Live, tax clock starts  │   │  times)          │
 └──────────────────────────┘   └────────────────────────────────┘   └──────────────────┘
```

Recommended: `create_market` + 2× `init_fee_account` + `launch` in **one transaction**. That is atomic, so nobody can get in between the steps. A tax-free "dev buy" by the creator can be added to it.

### 3.2 Tax curve (linear by default)

```
elapsed = max(0, now − launch_ts)
tax_bps(elapsed) = end_bps                                               if elapsed ≥ duration
                 = end_bps + (start_bps − end_bps) · (duration − elapsed) / duration   (floor)
```

With `start_bps = 9900`, `end_bps = 0`:

| Elapsed | Duration 90 s | Duration 600 s |
|---|---|---|
| 0 s | 99.0% | 99.0% |
| 30 s | 66.0% | 94.05% |
| 45 s | 49.5% | 91.57% |
| 60 s | 33.0% | 89.1% |
| 89 s | 1.1% | 84.31% |
| 300 s | 0% | 49.5% |
| 540 s | 0% | 9.9% |
| ≥ duration | 0% | 0% |

The curve only falls (property tested). A swap that lands later than its quote is therefore always equal or **better** for the buyer. Quotes with a stale clock are thus conservative.

### 3.3 What the tax applies to

- **Buys only** (quote mint → launch token), on the **input** in the quote mint (SOL or USDC).
- Exact-in: `tax = credited − floor(credited · (10000 − bps) / 10000)`. The tax rounds up; `effective = credited − tax` goes into the constant-product curve.
- Exact-out: the required `effective = in_given_out(...)`, then `gross = ceil(effective · 10000 / (10000 − bps))`. Because `bps ≤ 9900`, this never divides by 0. A property test shows that `floor(gross·(10000−bps)/10000) ≥ effective`.
- Sells: no tax (decision B2).
- **Creator exempt:** if `trader == pool.creator`, then `bps = 0`.
- Pipeline's own 0.25% fee comes on top of this. The router charges it on the whole amount the trader spent, tax included.

Example: a sniper buys for 10 SOL at t = 0. 9.9 SOL goes to the creator tax, 0.1 SOL into the curve, and the sniper also pays 0.025 SOL Pipeline fee.

### 3.4 Where the tax goes (by default: the creator)

- The tax stays in the quote vault, with `pool.accrued_tax_quote += tax`. It does **not** count towards `reserve_quote`, so the price stays clean.
- The custom action `claim_tax` (signed by the creator) moves `accrued_tax_quote` to a quote token account of their choice.
- There is **no liquidity withdrawal**: the launch liquidity is locked for good (the equivalent of "LP burned"). The creator therefore cannot drain the pool (rug).

### 3.5 Revenue model for the dev (B9: a share of the tax plus a small launch fee)

Solana users watch costs closely, so the fees stay low and visible:

| Source | Default | Hard cap in code | Who pays |
|---|---|---|---|
| **A. Share of the sniper tax** | 10% of every withheld tax | 20% | Out of the creator's part of the tax. The sniper pays nothing extra, and neither do ordinary buyers after the window |
| **B. Launch fee** | 0.01 SOL per `launch` | 0.05 SOL | The creator, once. For comparison: the rent for the pool and vaults together is about 0.007 SOL |

How it works:

- A global `ProtocolConfig` PDA (`["config"]`) holds `admin`, `treasury`, `protocol_tax_share_bps` and `launch_fee_lamports`. The program's upgrade authority creates it once. After that, the `admin` may change the values, but only within the caps.
- **Recorded per pool:** `pipeline_init_market` copies the tax share and the launch fee into the pool. A later config change affects new pools only.
- The launch fee is `min(pool.launch_fee_lamports, config.launch_fee_lamports)`. The creator therefore never pays more than applied at creation, and benefits from a later reduction (for example a 0 SOL promotion).
- The tax share: `protocol = floor(tax · share / 10000)` and `creator = tax − protocol`. Rounding favours the creator. Both amounts stay in the quote vault (Strategy Guide rule 10), as `accrued_protocol_quote` and `accrued_tax_quote`.
- `sweep_protocol_fees` is permissionless and sends `accrued_protocol_quote` only to the ATA of `config.treasury`, just like Pipeline's own `sweep_fees`.
- `pipeline_quote` uses the total tax. The share changes nothing about what the buyer receives, so quotes stay correct.

Example: a sniper buys for 10 SOL at t = 0. The tax is 9.9 SOL, of which 0.99 SOL goes to the dev treasury and 8.91 SOL to the creator. The sniper also pays 0.025 SOL to Pipeline.

## 4. State

```rust
#[account]
#[derive(InitSpace)]
pub struct LaunchPool {
    pub version: u8,                 // for future migrations
    pub status: PoolStatus,          // Pending | Live
    pub bump: u8,
    pub creator: Pubkey,             // exempt + may launch/claim
    pub token_mint: Pubkey,          // mint index 0 in MarketInfo
    pub quote_mint: Pubkey,          // mint index 1
    pub token_vault: Pubkey,         // ATA(pool, token_mint, token_program_token)
    pub quote_vault: Pubkey,         // ATA(pool, quote_mint, token_program_quote)
    pub market_signer: Pubkey,       // PDA(["market", ID, pool], PIPELINE)
    pub reserve_token: u64,
    pub reserve_quote: u64,
    pub accrued_tax_quote: u64,      // creator's part, in quote_vault, NOT in reserves
    pub accrued_protocol_quote: u64, // dev's part, in quote_vault, NOT in reserves
    pub tax_start_bps: u16,          // ≤ 9900
    pub tax_end_bps: u16,            // ≤ MAX_END_BPS (anti-honeypot cap)
    pub tax_duration_secs: u32,      // within [MIN_DURATION, MAX_DURATION]
    pub protocol_tax_share_bps: u16, // recorded at init, ≤ 2000
    pub launch_fee_lamports: u64,    // recorded at init, ≤ 0.05 SOL
    pub launch_ts: i64,              // 0 while Pending
    pub _reserved: [u8; 64],
}

#[account]
#[derive(InitSpace)]
pub struct ProtocolConfig {          // PDA ["config"]
    pub admin: Pubkey,               // may change settings within the caps (multisig recommended)
    pub treasury: Pubkey,            // receives launch fees and the tax share
    pub protocol_tax_share_bps: u16, // default 1000 (10%)
    pub launch_fee_lamports: u64,    // default 10_000_000 (0.01 SOL)
    pub bump: u8,
    pub pending_admin: Pubkey,       // proposed admin (propose_admin); default = none
    pub _reserved: [u8; 32],         // pending_admin took the first 32 of the 64 bytes
}
```

**PDA seeds** (Strategy Guide rule 8: parameters in the address, so a pool with bad parameters cannot occupy the address of a good one):

```
["launch", token_mint, quote_mint, creator, params_bytes(start_bps, end_bps, duration)]
```

`InitParams` (Borsh, in `init_data`): `{ creator: Pubkey, tax_start_bps: u16, tax_end_bps: u16, tax_duration_secs: u32 }`. Seed helpers cut the data into `creator` (32 bytes) and `params` (8 bytes), because a single seed may be at most 32 bytes. Data of the wrong length is refused.

## 5. Instructions and account validation

All token accounts are `InterfaceAccount<'info, Mint/TokenAccount>`, all token programs `Interface<'info, TokenInterface>`. Every transfer is `transfer_checked`, with the transfer-hook accounts (remaining accounts) passed on.

### 5.1 `pipeline_init_market(data: Vec<u8>)`: router, from `create_market`

| Account | Type | Constraints |
|---|---|---|
| `market_signer` | `Signer` | handler: `== PDA(["market", crate::ID, pool], PIPELINE_PROGRAM_ID)` |
| `pool` | `Account<LaunchPool>` | `init, payer = payer, space = 8 + INIT_SPACE, seeds = [...§4], bump` |
| `payer` | `Signer`, `mut` | the router's setup payer. Pays rent only, never reassigned |
| `system_program` | `Program<System>` | |
| `token_mint` | `InterfaceAccount<Mint>` | `mint::token_program = token_program_token` |
| `quote_mint` | `InterfaceAccount<Mint>` | `mint::token_program = token_program_quote`; handler: `!= token_mint` |
| `token_vault` | `InterfaceAccount<TokenAccount>` | `init_if_needed, associated_token::{mint = token_mint, authority = pool, token_program = token_program_token}` |
| `quote_vault` | likewise | likewise for quote |
| `token_program_token`, `token_program_quote` | `Interface<TokenInterface>` | |
| `associated_token_program` | `Program<AssociatedToken>` | |
| `config` | `Account<ProtocolConfig>` | `seeds = [b"config"], bump = config.bump` |

Handler: validates the params (`start ≤ 9900`, `end ≤ start`, `end ≤ MAX_END_BPS`, duration within bounds) and the mint policy (§6), copies the tax share and the launch fee from `config`, and sets `status = Pending`.

### 5.2 `pipeline_market_info()`

`pool: Account<LaunchPool>`. Anchor checks the owner and the discriminator, so `register_existing_market` on an arbitrary account of our program fails. Returns `MarketInfo { mints: [token, quote], vaults: [token_vault, quote_vault], supports_exact_out: true }` (decision B6).

### 5.3 `pipeline_resolve_accounts(ResolveRequest)` and `pipeline_quote(TradeRequest)`

Read-only. `pool: Account<LaunchPool>`; the other 9 fixed accounts are `UncheckedAccount` (placeholders, never read).

- Resolve returns `[output_vault (writable)]`, with `complete: true`.
- Quote calls `pool.quote(&trade, Clock::get()?.unix_timestamp)`. That is **the same function** as in swap. A Pending pool or an empty reserve gives an error.

### 5.4 `pipeline_swap(SwapRequest)`: router, from `swap`

| Account | Type | Constraints |
|---|---|---|
| `market_signer` | `Signer` | `address = pool.market_signer @ NotPipeline` (only through the router) |
| `pool` | `Account<LaunchPool>`, `mut` | `constraint = pool.status == Live @ NotLive` |
| `trader` | `UncheckedAccount` | identity; handler: `request.trade.trader == Some(trader.key())` |
| `trader_input`, `trader_output` | `UncheckedAccount`, `mut` | the token program checks them (the router measures balances) |
| `input_vault` | `UncheckedAccount`, `mut` | handler: `== pool.vault(input_index)` |
| `input_mint`, `output_mint` | `InterfaceAccount<Mint>` | handler: `== pool.mint(index)` (defence in depth; the router checks this too) |
| `input_token_program`, `output_token_program` | `Interface<TokenInterface>` | `== mint.owner` |
| remaining[0] | output vault | `== pool.vault(output_index)` |
| remaining[1..] | hook accounts | passed on to `transfer_checked` |

Handler, in this order:

1. `credited = credited_input(vault_balance, reserve_in + accrued_tax_in + accrued_protocol_in, amount_sent)`
2. `quote = pool.quote(trade, now)`, with the tax, the creator check and the CP maths
3. `require!(quote.amount_in ≤ credited)`, `refund = credited − quote.amount_in`
4. Refund and output through `transfer_checked`, signed by the pool PDA
5. `reserve_in += effective_in`, `accrued_protocol_in += protocol_share`, `accrued_tax_in += tax − protocol_share`, `reserve_out −= amount_out`. Updates come from the trade itself, not from vault balances, so donations stay out of the price.

### 5.5 `launch(amount_token: u64, amount_quote: u64)`: custom action, signed by the creator

| Account | Type | Constraints |
|---|---|---|
| `creator` | `Signer` | |
| `pool` | `Account<LaunchPool>`, `mut` | `has_one = creator @ NotCreator`, `has_one = token_mint`, `has_one = quote_mint`, `has_one = token_vault`, `has_one = quote_vault`, `constraint = status == Pending @ AlreadyLaunched` |
| `token_mint` / `quote_mint` | `InterfaceAccount<Mint>` | `mint::token_program = …` |
| `creator_token` | `InterfaceAccount<TokenAccount>`, `mut` | `token::mint = token_mint, token::authority = creator, token::token_program = token_program_token` |
| `creator_quote` | likewise | likewise for quote |
| `token_vault`, `quote_vault` | `InterfaceAccount<TokenAccount>`, `mut` | through `has_one` |
| token programs | `Interface<TokenInterface>` | |
| `config` | `Account<ProtocolConfig>` | `seeds = [b"config"], bump = config.bump` |
| `treasury` | `SystemAccount`, `mut` | `address = config.treasury @ WrongTreasury` |
| `system_program` | `Program<System>` | |
| remaining | hook accounts | |

Handler:

1. Both amounts must be greater than 0.
2. The mint policy is checked again. The freeze and mint authorities may have changed since init, and so may a transfer fee. For both mints: no fee authority, and the fee in force and a scheduled fee at most 5% (§6).
3. The launch fee `min(pool.launch_fee_lamports, config.launch_fee_lamports)` goes by system transfer from `creator` to `treasury` (skipped at 0).
4. What is credited is **what arrived**: the vault balance after minus before, which handles transfer-fee tokens.
5. `launch_ts = Clock.unix_timestamp`, `status = Live`.

### 5.6 `claim_tax()`: custom action, signed by the creator

| Account | Type | Constraints |
|---|---|---|
| `creator` | `Signer` | |
| `pool` | `Account<LaunchPool>`, `mut` | `has_one = creator`, `has_one = quote_mint`, `has_one = quote_vault`, `constraint = accrued_tax_quote > 0 @ NothingToClaim` |
| `quote_mint` | `InterfaceAccount<Mint>` | `mint::token_program = token_program_quote` |
| `quote_vault` | `InterfaceAccount<TokenAccount>`, `mut` | through `has_one` |
| `destination` | `InterfaceAccount<TokenAccount>`, `mut` | `token::mint = quote_mint, token::token_program = token_program_quote` |
| `token_program_quote` | `Interface<TokenInterface>` | |

Handler: sets `accrued_tax_quote` to 0 **before** the transfer (checks-effects-interactions) and moves the amount with `transfer_checked`.

### 5.7 `sweep_protocol_fees()`: custom action, anyone may call it

| Account | Type | Constraints |
|---|---|---|
| `pool` | `Account<LaunchPool>`, `mut` | `has_one = quote_mint`, `has_one = quote_vault`, `constraint = accrued_protocol_quote > 0 @ NothingToClaim` |
| `config` | `Account<ProtocolConfig>` | `seeds = [b"config"], bump = config.bump` |
| `quote_mint` | `InterfaceAccount<Mint>` | `mint::token_program = token_program_quote` |
| `quote_vault` | `InterfaceAccount<TokenAccount>`, `mut` | through `has_one` |
| `treasury_quote` | `InterfaceAccount<TokenAccount>`, `mut` | `associated_token::{mint = quote_mint, authority = config.treasury, token_program = token_program_quote}` |
| `token_program_quote` | `Interface<TokenInterface>` | |

Handler: sets `accrued_protocol_quote` to 0 and moves the amount to the treasury. Because the destination is fixed, permissionless is safe.

### 5.8 `initialize_config(...)` and `update_config(...)`: administration

- `initialize_config`: only the program's upgrade authority, once. Checked through `program.programdata_address() == Some(program_data.key())` and `program_data.upgrade_authority_address == Some(signer)`. Creates `config` with `init, seeds = [b"config"], bump`.
- `update_config(ConfigUpdate)`: `has_one = admin`. Validates `protocol_tax_share_bps ≤ 2000` and `launch_fee_lamports ≤ 0.05 SOL`. A launch fee is 0 or at least the rent-exempt minimum of an empty account (`Rent::get`); otherwise every launch into an empty treasury fails. Changes the `treasury` and the fees, not the admin. `initialize_config` validates the same.
- `propose_admin(new_admin)`: `has_one = admin`. Sets `pending_admin`. A new proposal replaces the previous one; `Pubkey::default()` withdraws it.
- `accept_admin()`: signed by `pending_admin`. Makes it the admin and clears the proposal. That way the config never ends up with an address that cannot sign. A multisig (for example a Squads vault PDA) can sign through a CPI.

### 5.9 Error codes

In this order (Anchor numbers them from 6000; new codes go at the end, so existing numbers stay):

`BadInitData`, `InvalidSchedule`, `NotPipeline`, `WrongAccount`, `TraderMismatch`, `NotCreator`, `NotLive`, `AlreadyLaunched`, `OneSidedDeposit`, `InsufficientInput`, `InsufficientLiquidity`, `NothingToClaim`, `ForbiddenMintExtension`, `FreezeAuthorityPresent`, `MintAuthorityPresent`, `SameMint`, `MathOverflow`, `WrongTreasury`, `FeeAboveCap`, `BadConfig`, `NotAdmin`, `NotUpgradeAuthority`, `TransferFeeAuthorityPresent`, `TransferFeeAboveCap`, `TaxedSell`, `LaunchFeeBelowRent`, `NotPendingAdmin`.

## 6. Token-2022 policy (launch token)

Checked in `pipeline_init_market` and again in `launch`, by parsing the mint with `StateWithExtensions<Mint>` and `get_extension_types()`. An SPL Token mint (no extensions) is always fine.

| Extension / property | Policy | Reason |
|---|---|---|
| Freeze authority | **Must be `None`** | Otherwise the creator could freeze the vault or buyers' wallets (honeypot) |
| Mint authority | **`None` at launch** (decision B5) | Otherwise unlimited minting and dumping is possible |
| MetadataPointer, TokenMetadata, Group(Pointer), GroupMember(Pointer) | Allowed | Cosmetic only |
| TransferFeeConfig | Allowed if `transfer_fee_config_authority` is `None` and the fee in force and a scheduled fee are at most 500 bps (at `launch`) | `credited_input` measures what arrives. Clients convert the output. With a fee authority, the fee could go to 100% after launch, after which every sell yields nothing (review H1) |
| TransferHook | **Refused** | A hook can refuse transfers into the vault and so block every sell; the hook authority can always replace the hook program (review H2) |
| InterestBearingConfig, ScaledUiAmount | Allowed | Affect only the UI amount. The maths runs on raw amounts |
| MintCloseAuthority | Allowed | Closing is only possible at supply 0 |
| **PermanentDelegate** | **Refused** | The delegate could drain the vaults |
| **NonTransferable** | **Refused** | Then nothing can be traded |
| **Pausable** | **Refused** | Can halt trading or trap buyers |
| **DefaultAccountState** | **Refused** | Accounts can start frozen |
| **ConfidentialTransfer\***, ConfidentialMintBurn | **Refused** | Obscures the vault accounting |
| Unknown (new) extension | **Refused** | Fail-closed |

The quote mint gets **no** freeze check (USDC has a freeze authority), but it does get the same extension allowlist and the same fee rule. One difference: the quote mint may have a **TransferHook** (the owner's choice). Its accounts are passed to every transfer. The call depth fits exactly: router(1) → strategy(2) → token(3) → hook(4). A malicious hook on the quote mint can block sells; that residual risk is described in the review under H2.

## 7. Repo structure and dependencies

```
Pipeline-Sniper-Tax/
├── Cargo.toml                      # host workspace: crates/*
├── rust-toolchain.toml             # 1.97.1 (same as Pipeline)
├── programs/
│   ├── Cargo.toml                  # SBF workspace, overflow-checks = true, lto = "fat"
│   └── sniper-tax/                 # Anchor program (Apache-2.0)
│       └── src/{lib.rs, state.rs, instructions/*.rs, policy.rs, errors.rs}
├── crates/
│   ├── sniper-tax-math/            # pure maths (no_std-friendly), testable on the host
│   └── sniper-tax-tests/           # LiteSVM integration tests against the real router
├── metadata/sniper-tax.json        # pipeline-strategy-metadata/1
├── scripts/devnet/                 # TS smoke test with @solana/kit and the Pipeline SDK
└── docs/PLAN.md                    # this document
```

- `anchor-lang = "=1.2.2"` (feature `init-if-needed`) and `anchor-spl = "=1.2.2"`: exactly the same versions as Pipeline.
- `pipeline-strategy-interface` as a git dependency, pinned to `rev = "a2d5bfb"` (Apache-2.0).
- The router `.so` for tests is built from the same pinned rev. The BSL 1.1 allows non-production use, which covers tests.
- Build: `cargo-build-sbf 4.6.0` (sBPF v3). Tests: `litesvm = "=0.18.2"`, `proptest`.
- CI (GitHub Actions): `fmt`, `clippy -D warnings`, `build-sbf`, `cargo test --workspace` and a keypair check (as in Pipeline).

## 8. Threat model

| # | Attack | Mitigation |
|---|---|---|
| A1 | Accrued tax or dev fee in the quote vault is credited as "free input" in the next swap. Because of the cap on `amount_sent`, this only matters for quote mints with a transfer fee, but then the unclaimed tax pays the difference | `credited_input(vault, reserve + accrued_tax + accrued_protocol, amount_sent)` plus a regression test (`a_transfer_fee_quote_mint_never_spends_unclaimed_tax`) |
| A2 | A sniper calls `pipeline_swap` directly (without the router and without the fee) with trader = creator | `market_signer` must be the router PDA and sign. Only the router can do that |
| A3 | A sniper passes `trade.trader = creator` | The router sets that value itself. We also check `trade.trader == accounts[2]` |
| A4 | Someone creates the creator's pool first (front-running `create_market`) | The address includes the creator and the params, so the result is identical to what the creator wanted. The front end then skips `create_market`. `launch` still requires the creator. One atomic tx is recommended |
| A5 | Vault ATAs are created in advance to block the pool | `init_if_needed` |
| A6 | Tokens are donated to a vault to manipulate the price | Reserves move only with trades and `launch`. Donations don't count |
| A7 | `register_existing_market` on an arbitrary account | `Account<LaunchPool>`, discriminator and owner check |
| A8 | The creator sets `end_bps = 9900` (permanent honeypot) | Hard cap `MAX_END_BPS` (proposed 500 = 5%) |
| A9 | Exact-out rounding lets someone escape the tax | The tax rounds up, gross rounds up. Property tests: `gross − tax ≥ effective` and `tax ≥ ceil(gross·bps/10000)` |
| A10 | Overflow with large amounts | u128 and the 256-bit `mul_div` from the reference, `overflow-checks = true` |
| A11 | Clock tricks | `elapsed` saturates at 0 (no negative time). Leader timestamps are off by at most a few seconds, which is acceptable at a scale of minutes |
| A12 | Freeze, permanent delegate or pause on the launch token | Mint policy (§6), checked at init and at launch |
| A13 | Trading before launch | The `status == Live` check plus the rule that an empty reserve is never priced |
| A14 | The creator uses the exemption for others (buys for third parties) | Known limitation: the creator can always do that. Document it |
| A15 | Tokens are traded outside this pool (another DEX) | Beyond our control. The tax protects only this pool. At launch the creator holds the whole supply, so there is no other liquidity unless the creator creates it |
| A16 | The admin raises the fees of existing pools | Fees are fixed per pool at init. The launch fee is `min(snapshot, current)`. Hard caps in the code |
| A17 | A fake `config` is passed | `Account<ProtocolConfig>` with `seeds = [b"config"]` |
| A18 | The dev fee is swept to a wrong account | The destination must be the ATA of `config.treasury` |
| A19 | The admin key is stolen | The damage stays within the caps and applies to new pools only. Recommended: the admin on a multisig (for example Squads) |
| A20 | The creator keeps a mint's fee authority and raises the fee to 100% after launch (honeypot, review H1) | At `launch`, for both mints: fee authority `None`, the fee in force and a scheduled fee ≤ 5% |
| A21 | A transfer hook on the launch token refuses transfers into the vault (honeypot, review H2) | `TransferHook` refused on the launch token |
| A22 | The admin hands the config to a wrong address (review L2) | Two steps: `propose_admin`, then `accept_admin` by the new admin |
| A23 | A launch fee below the rent-exempt minimum makes every launch into an empty treasury fail (review L1) | `update_config` and `initialize_config` refuse such a fee |

## 9. Test plan

**Unit and property tests (`sniper-tax-math`, proptest):**

- `tax_bps` falls monotonically in `elapsed`, stays between `end` and `start`, and is exactly `start` at 0 and `end` at ≥ duration.
- The exact-in and exact-out inversion is correct and minimal: `gross − 1` is just too little.
- `tax ≤ input`, and there is no overflow over the whole u64 range.
- Quote equals swap for the same `(state, now, trader)`.

**LiteSVM integration (the real router `.so` and our program):**

1. The happy path as one atomic tx: `create_market` → `init_fee_account` ×2 → `launch` → swap.
2. The tax at t = 0, T/2, T−1, T and T+1, with the clock set through `set_sysvar::<Clock>`, for 90 s and 600 s.
3. The creator pays 0% and a non-creator pays the expected rate. `SwapEvent.received` matches the quote.
4. `claim_tax`: only the creator, the amount is right, and a second claim gives `NothingToClaim`.
5. Regression A1: after a taxed swap, a second swap with a small input yields no extra output.
6. A direct call without the router gives `NotPipeline`. A wrong market signer fails too.
7. `register_existing_market` on a non-pool account fails.
8. ATAs created in advance, donations to vaults and a front-run `create_market`.
9. A swap before launch, a second `launch` and a `launch` by a non-creator all fail.
10. Exact-out: buy, refund, rounding.
11. Compute: quote < 200k CU (the quoter cap) and a swap well within budget.
12. Dev fee: the split is right (creator plus dev is exactly the tax), `sweep_protocol_fees` goes only to the treasury ATA, and the launch fee reaches the treasury.
13. Config: only the upgrade authority can create it, only the admin can change it, values above the cap are refused, and a change doesn't affect existing pools.

**Token-2022:**

- A launch token with TransferFee (input and output side).
- Refused: a fee authority, a fee above 5% (in force or scheduled), on both mints.
- TransferHook on the quote mint, with our own test hook that counts transfers. A hook on the launch token is refused.
- Refused mints: PermanentDelegate, NonTransferable, Pausable, DefaultAccountState, freeze authority present and mint authority present.
- A Token-2022 token against an SPL quote (WSOL).

## 10. Phases

| Phase | Contents | Done when |
|---|---|---|
| 0 | Repo skeleton, toolchain, CI, git dependency on Pipeline, router `.so` in the test harness | CI green on an empty program |
| 1 | `sniper-tax-math` with property tests | All invariants from §9 green |
| 2 | Anchor program: state, the 5 Pipeline instructions, `launch`, `claim_tax`, `sweep_protocol_fees`, config administration, mint policy | `cargo build-sbf` succeeds |
| 3 | LiteSVM integration and adversarial tests (§9) | All tests green |
| 4 | Token-2022 tests | All tests green |
| 5 | Metadata JSON, TS smoke test, deploy on devnet and `register_strategy` | A demo launch with a 90 s tax trades through Pipeline on devnet |
| 6 | Review: our own audit round, and possibly an external one | Then choose between an upgrade authority on a multisig and freezing the program |

### Progress

- **Phase 0, done:** workspace, `rust-toolchain.toml` (1.97.1), CI (fmt, clippy, sBPF build, router fetch, all tests, keypair check) and a pre-commit hook.
- **Phase 1, done:** `crates/sniper-tax-math` with 11 unit tests and 12 property tests (2,000 cases each) for every invariant from §9. Deliberately broken roundings are caught by the tests.
- **Phase 2, done:** `programs/sniper-tax` (Anchor 1.2.2), devnet program address `EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b`. Finding: the `transfer_checked` CPI of `anchor-spl` 1.2.2 does not pass on transfer-hook accounts, so the program uses its own helper.
- **Phase 3, done:** `crates/sniper-tax-tests`, 24 LiteSVM tests against the router binary live on devnet (`scripts/fetch-pipeline-router.sh`, pinned by sha256). They cover §9 items 1–13. For item 3, the tests check balances instead of decoding the `SwapEvent`. A swap uses fewer than 80,000 compute units and a quote fewer than 50,000. A deliberately removed A1 protection is caught by the tests.
- **Phase 5, done:** `metadata/sniper-tax.devnet.json` and `crates/sniper-tax-devnet`. The program is on devnet: `EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b`. The binary on chain is byte-for-byte identical to the tested one. It is registered with the Pipeline router and the config was created (10% tax share, 0.01 SOL launch fee, devnet treasury `HcQgsP4JmikD1SDGJAqLLa7K71CXcd7LaNACeeZbNZVh`, later moved to the owner's wallet and on 10 November to a wallet only for devnet, `CUiq3QQbXMsrgCpiN423zr2NTrdfPRuGUy9EQKQ6CGap`). The demo pool `GeajhDYc24MyRecNv1a69nSsa2kdUtwbhLec6xpqVG3B`:
  - a sniper about 5 s after launch paid 93.5% tax;
  - a buy after the window paid 0%;
  - the creator claimed 90% of the tax, the treasury received 10%.

  The metadata URI in the registration points to `main`. It only works once this PR is merged.
- **Phase 4, done:**
  - transfer-fee tokens as launch token and as quote mint;
  - refused mints (permanent delegate, freeze and mint authority);
  - transfer hooks on the launch token and on the quote mint, with a test hook that counts every transfer in an extra account. Every transfer path of the program is tested. A helper that doesn't pass on the hook accounts (like the one in `anchor-spl`) is caught by the tests. Since phase 6, a hook on the launch token is refused, so the hook test runs on the quote mint.
- **Extra tests:**
  - `extremes.rs`: a supply of nearly u64::MAX, dust at 99% tax, trying to buy the pool empty, exact-out at the edge.
  - `stress.rs`: a random sequence of trades, time jumps, claims, sweeps and donations. After every step it checks that the quote exactly equals the swap and the maths, that the vault accounting is exact, that k never falls, and that no token or lamport disappears. A long run of 60,000 steps gave 43,139 trades without a single failure.
  - Mutation test of the maths: 78 of 79 mutants caught; the survivor is equivalent to the original code.
  - Handoff for a next session: `docs/HANDOFF.md`.
- **Cleaned up:** a client crate (`crates/sniper-tax-client`) for types and instructions, used by the tests and the devnet tool. Clearer test options (`MintSpec`, `QuoteMint`). The program binary was unchanged.
- **Phase 6, self-review:** [`SECURITY-REVIEW.md`](SECURITY-REVIEW.md). Two high findings about protecting buyers:
  - H1: a fee authority can later create a honeypot; proven with a test;
  - H2: a transfer hook can block sells.

  Also operational points (M1: multisig or freezing) and smaller points. No finding lets anyone take money out of a pool without the cooperation of the creator or the administrator.
- **Phase 6, fixes (10 November):** H1, H2, L1, L2, I1 and I2 fixed in the program, following B10–B13. The new binary (sha256 `9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd`) is live on devnet since 10 November (slot 520838510). Tests in `findings.rs` (6) and `transfer_hook.rs` (now on the quote mint). A manual mutation round on the new checks: 20 of 20 mutants caught.
- **Docs in English (10 November):** README, PLAN, HANDOFF and SECURITY-REVIEW translated from Dutch.
- **Launch site (10 November):** the plan in [`LAUNCH-SITE.md`](https://github.com/MajocoReborn/Pipeline-Sniper-Tax/blob/main/docs/LAUNCH-SITE.md) is approved; milestone 1, the TypeScript client in `app/`, is done.
- **Own repository (13 November):** the protocol moved from `MajocoReborn/Pipeline-Sniper-Tax` (which keeps the launch site) to `MajocoReborn/pipeline-sniper-tax-strate`, with the program unchanged (sha256 `9f24dafa…`). The TypeScript client became the npm package in `sdk/`, and [`INTEGRATION.md`](INTEGRATION.md) is the guide for launchpads.

## 11. Decisions

All decisions have been made. B1–B8 were approved as recommended; B9 was chosen as A plus a small B. B10–B13 follow from the review (10 November):

| # | Question | Decision |
|---|---|---|
| B1 | Where does the tax go? | To the creator, claimable with `claim_tax` |
| B2 | Which trades are taxed? | Buys only |
| B3 | Shape of the curve | Linear |
| B4 | Duration and starting rate | Freely configurable within [60 s, 30 min], UI presets 90 s and 600 s, start 99% by default |
| B5 | Mint authority required to be revoked at launch? | Yes |
| B6 | Support exact-out? | Yes |
| B7 | Market type | Constant-product with real liquidity (v1); a bonding curve with a virtual reserve possibly in v2 |
| B8 | Permanent fee after the window (`end_bps`) | 0% by default, at most 5% |
| B9 | Revenue model for the program's dev | 10% of the sniper tax (cap 20%) plus 0.01 SOL per launch (cap 0.05 SOL), recorded per pool. See §3.5 |
| B10 | Transfer fee (review H1) | Only without a fee authority, with the fee in force and a scheduled fee at most 5%. For both mints |
| B11 | Transfer hook (review H2) | Refused on the launch token, allowed on the quote mint |
| B12 | Small points (review I1, I2, L1, L2) | In the same version |
| B13 | Deploy of the new version | The owner upgrades locally with their own wallet |
