# Handoff: current state (13 November 2026)

For a new session. The design and background are in [`PLAN.md`](PLAN.md), the review in
[`SECURITY-REVIEW.md`](SECURITY-REVIEW.md), and the guide for launchpads in
[`INTEGRATION.md`](INTEGRATION.md); this document says where everything is, how to test it
and what to watch out for.

**Latest step (13 November):** the protocol has its own repository,
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
- The devnet tool has a new command, `metadata`, to point the router registration at the
  metadata file in this repository.

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
| `crates/sniper-tax-math` | Pure maths: the tax curve, the tax split and constant-product. Used by the program |
| `crates/sniper-tax-client` | Addresses (`PoolKeys`, PDAs), mirrored account types and every instruction builder. No LiteSVM, no keypairs. `examples/ts_vectors.rs` writes the SDK's test vectors |
| `crates/sniper-tax-tests` | LiteSVM harness (`src/lib.rs`, on top of the client crate) and integration tests: `launch_pool.rs` (24), `findings.rs` (6), `extremes.rs` (4), `stress.rs` (2), `transfer_hook.rs` (1) |
| `test-programs/test-hook` | Test transfer hook that counts every transfer, in its own workspace so it doesn't touch the lockfile or the binary of the real program |
| `crates/sniper-tax-devnet` | Devnet tool on the client crate: `setup` (registration and config), `demo` (a complete launch), `config` (move the treasury, propose a new admin), `accept-admin` (the proposed admin takes over), `metadata` (point the registration at this repository's metadata) |
| `sdk/` | The TypeScript SDK, an npm package (`@majocoreborn/sniper-tax-sdk`), tested against `test/vectors.json` from the Rust client |
| `metadata/sniper-tax.devnet.json` | Strategy metadata for Pipeline terminals |
| `scripts/build-program.sh` | Builds the program at the fixed path and checks its sha256 (pitfall 1) |
| `scripts/check-devnet-program.sh` | Checks that the program on devnet is the local binary, byte for byte |
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
scripts/check-devnet-program.sh                          # the program on devnet is this binary
cargo test --workspace                                   # all Rust tests, ~30 s
cargo clippy --workspace --all-targets -- -D warnings
(cd programs && cargo clippy --all-targets -- -D warnings)
cargo fmt --all -- --check && (cd programs && cargo fmt --all -- --check)
(cd sdk && npm ci && npm test)                           # typecheck, prettier, vitest
(cd sdk && npm run vectors)                              # after changing the Rust client
```

`(cd programs && cargo build-sbf)` still works for development, but gives a different hash
outside `/home/user/Pipeline-Sniper-Tax` (pitfall 1). The tests pass with either.

Extra:

```sh
# Long stress test (60,000 steps, ~20 s in release)
STRESS_SEEDS=30 STRESS_STEPS=1000 cargo test -p sniper-tax-tests --release --test stress
# Mutation test of the maths (cargo install cargo-mutants --locked)
cargo mutants -p sniper-tax-math
```

**Expected results**

| Test | Result |
|---|---|
| `sniper-tax-math` | 13 unit tests and 12 property tests (2,000 cases each) |
| `sniper-tax-client` | 1 unit test (pool address equals the program's seeds) |
| `launch_pool` | 24 integration tests |
| `extremes` | 4 tests |
| `stress` | 2 tests |
| `findings` | 6 tests, one per fixed finding from `SECURITY-REVIEW.md` |
| `transfer_hook` | 1 test, with the hook on the quote mint, through every path in which the program makes transfers |
| `sdk` (`npm test`) | 33 tests: constants and caps, every pool's addresses and instructions, the integrator fee, account layouts, 324 tax rates, `verifyPoolKeys`, and 513 pricing cases, all against the Rust vectors |
| Long stress run | 43,139 trades, 12,814 of them taxed buys, without a single broken invariant |
| Mutation test | 78 of 79 mutants caught. The survivor is equivalent (I3), so `cargo mutants` exits with code 2 |

## Pitfalls: read this before changing anything

1. **The binary depends on the source, the toolchain and the path.**
   - Anchor puts the file and line number in error messages, so every shifted line in `programs/sniper-tax/src/*.rs` gives a different binary.
   - `cargo-build-sbf` 4.6.0 (platform tools v1.58) and `programs/Cargo.lock` determine the result.
   - **The path matters too** (found on 13 November; the earlier handoff said it didn't). `programs/` is its own workspace and `crates/sniper-tax-math` lies outside it, so Cargo identifies that crate by its absolute path. The path goes into the hash in every symbol name, the symbols are ordered by name, and so the layout of the binary moves (its size doesn't). The same source gave `1992abea…` in `/home/user/pipeline-sniper-tax-strategy`, `cbe82f94…` in another directory, and `9f24dafa…` only in `/home/user/Pipeline-Sniper-Tax`, where the cloud sessions built the devnet version. `scripts/build-program.sh` therefore builds there. CI does the same and fails unless the result is `9f24dafa…`, so the tests run against the exact devnet binary.
   - The current source gives sha256 `9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd` (375,568 bytes). Pipeline's API and `solana-verify get-program-hash` show the same program as `987678b54d6f68a6…`: they hash the code with its trailing zero bytes removed, not the whole `.so` file.
   - Devnet runs this version since 10 November (slot 520838510). Before that it ran `222ee7691fe1b9ca344f0831b5b9ead3b863a59ba38c26c5384a105a92f09c3a`.
   - **The next program change** needs a deploy anyway. Make the build path-independent then: move `sniper-tax-math` inside the `programs/` workspace (or build in a container with a fixed path, as `solana-verify` does) and drop the fixed path from `scripts/build-program.sh`.
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
19. **The registration's metadata URI points at a file on GitHub.** Until the owner runs `metadata` (see "Devnet"), it points at the old repository, `MajocoReborn/Pipeline-Sniper-Tax`, which keeps `metadata/sniper-tax.devnet.json` for that reason. Keep both copies the same until then. The router allows at most 200 bytes.

## Devnet

| | |
|---|---|
| Program | `EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b` (registered with the Pipeline router) |
| Upgrade authority, config admin and treasury | `CUiq3QQbXMsrgCpiN423zr2NTrdfPRuGUy9EQKQ6CGap`: a wallet only for devnet, created locally by the owner (7.0 devnet SOL on 10 November). Until 10 November all three were on the owner's general wallet `DEwdb…N8tYn`, which now has no role here |
| Version on devnet | sha256 `9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd`, upgraded on 10 November 2026 at slot 520838510 (signature `3QJQDcHTB1dmS3YDvwz1KPPQ83eZi9yuzxigk8Ymw8pfWWzmpmN1seVdd7FfnhEDCpojaT5S4cCAQSYbeMURW2Vc`). Before that: `222ee769…` (slot 520494490) |
| Config | `6wCct5q2tQ2WGfbtLcEzaH756aGnkMuSnnexaHmquL93`: 10% tax share, 0.01 SOL launch fee. `pending_admin` is empty (checked after the upgrade) |
| Demo pool | `GeajhDYc24MyRecNv1a69nSsa2kdUtwbhLec6xpqVG3B` |

**Pointing the registration at this repository.** The router registration's metadata URI
still points at `https://raw.githubusercontent.com/MajocoReborn/Pipeline-Sniper-Tax/main/metadata/sniper-tax.devnet.json`.
To move it here, the upgrade authority runs, after this repository's `main` holds the
metadata (and while the repository is public):

```sh
KEYPAIR=<path-to-wallet.json> cargo run -p sniper-tax-devnet -- metadata
```

It prints the current and the new URI and sends `update_metadata`. Pipeline's API should then
report the metadata `valid` again. Only after that may the old repository drop its copy.

**Deploying a new version.** Upgrading at the same address needs the devnet wallet `CUiq3QQ…`, and that key is never in a cloud session. The owner therefore does the upgrade locally (choice 4a). On Windows, use WSL.

1. Tools, once:

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust; the repo picks its version
   sh -c "$(curl -sSfL https://release.anza.xyz/stable/install)"    # Solana CLI; then open a new terminal
   cargo install cargo-build-sbf --version 4.6.0 --locked
   solana-keygen pubkey <path-to-wallet.json>                       # must print CUiq3QQ…6CGap
   ```
2. Build, from this repository, with the script that builds at the path the devnet version was built at (pitfall 1):

   ```sh
   git clone https://github.com/MajocoReborn/pipeline-sniper-tax-strategy.git
   cd pipeline-sniper-tax-strategy
   sudo mkdir -p /home/user && sudo chown "$USER" /home/user   # once
   scripts/build-program.sh                                    # fails unless the hash is 9f24dafa…
   ```

   Expected: `9f24dafa0168585de8e15a519a178ed14e6e2bb7d3f8443862eff1db6379f7cd`, 375,568 bytes. For a changed program, `scripts/build-program.sh --any` prints the new hash instead; don't deploy a binary whose hash you can't explain (a different `cargo-build-sbf`, a changed lockfile or local changes).
3. Upgrade, with the wallet as upgrade authority and as payer:

   ```sh
   solana program deploy programs/target/deploy/sniper_tax.so \
     --program-id EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b \
     --upgrade-authority <path-to-wallet.json> \
     --keypair <path-to-wallet.json> \
     --url devnet
   ```

   - Keep `--url devnet`. Without it the upgrade goes to the cluster in the local Solana config (`solana config get`).
   - The new binary is 12,744 bytes larger than the ProgramData account (362,824 bytes). A recent Solana CLI (2.x or later) extends the account itself. If the CLI still reports that the account is too small, extend it first with `solana program extend EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b 12744 --keypair <path-to-wallet.json> --url devnet`.
   - The deploy first writes the binary into a buffer (about 1.9 SOL, which comes back) and pays rent for the extra space (about 0.07 SOL).
   - If the deploy breaks off halfway, recover the buffer SOL with `solana program close --buffers --keypair <path-to-wallet.json> --url devnet`.
   - The program keypair is not needed, only the upgrade authority.
4. Check with `scripts/check-devnet-program.sh`, which compares the program on devnet with `programs/target/deploy/sniper_tax.so` (ignoring zeros after the binary if the account was made larger), then update `EXPECTED_SHA256` in `scripts/build-program.sh` and the table above. CI runs both scripts on every push.

After the upgrade nothing else is needed on-chain: the pool and config layouts are unchanged, and existing pools keep working.

**The upgrade of 10 November.** The owner deployed the binary built and tested in the cloud session, after checking its sha256 locally, so no local build was needed (for mainnet: build it yourself, or use a verifiable build). The CLI extended the ProgramData account by itself, to exactly 375,568 bytes, and closed the buffer; it cost about 0.067 SOL. Checked afterwards from the cloud session: the on-chain hash matches, the transaction is finalized, the config is intact, and a simulated `pipeline_quote` on the demo pool succeeds (3,775 compute units) with exactly the constant-product output, since its tax window is long over. A new session can also deploy the program to a new address; then change the three places from pitfall 2.

**Moving control to the devnet wallet (10 November).** All three roles moved from `DEwdb…` to `CUiq3QQ…`, checked on-chain afterwards:

- 7 devnet SOL with `solana transfer`;
- the upgrade authority with `solana program set-upgrade-authority … --new-upgrade-authority <new-keypair.json>`, so the new wallet co-signs and a typo is impossible;
- the treasury and an admin proposal with the devnet tool's `config` (signature `4Fiix5BY…`);
- the admin with `accept-admin`, signed by the new wallet (signature `3dXCrc3e…`).

The new wallet was derived with Phantom's path (`solana-keygen recover 'prompt://?key=0/0'`), so its seed phrase gives the same address in a wallet app. `solana-keygen new` derives without a path, which gives a different address from the same words.

**Administration.** `config` moves the treasury (keeping the fees) and, with `ADMIN`, proposes a new admin. The new admin takes over with `accept-admin`, with their own wallet as `KEYPAIR`. Until then, the current admin stays in charge. The metadata URI in the registration points to `main` of a public repository (see "Pointing the registration at this repository").

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

1. **Publish the SDK on npm.** The name `@majocoreborn/sniper-tax-sdk` needs the npm scope
   `majocoreborn` (a user or organisation the owner controls); rename in `sdk/package.json`
   and the imports otherwise, before the first release. Then add `NPM_TOKEN` as a repository
   secret and push a tag `sdk-v0.1.0`. Until then the launch site installs a tarball built
   from a pinned commit of this repository (see its `app/vendor/README.md`).
2. **Move the metadata URI** with `metadata` (above), then let the old repository drop its copy.
3. **Path-independent build** at the next program change (pitfall 1).
4. **Phase 6:** still open from [`SECURITY-REVIEW.md`](SECURITY-REVIEW.md): M1 and M2
   (operational), L3 (display extensions) and the residual risk of a hook on the quote mint.
   After that, an external audit.
5. **Before mainnet:**
   - a dedicated program keypair;
   - the treasury and the config admin on a multisig, for example Squads;
   - decide whether to freeze the upgrade authority;
   - a verifiable build (fixed path or container), so anyone can reproduce the binary.
