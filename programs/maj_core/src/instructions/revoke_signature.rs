use anchor_lang::prelude::*;

use crate::errors::MajError;
use crate::events::SignatureRevoked;
use crate::state::{AdminRecord, MajInstance, MajTransaction, SignatureRecord};

// ── Accounts context ─────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct RevokeSignature<'info> {
    #[account(
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(
        mut,
        seeds = [
            b"maj_tx",
            maj_instance.key().as_ref(),
            &maj_transaction.tx_index.to_le_bytes(),
        ],
        bump = maj_transaction.bump,
        constraint = maj_transaction.active @ MajError::TransactionNotActive,
        constraint = maj_transaction.maj_instance == maj_instance.key() @ MajError::OnlyAdmin,
    )]
    pub maj_transaction: Account<'info, MajTransaction>,

    /// Closing this PDA enforces the UserHasNotSigned guard:
    /// if the PDA does not exist, Anchor's constraint resolution fails before
    /// the handler runs.
    #[account(
        mut,
        close = admin,
        seeds = [b"sig", maj_transaction.key().as_ref(), admin.key().as_ref()],
        bump = signature_record.bump,
    )]
    pub signature_record: Account<'info, SignatureRecord>,

    /// Verifies the caller is a registered admin.
    #[account(
        seeds = [b"admin_record", admin.key().as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    #[account(mut)]
    pub admin: Signer<'info>,
}

// ── Handler ──────────────────────────────────────────────────────────────────

pub fn handler(ctx: Context<RevokeSignature>) -> Result<()> {
    let tx = &mut ctx.accounts.maj_transaction;
    tx.num_signatures -= 1;

    emit!(SignatureRevoked {
        tx_index: tx.tx_index,
        admin: ctx.accounts.admin.key(),
        num_signatures: tx.num_signatures,
    });

    Ok(())
}
