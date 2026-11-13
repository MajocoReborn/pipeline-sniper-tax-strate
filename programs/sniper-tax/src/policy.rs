//! Which mints a pool accepts (docs/PLAN.md section 6).
//!
//! Extensions are checked on an allowlist, so an extension this program doesn't know
//! (added to Token-2022 later) is refused. A mint's extensions are fixed when it is
//! created, so they are checked when the pool is created and again at launch. Its
//! authorities can still change between the two (the freeze and mint authorities can be
//! revoked, a transfer fee changed or frozen), so those are checked at launch, the last
//! moment before trading starts.

use anchor_lang::prelude::*;
use anchor_spl::token_2022::spl_token_2022::{
    extension::{
        transfer_fee::TransferFeeConfig, BaseStateWithExtensions, ExtensionType,
        StateWithExtensions,
    },
    state::Mint as MintState,
};
use anchor_spl::token_interface::Mint;

use crate::errors::SniperError;

/// The most a mint's transfer fee may take, now or after a scheduled change: 5%.
pub const MAX_TRANSFER_FEE_BPS: u16 = 500;

/// The side of a pool a mint is on. The two sides differ only in transfer hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MintRole {
    Token,
    Quote,
}

/// Extensions that can't take tokens from the pool or its traders, or stop trading.
fn is_allowed(extension: ExtensionType, role: MintRole) -> bool {
    match extension {
        // A hook can refuse transfers into the vault, and its authority can swap the hook
        // program at any time, so on the launch token it could block every sell (finding
        // H2). The quote mint may have one; its accounts are forwarded to every transfer.
        ExtensionType::TransferHook => role == MintRole::Quote,
        ExtensionType::Uninitialized
        // Vault deltas are measured, so a transfer fee is priced correctly. Its authority
        // and size are checked at launch (`check_transfer_fee`).
        | ExtensionType::TransferFeeConfig
        // A mint can only be closed at zero supply.
        | ExtensionType::MintCloseAuthority
        // Change only the UI amount; the maths uses raw amounts.
        | ExtensionType::InterestBearingConfig
        | ExtensionType::ScaledUiAmount
        // Metadata and groups are descriptive.
        | ExtensionType::MetadataPointer
        | ExtensionType::TokenMetadata
        | ExtensionType::GroupPointer
        | ExtensionType::TokenGroup
        | ExtensionType::GroupMemberPointer
        | ExtensionType::TokenGroupMember => true,
        _ => false,
    }
}

/// A Token-2022 mint's state, or a refusal if it doesn't parse.
fn unpack(data: &[u8]) -> Result<StateWithExtensions<'_, MintState>> {
    StateWithExtensions::<MintState>::unpack(data)
        .map_err(|_| error!(SniperError::ForbiddenMintExtension))
}

/// Refuses a mint with an extension outside the allowlist for its side, such as a
/// permanent delegate (could drain the vaults), non-transferability, pausing, a default
/// frozen state, confidential transfers, or a transfer hook on the launch token. SPL Token
/// mints have no extensions.
pub fn check_extensions(mint: &AccountInfo, role: MintRole) -> Result<()> {
    if *mint.owner == anchor_spl::token::ID {
        return Ok(());
    }
    let data = mint.try_borrow_data()?;
    let extensions = unpack(&data)?
        .get_extension_types()
        .map_err(|_| error!(SniperError::ForbiddenMintExtension))?;
    require!(
        extensions.into_iter().all(|e| is_allowed(e, role)),
        SniperError::ForbiddenMintExtension
    );
    Ok(())
}

/// A transfer fee must be fixed for good, with no `transfer_fee_config_authority`, and at
/// most [`MAX_TRANSFER_FEE_BPS`]: both the fee in force and one scheduled for a later
/// epoch. Otherwise the authority could raise the fee to 100% after launch, so that every
/// sell delivers nothing (finding H1). Applies to both mints of a pool.
pub fn check_transfer_fee(mint: &AccountInfo) -> Result<()> {
    if *mint.owner == anchor_spl::token::ID {
        return Ok(());
    }
    let data = mint.try_borrow_data()?;
    let state = unpack(&data)?;
    let has_fee = state
        .get_extension_types()
        .map_err(|_| error!(SniperError::ForbiddenMintExtension))?
        .contains(&ExtensionType::TransferFeeConfig);
    if !has_fee {
        return Ok(());
    }
    let config = state
        .get_extension::<TransferFeeConfig>()
        .map_err(|_| error!(SniperError::ForbiddenMintExtension))?;
    require!(
        Option::<Pubkey>::from(config.transfer_fee_config_authority).is_none(),
        SniperError::TransferFeeAuthorityPresent
    );
    // The newer fee applies from its epoch on; before that the older one. An older fee
    // that no longer applies stays in the mint, and is ignored.
    let in_force = config.get_epoch_fee(Clock::get()?.epoch);
    for fee in [in_force, &config.newer_transfer_fee] {
        require!(
            u16::from(fee.transfer_fee_basis_points) <= MAX_TRANSFER_FEE_BPS,
            SniperError::TransferFeeAboveCap
        );
    }
    Ok(())
}

/// A launch token must have no freeze authority (which could freeze the vault or buyers)
/// and no mint authority (which could mint and dump without limit).
pub fn check_launch_authorities(mint: &InterfaceAccount<Mint>) -> Result<()> {
    require!(
        mint.freeze_authority.is_none(),
        SniperError::FreezeAuthorityPresent
    );
    require!(
        mint.mint_authority.is_none(),
        SniperError::MintAuthorityPresent
    );
    Ok(())
}
