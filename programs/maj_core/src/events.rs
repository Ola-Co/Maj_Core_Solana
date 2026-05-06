use anchor_lang::prelude::*;

#[event]
pub struct TransactionProposed {
    pub tx_index: u64,
    pub to: Pubkey,
    pub value: u64,
    pub data: Vec<u8>,
    pub proposed_by: Pubkey,
}

#[event]
pub struct TransactionSigned {
    pub tx_index: u64,
    pub admin: Pubkey,
    pub num_signatures: u32,
}

#[event]
pub struct TransactionExecuted {
    pub tx_index: u64,
    pub to: Pubkey,
    pub value: u64,
    pub data: Vec<u8>,
}

#[event]
pub struct SignatureRevoked {
    pub tx_index: u64,
    pub admin: Pubkey,
    pub num_signatures: u32,
}

#[event]
pub struct TransactionCancelled {
    pub tx_index: u64,
}

#[event]
pub struct AdminAdded {
    pub maj_instance: Pubkey,
    pub new_admin: Pubkey,
}

#[event]
pub struct AdminRemoved {
    pub maj_instance: Pubkey,
    pub admin_removed: Pubkey,
}

#[event]
pub struct SignaturesRequiredChanged {
    pub maj_instance: Pubkey,
    pub signatures_required: u64,
}

#[event]
pub struct MajDeployed {
    pub maj_instance: Pubkey,
    pub admins: Vec<Pubkey>,
    pub sigs_required: u64,
}

#[event]
pub struct AdminStatusUpdated {
    pub admin: Pubkey,
    pub maj_instance: Pubkey,
    pub status: bool,
}
