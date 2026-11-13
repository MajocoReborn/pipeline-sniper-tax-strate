# Integrating the sniper-tax launch pool (guide for launchpads)

This guide is for launchpads and front ends that want to launch tokens into a sniper-tax pool
and trade them through the [Pipeline](https://github.com/MetalLegBob/pipeline) router. It
covers launching a pool, reading the tax, trading, claiming, and every fee involved, with its
cap.

The TypeScript examples use the SDK in [`sdk/`](../sdk) (`@majocoreborn/sniper-tax-sdk`),
Pipeline's SDK (`@pipeline-sol/sdk`) and `@solana/kit`. Rust integrators get the same
builders from `crates/sniper-tax-client`. The SDK is tested byte for byte against the Rust
client, which the program's own integration tests use.

## What the pool does

- A constant-product pool between a launch token and a quote mint (usually wrapped SOL),
  with the creator's liquidity locked for good.
- **A buy pays a tax on its input:** at most 99% right after launch, falling linearly to a
  permanent rate (at most 5%, often 0%) over a window of 60 seconds to 30 minutes. The
  creator chooses the schedule; it is part of the pool's address and never changes.
- **Sells are never taxed.** The creator's own wallet trades tax-free.
- The tax stays in the pool's quote vault until it is claimed: the creator's part with
  `claim_tax`, the protocol's part with `sweep_protocol_fees` (to the protocol treasury).
- Pools trade only through the Pipeline router; the program checks that the router's market
  signer signs every swap.

## Addresses

| | Devnet |
|---|---|
| Program | `EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b` (`PROGRAM_ID`) |
| Protocol config | `6wCct5q2tQ2WGfbtLcEzaH756aGnkMuSnnexaHmquL93` (`configAddress()`) |
| Treasury | `CUiq3QQbXMsrgCpiN423zr2NTrdfPRuGUy9EQKQ6CGap`: read it from the config, don't hard-code it |
| Pipeline router | `PiPEf4fFBpYXCtgfiWgVWzZNXnq2wZ7bnXidJCTwf34` |
| Pipeline API | <https://pipeline-devnet.up.railway.app> |

The program is not on mainnet yet.

## Fees

Every fee a trader or creator can pay, who receives it, and the most it can ever be.

| Fee | Paid by | Goes to | Today (devnet) | Cap |
|---|---|---|---|---|
| **Sniper tax** | Buyers other than the creator | Creator and protocol, below | The pool's schedule | 99% at launch (`MAX_START_BPS`), 5% after the window (`MAX_END_BPS`) |
| **Protocol share of the tax** | Out of the tax (not on top of it) | Protocol treasury | 10% of the tax | 20% (`MAX_PROTOCOL_TAX_SHARE_BPS`), fixed per pool |
| **Launch fee** | The creator, once, at `launch` | Protocol treasury | 0.01 SOL | 0.05 SOL (`MAX_LAUNCH_FEE_LAMPORTS`); per pool it can only go down |
| **Pipeline fee** | Every trader, on top of the input | Pipeline | 0.25% | Fixed by the router |
| **Integrator fee** | Traders on your front end, on top of the input | You | What you set | None in the router: the trader signs the rate |

How the caps hold:

- The protocol's two fees live in the config (`decodeProtocolConfig`). The admin can change
  them, but the program refuses anything above the caps.
- **Each pool records both at `create_market`.** Its share of the tax never changes after
  that. Its launch fee is the lower of the recorded and the current fee
  (`launch_fee_due`), so a creator never pays more than was shown when the pool was
  created, and gets any later cut.
- The creator receives the rest of the tax: 90% today, never less than 80%.

Example: a sniper buys for 10 SOL at the start of the window with a 99% start rate. 9.9 SOL is
tax: 8.91 SOL for the creator, 0.99 SOL for the protocol. 0.1 SOL buys tokens. Pipeline
charges 0.025 SOL on top, and your integrator fee, if any.

### Earning as a launchpad: Pipeline's integrator fee

The program takes nothing for launchpads. You earn through **Pipeline's integrator fee**, on
the trades made through your front end:

- The router charges `ceil(net_sent × integratorFeeBps / 10,000)` on top of what the trader
  sends, and transfers it to your fee account in the same instruction.
- The trader signs the rate. The router has no cap, so **show the rate before the trader
  signs**: a fee hidden in a transaction is exactly what this protocol is built to avoid.
- The fee is in the **input mint** of each trade: wrapped SOL on a buy, the launch token on a
  sell. The fee account must be a token account of that mint and must exist before the trade.
  Many front ends charge on buys only, so they only need a wrapped-SOL account.
- Traders who use another front end, or call the router themselves, don't pay your fee.

```ts
import { exactIn, swapIx } from '@majocoreborn/sniper-tax-sdk';

const args = exactIn(amountIn, minAmountOut, 50); // 0.5% integrator fee
const ix = await swapIx(keys, trader, 'buy', args, [], [], launchpadWsolAccount);
```

The most a trade can cost, before signing, is Pipeline's `maxTotalSpend(amountIn,
integratorFeeBps)`; to spend at most a budget, use `amountForBudget(budget,
integratorFeeBps)`. Both are in `@pipeline-sol/sdk`.

## 1. Launch a pool

A launch is a token and a pool. The SDK builds the pool's instructions; the token is yours to
create with the usual token programs (`@solana-program/token-2022` or `@solana-program/token`).

### The token

The program checks the launch token at `create_market` and again at `launch`, and refuses it
otherwise. The token must have:

- **no mint authority** (revoke it after minting the supply) and **no freeze authority**;
- **no transfer hook**;
- if it has a transfer fee: **no fee authority**, and the fee in force and any scheduled fee at
  most 5%;
- only Token-2022 extensions that can't hurt buyers: no permanent delegate, no
  non-transferable, default-frozen or pausable token. The full list is in
  [`PLAN.md` §6](PLAN.md#6-token-2022-policy-launch-token).

The quote mint may have a transfer hook and needn't revoke its freeze authority (USDC has
one), but the same transfer-fee rule applies to it.

### The pool

The pool's address is a PDA of `["launch", token mint, quote mint, creator, schedule]`, so
nobody can take the creator's pool by creating it first. Schedules must meet:

| | Bound |
|---|---|
| `taxStartBps` | ≤ 9,900 (99%) |
| `taxEndBps` | ≤ 500 (5%) and ≤ `taxStartBps` |
| `taxDurationSecs` | 60 to 1,800 |

`isValidSchedule` checks this before you send anything.

Send these in **one transaction**, so nobody can get in between and the pool goes live with
its liquidity at once:

1. the creator's wrapped-SOL account, and wrapping the SOL for the pool
   (create the associated token account, transfer, `syncNative`);
2. `createMarketIx(keys, payer)`: the router's `create_market`. The router lends the pool's
   setup up to `MAX_SETUP_LAMPORTS` (0.1 SOL) from `payer` for rent, and returns what is left;
3. `feeAccountIxs(keys, payer)`: the router's fee accounts for both mints;
4. `launchIx(keys, treasury, amountToken, amountQuote)`, signed by the creator: moves the
   liquidity from the creator's associated token accounts into the vaults, pays the launch fee
   and starts the tax clock;
5. optionally, close the creator's wrapped-SOL account to get its rent back.

```ts
import { createSolanaRpc, fetchEncodedAccount } from '@solana/kit';
import { TOKEN_2022_PROGRAM_ADDRESS, TOKEN_PROGRAM_ADDRESS } from '@pipeline-sol/sdk';
import {
    NATIVE_MINT,
    configAddress,
    createMarketIx,
    decodeProtocolConfig,
    feeAccountIxs,
    isValidSchedule,
    launchIx,
    poolKeys,
} from '@majocoreborn/sniper-tax-sdk';

const params = { creator, taxStartBps: 9_900, taxEndBps: 100, taxDurationSecs: 600 };
if (!isValidSchedule(params)) throw new Error('schedule out of bounds');

const keys = await poolKeys(params, mint, TOKEN_2022_PROGRAM_ADDRESS, NATIVE_MINT, TOKEN_PROGRAM_ADDRESS);

const config = await fetchEncodedAccount(rpc, await configAddress());
const protocol = config.exists ? decodeProtocolConfig(config.data) : null;
if (!protocol) throw new Error('no protocol config');

const instructions = [
    // ...create the creator's wrapped-SOL account, transfer amountQuote lamports, syncNative...
    await createMarketIx(keys, creator),
    ...(await feeAccountIxs(keys, creator)),
    await launchIx(keys, protocol.treasury, amountToken, amountQuote),
    // ...close the creator's wrapped-SOL account...
];
```

Show the creator, before they sign: the launch fee (`protocol.launchFeeLamports`), the
protocol's share of the tax (`protocol.protocolTaxShareBps`), and the rent. On devnet a launch
of 0.1 SOL of liquidity cost 0.12527 SOL in all: the 0.1 SOL, the 0.01 SOL launch fee, rent
for the pool, vaults and fee accounts, and transaction fees. The pool transaction used about
200,000 compute units.

`launch` takes its tokens and quote from the creator's **associated** token accounts. Whatever
of the supply the creator doesn't put in the pool stays in their wallet.

## 2. Read the tax

```ts
import { currentTaxBps, decodeLaunchPool, secondsUntilWindowEnds } from '@majocoreborn/sniper-tax-sdk';

const account = await fetchEncodedAccount(rpc, poolAddress);
const pool = account.exists && account.programAddress === PROGRAM_ID ? decodeLaunchPool(account.data) : null;

const taxBps = currentTaxBps(pool, now); // what an anonymous buy pays at `now`
const left = secondsUntilWindowEnds(pool, now); // 0 once only the permanent rate is left
const unclaimed = pool.accruedTaxQuote; // the creator's tax, not yet claimed
```

- `now` is the **cluster's** time in Unix seconds (the `Clock` sysvar, or a recent block's
  time), not the browser's clock: the program uses the cluster's.
- The rate is integer maths identical to the program's, so the number you show is the number
  the program charges at that second. The creator pays 0 (`applicableTaxBps` with
  `traderIsCreator`), and so does every sell.
- If you find pools anywhere but on the chain at their own address (an indexer, Pipeline's API),
  check them with `verifyPoolKeys(address, pool)`: it returns the pool's keys only if the
  account sits at the address its own fields derive, with the vaults they derive.
- Pipeline's API lists every market of the program, with trades and trust flags. Its candles
  price each trade by what the trader paid, tax included, so a sniper buy looks like a price
  spike; chart from the reserves (`pool.reserveQuote / pool.reserveToken`) instead.

## 3. Quote and trade

The pool's maths is in the SDK, with the program's rounding:

```ts
import { poolPricing, quoteExactIn } from '@majocoreborn/sniper-tax-sdk';

const result = quoteExactIn(poolPricing(pool, 'buy', trader, now), amountIn);
if (result.ok) {
    result.quote.amountOut; // tokens out
    result.quote.creatorTax; // and result.quote.protocolFee: the tax on this buy
}
```

For the program's own answer, simulate `quoteIx(keys, side, mode, amount, trader)` (the
strategy's `pipeline_quote`), or use Pipeline's quoter or API.

A buy: create the trader's wrapped-SOL account and wrap, create their account for the token,
`swapIx(keys, trader, 'buy', exactIn(amountIn, minAmountOut, feeBps), [], [], feeAccount)`,
then close the wrapped-SOL account. A sell is the same with `'sell'`, unwrapping the output.

- **The tax falls while a buy is on its way.** Quote at the latest time the trade can land, or
  allow for the higher rate in `minAmountOut`; a buy right after launch can land seconds later
  at a lower rate, never a higher one.
- `minAmountOut` is what must arrive in the trader's account; the router enforces it.
- The router takes its fee from the trader's input account after the swap: a trader selling
  their whole balance must leave room for it (`amountForBudget`).
- A quote mint with a transfer hook: pass its hook accounts as `outputHooks` on a sell and
  `inputHooks` on a buy.

## 4. Claim

The creator claims their tax with `claim_tax`, to any token account of the quote mint they
choose. For wrapped SOL: create their wrapped-SOL account, claim, close it to unwrap.

```ts
import { claimTaxIx, creatorQuoteAccount, verifyPoolKeys } from '@majocoreborn/sniper-tax-sdk';

const keys = await verifyPoolKeys(poolAddress, pool);
if (!keys || pool.accruedTaxQuote === 0n) return;
const destination = await creatorQuoteAccount(keys);
const instructions = [
    // ...create `destination` (idempotent)...
    await claimTaxIx(keys, destination),
    // ...close `destination` to unwrap...
];
```

- Only the pool's creator can sign it; the program refuses an empty claim (`NothingToClaim`).
- Claiming never touches the reserves or the protocol's part.
- To list a creator's pools, use `getProgramAccounts` on the program with a `memcmp` filter on
  the creator at byte offset 11 (8 bytes of discriminator, then version, status and bump). Some
  mainnet RPC providers restrict this call.

The protocol's part goes to the treasury with `sweepIx(keys, treasury, treasuryQuoteAccount)`.
Anyone may send it, because the destination is fixed: the treasury's associated token account
for the quote mint.

## Errors

The program's error codes are in [`programs/sniper-tax/src/errors.rs`](../programs/sniper-tax/src/errors.rs),
numbered from 6000 in order. Router and strategy codes overlap (the router's
`InsufficientOutput` and the program's `NotUpgradeAuthority` are both 6021), so recognise
slippage by the router's log line rather than by the number alone.

## Further reading

- [`PLAN.md`](PLAN.md): the design, every instruction's account validation, the threat model.
- [`SECURITY-REVIEW.md`](SECURITY-REVIEW.md): the self-review, its findings and fixes.
- Pipeline's [integrator guide](https://github.com/MetalLegBob/pipeline/blob/main/docs/INTEGRATOR-GUIDE.md):
  discovery, quoting, swaps, events and what to show users.
