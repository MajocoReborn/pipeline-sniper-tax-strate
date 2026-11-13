//! Token helpers that work with SPL Token and Token-2022 alike.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_spl::token_2022::spl_token_2022;

use crate::errors::SniperError;

/// A token account's balance, read from its raw data. SPL Token and Token-2022 share the
/// base layout, with the amount at bytes 64..72.
pub fn balance(account: &AccountInfo) -> Result<u64> {
    let data = account.try_borrow_data()?;
    let amount = data.get(64..72).ok_or(SniperError::WrongAccount)?;
    Ok(u64::from_le_bytes(amount.try_into().unwrap()))
}

/// `transfer_checked` with transfer-hook accounts appended, so Token-2022 can resolve and
/// invoke a mint's hook. Anchor's own `transfer_checked` CPI drops them. Accounts the hook
/// doesn't use are ignored by the token program.
#[allow(clippy::too_many_arguments)]
pub fn transfer_checked<'info>(
    token_program: &AccountInfo<'info>,
    from: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    hook_accounts: &[AccountInfo<'info>],
    amount: u64,
    decimals: u8,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let mut instruction = spl_token_2022::instruction::transfer_checked(
        token_program.key,
        from.key,
        mint.key,
        to.key,
        authority.key,
        &[],
        amount,
        decimals,
    )?;
    let mut infos = Vec::with_capacity(5 + hook_accounts.len());
    infos.extend([from.clone(), mint.clone(), to.clone(), authority.clone()]);
    for account in hook_accounts {
        instruction.accounts.push(if account.is_writable {
            AccountMeta::new(*account.key, false)
        } else {
            AccountMeta::new_readonly(*account.key, false)
        });
        infos.push(account.clone());
    }
    infos.push(token_program.clone());
    invoke_signed(&instruction, &infos, signer_seeds)?;
    Ok(())
}
