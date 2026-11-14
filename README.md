# Pipeline Sniper Tax

A launch pool for the [Pipeline](https://github.com/MetalLegBob/pipeline) router on Solana that
taxes snipers. A buy right after launch pays up to 99% tax, and that tax falls to a small
permanent rate (at most 5%, often 0%) over a window the creator chooses, from 1 to 30 minutes.
Sells are never taxed and the creator's own wallet is exempt. The tax goes to the creator (70%)
and the protocol (30%); the tax on buys in the launch block (the launch slot and the next, where
bundled snipes land) goes to the protocol in full. Works with SPL Token and Token-2022. To protect buyers, a pool
refuses a launch token with a mint or freeze authority or a transfer hook, and a transfer fee
that can still change or is above 5%.

> **Status:** live on devnet, not yet on mainnet. Self-reviewed
> ([`docs/SECURITY-REVIEW.md`](docs/SECURITY-REVIEW.md)); no external audit yet.

**Launchpads and front ends:** start with the [integration guide](docs/INTEGRATION.md):
launching a pool, reading the tax, trading, claiming, and every fee with its cap. The
TypeScript SDK is in [`sdk/`](sdk) (`@majocoreborn/sniper-tax-sdk`); Rust integrators use
`crates/sniper-tax-client`.

## Fees at a glance

| Fee | Today (devnet) | Cap in the program |
|---|---|---|
| Protocol share of the sniper tax | 30% of the tax | 30%, fixed per pool when it is created |
| Tax on buys in the launch block (launch slot and the next) | All of it to the protocol | Those two slots only |
| Launch fee, paid once by the creator | 0.01 SOL | 0.05 SOL; per pool it can only go down |
| Pipeline's router fee | 0.25% per trade | Set by Pipeline |
| Integrator fee (your front end's) | What you set | None in the router; the trader signs it |

Launchpads earn through Pipeline's integrator fee on the trades they route. Details in the
[guide](docs/INTEGRATION.md#fees).

## Devnet

| | |
|---|---|
| Program | [`EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b`](https://explorer.solana.com/address/EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b?cluster=devnet), registered with the Pipeline router |
| Binary | sha256 `9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd`, reproduced by CI on every push and compared with devnet |
| Demo pool | [`GeajhDYc24MyRecNv1a69nSsa2kdUtwbhLec6xpqVG3B`](https://explorer.solana.com/address/GeajhDYc24MyRecNv1a69nSsa2kdUtwbhLec6xpqVG3B?cluster=devnet) |
| Upgrade authority, config admin and treasury | `CUiq3QQbXMsrgCpiN423zr2NTrdfPRuGUy9EQKQ6CGap` (a wallet only for devnet) |
| Launch site | <https://sniper-tax-launch-devnet.majoco21.workers.dev>, from [`MajocoReborn/Pipeline-Sniper-Tax`](https://github.com/MajocoReborn/Pipeline-Sniper-Tax) |

## Repository

| Path | Contents |
|---|---|
| `programs/sniper-tax` | The Anchor program: the Pipeline strategy |
| `programs/sniper-tax-math` | Pure maths: the tax curve, the tax split, the launch block and constant-product pricing, with property tests |
| `crates/sniper-tax-client` | Addresses, account types and instruction builders, for launchers, scripts and integrators |
| `crates/sniper-tax-tests` | Integration tests against the real Pipeline router from devnet |
| `crates/sniper-tax-devnet` | Registration, config, metadata and a demo launch on devnet |
| `test-programs/test-hook` | Test transfer hook for the integration tests |
| `sdk/` | The TypeScript SDK (npm package), tested against vectors from the Rust client |
| `metadata/sniper-tax.devnet.json` | Strategy metadata for Pipeline terminals (devnet) |
| `scripts/` | Reproducible program build, the devnet check, the router fetch and the keypair check |
| `docs/INTEGRATION.md` | Guide for launchpads |
| `docs/PLAN.md` | Design, account validation, threat model, test plan and phases |
| `docs/SECURITY-REVIEW.md` | Self-review: findings, evidence, fixes and their tests |
| `docs/HANDOFF.md` | Current state, commands, pitfalls and devnet details |

## Build and test

The Rust version in `rust-toolchain.toml` is picked automatically. You need `cargo-build-sbf`
4.6.0 (`cargo install cargo-build-sbf --version 4.6.0 --locked`) and Node 20.19 or later.

```sh
scripts/build-program.sh               # the program, byte for byte the devnet version
(cd test-programs && cargo build-sbf)  # the test hook
scripts/fetch-pipeline-router.sh       # the router live on devnet, into fixtures/
scripts/check-devnet-program.sh        # the program on devnet is this binary
cargo test --workspace                 # maths, property and integration tests
(cd sdk && npm ci && npm test)         # the TypeScript SDK
```

The program's binary depends on the repository's path on disk, so `scripts/build-program.sh`
builds at the path the devnet version was built at, `/home/user/Pipeline-Sniper-Tax`
(creating it may need `sudo` once). Why, and how to remove that at the next program change,
is in [`docs/HANDOFF.md`](docs/HANDOFF.md) (pitfall 1).

## Deploying your own

You need the Solana CLI 4.4 and a wallet with about 6 devnet SOL. The program address in
`declare_id!` belongs to the keypair in `programs/target/deploy/`. To deploy your own copy,
create your own keypair and change the address in `programs/sniper-tax/src/lib.rs`,
`crates/sniper-tax-client/src/lib.rs`, `sdk/src/constants.ts` and the metadata.

```sh
solana program deploy programs/target/deploy/sniper_tax.so \
  --program-id programs/target/deploy/sniper_tax-keypair.json --keypair <wallet> --url devnet
KEYPAIR=<wallet> TREASURY=<address> cargo run -p sniper-tax-devnet -- setup
KEYPAIR=<wallet> cargo run -p sniper-tax-devnet -- demo --wait
```

`setup` registers the program with the Pipeline router and creates the config. `config`
(signed by the current admin) moves the treasury (`TREASURY`) and, with `ADMIN`, proposes a new
admin, who takes over with `accept-admin`. `metadata` (signed by the upgrade authority) points
the registration at this repository's metadata. `demo` creates a test token, launches a pool in
one transaction, has a sniper buy right after launch, buys again after the window (with
`--wait`), and claims and sweeps the tax. Upgrading the existing devnet program is described
in [`docs/HANDOFF.md`](docs/HANDOFF.md).

Keypairs are never committed: `scripts/check-no-keypairs.sh` and the pre-commit hook
(`git config core.hooksPath .githooks`) check for them.

## History

Until 13 November 2026 this code lived in
[`MajocoReborn/Pipeline-Sniper-Tax`](https://github.com/MajocoReborn/Pipeline-Sniper-Tax)
(up to commit `7b7517c`), which keeps the launch site. Its history is there.

## License

Apache-2.0, see [`LICENSE`](LICENSE). Copyright 2026 MajocoReborn.

Parts of the test harness, the way the program builds Pipeline instructions, and the keypair
check script are adapted from [Pipeline](https://github.com/MetalLegBob/pipeline) (Apache-2.0).
The files themselves say where.
