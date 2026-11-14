//! LiteSVM harness: the sniper-tax program against the real Pipeline router.
//!
//! The router is the exact binary live on devnet (`scripts/fetch-pipeline-router.sh`
//! downloads it into `fixtures/pipeline.so`); the program comes from
//! `programs/target/deploy` (`cd programs && cargo build-sbf`) and the test transfer hook
//! from `test-programs/target/deploy`. Instructions come from `sniper-tax-client`, so the
//! tests also pin the client's wire format.
//!
//! Adapted in part from Pipeline's LiteSVM harness (`crates/pipeline-tests/tests/common/mod.rs`
//! in MetalLegBob/pipeline, Apache-2.0): the native-mint and upgrade-authority helpers, mint
//! and token-account setup, the transaction-size check and `assert_fails_with`.

#![allow(clippy::result_large_err)]

use std::path::PathBuf;

use borsh::BorshDeserialize;
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
pub use sniper_tax_client::*;
use solana_clock::Clock;
pub use solana_keypair::Keypair;
pub use solana_signer::Signer;
use solana_transaction::Transaction;
use spl_token_2022_interface::extension::ExtensionType;
use spl_token_2022_interface::instruction::AuthorityType;

pub const SOL: u64 = 1_000_000_000;
pub const ROUTER_LISTING_FEE: u64 = 10_000_000;
pub const LAUNCH_FEE: u64 = 10_000_000;
pub const PROTOCOL_SHARE_BPS: u16 = 1_000;
/// The clock at launch in every test.
pub const LAUNCH_TS: i64 = 1_800_000_000;
/// Solana's packet limit for a serialised transaction.
pub const MAX_TRANSACTION_SIZE: usize = 1_232;

pub type TxResult = Result<TransactionMetadata, FailedTransactionMetadata>;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ---------------------------------------------------------------------------------------
// Pools
// ---------------------------------------------------------------------------------------

/// A mint to create for a test. Its mint authority is the environment's admin.
#[derive(Clone, Debug)]
pub struct MintSpec {
    pub program: Pubkey,
    pub decimals: u8,
    /// Token-2022 transfer fee: `(bps, maximum)`. Fixed for good unless
    /// `transfer_fee_authority` is set.
    pub transfer_fee: Option<(u16, u64)>,
    /// Keep the admin as the transfer fee's config authority, who can change the fee.
    pub transfer_fee_authority: bool,
    /// Token-2022 transfer hook program (the test hook).
    pub transfer_hook: Option<Pubkey>,
    pub permanent_delegate: bool,
    pub freeze_authority: bool,
}

impl MintSpec {
    pub fn spl(decimals: u8) -> Self {
        MintSpec {
            program: router::TOKEN_PROGRAM_ID,
            decimals,
            transfer_fee: None,
            transfer_fee_authority: false,
            transfer_hook: None,
            permanent_delegate: false,
            freeze_authority: false,
        }
    }

    pub fn token_2022(decimals: u8) -> Self {
        MintSpec {
            program: router::TOKEN_2022_PROGRAM_ID,
            ..MintSpec::spl(decimals)
        }
    }

    pub fn with_transfer_fee(self, bps: u16, maximum: u64) -> Self {
        MintSpec {
            transfer_fee: Some((bps, maximum)),
            ..self
        }
    }

    /// Keeps the admin as the transfer fee's config authority.
    pub fn with_transfer_fee_authority(self) -> Self {
        MintSpec {
            transfer_fee_authority: true,
            ..self
        }
    }

    pub fn with_transfer_hook(self, hook: Pubkey) -> Self {
        MintSpec {
            transfer_hook: Some(hook),
            ..self
        }
    }

    pub fn with_permanent_delegate(self) -> Self {
        MintSpec {
            permanent_delegate: true,
            ..self
        }
    }

    pub fn with_freeze_authority(self) -> Self {
        MintSpec {
            freeze_authority: true,
            ..self
        }
    }
}

/// The quote side of a test pool.
#[derive(Clone, Debug)]
pub enum QuoteMint {
    WrappedSol,
    New(MintSpec),
}

/// How to create a launch pool in a test.
#[derive(Clone, Debug)]
pub struct PoolOptions {
    pub token: MintSpec,
    pub quote: QuoteMint,
    pub revoke_mint_authority: bool,
    pub tax_start_bps: u16,
    pub tax_end_bps: u16,
    pub tax_duration_secs: u32,
    pub token_amount: u64,
    pub quote_amount: u64,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            token: MintSpec::spl(6),
            quote: QuoteMint::WrappedSol,
            revoke_mint_authority: true,
            tax_start_bps: 9_900,
            tax_end_bps: 0,
            tax_duration_secs: 90,
            token_amount: 800_000_000_000_000,
            quote_amount: 5 * SOL,
        }
    }
}

/// A test pool: its addresses (through `Deref`), its creator and how it was made.
pub struct Pool {
    pub keys: PoolKeys,
    pub creator: Keypair,
    pub options: PoolOptions,
}

impl std::ops::Deref for Pool {
    type Target = PoolKeys;

    fn deref(&self) -> &PoolKeys {
        &self.keys
    }
}

// ---------------------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------------------

pub struct Env {
    pub svm: LiteSVM,
    /// Pays for everything; upgrade authority of the router and of this program; admin
    /// of both configs.
    pub admin: Keypair,
    /// Receives launch fees and owns the accounts protocol tax is swept to.
    pub treasury: Pubkey,
    pub router_fee_recipient: Pubkey,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    /// Router and program deployed, both configs initialised, program registered.
    pub fn new() -> Self {
        let mut env = Self::without_protocol_config();
        let args = env.default_config();
        let ix = initialize_config_ix(&env.admin.pubkey(), &args);
        env.send(&[ix], &[]).expect("initialize_config");
        env
    }

    /// Like [`Env::new`], without this program's config.
    pub fn without_protocol_config() -> Self {
        let mut svm = LiteSVM::new();
        let admin = Keypair::new();
        svm.airdrop(&admin.pubkey(), 10_000 * SOL).unwrap();
        svm.add_program_from_file(router::PROGRAM_ID, repo_root().join("fixtures/pipeline.so"))
            .expect("router binary: run scripts/fetch-pipeline-router.sh");
        set_upgrade_authority(&mut svm, &router::PROGRAM_ID, Some(&admin.pubkey()));
        svm.add_program_from_file(
            PROGRAM_ID,
            repo_root().join("programs/target/deploy/sniper_tax.so"),
        )
        .expect("program binary: cd programs && cargo build-sbf");
        set_upgrade_authority(&mut svm, &PROGRAM_ID, Some(&admin.pubkey()));
        add_native_mint(&mut svm);

        let mut env = Env {
            svm,
            admin,
            treasury: Pubkey::new_unique(),
            router_fee_recipient: Pubkey::new_unique(),
        };
        env.set_time(LAUNCH_TS - 60);
        let ix = router::initialize_config(
            &env.admin.pubkey(),
            &router::ConfigArgs {
                listing_fee_lamports: ROUTER_LISTING_FEE,
                listing_fee_recipient: env.router_fee_recipient,
                treasury_owner: Pubkey::new_unique(),
            },
        );
        env.send(&[ix], &[]).expect("router initialize_config");
        let admin = env.admin.pubkey();
        let ix = router::register_strategy(
            &admin,
            &admin,
            &PROGRAM_ID,
            &env.router_fee_recipient,
            "https://example.com/sniper-tax.json",
            u64::MAX,
        );
        env.send(&[ix], &[]).expect("register_strategy");
        env
    }

    pub fn default_config(&self) -> ConfigArgs {
        ConfigArgs {
            admin: self.admin.pubkey(),
            treasury: self.treasury,
            protocol_tax_share_bps: PROTOCOL_SHARE_BPS,
            launch_fee_lamports: LAUNCH_FEE,
        }
    }

    /// The default settings, as `update_config` arguments.
    pub fn default_update(&self) -> ConfigUpdate {
        ConfigUpdate {
            treasury: self.treasury,
            protocol_tax_share_bps: PROTOCOL_SHARE_BPS,
            launch_fee_lamports: LAUNCH_FEE,
        }
    }

    // ------------------------------------------------------------------ transactions

    /// Sends a transaction paid for by the admin, with extra signers. Asserts it fits in
    /// a real transaction.
    pub fn send(&mut self, instructions: &[Instruction], signers: &[&Keypair]) -> TxResult {
        let tx = self.transaction(instructions, signers);
        let size = bincode::serialize(&tx)
            .map(|b| b.len())
            .unwrap_or(usize::MAX);
        assert!(
            size <= MAX_TRANSACTION_SIZE,
            "transaction is {size} bytes, over Solana's {MAX_TRANSACTION_SIZE}-byte limit"
        );
        self.svm.send_transaction(tx)
    }

    fn transaction(&mut self, instructions: &[Instruction], signers: &[&Keypair]) -> Transaction {
        self.svm.expire_blockhash();
        let mut all: Vec<&Keypair> = vec![&self.admin];
        all.extend_from_slice(signers);
        Transaction::new_signed_with_payer(
            instructions,
            Some(&self.admin.pubkey()),
            &all,
            self.svm.latest_blockhash(),
        )
    }

    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    pub fn set_time(&mut self, unix_timestamp: i64) {
        let mut clock = self.svm.get_sysvar::<Clock>();
        clock.unix_timestamp = unix_timestamp;
        clock.slot += 1;
        self.svm.set_sysvar::<Clock>(&clock);
    }

    pub fn slot(&self) -> u64 {
        self.svm.get_sysvar::<Clock>().slot
    }

    /// Moves `slots` slots ahead without moving the time.
    pub fn advance_slots(&mut self, slots: u64) {
        let mut clock = self.svm.get_sysvar::<Clock>();
        clock.slot += slots;
        self.svm.set_sysvar::<Clock>(&clock);
    }

    /// Moves past the launch block (the launch slot and the next), at the same time, so the
    /// next trades split their tax with the pool's recorded share.
    pub fn skip_launch_block(&mut self) {
        self.advance_slots(sniper_tax_math::LAUNCH_BLOCK_SLOTS);
    }

    pub fn funded_keypair(&mut self) -> Keypair {
        let keypair = Keypair::new();
        self.svm.airdrop(&keypair.pubkey(), 100 * SOL).unwrap();
        keypair
    }

    // ------------------------------------------------------------------ reading

    pub fn account_data(&self, address: &Pubkey) -> Vec<u8> {
        self.svm
            .get_account(address)
            .map(|a| a.data)
            .unwrap_or_default()
    }

    pub fn lamports(&self, address: &Pubkey) -> u64 {
        self.svm
            .get_account(address)
            .map(|a| a.lamports)
            .unwrap_or(0)
    }

    pub fn token_balance(&self, account: &Pubkey) -> u64 {
        let data = self.account_data(account);
        if data.len() < 72 {
            return 0;
        }
        u64::from_le_bytes(data[64..72].try_into().unwrap())
    }

    pub fn pool_state(&self, pool: &PoolKeys) -> LaunchPool {
        LaunchPool::decode(&self.account_data(&pool.address)).expect("pool account")
    }

    pub fn config_state(&self) -> ProtocolConfig {
        ProtocolConfig::decode(&self.account_data(&config_address())).expect("config account")
    }

    /// The vaults hold at least what the pool's books say: reserves plus unclaimed tax.
    pub fn assert_solvent(&self, pool: &PoolKeys) {
        let state = self.pool_state(pool);
        assert!(self.token_balance(&pool.token_vault) >= state.reserve_token);
        assert!(
            u128::from(self.token_balance(&pool.quote_vault))
                >= u128::from(state.reserve_quote)
                    + u128::from(state.accrued_tax_quote)
                    + u128::from(state.accrued_protocol_quote)
        );
    }

    // ------------------------------------------------------------------ tokens

    /// Creates a mint whose mint authority is the admin. A mint with the test hook gets
    /// the hook's accounts set up too. The admin withdraws withheld transfer fees.
    pub fn create_mint(&mut self, spec: &MintSpec) -> Pubkey {
        let mint = Keypair::new();
        let program = &spec.program;
        let mut extensions = vec![];
        if spec.transfer_hook.is_some() {
            extensions.push(ExtensionType::TransferHook);
        }
        if spec.transfer_fee.is_some() {
            extensions.push(ExtensionType::TransferFeeConfig);
        }
        if spec.permanent_delegate {
            extensions.push(ExtensionType::PermanentDelegate);
        }
        let space = if *program == router::TOKEN_PROGRAM_ID {
            82
        } else {
            ExtensionType::try_calculate_account_len::<spl_token_2022_interface::state::Mint>(
                &extensions,
            )
            .unwrap()
        };
        let rent = self.svm.minimum_balance_for_rent_exemption(space);
        let authority = self.admin.pubkey();
        let mut ixs = vec![solana_system_interface::instruction::create_account(
            &authority,
            &mint.pubkey(),
            rent,
            space as u64,
            program,
        )];
        if let Some((bps, max)) = spec.transfer_fee {
            ixs.push(
                spl_token_2022_interface::extension::transfer_fee::instruction::initialize_transfer_fee_config(
                    program,
                    &mint.pubkey(),
                    spec.transfer_fee_authority.then_some(&authority),
                    Some(&authority),
                    bps,
                    max,
                )
                .unwrap(),
            );
        }
        if let Some(hook) = spec.transfer_hook {
            ixs.push(
                spl_token_2022_interface::extension::transfer_hook::instruction::initialize(
                    program,
                    &mint.pubkey(),
                    None,
                    Some(hook),
                )
                .unwrap(),
            );
        }
        if spec.permanent_delegate {
            ixs.push(
                spl_token_2022_interface::instruction::initialize_permanent_delegate(
                    program,
                    &mint.pubkey(),
                    &authority,
                )
                .unwrap(),
            );
        }
        ixs.push(
            spl_token_2022_interface::instruction::initialize_mint2(
                program,
                &mint.pubkey(),
                &authority,
                spec.freeze_authority.then_some(&authority),
                spec.decimals,
            )
            .unwrap(),
        );
        self.send(&ixs, &[&mint]).expect("create mint");
        if let Some(hook) = spec.transfer_hook {
            let ix = test_hook::initialize_ix(&hook, &self.admin.pubkey(), &mint.pubkey());
            self.send(&[ix], &[]).expect("initialize the test hook");
        }
        mint.pubkey()
    }

    /// Loads the test transfer hook (`test-programs/test-hook`) and returns its ID.
    pub fn load_test_hook(&mut self) -> Pubkey {
        if self.svm.get_account(&test_hook::ID).is_none() {
            self.svm
                .add_program_from_file(
                    test_hook::ID,
                    repo_root().join("test-programs/target/deploy/test_hook.so"),
                )
                .expect("test hook binary: cd test-programs && cargo build-sbf");
        }
        test_hook::ID
    }

    /// How many transfers of `mint` the test hook has seen.
    pub fn hook_count(&self, mint: &Pubkey) -> u64 {
        let data = self.account_data(&test_hook::counter(mint));
        u64::from_le_bytes(data[..8].try_into().expect("hook counter"))
    }

    /// Creates (idempotently) `owner`'s associated token account.
    pub fn create_ata(&mut self, owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
        let ix = spl_associated_token_account_interface::instruction::create_associated_token_account_idempotent(
            &self.admin.pubkey(),
            owner,
            mint,
            token_program,
        );
        self.send(&[ix], &[]).expect("create ata");
        router_pda::associated_token(owner, mint, token_program)
    }

    /// Mints `amount` into `account`, or wraps that many lamports for wrapped SOL.
    pub fn mint_to(
        &mut self,
        mint: &Pubkey,
        token_program: &Pubkey,
        account: &Pubkey,
        amount: u64,
    ) {
        let authority = self.admin.pubkey();
        if router::wallet::is_native(mint, token_program) {
            let ixs = [
                router::wallet::system_transfer(&authority, account, amount),
                router::wallet::sync_native(account),
            ];
            self.send(&ixs, &[]).expect("wrap SOL");
            return;
        }
        let ix = spl_token_2022_interface::instruction::mint_to(
            token_program,
            mint,
            account,
            &authority,
            &[],
            amount,
        )
        .unwrap();
        self.send(&[ix], &[]).expect("mint_to");
    }

    pub fn revoke_mint_authority(&mut self, mint: &Pubkey, token_program: &Pubkey) {
        let ix = spl_token_2022_interface::instruction::set_authority(
            token_program,
            mint,
            None,
            AuthorityType::MintTokens,
            &self.admin.pubkey(),
            &[],
        )
        .unwrap();
        self.send(&[ix], &[]).expect("revoke mint authority");
    }

    /// A new wallet holding `amount` of the pool's quote mint, with an empty token account.
    pub fn buyer(&mut self, pool: &PoolKeys, amount: u64) -> Keypair {
        let trader = self.funded_keypair();
        let quote = self.create_ata(&trader.pubkey(), &pool.quote_mint, &pool.quote_program);
        self.mint_to(&pool.quote_mint, &pool.quote_program, &quote, amount);
        self.create_ata(&trader.pubkey(), &pool.token_mint, &pool.token_program);
        trader
    }

    pub fn quote_account(&self, pool: &PoolKeys, owner: &Pubkey) -> Pubkey {
        router_pda::associated_token(owner, &pool.quote_mint, &pool.quote_program)
    }

    pub fn token_account(&self, pool: &PoolKeys, owner: &Pubkey) -> Pubkey {
        router_pda::associated_token(owner, &pool.token_mint, &pool.token_program)
    }

    // ------------------------------------------------------------------ pools

    /// Creates the mints and funds the creator. Nothing is created on the router yet.
    pub fn prepare_pool(&mut self, options: PoolOptions) -> Pool {
        let creator = self.funded_keypair();
        let token_mint = self.create_mint(&options.token);
        let token_program = options.token.program;
        let (quote_mint, quote_program) = match &options.quote {
            QuoteMint::WrappedSol => (router::wallet::NATIVE_MINT, router::TOKEN_PROGRAM_ID),
            QuoteMint::New(spec) => (self.create_mint(spec), spec.program),
        };
        let params = InitParams {
            creator: creator.pubkey(),
            tax_start_bps: options.tax_start_bps,
            tax_end_bps: options.tax_end_bps,
            tax_duration_secs: options.tax_duration_secs,
        };
        let keys = PoolKeys::new(params, token_mint, token_program, quote_mint, quote_program);

        // The creator keeps a tenth of the supply outside the pool.
        self.create_ata(&creator.pubkey(), &token_mint, &token_program);
        let supply = options.token_amount + options.token_amount / 10;
        self.mint_to(&token_mint, &token_program, &keys.creator_token(), supply);
        if options.revoke_mint_authority {
            self.revoke_mint_authority(&token_mint, &token_program);
        }
        self.create_ata(&creator.pubkey(), &quote_mint, &quote_program);
        self.mint_to(
            &quote_mint,
            &quote_program,
            &keys.creator_quote(),
            options.quote_amount + 10 * SOL,
        );
        Pool {
            keys,
            creator,
            options,
        }
    }

    /// `create_market` and both fee accounts, paid by the admin, without launching.
    pub fn create_pool(&mut self, options: PoolOptions) -> (Pool, TxResult) {
        let pool = self.prepare_pool(options);
        let admin = self.admin.pubkey();
        let mut ixs = vec![pool.create_market_ix(&admin)];
        ixs.extend(pool.fee_account_ixs(&admin));
        let result = self.send(&ixs, &[]);
        (pool, result)
    }

    /// The creator's `launch` with the pool's planned amounts.
    pub fn launch_ix(&self, pool: &Pool) -> Instruction {
        pool.launch_ix(
            &self.treasury,
            pool.options.token_amount,
            pool.options.quote_amount,
        )
    }

    /// The creator's `launch`. Afterwards the clock is past the launch block, as in
    /// `launched_pool`.
    pub fn launch(&mut self, pool: &Pool) -> TxResult {
        let ix = self.launch_ix(pool);
        let creator = pool.creator.insecure_clone();
        let result = self.send(&[ix], &[&creator]);
        self.skip_launch_block();
        result
    }

    /// A pool created and launched at `LAUNCH_TS`: `create_market`, both fee accounts and
    /// `launch` in one transaction, as a front-end would send them. Afterwards the clock is
    /// still at `LAUNCH_TS`, but past the launch block, so trades see the pool's recorded
    /// tax split; `launched_pool_in_launch_block` stays in it.
    pub fn launched_pool(&mut self, options: PoolOptions) -> Pool {
        let pool = self.launched_pool_in_launch_block(options);
        self.skip_launch_block();
        pool
    }

    /// As `launched_pool`, but the clock stays in the launch slot.
    pub fn launched_pool_in_launch_block(&mut self, options: PoolOptions) -> Pool {
        let pool = self.prepare_pool(options);
        self.set_time(LAUNCH_TS);
        let creator = pool.creator.insecure_clone();
        let mut ixs = vec![pool.create_market_ix(&creator.pubkey())];
        ixs.extend(pool.fee_account_ixs(&creator.pubkey()));
        ixs.push(self.launch_ix(&pool));
        self.send(&ixs, &[&creator])
            .expect("create and launch in one transaction");
        pool
    }

    // ------------------------------------------------------------------ trading

    pub fn swap(
        &mut self,
        pool: &PoolKeys,
        trader: &Keypair,
        side: Side,
        args: router::SwapArgs,
    ) -> TxResult {
        let ix = pool.swap_ix(&trader.pubkey(), side, &args);
        let trader = trader.insecure_clone();
        self.send(&[ix], &[&trader])
    }

    /// Simulates `pipeline_quote`, as an SDK would.
    pub fn quote(
        &mut self,
        pool: &PoolKeys,
        side: Side,
        mode: TradeMode,
        amount: u64,
        trader: Option<Pubkey>,
    ) -> Result<psi::QuoteResult, String> {
        self.simulate_quote(pool, side, mode, amount, trader)
            .map(|(quote, _)| quote)
    }

    /// Simulates `pipeline_quote` and also returns the compute units it used.
    pub fn simulate_quote(
        &mut self,
        pool: &PoolKeys,
        side: Side,
        mode: TradeMode,
        amount: u64,
        trader: Option<Pubkey>,
    ) -> Result<(psi::QuoteResult, u64), String> {
        let ix = pool.quote_ix(side, mode, amount, trader);
        let tx = self.transaction(&[ix], &[]);
        match self.svm.simulate_transaction(tx) {
            Ok(info) => psi::QuoteResult::try_from_slice(&info.meta.return_data.data)
                .map(|quote| (quote, info.meta.compute_units_consumed))
                .map_err(|e| e.to_string()),
            Err(failed) => Err(failed.meta.logs.join("\n")),
        }
    }
}

/// Adds the wrapped-SOL mint as it exists on every cluster: SPL Token, 9 decimals, no
/// authorities.
fn add_native_mint(svm: &mut LiteSVM) {
    let mut data = vec![0u8; 82];
    data[44] = 9;
    data[45] = 1;
    let account = solana_account::Account {
        lamports: svm.minimum_balance_for_rent_exemption(82),
        data,
        owner: router::TOKEN_PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    };
    svm.set_account(router::wallet::NATIVE_MINT, account)
        .expect("native mint");
}

/// Rewrites the upgrade authority in a loader-v3 program's ProgramData header.
pub fn set_upgrade_authority(svm: &mut LiteSVM, program: &Pubkey, authority: Option<&Pubkey>) {
    let program_data = router_pda::program_data(program);
    let mut account = svm.get_account(&program_data).expect("program data");
    match authority {
        Some(key) => {
            account.data[12] = 1;
            account.data[13..45].copy_from_slice(key.as_ref());
        }
        None => {
            account.data[12] = 0;
            account.data[13..45].fill(0);
        }
    }
    svm.set_account(program_data, account).unwrap();
}

/// Asserts a transaction failed and that its logs or error mention `needle`.
pub fn assert_fails_with(result: TxResult, needle: &str) {
    match result {
        Ok(meta) => panic!(
            "expected failure containing {needle:?}, but succeeded:\n{}",
            meta.logs.join("\n")
        ),
        Err(failed) => {
            let logs = failed.meta.logs.join("\n");
            assert!(
                logs.contains(needle) || format!("{:?}", failed.err).contains(needle),
                "expected failure containing {needle:?}, got {:?}:\n{logs}",
                failed.err
            );
        }
    }
}

/// `ceil(amount × bps / 10,000)`.
pub fn ceil_bps(amount: u64, bps: u16) -> u64 {
    (u128::from(amount) * u128::from(bps)).div_ceil(10_000) as u64
}

/// Addresses and instructions of the test transfer hook (`test-programs/test-hook`), which
/// counts every transfer of its mint in a counter account listed as its extra account.
pub mod test_hook {
    use super::*;

    pub const ID: Pubkey = Pubkey::new_from_array([0x7e; 32]);

    pub fn validation(mint: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"extra-account-metas", mint.as_ref()], &ID).0
    }

    pub fn counter(mint: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[b"counter", mint.as_ref()], &ID).0
    }

    pub fn initialize_ix(hook: &Pubkey, payer: &Pubkey, mint: &Pubkey) -> Instruction {
        Instruction::new_with_bytes(
            *hook,
            b"cnthkini",
            vec![
                AccountMeta::new(*payer, true),
                AccountMeta::new(validation(mint), false),
                AccountMeta::new(counter(mint), false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new_readonly(router::SYSTEM_PROGRAM_ID, false),
            ],
        )
    }

    /// The accounts Token-2022 needs to run the hook for `mint`.
    pub fn accounts(mint: &Pubkey) -> Vec<AccountMeta> {
        vec![
            AccountMeta::new_readonly(ID, false),
            AccountMeta::new_readonly(validation(mint), false),
            AccountMeta::new(counter(mint), false),
        ]
    }
}
