use anchor_lang::prelude::*;

#[error_code]
pub enum MajError {
    #[msg("Duplicate admin address")]
    DuplicateAdminAddress,

    #[msg("Zero address not allowed")]
    ZeroAddress,

    #[msg("Too many signatures required")]
    TooManySignaturesRequired,

    #[msg("Too few signatures required (minimum 2)")]
    TooFewSignaturesRequired,

    #[msg("Caller is not an admin")]
    OnlyAdmin,

    #[msg("Transaction is not active")]
    TransactionNotActive,

    #[msg("Admin has already signed this transaction")]
    DuplicateSignature,

    #[msg("Insufficient signatures to execute")]
    InsufficientSignatures,

    #[msg("CPI call failed")]
    TransactionFailed,

    #[msg("Admin has not signed this transaction")]
    UserHasNotSigned,

    #[msg("Only the proposer can cancel this transaction")]
    OnlyProposerCanCancel,

    #[msg("Instruction must be called via multisig execution (onlyMaj)")]
    OnlyMaj,

    #[msg("Address is not an admin")]
    AddressIsNotAdmin,

    #[msg("Minimum two admins required")]
    TwoAdminMinimum,

    #[msg("A Maj with this name already exists")]
    NameTaken,

    #[msg("No Maj found with this name")]
    NameNotFound,

    #[msg("Address is not blacklisted")]
    NotBlacklisted,

    #[msg("Address is blacklisted")]
    AddressIsBlacklisted,

    #[msg("Name exceeds maximum length (64 chars)")]
    NameTooLong,
}
