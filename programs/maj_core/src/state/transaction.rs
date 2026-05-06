use anchor_lang::prelude::*;

/// A compact, borsh-serializable representation of an account meta entry
/// used when encoding CPI payloads inside a MajTransaction.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SerializedAccountMeta {
    pub pubkey: Pubkey,       // 32 bytes
    pub is_signer: bool,      // 1 byte
    pub is_writable: bool,    // 1 byte
}                             // total: 34 bytes each

impl SerializedAccountMeta {
    pub const SIZE: usize = 32 + 1 + 1; // 34
}

/// A proposed (and potentially executed) multisig transaction.
/// Seeds: [b"maj_tx", maj_instance.as_ref(), tx_index.to_le_bytes()]
///
/// The account is initialised with minimum space; data/account_metas are
/// Borsh-variable so the account is realloced if the proposal carries a
/// non-empty payload.
#[account]
pub struct MajTransaction {
    pub maj_instance: Pubkey,
    pub tx_index: u64,
    /// Destination pubkey.  For SOL transfers this is the recipient; for
    /// CPI calls it mirrors the program_id field.
    pub to: Pubkey,
    /// Lamports to transfer (0 for pure CPI calls).
    pub value: u64,
    pub proposed_by: Pubkey,
    pub active: bool,
    pub executed: bool,
    /// u32 (not u8) so large admin sets can all sign.
    pub num_signatures: u32,
    /// None = SOL transfer; Some(pid) = CPI call to pid.
    pub program_id: Option<Pubkey>,
    /// Borsh-encoded instruction data for CPI calls (empty for SOL transfer).
    pub data: Vec<u8>,
    /// Ordered accounts for the CPI call.
    pub account_metas: Vec<SerializedAccountMeta>,
    pub bump: u8,
}

impl MajTransaction {
    /// Minimum initial allocation (empty data + empty account_metas).
    pub const INITIAL_SPACE: usize = 8   // discriminator
        + 32    // maj_instance
        + 8     // tx_index
        + 32    // to
        + 8     // value
        + 32    // proposed_by
        + 1     // active
        + 1     // executed
        + 4     // num_signatures (u32)
        + 33    // Option<Pubkey>
        + 4     // data vec prefix
        + 4     // account_metas vec prefix
        + 1;    // bump  →  total: 170

    /// Dynamic space including actual payload sizes.
    pub fn space_for(data_len: usize, meta_count: usize) -> usize {
        8 + 32 + 8 + 32 + 8 + 32 + 1 + 1 + 4 + 33 + 4 + data_len
            + 4 + (meta_count * SerializedAccountMeta::SIZE) + 1
    }
}

/// Existence of this PDA means `signer` has approved `maj_tx`.
/// Seeds: [b"sig", maj_tx.as_ref(), signer.as_ref()]
/// Created by sign_transaction; closed (rent returned) by revoke_signature.
#[account]
pub struct SignatureRecord {
    pub bump: u8,
}

impl SignatureRecord {
    pub const SPACE: usize = 8 + 1; // 9
}
