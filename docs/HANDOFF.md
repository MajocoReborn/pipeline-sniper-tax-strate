# Handoff: current state (14 November 2026)

For a new session. The design and background are in [`PLAN.md`](PLAN.md), the review in
[`SECURITY-REVIEW.md`](SECURITY-REVIEW.md), and the guide for launchpads in
[`INTEGRATION.md`](INTEGRATION.md); this document says where everything is, how to test it
and what to watch out for.

**Latest step (14 November): the revenue model B14, in the program, not yet on devnet.**
The owner chose 70% of the tax for the creator and 30% for the dev, and all of the tax on
buys in the launch block (the launch slot and the next, where bundled snipes land) for the
dev. What is taxable is unchanged: buys by anyone but the creator; sells never.

- The program's cap on the protocol share is 30% (was 20%); `setup`'s default is 30%.
- `launch` records `launch_slot` (in the pool's reserved bytes, so the layout and size are
  unchanged); `LaunchPool::quote` gives the protocol the whole tax when the trade's slot is
  `launch_slot` or the next (`math::protocol_share_at`). Pools launched before have
  `launch_slot = 0` and keep their split. Traders pay and receive exactly the same.
- `sniper-tax-math` moved into the `programs/` workspace, which makes the build independent
  of the repository's path (pitfall 1).
- New binary: sha256 `db635b29f127a8b249e3b4a83693482abec03cf171a4b3783da0c3cec818e24f`,
  375,712 bytes. Devnet still runs `9f24dafa…` until the owner upgrades ("Devnet"), then sets
  the share to 30% with `config`.
- The devnet tool's `config` sets the fees (`TAX_SHARE_BPS`, `LAUNCH_FEE`), and `sweep`
  collects the protocol's tax from every pool (`--dry-run` lists it).
- SDK 0.2.0: `launchSlot` in `LaunchPool`, `protocolShareAt`, `LAUNCH_BLOCK_SLOTS`, an
  optional `slot` in `poolPricing`, the 30% cap.
- Tests: `launch_block.rs` (7), new unit and property tests in the maths, and every
  existing test passes against the new binary (the harness steps past the launch block after
  `launch`, so they check the recorded split).

**Before that (13 November):** the protocol has its own repository,
`MajocoReborn/pipeline-sniper-tax-strategy`. It came from `MajocoReborn/Pipeline-Sniper-Tax`
(up to commit `7b7517c`), which keeps the launch site (`app/`) and now installs the
TypeScript SDK from here. History before the move is in that repository.

- The program is unchanged: built with `scripts/build-program.sh` it is byte for byte the
  version on devnet, sha256 `9f24dafa…`, at the same address `EZwLuZ8m…`. The build turned
  out to depend on the repository's path on disk (pitfall 1); the script and CI take care of
  that.
- The site's TypeScript client is now the npm package in `sdk/`
  (`@majocoreborn/sniper-tax-sdk`), on Pipeline's published `@pipeline-sol/sdk` instead of a
  vendored copy, with the fee caps, an optional integrator fee in the swap builders and
  `verifyPoolKeys` added. Not yet on npm: see "Open items".
- `docs/INTEGRATION.md`: launching a pool, reading the tax, trading, claiming, and every fee
  with its cap.
- The devnet tool has a new command, `metadata`. The owner used it the same day to point the
  router registration at the metadata file in this repository (see "Devnet").

## Repo and branch

- Repo: `MajocoReborn/pipeline-sniper-tax-strategy`.
- New work starts on a fresh branch from `main`.
- CI (`.github/workflows/ci.yml`) has two jobs:
  - `build-and-test`: formatting and clippy; the program built at the fixed path and checked
    against `9f24dafa…`; the program on devnet checked against it; the test hook; the router
    fetched from devnet; all Rust tests; and the SDK's vectors up to date;
  - `sdk`: typecheck, prettier, vitest, build and `npm pack --dry-run`.
- `.github/workflows/publish-sdk.yml` publishes `sdk/` to npm on a tag `sdk-v<version>`. It
  needs the repository secret `NPM_TOKEN`.

## What is where

| Path | Contents |
|---|---|
| `programs/sniper-tax` | The Anchor 1.2.2 program (the Pipeline strategy). `lib.rs` (instructions and account structs), `state.rs`, `policy.rs` (Token-2022 policy), `token.rs` (transfer helper), `errors.rs` |
| `programs/sniper-tax-math` | Pure maths: the tax curve, the tax split, the launch block and constant-product. Used by the program; a member of the `programs/` workspace (pitfall 1) |
| `crates/sniper-tax-client` | Addresses (`PoolKeys`, PDAs), mirrored account types and every instruction builder. No LiteSVM, no keypairs. `examples/ts_vectors.rs` writes the SDK's test vectors |
| `crates/sniper-tax-tests` | LiteSVM harness (`src/lib.rs`, on top of the client crate) and integration tests: `launch_pool.rs` (24), `launch_block.rs` (7), `findings.rs` (6), `extremes.rs` (4), `stress.rs` (2), `transfer_hook.rs` (1) |
| `test-programs/test-hook` | Test transfer hook that counts every transfer, in its own workspace so it doesn't touch the lockfile or the binary of the real program |
| `crates/sniper-tax-devnet` | Devnet tool on the client crate: `setup` (registration and config), `demo` (a complete launch), `config` (the treasury and the fees; propose a new admin), `accept-admin` (the proposed admin takes over), `metadata` (point the registration at this repository's metadata), `sweep` (collect the protocol's tax from every pool) |
| `sdk/` | The TypeScript SDK, an npm package (`@majocoreborn/sniper-tax-sdk`), tested against `test/vectors.json` from the Rust client |
| `metadata/sniper-tax.devnet.json` | Strategy metadata for Pipeline terminals |
| `scripts/build-program.sh` | Builds the program and checks its sha256 |
| `scripts/check-devnet-program.sh` | Checks that devnet runs the pinned version (`DEVNET_SHA256`), and whether that is the local build |
| `scripts/fetch-pipeline-router.sh` | Fetches the router binary from devnet, pinned by sha256, into `fixtures/pipeline.so` |
| `scripts/check-no-keypairs.sh` | Refuses keypair files; run by CI and the pre-commit hook (`git config core.hooksPath .githooks`) |
| `docs/INTEGRATION.md` | Guide for launchpads |
| `docs/PLAN.md` | Design, account validation, threat model, decisions, progress |
| `docs/SECURITY-REVIEW.md` | Self-review: findings, fixes and their tests |

## Build and test

```sh
cargo install cargo-build-sbf --version 4.6.0 --locked   # once; downloads ~1.6 GB of platform tools
scripts/build-program.sh                                 # program -> programs/target/deploy, checked
(cd test-programs && cargo build-sbf)                    # test hook -> test-programs/target/deploy
scripts/fetch-pipeline-router.sh                         # router -> fixtures/pipeline.so (needs devnet RPC)
scripts/check-devnet-program.sh                          # devnet runs the pinned version
cargo test --workspace                                   # all Rust tests, ~30 s
cargo clippy --workspace --all-targets -- -D warnings
(cd programs && cargo clippy --all-targets -- -D warnings)
cargo fmt --all -- --check && (cd programs && cargo fmt --all -- --check)
(cd sdk && npm ci && npm test)                           # typecheck, prettier, vitest
(cd sdk && npm run vectors)                              # after changing the Rust client
```

`(cd programs && cargo build-sbf)` gives the same binary; the script only adds the check.

Extra:

```sh
# Long stress test (60,000 steps, ~20 s in release)
STRESS_SEEDS=30 STRESS_STEPS=1000 cargo test -p sniper-tax-tests --release --test stress
# Mutation test of the maths (cargo install cargo-mutants --locked)
(cd programs && cargo mutants -p sniper-tax-math)
```

**Expected results**

| Test | Result |
|---|---|
| `sniper-tax-math` | 14 unit tests and 14 property tests (2,000 cases each), run by `cargo test --workspace` (the crate is a path dependency inside the repository) |
| `sniper-tax-client` | 1 unit test (pool address equals the program's seeds) |
| `launch_pool` | 24 integration tests |
| `launch_block` | 7 tests: the slot is recorded, the launch slot and the next give the protocol the whole tax, the slot after splits, the trader's side is unchanged, quotes match, the creator and sells stay untaxed, the sweep, and pools launched before keep their split |
| `extremes` | 4 tests |
| `stress` | 2 tests |
| `findings` | 6 tests, one per fixed finding from `SECURITY-REVIEW.md` |
| `transfer_hook` | 1 test, with the hook on the quote mint, through every path in which the program makes transfers |
| `sdk` (`npm test`) | 35 tests: constants and caps, every pool's addresses and instructions, the integrator fee, account layouts, 324 tax rates, the launch block, `verifyPoolKeys`, and the pricing cases, all against the Rust vectors |
| Long stress run | 43,139 trades, 12,814 of them taxed buys, without a single broken invariant |
| Mutation test | 78 of 79 mutants caught. The survivor is equivalent (I3), so `cargo mutants` exits with code 2 |

## Pitfalls: read this before changing anything

1. **The binary depends on the source and the toolchain, no longer on the path.**
   - Anchor puts the file and line number in error messages, so every shifted line in `programs/sniper-tax/src/*.rs` gives a different binary.
   - `cargo-build-sbf` 4.6.0 (platform tools v1.58) and `programs/Cargo.lock` determine the result.
   - **Until 14 November the path mattered too.** `crates/sniper-tax-math` lay outside the `programs/` workspace, so Cargo identified it by its absolute path; the path went into the hash in every symbol name, the symbols are ordered by name, and the layout of the binary moved (its size didn't). The devnet version `9f24dafa…` comes out only with the repository at `/home/user/Pipeline-Sniper-Tax`, where the cloud sessions built it (checked: the same source gave `1992abea…` and `cbe82f94…` elsewhere). To rebuild it, check out `8dba1e0` there and run `cd programs && cargo build-sbf`.
   - Since then `sniper-tax-math` is a member of `programs/` (`programs/sniper-tax-math`), and the binary is the same wherever the repository is: `db635b29…` came out identically in three different directories.
   - Pipeline's API and `solana-verify get-program-hash` hash the code with its trailing zero bytes removed, not the whole `.so` file, so they show a different value for the same program.
   - Keep the maths crate in `programs/`. A crate the program uses that lives outside that workspace brings the path dependence back.
2. **The program address is in three places:** `declare_id!` in `programs/sniper-tax/src/lib.rs`, `PROGRAM_ID` in `crates/sniper-tax-client/src/lib.rs`, and the metadata JSON. The program keypair only existed in the old cloud environment. A fresh `cargo build-sbf` creates a new keypair in `target/deploy/`. The tests don't need it, because LiteSVM loads the `.so` at the fixed address; only deploying to a new address needs the keypair.
3. **The `transfer_checked` CPI of `anchor-spl` 1.2.2 does not pass on transfer-hook accounts.** Therefore always use the program's own `token::transfer_checked`, never Anchor's.
4. **Anchor 1.x refuses the same writable account twice** (`ConstraintDuplicateMutableAccount`), before our own constraints run.
5. **The router takes its 0.25% fee from the trader's input account after the swap.** Tests that sell everything therefore fail with "insufficient funds"; sell at most 99%.
6. **Output hook accounts:** according to Pipeline's `INTEGRATOR-GUIDE.md`, the client puts the output mint's hook accounts after the requested strategy accounts. `pipeline_swap` passes everything after the output vault to both transfers. Tested in `transfer_hook.rs`, with a sell that pays out the hooked quote mint.
7. **The router is pinned.** If Pipeline upgrades the router on devnet, `fetch-pipeline-router.sh` stops. Then check what changed and run `--accept-new`.
8. **The cloud environment's sandbox refused to build the cloned Pipeline repo** (external code). That is why the router is downloaded instead of built. The Solana CLI 4.4.1 (`cargo install solana-cli`) needs Rust 1.98.1 and `libudev-dev`.
9. **The test hook program has its own workspace** (`test-programs/`). In the `programs/` workspace it changes the shared lockfile, and with it the binary of the real program.
10. **New error codes go at the end only.** Anchor numbers error codes in order (6000, 6001, …). New codes therefore come after the existing ones in `errors.rs`, so clients keep the old numbers.
11. **`update_config` has new arguments** (`ConfigUpdate`: treasury, tax share, launch fee; no admin any more). The old and new versions therefore don't fit together. After the upgrade, don't use an old build of the devnet tool; and use the new tool's `config` or `accept-admin` only after the upgrade: the old program doesn't know `propose_admin` and `accept_admin`, and refuses the new arguments of `update_config`.
12. **The metadata names the treasury as a fixed address** (`metadata/sniper-tax.devnet.json`, for `launch` and `sweep_protocol_fees`). If the admin moves the treasury, update the metadata too. Otherwise `launch` from a terminal fails with `WrongTreasury`. On 10 November it went from the old treasury `HcQgs…` to `DEwdb…`, and then to the devnet wallet `CUiq3QQ…`.
13. **The Solana CLI needs a keypair file** (a JSON array of 64 bytes). A wallet that exists only as a private key, or only in a wallet app, must first be turned into one, locally: from the seed phrase with `solana-keygen recover 'prompt://?key=0/0'`, or from the private key through a hidden prompt. Never paste a key or seed phrase into a chat. The 12 words the CLI prints during a deploy belong to the temporary buffer account, not to the wallet.
14. **Custom error codes are ambiguous.** The router's `InsufficientOutput` and our `NotUpgradeAuthority` are both 6021, so recognise slippage by the log line, not the number. After serialisation the RPC reports codes as strings (`{"Custom":"1"}`).
15. **The public devnet RPC is several nodes, a few slots apart.** Right after a trade, a plain read can come from a node that hasn't seen it: on 11 November the launch site quoted a sell from the reserves before the visitor's own buy. Read as of the trade's slot instead (`minContextSlot`), and ask again while the node is behind.
16. **The public devnet RPC rate-limits bursts** (HTTP 429), for example right after a launch. Wait and retry.
17. **Listing a creator's pools needs `getProgramAccounts` with filters.** The public devnet RPC allows it; some mainnet RPC providers restrict or charge extra for it. Check that before choosing a mainnet RPC, or a creator's pools can't be listed.
18. **The TypeScript SDK mirrors the Rust client.** After changing `crates/sniper-tax-client`, the maths or a layout in the program, regenerate `sdk/test/vectors.json` (`cd sdk && npm run vectors`) and make the TypeScript match. CI regenerates the vectors and fails if the committed file differs.
19. **The registration's metadata URI points at a file on GitHub:** `metadata/sniper-tax.devnet.json` on this repository's `main` (see "Devnet"). Keep the repository public and the file at that path, or Pipeline's API reports the metadata unreachable; to move it, change `METADATA_URI` in the devnet tool and run `metadata`. The router allows at most 200 bytes.

## Devnet

| | |
|---|---|
| Program | `EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b` (registered with the Pipeline router) |
| Upgrade authority, config admin and treasury | `CUiq3QQbXMsrgCpiN423zr2NTrdfPRuGUy9EQKQ6CGap`: a wallet only for devnet, created locally by the owner (7.0 devnet SOL on 10 November). Until 10 November all three were on the owner's general wallet `DEwdb…N8tYn`, which now has no role here |
| Version on devnet | sha256 `9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd`, upgraded on 10 November 2026 at slot 520838510 (signature `3QJQDcHTB1dmS3YDvwz1KPPQ83eZi9yuzxigk8Ymw8pfWWzmpmN1seVdd7FfnhEDCpojaT5S4cCAQSYbeMURW2Vc`). Before that: `222ee769…` (slot 520494490) |
| Config | `6wCct5q2tQ2WGfbtLcEzaH756aGnkMuSnnexaHmquL93`: 10% tax share, 0.01 SOL launch fee. `pending_admin` is empty (checked after the upgrade) |
| Demo pool | `GeajhDYc24MyRecNv1a69nSsa2kdUtwbhLec6xpqVG3B` |

**The registration's metadata.** Since 13 November the router registration points at
`https://raw.githubusercontent.com/MajocoReborn/pipeline-sniper-tax-strategy/main/metadata/sniper-tax.devnet.json`
(before: the same file in `MajocoReborn/Pipeline-Sniper-Tax`). The owner moved it with the
devnet tool, signed by the upgrade authority `CUiq3QQ…` (transaction
`3k6KzNwkDMrd81kJq5s1vjx8bS2aSkqc6R6zqJ7RssGsdFPBb8VCp2yuRNKEZzrkv2SLHT34tQYyWNcvYhTre8Bt`,
slot 521953366, finalized; the URL serves the file). The old repository no longer holds a
copy. To point it elsewhere later, change `METADATA_URI` and run:

```sh
KEYPAIR=<path-to-wallet.json> cargo run -p sniper-tax-devnet -- metadata
```

It prints the current and the new URI and sends `update_metadata`.

**Deploying a new version.** Upgrading at the same address needs the devnet wallet `CUiq3QQ…`, and that key is never in a cloud session. The owner therefore does the upgrade locally (choice 4a). On Windows, use WSL.

1. Tools, once:

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust; the repo picks its version
   sh -c "$(curl -sSfL https://release.anza.xyz/stable/install)"    # Solana CLI; then open a new terminal
   cargo install cargo-build-sbf --version 4.6.0 --locked
   solana-keygen pubkey <path-to-wallet.json>                       # must print CUiq3QQ…6CGap
   ```
2. Build, from this repository (anywhere on disk):

   ```sh
   git clone https://github.com/MajocoReborn/pipeline-sniper-tax-strategy.git
   cd pipeline-sniper-tax-strategy
   scripts/build-program.sh          # fails unless the hash is EXPECTED_SHA256 in the script
   ```

   Expected for the B14 version: `db635b29f127a8b249e3b4a83693482abec03cf171a4b3783da0c3cec818e24f`, 375,712 bytes. For a changed program, `scripts/build-program.sh --any` prints the new hash instead; don't deploy a binary whose hash you can't explain (a different `cargo-build-sbf`, a changed lockfile or local changes).
3. Upgrade, with the wallet as upgrade authority and as payer:

   ```sh
   solana program deploy programs/target/deploy/sniper_tax.so \
     --program-id EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b \
     --upgrade-authority <path-to-wallet.json> \
     --keypair <path-to-wallet.json> \
     --url devnet
   ```

   - Keep `--url devnet`. Without it the upgrade goes to the cluster in the local Solana config (`solana config get`).
   - The B14 binary is 144 bytes larger than the ProgramData account (375,568 bytes). A recent Solana CLI (2.x or later) extends the account itself. If the CLI still reports that the account is too small, extend it first with `solana program extend EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b 144 --keypair <path-to-wallet.json> --url devnet`.
   - The deploy first writes the binary into a buffer (about 2.7 SOL, which comes back) and pays rent for the extra space (about 0.001 SOL).
   - If the deploy breaks off halfway, recover the buffer SOL with `solana program close --buffers --keypair <path-to-wallet.json> --url devnet`.
   - The program keypair is not needed, only the upgrade authority.
4. Check: set `DEVNET_SHA256` and `DEVNET_SIZE` in `scripts/check-devnet-program.sh` to the new version (for B14: `db635b29…` and 375712), run it, and update the table above. CI runs it on every push, so it fails until the pin matches devnet.

After the upgrade the pool and config layouts are unchanged, and existing pools keep working.

**After the B14 upgrade:** set the protocol's share to 30% for new pools, signed by the admin:

```sh
KEYPAIR=<path-to-wallet.json> TAX_SHARE_BPS=3000 cargo run -p sniper-tax-devnet -- config
```

The old program refuses a share above 20% (`FeeAboveCap`), so this only works after the upgrade. Pools created earlier keep their 10%. To collect the protocol's tax, at any time:

```sh
KEYPAIR=<path-to-wallet.json> cargo run -p sniper-tax-devnet -- sweep --dry-run   # list
KEYPAIR=<path-to-wallet.json> cargo run -p sniper-tax-devnet -- sweep             # sweep and unwrap
```

**The upgrade of 10 November.** The owner deployed the binary built and tested in the cloud session, after checking its sha256 locally, so no local build was needed (for mainnet: build it yourself, or use a verifiable build). The CLI extended the ProgramData account by itself, to exactly 375,568 bytes, and closed the buffer; it cost about 0.067 SOL. Checked afterwards from the cloud session: the on-chain hash matches, the transaction is finalized, the config is intact, and a simulated `pipeline_quote` on the demo pool succeeds (3,775 compute units) with exactly the constant-product output, since its tax window is long over. A new session can also deploy the program to a new address; then change the three places from pitfall 2.

**Moving control to the devnet wallet (10 November).** All three roles moved from `DEwdb…` to `CUiq3QQ…`, checked on-chain afterwards:

- 7 devnet SOL with `solana transfer`;
- the upgrade authority with `solana program set-upgrade-authority … --new-upgrade-authority <new-keypair.json>`, so the new wallet co-signs and a typo is impossible;
- the treasury and an admin proposal with the devnet tool's `config` (signature `4Fiix5BY…`);
- the admin with `accept-admin`, signed by the new wallet (signature `3dXCrc3e…`).

The new wallet was derived with Phantom's path (`solana-keygen recover 'prompt://?key=0/0'`), so its seed phrase gives the same address in a wallet app. `solana-keygen new` derives without a path, which gives a different address from the same words.

**Administration.** `config` sets any of the treasury (`TREASURY`), the protocol's share of the tax (`TAX_SHARE_BPS`, at most 3000) and the launch fee (`LAUNCH_FEE`, in lamports, at most 50,000,000); what isn't set stays. New fees apply to pools created afterwards. With `ADMIN` it proposes a new admin. The new admin takes over with `accept-admin`, with their own wallet as `KEYPAIR`. Until then, the current admin stays in charge. The metadata URI in the registration points to `main` of a public repository (see "The registration's metadata").

## Cleaned up

- **Client crate:** `crates/sniper-tax-client` holds the account types (one mirror of the program, in one place), the PDAs and every instruction builder. `launch_ix` and `claim_tax_ix` take the creator from the pool parameters. The test harness and the devnet tool build on it; the devnet tool no longer depends on LiteSVM.
- **Test options:** mints are described with `MintSpec` (`MintSpec::token_2022(6).with_transfer_fee(..)`) and the quote side with `QuoteMint`, instead of loose flags and an `Option<Option<..>>`.
- **Router pinned again:** Pipeline rebuilt the same router code (slot 520510501). Every test passes against it.
- **Docs in English** (10 November): README, PLAN, HANDOFF and SECURITY-REVIEW were translated from Dutch, so everyone can read them. Code and comments were already English.

## Fixed in the program (10 November)

Following the owner's choices, in one version (sha256 `9f24dafa…`). Details and tests per finding are in `SECURITY-REVIEW.md`.

| Finding | Change | Error code | Tests |
|---|---|---|---|
| H1 | `policy::check_transfer_fee` at `launch`, for both mints: no `transfer_fee_config_authority`, and the fee in force and a scheduled fee at most 500 bps | `TransferFeeAuthorityPresent`, `TransferFeeAboveCap` | `findings.rs`: three H1 tests, each for the launch token and the quote mint |
| H2 | `TransferHook` refused on the launch token (`policy::MintRole`), at `create_market` and at `launch`. Allowed on the quote mint | `ForbiddenMintExtension` | `finding_h2_a_hooked_launch_token_is_refused`, `a_hooked_quote_mint_works_for_every_transfer` |
| I1 | A dedicated error code for "a sell with tax" | `TaxedSell` | Unreachable; the maths never taxes a sell |
| I2 | Constraint `destination != quote_vault` in `ClaimTax` removed; Anchor already refuses it | `ConstraintDuplicateMutableAccount` | `the_creator_claims_their_tax_once` |
| L1 | `initialize_config` and `update_config` refuse a launch fee between 0 and the rent-exempt minimum | `LaunchFeeBelowRent` | `finding_l1_a_launch_fee_below_rent_is_refused` |
| L2 | `update_config` no longer changes the admin; new: `propose_admin` and `accept_admin`, with `pending_admin` in the config's reserved bytes | `NotPendingAdmin` | `finding_l2_the_admin_moves_only_when_the_successor_accepts` |

A manual mutation round on the program confirms that the tests guard these checks: twenty mutants, each built and tested one at a time, and all twenty caught. Examples: inverting the hook rule, not checking the fee authority or one of the two fees, an off-by-one bps or lamport at the boundary, and skipping a step of the admin transfer.

## Still to clean up

- **Redundant safety check:** the filter `*reserve > 0` in `Pricing::settle` (in the maths crate, which the program uses) can never trigger. It is deliberately kept as a defence (I3).

## Deliberately left as is

- **The devnet tool** speaks JSON-RPC through `curl`. That works and keeps the tool small; a real RPC client (`solana-rpc-client`) pulls in many dependencies for a helper script.
- **Language:** everything in the repository and on GitHub is in English; conversations with the owner are in Dutch (see `CLAUDE.md`).

## The owner's choices (10 November)

1. **H1, fee authority: a.** A mint with a transfer fee is allowed only if its `transfer_fee_config_authority` is `None` and both the fee in force and a scheduled fee are at most 500 bps (5%). The same rule applies to the quote mint.
2. **H2, transfer hooks: a.** Hooks on the launch token are refused. The quote mint may keep a hook, without an allowlist. Its residual risk is described in the review under H2.
3. **Small program points: a.** I1, I2, L1 and L2 are in the same version.
4. **Deploy: a.** The owner upgrades locally with their own wallet; the commands are under "Devnet".

## Full run (10 November)

Run after the changes, with the exit code of every step:

| Step | Exit code | Result |
|---|---|---|
| `scripts/check-no-keypairs.sh` | 0 | No keypairs in the repo |
| `cargo fmt --check`, in all three workspaces | 0, 0, 0 | |
| `cargo clippy -D warnings`, in all three workspaces | 0, 0, 0 | |
| `cargo build-sbf` in `programs/`, from an empty `target/` | 0 | sha256 `9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd`, 375,568 bytes |
| `cargo build-sbf` in `test-programs/`, from an empty `target/` | 0 | |
| `scripts/fetch-pipeline-router.sh` | 0 | The router is unchanged (slot 520510501) |
| `cargo test --workspace` | 0 | 63 tests |
| Long stress test (`STRESS_SEEDS=30 STRESS_STEPS=1000`, release) | 0 | 43,139 trades, 12,814 taxed buys, no failure. Same as the previous run: the stress test uses mints the new policy doesn't affect |
| `cargo mutants -p sniper-tax-math` | 2 | 78 of 79 caught. Exit code 2 means "mutants missed": only the known, equivalent mutant in `Pricing::settle` (I3). The maths crate did not change |

In addition, the unchanged source (commit `8bb831b`) built exactly `222ee769…`, the version on devnet, in a fresh environment. The build is therefore reproducible.

## Open items

1. **Deploy B14 on devnet** (the owner, "Devnet": build, upgrade, check), then set the share
   to 30% with `config`, and pin `DEVNET_SHA256` in `scripts/check-devnet-program.sh`.
2. **The launch site** (`MajocoReborn/Pipeline-Sniper-Tax`) moves to SDK 0.2.0 after that:
   its per-trade split (`history.ts`) and the creator dashboard should use the launch block
   (`protocolShareAt` with each trade's slot), and its texts the 70/30 split.
3. **Publish the SDK on npm.** The name `@majocoreborn/sniper-tax-sdk` needs the npm scope
   `majocoreborn` (a user or organisation the owner controls); rename in `sdk/package.json`
   and the imports otherwise, before the first release. Then add `NPM_TOKEN` as a repository
   secret and push a tag `sdk-v0.2.0`. Until then the launch site installs a tarball built
   from a pinned commit of this repository (see its `app/vendor/README.md`).
4. **Phase 6:** still open from [`SECURITY-REVIEW.md`](SECURITY-REVIEW.md): M1 and M2
   (operational), L3 (display extensions) and the residual risk of a hook on the quote mint.
   After that, an external audit.
5. **Before mainnet:**
   - a dedicated program keypair;
   - the upgrade authority, the config admin and the treasury on a Ledger or a multisig (for
     example Squads with a Ledger as one member). The devnet tool reads only keypair files,
     so admin commands from a Ledger need support added first;
   - decide whether to freeze the upgrade authority;
   - a verifiable build, so anyone can reproduce the binary.
