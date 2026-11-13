# @majocoreborn/sniper-tax-sdk

TypeScript SDK for the sniper-tax launch pool, a strategy on the
[Pipeline](https://github.com/MetalLegBob/pipeline) router on Solana. Built on
[`@solana/kit`](https://github.com/anza-xyz/kit) and Pipeline's
[`@pipeline-sol/sdk`](https://www.npmjs.com/package/@pipeline-sol/sdk).

- **Addresses:** `PROGRAM_ID`, `configAddress`, `poolAddress`, `poolKeys` (every address of a
  pool: market, vaults, token programs) and `verifyPoolKeys` (the keys of a pool read from
  the chain, only if it sits at its own address).
- **Accounts:** `decodeLaunchPool`, `decodeProtocolConfig`, and their codecs.
- **Instructions:** `createMarketIx`, `feeAccountIxs`, `launchIx`, `claimTaxIx`, `sweepIx`,
  `quoteIx` and `swapIx` (with an optional integrator fee).
- **Tax and pricing:** `currentTaxBps`, `taxBpsAt`, `applicableTaxBps`,
  `secondsUntilWindowEnds`, `isValidSchedule`, `quoteExactIn`, `quoteExactOut`,
  `poolPricing`, with the program's exact integer rounding.
- **Caps:** `MAX_START_BPS`, `MAX_END_BPS`, the duration bounds,
  `MAX_PROTOCOL_TAX_SHARE_BPS` and `MAX_LAUNCH_FEE_LAMPORTS`.

Every address, instruction byte, account layout, tax rate and quote is tested against vectors
written by the Rust client (`crates/sniper-tax-client`), which the program's integration
tests use: 2,000+ cases in `test/vectors.json`.

## Install

```sh
npm install @majocoreborn/sniper-tax-sdk @solana/kit
```

Until the first release is on npm, build a package from this repository:

```sh
cd sdk && npm ci && npm pack     # majocoreborn-sniper-tax-sdk-<version>.tgz
```

## Use

```ts
import { fetchEncodedAccount } from '@solana/kit';
import { PROGRAM_ID, currentTaxBps, decodeLaunchPool } from '@majocoreborn/sniper-tax-sdk';

const account = await fetchEncodedAccount(rpc, poolAddress);
const pool = account.exists && account.programAddress === PROGRAM_ID ? decodeLaunchPool(account.data) : null;
const taxBps = pool ? currentTaxBps(pool, clusterUnixTime) : null;
```

Launching, trading, claiming and every fee with its cap are in the
[integration guide](../docs/INTEGRATION.md).

## Develop

```sh
npm ci
npm test            # typecheck, prettier, vitest
npm run build       # dist/
npm run vectors     # after changing the Rust client: regenerate test/vectors.json
```

## License

Apache-2.0. Copyright 2026 MajocoReborn.
