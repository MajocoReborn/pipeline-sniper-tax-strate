use anchor_lang::prelude::*;
use sniper_tax_math::MathError;

#[error_code]
pub enum SniperError {
    #[msg("Init data is not valid InitParams")]
    BadInitData,
    #[msg("Tax schedule is outside the allowed bounds")]
    InvalidSchedule,
    #[msg("Caller is not this pool's Pipeline market")]
    NotPipeline,
    #[msg("Unexpected account")]
    WrongAccount,
    #[msg("Trade's trader does not match the trader account")]
    TraderMismatch,
    #[msg("Only the pool's creator can do this")]
    NotCreator,
    #[msg("Pool has not launched")]
    NotLive,
    #[msg("Pool has already launched")]
    AlreadyLaunched,
    #[msg("A launch needs both tokens")]
    OneSidedDeposit,
    #[msg("Not enough input for the requested output")]
    InsufficientInput,
    #[msg("Pool cannot provide that much output")]
    InsufficientLiquidity,
    #[msg("Nothing to claim")]
    NothingToClaim,
    #[msg("Mint has an extension this pool does not accept")]
    ForbiddenMintExtension,
    #[msg("Launch token still has a freeze authority")]
    FreezeAuthorityPresent,
    #[msg("Launch token still has a mint authority")]
    MintAuthorityPresent,
    #[msg("Token and quote mints must differ")]
    SameMint,
    #[msg("Arithmetic overflow")]
    MathOverflow,
    #[msg("Treasury does not match the config")]
    WrongTreasury,
    #[msg("Fee setting is above its cap")]
    FeeAboveCap,
    #[msg("Config admin and treasury must be set")]
    BadConfig,
    #[msg("Only the config admin can do this")]
    NotAdmin,
    #[msg("Signer is not the program's upgrade authority")]
    NotUpgradeAuthority,
    // Added after the first devnet release; appended so earlier codes keep their numbers.
    #[msg("Mint's transfer fee can still be changed")]
    TransferFeeAuthorityPresent,
    #[msg("Mint's transfer fee is above 5%")]
    TransferFeeAboveCap,
    #[msg("A sell cannot carry tax")]
    TaxedSell,
    #[msg("Launch fee must be 0 or at least the rent-exempt minimum")]
    LaunchFeeBelowRent,
    #[msg("Only the proposed admin can accept")]
    NotPendingAdmin,
}

/// Maps a maths error to the program's error.
pub fn math_error(error: MathError) -> Error {
    match error {
        MathError::InvalidSchedule => error!(SniperError::InvalidSchedule),
        MathError::FeeAboveCap => error!(SniperError::FeeAboveCap),
        MathError::InsufficientLiquidity => error!(SniperError::InsufficientLiquidity),
        MathError::Overflow => error!(SniperError::MathOverflow),
    }
}
