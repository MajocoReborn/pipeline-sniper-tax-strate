//! Token-2022 transfer hook used only in tests. It counts every transfer of its mint in a
//! counter account, which it lists as its one extra account. A test can then see that
//! the hook ran and that the extra account reached it through every program in between.
//! Modeled on Pipeline's `programs/test-hook` (MetalLegBob/pipeline, Apache-2.0).
//!
//! Instructions:
//! - `initialize`: accounts payer (signer), validation PDA `["extra-account-metas", mint]`,
//!   counter PDA `["counter", mint]`, mint, system program. Creates both accounts.
//! - Execute (the transfer-hook interface): accounts source, mint, destination, authority,
//!   validation, counter. Adds one to the counter.

use solana_program::{
    account_info::AccountInfo,
    entrypoint,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};

/// `sha256("spl-transfer-hook-interface:execute")[..8]`
const EXECUTE: [u8; 8] = [105, 37, 101, 197, 75, 251, 102, 26];
/// This program's own setup instruction.
const INITIALIZE: [u8; 8] = *b"cnthkini";

entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    match data.get(..8) {
        Some(d) if d == EXECUTE => execute(program_id, accounts),
        Some(d) if d == INITIALIZE => initialize(program_id, accounts),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn counter_address(program_id: &Pubkey, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"counter", mint.as_ref()], program_id)
}

fn execute(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let mint = accounts.get(1).ok_or(ProgramError::NotEnoughAccountKeys)?;
    let counter = accounts.get(5).ok_or(ProgramError::NotEnoughAccountKeys)?;
    if *counter.key != counter_address(program_id, mint.key).0 || counter.owner != program_id {
        return Err(ProgramError::InvalidAccountData);
    }
    let mut data = counter.try_borrow_mut_data()?;
    let count = u64::from_le_bytes(data[..8].try_into().unwrap());
    data[..8].copy_from_slice(&(count + 1).to_le_bytes());
    Ok(())
}

fn initialize(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let [payer, validation, counter, mint, system_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let (validation_key, validation_bump) =
        Pubkey::find_program_address(&[b"extra-account-metas", mint.key.as_ref()], program_id);
    let (counter_key, counter_bump) = counter_address(program_id, mint.key);
    if *validation.key != validation_key || *counter.key != counter_key {
        return Err(ProgramError::InvalidSeeds);
    }

    // TLV entry for Execute: discriminator, length, then a PodSlice holding one 35-byte
    // ExtraAccountMeta: type 0 (fixed address), the counter, not a signer, writable.
    let mut tlv = EXECUTE.to_vec();
    tlv.extend_from_slice(&(4u32 + 35).to_le_bytes());
    tlv.extend_from_slice(&1u32.to_le_bytes());
    tlv.push(0);
    tlv.extend_from_slice(counter_key.as_ref());
    tlv.push(0);
    tlv.push(1);

    create_pda(
        payer,
        validation,
        system_program,
        program_id,
        tlv.len(),
        &[
            b"extra-account-metas",
            mint.key.as_ref(),
            &[validation_bump],
        ],
    )?;
    validation.try_borrow_mut_data()?.copy_from_slice(&tlv);
    create_pda(
        payer,
        counter,
        system_program,
        program_id,
        8,
        &[b"counter", mint.key.as_ref(), &[counter_bump]],
    )
}

fn create_pda<'a>(
    payer: &AccountInfo<'a>,
    account: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    owner: &Pubkey,
    space: usize,
    seeds: &[&[u8]],
) -> ProgramResult {
    let lamports = Rent::get()?.minimum_balance(space);
    // System program CreateAccount: u32 tag 0, lamports, space, owner.
    let mut data = 0u32.to_le_bytes().to_vec();
    data.extend_from_slice(&lamports.to_le_bytes());
    data.extend_from_slice(&(space as u64).to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    let create_account = Instruction {
        program_id: *system_program.key,
        accounts: vec![
            AccountMeta::new(*payer.key, true),
            AccountMeta::new(*account.key, true),
        ],
        data,
    };
    invoke_signed(
        &create_account,
        &[payer.clone(), account.clone(), system_program.clone()],
        &[seeds],
    )
}
