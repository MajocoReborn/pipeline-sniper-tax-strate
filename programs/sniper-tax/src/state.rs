//! Accounts and instruction arguments.

use anchor_lang::prelude::*;
use pipeline_strategy_interface as psi;
use sniper_tax_math::{self as math, Side, TaxSchedule};

use crate::errors::{math_error, SniperError};

/// Seed of a launch pool: `["launch", token_mint, quote_mint, creator, params]`.
pub const POOL_SEED: &[u8] = b"launch";
/// Seed of the protocol config: `["config"]`.
pub const CONFIG_SEED: &[u8] = b"config";

/// Mint indices in the pool's `MarketInfo`.
pub const TOKEN_INDEX: u8 = 0;
pub const QUOTE_INDEX: u8 = 1;

/// Length of Borsh-encoded [`InitParams`]: the creator (32 bytes) and the schedule (8).
pub const INIT_PARAMS_LEN: usize = 40;
pub const CREATOR_SEED_LEN: usize = 32;

pub const POOL_VERSION: u8 = 1;

/// Setup data for `pipeline_init_market`, Borsh-encoded in the router's `init_data`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitParams {
    /// The wallet that launches the pool, is exempt from its tax and receives it.
    pub creator: Pubkey,
    pub tax_start_bps: u16,
    pub tax_end_bps: u16,
    pub tax_duration_secs: u32,
}

/// Settings for `initialize_config`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigArgs {
    pub admin: Pubkey,
    pub treasury: Pubkey,
    pub protocol_tax_share_bps: u16,
    pub launch_fee_lamports: u64,
}

impl ConfigArgs {
    pub fn validate(&self) -> Result<()> {
        require_keys_neq!(self.admin, Pubkey::default(), SniperError::BadConfig);
        validate_settings(
            self.treasury,
            self.protocol_tax_share_bps,
            self.launch_fee_lamports,
        )
    }
}

/// Settings for `update_config`. The admin moves only through `propose_admin` and
/// `accept_admin`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigUpdate {
    pub treasury: Pubkey,
    pub protocol_tax_share_bps: u16,
    pub launch_fee_lamports: u64,
}

impl ConfigUpdate {
    pub fn validate(&self) -> Result<()> {
        validate_settings(
            self.treasury,
            self.protocol_tax_share_bps,
            self.launch_fee_lamports,
        )
    }
}

/// The treasury must be set and the fees within their caps. A launch fee is 0 or at least
/// the rent-exempt minimum of an empty account: a smaller fee paid into an empty treasury
/// would leave it below that minimum, and every launch would fail (finding L1).
fn validate_settings(treasury: Pubkey, tax_share_bps: u16, launch_fee: u64) -> Result<()> {
    require_keys_neq!(treasury, Pubkey::default(), SniperError::BadConfig);
    math::validate_protocol_fees(tax_share_bps, launch_fee).map_err(math_error)?;
    require!(
        launch_fee == 0 || launch_fee >= Rent::get()?.minimum_balance(0),
        SniperError::LaunchFeeBelowRent
    );
    Ok(())
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum PoolStatus {
    /// Created through Pipeline, waiting for the creator's `launch`.
    Pending,
    /// Launched: trading, and the tax clock is running.
    Live,
}

/// The protocol's fee settings. One per program, at `["config"]`.
#[account]
#[derive(InitSpace)]
pub struct ProtocolConfig {
    /// May change these settings, within the caps compiled into the program.
    pub admin: Pubkey,
    /// Receives launch fees, and owns the token accounts protocol tax is swept to.
    pub treasury: Pubkey,
    /// Share of each sniper tax that goes to the treasury. Recorded in each new pool.
    pub protocol_tax_share_bps: u16,
    /// Fee a creator pays at launch. Recorded in each new pool.
    pub launch_fee_lamports: u64,
    pub bump: u8,
    /// Proposed by the admin, becomes the admin by signing `accept_admin`.
    /// `Pubkey::default()` when nothing is proposed. Takes the first 32 reserved bytes, so
    /// a config created before it existed reads as having no proposal.
    pub pending_admin: Pubkey,
    pub _reserved: [u8; 32],
}

/// A launch pool: a constant-product market between a launch token (index 0) and a quote
/// mint (index 1) whose buys pay a falling sniper tax.
#[account]
#[derive(InitSpace)]
pub struct LaunchPool {
    pub version: u8,
    pub status: PoolStatus,
    pub bump: u8,
    /// Launches the pool, trades tax-free and receives the creator's part of the tax.
    pub creator: Pubkey,
    pub token_mint: Pubkey,
    pub quote_mint: Pubkey,
    /// Associated token account of the pool for `token_mint`.
    pub token_vault: Pubkey,
    /// Associated token account of the pool for `quote_mint`. Also holds unclaimed tax.
    pub quote_vault: Pubkey,
    /// The Pipeline market PDA allowed to call `pipeline_swap`.
    pub market_signer: Pubkey,
    pub reserve_token: u64,
    pub reserve_quote: u64,
    /// The creator's tax, held in `quote_vault` and kept out of the reserves.
    pub accrued_tax_quote: u64,
    /// The protocol's tax, held in `quote_vault` and kept out of the reserves.
    pub accrued_protocol_quote: u64,
    pub tax_start_bps: u16,
    pub tax_end_bps: u16,
    pub tax_duration_secs: u32,
    /// The protocol's share of each tax, recorded when the pool was created.
    pub protocol_tax_share_bps: u16,
    /// The launch fee recorded when the pool was created. The creator pays the lower of
    /// this and the current fee.
    pub launch_fee_lamports: u64,
    /// When the pool launched (Unix seconds); 0 while pending.
    pub launch_ts: i64,
    /// The slot the pool launched in; 0 while pending, and for pools launched before this
    /// field existed. The tax on buys in this slot and the next goes to the protocol.
    pub launch_slot: u64,
    pub _reserved: [u8; 56],
}

/// Owned copies of a pool's PDA seeds, so the pool can sign while it is borrowed mutably.
pub struct PoolSigner {
    token_mint: Pubkey,
    quote_mint: Pubkey,
    creator: Pubkey,
    params: [u8; 8],
    bump: [u8; 1],
}

impl PoolSigner {
    pub fn seeds(&self) -> [&[u8]; 6] {
        [
            POOL_SEED,
            self.token_mint.as_ref(),
            self.quote_mint.as_ref(),
            self.creator.as_ref(),
            &self.params,
            &self.bump,
        ]
    }
}

impl LaunchPool {
    pub fn schedule(&self) -> TaxSchedule {
        TaxSchedule {
            start_bps: self.tax_start_bps,
            end_bps: self.tax_end_bps,
            duration_secs: self.tax_duration_secs,
        }
    }

    /// The schedule as it appears in the pool's seeds: the last 8 bytes of `InitParams`.
    pub fn params_seed(&self) -> [u8; 8] {
        let mut seed = [0u8; 8];
        seed[..2].copy_from_slice(&self.tax_start_bps.to_le_bytes());
        seed[2..4].copy_from_slice(&self.tax_end_bps.to_le_bytes());
        seed[4..].copy_from_slice(&self.tax_duration_secs.to_le_bytes());
        seed
    }

    pub fn signer(&self) -> PoolSigner {
        PoolSigner {
            token_mint: self.token_mint,
            quote_mint: self.quote_mint,
            creator: self.creator,
            params: self.params_seed(),
            bump: [self.bump],
        }
    }

    /// Buy: quote in, token out. Sell: token in, quote out.
    pub fn side(trade: &psi::TradeRequest) -> Result<Side> {
        match (trade.input_index, trade.output_index) {
            (QUOTE_INDEX, TOKEN_INDEX) => Ok(Side::Buy),
            (TOKEN_INDEX, QUOTE_INDEX) => Ok(Side::Sell),
            _ => err!(SniperError::WrongAccount),
        }
    }

    /// Input and output vaults for a trade.
    pub fn vaults(&self, side: Side) -> (Pubkey, Pubkey) {
        match side {
            Side::Buy => (self.quote_vault, self.token_vault),
            Side::Sell => (self.token_vault, self.quote_vault),
        }
    }

    /// Input and output mints for a trade.
    pub fn mints(&self, side: Side) -> (Pubkey, Pubkey) {
        match side {
            Side::Buy => (self.quote_mint, self.token_mint),
            Side::Sell => (self.token_mint, self.quote_mint),
        }
    }

    /// Input and output reserves for a trade.
    pub fn reserves(&self, side: Side) -> (u64, u64) {
        match side {
            Side::Buy => (self.reserve_quote, self.reserve_token),
            Side::Sell => (self.reserve_token, self.reserve_quote),
        }
    }

    /// What the pool has on its books in a trade's input vault: the reserve, plus unclaimed
    /// tax in the quote vault. Input is credited only above this.
    pub fn input_books(&self, side: Side) -> u64 {
        match side {
            Side::Buy => math::accounted_balance(
                self.reserve_quote,
                self.accrued_tax_quote,
                self.accrued_protocol_quote,
            ),
            Side::Sell => self.reserve_token,
        }
    }

    /// Prices a trade at `now`, in `slot`. Used by both `pipeline_quote` and `pipeline_swap`.
    pub fn quote(&self, trade: &psi::TradeRequest, now: i64, slot: u64) -> Result<math::Quote> {
        require!(self.status == PoolStatus::Live, SniperError::NotLive);
        let side = Self::side(trade)?;
        let trader_is_creator = trade.trader == Some(self.creator.to_bytes());
        let tax_bps = math::applicable_tax_bps(
            &self.schedule(),
            self.launch_ts,
            now,
            side,
            trader_is_creator,
        );
        let (reserve_in, reserve_out) = self.reserves(side);
        let pricing = math::Pricing {
            reserve_in,
            reserve_out,
            tax_bps,
            protocol_share_bps: math::protocol_share_at(
                self.protocol_tax_share_bps,
                self.launch_slot,
                slot,
            ),
        };
        match trade.mode {
            psi::TradeMode::ExactIn => pricing.quote_exact_in(trade.amount),
            psi::TradeMode::ExactOut => pricing.quote_exact_out(trade.amount),
        }
        .map_err(math_error)
    }

    /// Records a priced trade: new reserves, and the tax left in the input vault.
    pub fn apply(&mut self, side: Side, quote: &math::Quote) -> Result<()> {
        match side {
            Side::Buy => {
                self.reserve_quote = quote.new_reserve_in;
                self.reserve_token = quote.new_reserve_out;
                self.accrued_tax_quote = self
                    .accrued_tax_quote
                    .checked_add(quote.creator_tax)
                    .ok_or(SniperError::MathOverflow)?;
                self.accrued_protocol_quote = self
                    .accrued_protocol_quote
                    .checked_add(quote.protocol_fee)
                    .ok_or(SniperError::MathOverflow)?;
            }
            Side::Sell => {
                // Sells are never taxed, so there is nothing to accrue in the token vault.
                require!(
                    quote.creator_tax == 0 && quote.protocol_fee == 0,
                    SniperError::TaxedSell
                );
                self.reserve_token = quote.new_reserve_in;
                self.reserve_quote = quote.new_reserve_out;
            }
        }
        Ok(())
    }
}
