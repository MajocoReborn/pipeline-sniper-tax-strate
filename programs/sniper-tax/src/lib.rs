//! Sniper-tax launch pool: a Pipeline strategy.
//!
//! A constant-product market between a launch token and a quote mint (such as WSOL).
//! Buys pay a sniper tax on their input that starts high at launch (99% by default) and
//! falls linearly to a low end rate over a short window (for example 90 or 600 seconds).
//! The creator trades tax-free. The tax stays in the quote vault, outside the reserves, and
//! is shared between the creator (`claim_tax`) and the protocol treasury
//! (`sweep_protocol_fees`). Launch liquidity can never be withdrawn.
//!
//! Lifecycle: anyone creates the pool through the router's `create_market` (the creator's
//! address and the schedule are part of the pool's address); the creator then calls
//! `launch`, which deposits both tokens and starts the tax clock. Both fit in one
//! transaction. Design, constraints and threat model: docs/PLAN.md. The Pipeline entry points follow Pipeline's reference strategy (`programs/test-strategy-cp` in MetalLegBob/pipeline, Apache-2.0).

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;
use anchor_lang::system_program;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};
use pipeline_strategy_interface::{self as psi, seeds as pipeline_seeds, TradeMode};
use sniper_tax_math::{self as math, TaxSchedule};

pub mod errors;
pub mod policy;
pub mod state;
pub mod token;

pub use errors::SniperError;
pub use policy::MintRole;
pub use state::*;

declare_id!("EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b");

#[program]
pub mod sniper_tax {
    use super::*;

    // ----------------------------------------------------------------------------------
    // Pipeline strategy interface
    // ----------------------------------------------------------------------------------

    /// Called by the router from `create_market`. `data` is a Borsh-encoded [`InitParams`].
    pub fn pipeline_init_market(ctx: Context<InitMarket>, data: Vec<u8>) -> Result<()> {
        // The seeds were derived from the raw bytes, so they must be exactly the params.
        require!(data.len() == INIT_PARAMS_LEN, SniperError::BadInitData);
        let params = InitParams::try_from_slice(&data).map_err(|_| SniperError::BadInitData)?;
        require_keys_neq!(params.creator, Pubkey::default(), SniperError::BadInitData);
        TaxSchedule::new(
            params.tax_start_bps,
            params.tax_end_bps,
            params.tax_duration_secs,
        )
        .map_err(errors::math_error)?;

        let accounts = &ctx.accounts;
        require_keys_neq!(
            accounts.token_mint.key(),
            accounts.quote_mint.key(),
            SniperError::SameMint
        );
        let pool_key = accounts.pool.key();
        let market_signer = pipeline_market_signer(&pool_key);
        require_keys_eq!(
            accounts.market_signer.key(),
            market_signer,
            SniperError::NotPipeline
        );
        policy::check_extensions(&accounts.token_mint.to_account_info(), MintRole::Token)?;
        policy::check_extensions(&accounts.quote_mint.to_account_info(), MintRole::Quote)?;

        let config = &accounts.config;
        let (protocol_tax_share_bps, launch_fee_lamports) =
            (config.protocol_tax_share_bps, config.launch_fee_lamports);
        let token_mint = accounts.token_mint.key();
        let quote_mint = accounts.quote_mint.key();
        let token_vault = accounts.token_vault.key();
        let quote_vault = accounts.quote_vault.key();

        let pool = &mut ctx.accounts.pool;
        pool.version = POOL_VERSION;
        pool.status = PoolStatus::Pending;
        pool.bump = ctx.bumps.pool;
        pool.creator = params.creator;
        pool.token_mint = token_mint;
        pool.quote_mint = quote_mint;
        pool.token_vault = token_vault;
        pool.quote_vault = quote_vault;
        pool.market_signer = market_signer;
        pool.reserve_token = 0;
        pool.reserve_quote = 0;
        pool.accrued_tax_quote = 0;
        pool.accrued_protocol_quote = 0;
        pool.tax_start_bps = params.tax_start_bps;
        pool.tax_end_bps = params.tax_end_bps;
        pool.tax_duration_secs = params.tax_duration_secs;
        pool.protocol_tax_share_bps = protocol_tax_share_bps;
        pool.launch_fee_lamports = launch_fee_lamports;
        pool.launch_ts = 0;
        pool._reserved = [0; 64];
        Ok(())
    }

    /// Called by the router at market creation and by `register_existing_market`. The
    /// `Account` type checks this really is a launch pool.
    pub fn pipeline_market_info(ctx: Context<MarketInfoAccounts>) -> Result<()> {
        let pool = &ctx.accounts.pool;
        let info = psi::MarketInfo {
            mints: vec![pool.token_mint.to_bytes(), pool.quote_mint.to_bytes()],
            vaults: vec![pool.token_vault.to_bytes(), pool.quote_vault.to_bytes()],
            supports_exact_out: true,
        };
        set_return_data(&borsh_encode(&info)?);
        Ok(())
    }

    /// The only strategy-specific account a swap needs is the output vault.
    pub fn pipeline_resolve_accounts(
        ctx: Context<ReadTrade>,
        request: psi::ResolveRequest,
    ) -> Result<()> {
        let side = LaunchPool::side(&request.trade)?;
        let (_, output_vault) = ctx.accounts.pool.vaults(side);
        let result = psi::ResolveResult {
            accounts: vec![psi::AccountRef {
                address: output_vault.to_bytes(),
                writable: true,
            }],
            complete: true,
        };
        set_return_data(&borsh_encode(&result)?);
        Ok(())
    }

    /// Read-only quote at the current clock, with the same maths as `pipeline_swap`.
    /// Anonymous quotes (`trader: None`) pay the tax.
    pub fn pipeline_quote(ctx: Context<ReadTrade>, request: psi::TradeRequest) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let quote = ctx.accounts.pool.quote(&request, now)?;
        let result = psi::QuoteResult {
            amount_in: quote.amount_in,
            amount_out: quote.amount_out,
        };
        set_return_data(&borsh_encode(&result)?);
        Ok(())
    }

    /// Called by the router from `swap`, signed by this pool's market PDA. Remaining
    /// accounts: the output vault, then transfer-hook accounts.
    pub fn pipeline_swap<'info>(
        ctx: Context<'info, SwapAccounts<'info>>,
        request: psi::SwapRequest,
    ) -> Result<()> {
        let trade = &request.trade;
        // The router sets both to its signing trader; the creator exemption relies on it.
        require!(
            trade.trader == Some(ctx.accounts.trader.key().to_bytes()),
            SniperError::TraderMismatch
        );
        let pool = &ctx.accounts.pool;
        let side = LaunchPool::side(trade)?;
        let (input_vault, output_vault) = pool.vaults(side);
        let (input_mint, output_mint) = pool.mints(side);
        require_keys_eq!(
            ctx.accounts.input_vault.key(),
            input_vault,
            SniperError::WrongAccount
        );
        require_keys_eq!(
            ctx.accounts.input_mint.key(),
            input_mint,
            SniperError::WrongAccount
        );
        require_keys_eq!(
            ctx.accounts.output_mint.key(),
            output_mint,
            SniperError::WrongAccount
        );
        let (output_vault_info, hook_accounts) = ctx
            .remaining_accounts
            .split_first()
            .ok_or(SniperError::WrongAccount)?;
        require_keys_eq!(
            output_vault_info.key(),
            output_vault,
            SniperError::WrongAccount
        );

        // Credit only what arrived above the books (reserve plus unclaimed tax), capped at
        // what the router sent, so unclaimed tax and donations are never free input.
        let vault_balance = token::balance(&ctx.accounts.input_vault.to_account_info())?;
        let credited =
            psi::credited_input(vault_balance, pool.input_books(side), request.amount_sent);
        let now = Clock::get()?.unix_timestamp;
        let quote = pool.quote(
            &psi::TradeRequest {
                amount: match trade.mode {
                    TradeMode::ExactIn => credited,
                    TradeMode::ExactOut => trade.amount,
                },
                ..trade.clone()
            },
            now,
        )?;
        require!(quote.amount_in <= credited, SniperError::InsufficientInput);
        let refund = credited - quote.amount_in;

        let signer = pool.signer();
        let seeds = signer.seeds();
        let pool_info = ctx.accounts.pool.to_account_info();
        if refund > 0 {
            token::transfer_checked(
                &ctx.accounts.input_token_program.to_account_info(),
                &ctx.accounts.input_vault.to_account_info(),
                &ctx.accounts.input_mint.to_account_info(),
                &ctx.accounts.trader_input.to_account_info(),
                &pool_info,
                hook_accounts,
                refund,
                ctx.accounts.input_mint.decimals,
                &[&seeds],
            )?;
        }
        if quote.amount_out > 0 {
            token::transfer_checked(
                &ctx.accounts.output_token_program.to_account_info(),
                output_vault_info,
                &ctx.accounts.output_mint.to_account_info(),
                &ctx.accounts.trader_output.to_account_info(),
                &pool_info,
                hook_accounts,
                quote.amount_out,
                ctx.accounts.output_mint.decimals,
                &[&seeds],
            )?;
        }
        // The tax stays in the input vault (Pipeline strategy rule 10); only the books move.
        ctx.accounts.pool.apply(side, &quote)
    }

    // ----------------------------------------------------------------------------------
    // Custom actions, called directly
    // ----------------------------------------------------------------------------------

    /// The creator deposits both tokens and starts the tax clock. Pays the launch fee.
    /// Remaining accounts: transfer-hook accounts for either mint.
    pub fn launch<'info>(
        ctx: Context<'info, Launch<'info>>,
        amount_token: u64,
        amount_quote: u64,
    ) -> Result<()> {
        require!(
            amount_token > 0 && amount_quote > 0,
            SniperError::OneSidedDeposit
        );
        let accounts = &ctx.accounts;
        let token_mint = accounts.token_mint.to_account_info();
        let quote_mint = accounts.quote_mint.to_account_info();
        policy::check_extensions(&token_mint, MintRole::Token)?;
        policy::check_extensions(&quote_mint, MintRole::Quote)?;
        policy::check_launch_authorities(&accounts.token_mint)?;
        policy::check_transfer_fee(&token_mint)?;
        policy::check_transfer_fee(&quote_mint)?;

        let fee = math::launch_fee_due(
            accounts.pool.launch_fee_lamports,
            accounts.config.launch_fee_lamports,
        );
        if fee > 0 {
            system_program::transfer(
                CpiContext::new(
                    system_program::ID,
                    system_program::Transfer {
                        from: accounts.creator.to_account_info(),
                        to: accounts.treasury.to_account_info(),
                    },
                ),
                fee,
            )?;
        }

        // Reserves are what arrived, so tokens sent to the vaults beforehand (donations)
        // and transfer fees stay out of the opening price.
        let mut arrived = [0u64; 2];
        for (slot, (from, mint, vault, program, amount)) in [
            (
                &accounts.creator_token,
                &accounts.token_mint,
                &accounts.token_vault,
                &accounts.token_program_token,
                amount_token,
            ),
            (
                &accounts.creator_quote,
                &accounts.quote_mint,
                &accounts.quote_vault,
                &accounts.token_program_quote,
                amount_quote,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let vault = vault.to_account_info();
            let before = token::balance(&vault)?;
            token::transfer_checked(
                &program.to_account_info(),
                &from.to_account_info(),
                &mint.to_account_info(),
                &vault,
                &accounts.creator.to_account_info(),
                ctx.remaining_accounts,
                amount,
                mint.decimals,
                &[],
            )?;
            arrived[slot] = token::balance(&vault)?
                .checked_sub(before)
                .ok_or(SniperError::MathOverflow)?;
        }
        require!(
            arrived[0] > 0 && arrived[1] > 0,
            SniperError::OneSidedDeposit
        );

        let pool = &mut ctx.accounts.pool;
        pool.reserve_token = arrived[0];
        pool.reserve_quote = arrived[1];
        pool.launch_ts = Clock::get()?.unix_timestamp;
        pool.status = PoolStatus::Live;
        Ok(())
    }

    /// The creator claims their tax to any account in the quote mint.
    /// Remaining accounts: the quote mint's transfer-hook accounts.
    pub fn claim_tax<'info>(ctx: Context<'info, ClaimTax<'info>>) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        let amount = pool.accrued_tax_quote;
        pool.accrued_tax_quote = 0;
        let signer = pool.signer();
        token::transfer_checked(
            &ctx.accounts.token_program_quote.to_account_info(),
            &ctx.accounts.quote_vault.to_account_info(),
            &ctx.accounts.quote_mint.to_account_info(),
            &ctx.accounts.destination.to_account_info(),
            &ctx.accounts.pool.to_account_info(),
            ctx.remaining_accounts,
            amount,
            ctx.accounts.quote_mint.decimals,
            &[&signer.seeds()],
        )
    }

    /// Anyone moves a pool's protocol tax to the treasury's associated token account.
    /// Remaining accounts: the quote mint's transfer-hook accounts.
    pub fn sweep_protocol_fees<'info>(ctx: Context<'info, SweepProtocolFees<'info>>) -> Result<()> {
        let pool = &mut ctx.accounts.pool;
        let amount = pool.accrued_protocol_quote;
        pool.accrued_protocol_quote = 0;
        let signer = pool.signer();
        token::transfer_checked(
            &ctx.accounts.token_program_quote.to_account_info(),
            &ctx.accounts.quote_vault.to_account_info(),
            &ctx.accounts.quote_mint.to_account_info(),
            &ctx.accounts.treasury_quote.to_account_info(),
            &ctx.accounts.pool.to_account_info(),
            ctx.remaining_accounts,
            amount,
            ctx.accounts.quote_mint.decimals,
            &[&signer.seeds()],
        )
    }

    // ----------------------------------------------------------------------------------
    // Protocol config
    // ----------------------------------------------------------------------------------

    /// Creates the protocol config. Only the program's upgrade authority, once.
    pub fn initialize_config(ctx: Context<InitializeConfig>, args: ConfigArgs) -> Result<()> {
        args.validate()?;
        let config = &mut ctx.accounts.config;
        config.admin = args.admin;
        config.treasury = args.treasury;
        config.protocol_tax_share_bps = args.protocol_tax_share_bps;
        config.launch_fee_lamports = args.launch_fee_lamports;
        config.bump = ctx.bumps.config;
        config.pending_admin = Pubkey::default();
        config._reserved = [0; 32];
        Ok(())
    }

    /// Changes the treasury and fees, within the caps. Pools already created keep the
    /// tax share and launch fee they recorded (a launch pays the lower fee).
    pub fn update_config(ctx: Context<UpdateConfig>, args: ConfigUpdate) -> Result<()> {
        args.validate()?;
        let config = &mut ctx.accounts.config;
        config.treasury = args.treasury;
        config.protocol_tax_share_bps = args.protocol_tax_share_bps;
        config.launch_fee_lamports = args.launch_fee_lamports;
        Ok(())
    }

    /// The admin proposes a successor, who takes over by signing `accept_admin`, so a
    /// mistyped address or one that can't sign never takes the config (finding L2). A new
    /// proposal replaces the last; `Pubkey::default()` withdraws it.
    pub fn propose_admin(ctx: Context<UpdateConfig>, new_admin: Pubkey) -> Result<()> {
        ctx.accounts.config.pending_admin = new_admin;
        Ok(())
    }

    /// The proposed admin takes over the config.
    pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.new_admin.key();
        config.pending_admin = Pubkey::default();
        Ok(())
    }
}

/// The Pipeline market PDA for a pool of this program: the only signer `pipeline_swap`
/// accepts for it.
pub fn pipeline_market_signer(pool: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[pipeline_seeds::MARKET, crate::ID.as_ref(), pool.as_ref()],
        &Pubkey::new_from_array(psi::PIPELINE_PROGRAM_ID),
    )
    .0
}

fn borsh_encode<T: AnchorSerialize>(value: &T) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();
    value.serialize(&mut encoded)?;
    Ok(encoded)
}

/// The creator part of the pool's seeds: the first 32 bytes of the init data. Short data
/// still derives an address, then fails the length check in the handler.
fn creator_seed(data: &[u8]) -> &[u8] {
    data.get(..CREATOR_SEED_LEN)
        .unwrap_or(&[0; CREATOR_SEED_LEN])
}

/// The schedule part of the pool's seeds: bytes 32..40 of the init data.
fn params_seed(data: &[u8]) -> &[u8] {
    data.get(CREATOR_SEED_LEN..INIT_PARAMS_LEN)
        .unwrap_or(&[0; INIT_PARAMS_LEN - CREATOR_SEED_LEN])
}

// --------------------------------------------------------------------------------------
// Accounts
// --------------------------------------------------------------------------------------

/// `pipeline_init_market`: the router's four fixed accounts, then this strategy's own.
#[derive(Accounts)]
#[instruction(data: Vec<u8>)]
pub struct InitMarket<'info> {
    /// The router's market PDA; compared with the expected address in the handler.
    pub market_signer: Signer<'info>,
    #[account(
        init,
        payer = payer,
        space = 8 + LaunchPool::INIT_SPACE,
        seeds = [
            POOL_SEED,
            token_mint.key().as_ref(),
            quote_mint.key().as_ref(),
            creator_seed(&data),
            params_seed(&data),
        ],
        bump,
    )]
    pub pool: Account<'info, LaunchPool>,
    /// The router's setup payer, funded by the market creator. Pays rent only.
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
    #[account(mint::token_program = token_program_token)]
    pub token_mint: InterfaceAccount<'info, Mint>,
    #[account(mint::token_program = token_program_quote)]
    pub quote_mint: InterfaceAccount<'info, Mint>,
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = token_mint,
        associated_token::authority = pool,
        associated_token::token_program = token_program_token,
    )]
    pub token_vault: InterfaceAccount<'info, TokenAccount>,
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = quote_mint,
        associated_token::authority = pool,
        associated_token::token_program = token_program_quote,
    )]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,
    pub token_program_token: Interface<'info, TokenInterface>,
    pub token_program_quote: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, ProtocolConfig>,
}

#[derive(Accounts)]
pub struct MarketInfoAccounts<'info> {
    pub pool: Account<'info, LaunchPool>,
}

/// The fixed trade accounts, read-only, for quotes and account resolution. Only the pool
/// is read; the others may be placeholders.
#[derive(Accounts)]
pub struct ReadTrade<'info> {
    /// CHECK: unused in reads.
    pub market_signer: UncheckedAccount<'info>,
    pub pool: Account<'info, LaunchPool>,
    /// CHECK: unused in reads.
    pub trader: UncheckedAccount<'info>,
    /// CHECK: unused in reads.
    pub trader_input: UncheckedAccount<'info>,
    /// CHECK: unused in reads.
    pub trader_output: UncheckedAccount<'info>,
    /// CHECK: unused in reads.
    pub input_vault: UncheckedAccount<'info>,
    /// CHECK: unused in reads.
    pub input_mint: UncheckedAccount<'info>,
    /// CHECK: unused in reads.
    pub output_mint: UncheckedAccount<'info>,
    /// CHECK: unused in reads.
    pub input_token_program: UncheckedAccount<'info>,
    /// CHECK: unused in reads.
    pub output_token_program: UncheckedAccount<'info>,
}

/// `pipeline_swap`: the router's ten fixed accounts. Remaining: output vault, hooks.
#[derive(Accounts)]
pub struct SwapAccounts<'info> {
    /// Only the router can sign for this pool's market PDA, so only routed swaps pass.
    #[account(address = pool.market_signer @ SniperError::NotPipeline)]
    pub market_signer: Signer<'info>,
    #[account(mut, constraint = pool.status == PoolStatus::Live @ SniperError::NotLive)]
    pub pool: Account<'info, LaunchPool>,
    /// CHECK: identity only, never a signer here. The router passes its signing trader.
    pub trader: UncheckedAccount<'info>,
    /// CHECK: receives refunds; the token program checks it.
    #[account(mut)]
    pub trader_input: UncheckedAccount<'info>,
    /// CHECK: receives output; the token program checks it.
    #[account(mut)]
    pub trader_output: UncheckedAccount<'info>,
    /// CHECK: compared with the pool's vault for the input mint in the handler.
    #[account(mut)]
    pub input_vault: UncheckedAccount<'info>,
    #[account(mint::token_program = input_token_program)]
    pub input_mint: InterfaceAccount<'info, Mint>,
    #[account(mint::token_program = output_token_program)]
    pub output_mint: InterfaceAccount<'info, Mint>,
    pub input_token_program: Interface<'info, TokenInterface>,
    pub output_token_program: Interface<'info, TokenInterface>,
}

/// `launch`, signed by the creator.
#[derive(Accounts)]
pub struct Launch<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(
        mut,
        has_one = creator @ SniperError::NotCreator,
        has_one = token_mint @ SniperError::WrongAccount,
        has_one = quote_mint @ SniperError::WrongAccount,
        has_one = token_vault @ SniperError::WrongAccount,
        has_one = quote_vault @ SniperError::WrongAccount,
        constraint = pool.status == PoolStatus::Pending @ SniperError::AlreadyLaunched,
    )]
    pub pool: Account<'info, LaunchPool>,
    #[account(mint::token_program = token_program_token)]
    pub token_mint: InterfaceAccount<'info, Mint>,
    #[account(mint::token_program = token_program_quote)]
    pub quote_mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        token::mint = token_mint,
        token::authority = creator,
        token::token_program = token_program_token,
    )]
    pub creator_token: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        token::mint = quote_mint,
        token::authority = creator,
        token::token_program = token_program_quote,
    )]
    pub creator_quote: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub token_vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,
    pub token_program_token: Interface<'info, TokenInterface>,
    pub token_program_quote: Interface<'info, TokenInterface>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, ProtocolConfig>,
    /// Receives the launch fee.
    #[account(mut, address = config.treasury @ SniperError::WrongTreasury)]
    pub treasury: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// `claim_tax`, signed by the creator.
#[derive(Accounts)]
pub struct ClaimTax<'info> {
    pub creator: Signer<'info>,
    #[account(
        mut,
        has_one = creator @ SniperError::NotCreator,
        has_one = quote_mint @ SniperError::WrongAccount,
        has_one = quote_vault @ SniperError::WrongAccount,
        constraint = pool.accrued_tax_quote > 0 @ SniperError::NothingToClaim,
    )]
    pub pool: Account<'info, LaunchPool>,
    #[account(mint::token_program = token_program_quote)]
    pub quote_mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,
    /// Any account in the quote mint. The vault itself is refused by Anchor, which
    /// rejects the same writable account twice.
    #[account(
        mut,
        token::mint = quote_mint,
        token::token_program = token_program_quote,
    )]
    pub destination: InterfaceAccount<'info, TokenAccount>,
    pub token_program_quote: Interface<'info, TokenInterface>,
}

/// `sweep_protocol_fees`: anyone may call it; the destination is fixed.
#[derive(Accounts)]
pub struct SweepProtocolFees<'info> {
    #[account(
        mut,
        has_one = quote_mint @ SniperError::WrongAccount,
        has_one = quote_vault @ SniperError::WrongAccount,
        constraint = pool.accrued_protocol_quote > 0 @ SniperError::NothingToClaim,
    )]
    pub pool: Account<'info, LaunchPool>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, ProtocolConfig>,
    #[account(mint::token_program = token_program_quote)]
    pub quote_mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,
    /// CHECK: identity only; must be the config's treasury.
    #[account(address = config.treasury @ SniperError::WrongTreasury)]
    pub treasury: UncheckedAccount<'info>,
    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = treasury,
        associated_token::token_program = token_program_quote,
    )]
    pub treasury_quote: InterfaceAccount<'info, TokenAccount>,
    pub token_program_quote: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init,
        payer = authority,
        space = 8 + ProtocolConfig::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Account<'info, ProtocolConfig>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ SniperError::NotUpgradeAuthority)]
    pub program: Program<'info, crate::program::SniperTax>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ SniperError::NotUpgradeAuthority)]
    pub program_data: Account<'info, ProgramData>,
    pub system_program: Program<'info, System>,
}

/// `update_config` and `propose_admin`, signed by the admin.
#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ SniperError::NotAdmin)]
    pub config: Account<'info, ProtocolConfig>,
}

/// `accept_admin`, signed by the proposed admin.
#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    pub new_admin: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = config.pending_admin == new_admin.key()
            && new_admin.key() != Pubkey::default() @ SniperError::NotPendingAdmin,
    )]
    pub config: Account<'info, ProtocolConfig>,
}
