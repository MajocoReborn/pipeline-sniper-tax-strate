//! Client for the sniper-tax Pipeline strategy: its addresses, account layouts and
//! instructions, for tests, scripts, launchers and integrators.
//!
//! The account types mirror the program's (`programs/sniper-tax/src/state.rs`) field for
//! field; the integration tests decode real accounts with them, so a drift fails there.
//! Router instructions come from Pipeline's own client, re-exported as [`router`].

pub use pipeline_client::{self as router, pda as router_pda, AccountMeta, Instruction, Pubkey};
pub use pipeline_strategy_interface::{self as psi, TradeMode};
pub use sniper_tax_math::Side;

use borsh::{BorshDeserialize, BorshSerialize};

/// The program's address on devnet. Deploying elsewhere means changing it here, in the
/// program's `declare_id!` and in the metadata.
pub const PROGRAM_ID: Pubkey = router::pubkey!("EZwLuZ8mSiBoeKtbHwQK4kxCqqj4JuureT1kHpTK3i6b");

/// Mint indices in a pool's `MarketInfo`.
pub const TOKEN_INDEX: u8 = 0;
pub const QUOTE_INDEX: u8 = 1;

/// The most SOL a pool's setup may borrow from whoever calls `create_market`. Setup needs
/// well under 0.01 SOL of rent; the router returns what isn't spent.
pub const MAX_SETUP_LAMPORTS: u64 = 100_000_000;

// ---------------------------------------------------------------------------------------
// Account layouts and arguments
// ---------------------------------------------------------------------------------------

/// Setup data for `create_market`: who launches the pool and its tax schedule.
#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitParams {
    pub creator: Pubkey,
    pub tax_start_bps: u16,
    pub tax_end_bps: u16,
    pub tax_duration_secs: u32,
}

/// Arguments of `initialize_config`.
#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigArgs {
    pub admin: Pubkey,
    pub treasury: Pubkey,
    pub protocol_tax_share_bps: u16,
    pub launch_fee_lamports: u64,
}

/// Arguments of `update_config`. The admin moves with `propose_admin` and `accept_admin`.
#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigUpdate {
    pub treasury: Pubkey,
    pub protocol_tax_share_bps: u16,
    pub launch_fee_lamports: u64,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolStatus {
    Pending,
    Live,
}

/// A launch pool, as stored on-chain.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct LaunchPool {
    pub version: u8,
    pub status: PoolStatus,
    pub bump: u8,
    pub creator: Pubkey,
    pub token_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub token_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub market_signer: Pubkey,
    pub reserve_token: u64,
    pub reserve_quote: u64,
    pub accrued_tax_quote: u64,
    pub accrued_protocol_quote: u64,
    pub tax_start_bps: u16,
    pub tax_end_bps: u16,
    pub tax_duration_secs: u32,
    pub protocol_tax_share_bps: u16,
    pub launch_fee_lamports: u64,
    pub launch_ts: i64,
    pub reserved: [u8; 64],
}

impl LaunchPool {
    /// Decodes a pool account, checking its Anchor discriminator.
    pub fn decode(data: &[u8]) -> Option<Self> {
        router::decode_account("LaunchPool", data)
    }
}

/// The protocol config, as stored on-chain.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct ProtocolConfig {
    pub admin: Pubkey,
    pub treasury: Pubkey,
    pub protocol_tax_share_bps: u16,
    pub launch_fee_lamports: u64,
    pub bump: u8,
    /// The proposed next admin; `Pubkey::default()` when there is none.
    pub pending_admin: Pubkey,
    pub reserved: [u8; 32],
}

impl ProtocolConfig {
    /// Decodes the config account, checking its Anchor discriminator.
    pub fn decode(data: &[u8]) -> Option<Self> {
        router::decode_account("ProtocolConfig", data)
    }
}

// ---------------------------------------------------------------------------------------
// Addresses and program-wide instructions
// ---------------------------------------------------------------------------------------

pub fn config_address() -> Pubkey {
    Pubkey::find_program_address(&[b"config"], &PROGRAM_ID).0
}

/// A pool's address: `["launch", token_mint, quote_mint, creator, schedule]`.
pub fn pool_address(token_mint: &Pubkey, quote_mint: &Pubkey, params: &InitParams) -> Pubkey {
    let encoded = borsh::to_vec(params).expect("InitParams encodes");
    Pubkey::find_program_address(
        &[
            b"launch",
            token_mint.as_ref(),
            quote_mint.as_ref(),
            &encoded[..32],
            &encoded[32..40],
        ],
        &PROGRAM_ID,
    )
    .0
}

/// An instruction for this program: Anchor discriminator plus Borsh arguments.
pub fn program_ix(
    name: &str,
    args: &impl BorshSerialize,
    accounts: Vec<AccountMeta>,
) -> Instruction {
    let mut data = router::instruction_discriminator(name).to_vec();
    args.serialize(&mut data).expect("arguments encode");
    Instruction::new_with_bytes(PROGRAM_ID, &data, accounts)
}

/// `initialize_config`, signed by the program's upgrade authority.
pub fn initialize_config_ix(authority: &Pubkey, args: &ConfigArgs) -> Instruction {
    program_ix(
        "initialize_config",
        args,
        vec![
            AccountMeta::new(*authority, true),
            AccountMeta::new(config_address(), false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
            AccountMeta::new_readonly(router_pda::program_data(&PROGRAM_ID), false),
            AccountMeta::new_readonly(router::SYSTEM_PROGRAM_ID, false),
        ],
    )
}

/// `update_config`, signed by the config's admin.
pub fn update_config_ix(admin: &Pubkey, args: &ConfigUpdate) -> Instruction {
    program_ix(
        "update_config",
        args,
        vec![
            AccountMeta::new_readonly(*admin, true),
            AccountMeta::new(config_address(), false),
        ],
    )
}

/// `propose_admin`, signed by the config's admin. `Pubkey::default()` withdraws a proposal.
pub fn propose_admin_ix(admin: &Pubkey, new_admin: &Pubkey) -> Instruction {
    program_ix(
        "propose_admin",
        new_admin,
        vec![
            AccountMeta::new_readonly(*admin, true),
            AccountMeta::new(config_address(), false),
        ],
    )
}

/// `accept_admin`, signed by the proposed admin.
pub fn accept_admin_ix(new_admin: &Pubkey) -> Instruction {
    program_ix(
        "accept_admin",
        &(),
        vec![
            AccountMeta::new_readonly(*new_admin, true),
            AccountMeta::new(config_address(), false),
        ],
    )
}

pub fn exact_in(amount_in: u64, min_amount_out: u64) -> router::SwapArgs {
    router::SwapArgs {
        mode: TradeMode::ExactIn,
        amount_in,
        amount_out: 0,
        min_amount_out,
        integrator_fee_bps: 0,
        input_hook_accounts: 0,
        extra: vec![],
    }
}

pub fn exact_out(max_amount_in: u64, amount_out: u64) -> router::SwapArgs {
    router::SwapArgs {
        mode: TradeMode::ExactOut,
        amount_in: max_amount_in,
        amount_out,
        min_amount_out: amount_out,
        integrator_fee_bps: 0,
        input_hook_accounts: 0,
        extra: vec![],
    }
}

// ---------------------------------------------------------------------------------------
// Pools
// ---------------------------------------------------------------------------------------

/// Every address of one pool, and the instructions that use them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolKeys {
    pub address: Pubkey,
    /// The router's market account for this pool.
    pub market: Pubkey,
    pub params: InitParams,
    pub token_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub token_program: Pubkey,
    pub quote_program: Pubkey,
    pub token_vault: Pubkey,
    pub quote_vault: Pubkey,
}

/// One direction of a trade through a pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Legs {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub input_program: Pubkey,
    pub output_program: Pubkey,
    pub input_vault: Pubkey,
    pub output_vault: Pubkey,
    pub input_index: u8,
    pub output_index: u8,
}

impl PoolKeys {
    pub fn new(
        params: InitParams,
        token_mint: Pubkey,
        token_program: Pubkey,
        quote_mint: Pubkey,
        quote_program: Pubkey,
    ) -> Self {
        let address = pool_address(&token_mint, &quote_mint, &params);
        PoolKeys {
            address,
            market: router_pda::market(&PROGRAM_ID, &address),
            params,
            token_mint,
            quote_mint,
            token_program,
            quote_program,
            token_vault: router_pda::associated_token(&address, &token_mint, &token_program),
            quote_vault: router_pda::associated_token(&address, &quote_mint, &quote_program),
        }
    }

    /// The creator's associated token account for the launch token.
    pub fn creator_token(&self) -> Pubkey {
        router_pda::associated_token(&self.params.creator, &self.token_mint, &self.token_program)
    }

    /// The creator's associated token account for the quote mint.
    pub fn creator_quote(&self) -> Pubkey {
        router_pda::associated_token(&self.params.creator, &self.quote_mint, &self.quote_program)
    }

    pub fn legs(&self, side: Side) -> Legs {
        match side {
            Side::Buy => Legs {
                input_mint: self.quote_mint,
                output_mint: self.token_mint,
                input_program: self.quote_program,
                output_program: self.token_program,
                input_vault: self.quote_vault,
                output_vault: self.token_vault,
                input_index: QUOTE_INDEX,
                output_index: TOKEN_INDEX,
            },
            Side::Sell => Legs {
                input_mint: self.token_mint,
                output_mint: self.quote_mint,
                input_program: self.token_program,
                output_program: self.quote_program,
                input_vault: self.token_vault,
                output_vault: self.quote_vault,
                input_index: TOKEN_INDEX,
                output_index: QUOTE_INDEX,
            },
        }
    }

    /// The router's `create_market` for this pool. Anyone may pay for it.
    pub fn create_market_ix(&self, payer: &Pubkey) -> Instruction {
        router::create_market(&router::CreateMarket {
            creator: *payer,
            strategy_program: PROGRAM_ID,
            market_state: self.address,
            market_state_signs: false,
            init_data: borsh::to_vec(&self.params).expect("InitParams encodes"),
            max_creator_lamports: MAX_SETUP_LAMPORTS,
            strategy_accounts: vec![
                AccountMeta::new_readonly(self.token_mint, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new(self.token_vault, false),
                AccountMeta::new(self.quote_vault, false),
                AccountMeta::new_readonly(self.token_program, false),
                AccountMeta::new_readonly(self.quote_program, false),
                AccountMeta::new_readonly(router::ASSOCIATED_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(config_address(), false),
            ],
        })
    }

    /// The router's fee accounts for both mints. Anyone may pay for them.
    pub fn fee_account_ixs(&self, payer: &Pubkey) -> [Instruction; 2] {
        [
            router::init_fee_account(payer, &self.market, &self.token_mint, &self.token_program),
            router::init_fee_account(payer, &self.market, &self.quote_mint, &self.quote_program),
        ]
    }

    /// `launch`, signed by the creator, from their associated token accounts. Append the
    /// mints' transfer-hook accounts, if any.
    pub fn launch_ix(
        &self,
        treasury: &Pubkey,
        amount_token: u64,
        amount_quote: u64,
    ) -> Instruction {
        program_ix(
            "launch",
            &(amount_token, amount_quote),
            vec![
                AccountMeta::new(self.params.creator, true),
                AccountMeta::new(self.address, false),
                AccountMeta::new_readonly(self.token_mint, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new(self.creator_token(), false),
                AccountMeta::new(self.creator_quote(), false),
                AccountMeta::new(self.token_vault, false),
                AccountMeta::new(self.quote_vault, false),
                AccountMeta::new_readonly(self.token_program, false),
                AccountMeta::new_readonly(self.quote_program, false),
                AccountMeta::new_readonly(config_address(), false),
                AccountMeta::new(*treasury, false),
                AccountMeta::new_readonly(router::SYSTEM_PROGRAM_ID, false),
            ],
        )
    }

    /// `claim_tax`, signed by the creator, to any account in the quote mint. Append the
    /// quote mint's transfer-hook accounts, if any.
    pub fn claim_tax_ix(&self, destination: &Pubkey) -> Instruction {
        program_ix(
            "claim_tax",
            &(),
            vec![
                AccountMeta::new_readonly(self.params.creator, true),
                AccountMeta::new(self.address, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new(self.quote_vault, false),
                AccountMeta::new(*destination, false),
                AccountMeta::new_readonly(self.quote_program, false),
            ],
        )
    }

    /// `sweep_protocol_fees` to `destination`, which must be the treasury's associated token
    /// account for the quote mint. Anyone may send it. Append the quote mint's
    /// transfer-hook accounts, if any.
    pub fn sweep_ix(&self, treasury: &Pubkey, destination: &Pubkey) -> Instruction {
        program_ix(
            "sweep_protocol_fees",
            &(),
            vec![
                AccountMeta::new(self.address, false),
                AccountMeta::new_readonly(config_address(), false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new(self.quote_vault, false),
                AccountMeta::new_readonly(*treasury, false),
                AccountMeta::new(*destination, false),
                AccountMeta::new_readonly(self.quote_program, false),
            ],
        )
    }

    /// A swap through the router, from and to `trader`'s associated token accounts.
    pub fn swap_ix(&self, trader: &Pubkey, side: Side, args: &router::SwapArgs) -> Instruction {
        self.swap_ix_with_hooks(trader, side, args, vec![], vec![])
    }

    /// A swap with transfer-hook accounts: the output mint's go after the strategy's own
    /// accounts, the input mint's are the router's `input_hook_accounts`.
    pub fn swap_ix_with_hooks(
        &self,
        trader: &Pubkey,
        side: Side,
        args: &router::SwapArgs,
        output_hooks: Vec<AccountMeta>,
        input_hooks: Vec<AccountMeta>,
    ) -> Instruction {
        let legs = self.legs(side);
        let mut strategy_accounts = vec![AccountMeta::new(legs.output_vault, false)];
        strategy_accounts.extend(output_hooks);
        let accounts = router::SwapAccounts {
            trader: *trader,
            strategy_program: PROGRAM_ID,
            market_state: self.address,
            trader_input: router_pda::associated_token(
                trader,
                &legs.input_mint,
                &legs.input_program,
            ),
            trader_output: router_pda::associated_token(
                trader,
                &legs.output_mint,
                &legs.output_program,
            ),
            input_vault: legs.input_vault,
            input_mint: legs.input_mint,
            output_mint: legs.output_mint,
            input_token_program: legs.input_program,
            output_token_program: legs.output_program,
            integrator_fee_account: None,
            strategy_accounts,
            input_hook_accounts: input_hooks,
        };
        router::swap(&accounts, args)
    }

    /// `pipeline_quote`, for simulation. Only the market signer and the pool are read;
    /// the other fixed accounts are placeholders. A `None` trader pays the tax.
    pub fn quote_ix(
        &self,
        side: Side,
        mode: TradeMode,
        amount: u64,
        trader: Option<Pubkey>,
    ) -> Instruction {
        let legs = self.legs(side);
        let request = psi::TradeRequest {
            mode,
            input_index: legs.input_index,
            output_index: legs.output_index,
            amount,
            trader: trader.map(|t| t.to_bytes()),
            extra: vec![],
        };
        let mut accounts = vec![
            AccountMeta::new_readonly(self.market, false),
            AccountMeta::new_readonly(self.address, false),
        ];
        accounts.extend(std::iter::repeat_n(
            AccountMeta::new_readonly(Pubkey::default(), false),
            8,
        ));
        program_ix("pipeline_quote", &request, accounts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_keys_derive_the_seeds_the_program_uses() {
        let params = InitParams {
            creator: Pubkey::new_from_array([1; 32]),
            tax_start_bps: 9_900,
            tax_end_bps: 0,
            tax_duration_secs: 90,
        };
        let (token, quote) = (
            Pubkey::new_from_array([2; 32]),
            Pubkey::new_from_array([3; 32]),
        );
        let keys = PoolKeys::new(
            params,
            token,
            router::TOKEN_PROGRAM_ID,
            quote,
            router::TOKEN_PROGRAM_ID,
        );
        // The schedule seed is the little-endian (u16, u16, u32) after the creator.
        let mut schedule = 9_900u16.to_le_bytes().to_vec();
        schedule.extend(0u16.to_le_bytes());
        schedule.extend(90u32.to_le_bytes());
        let expected = Pubkey::find_program_address(
            &[
                b"launch",
                token.as_ref(),
                quote.as_ref(),
                params.creator.as_ref(),
                &schedule,
            ],
            &PROGRAM_ID,
        )
        .0;
        assert_eq!(keys.address, expected);
        assert_eq!(keys.legs(Side::Buy).input_mint, quote);
        assert_eq!(keys.legs(Side::Sell).output_vault, keys.quote_vault);
        assert_eq!(
            keys.launch_ix(&Pubkey::default(), 1, 1).accounts[0].pubkey,
            params.creator
        );
    }
}
