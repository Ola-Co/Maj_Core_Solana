use anchor_lang::prelude::*;

use crate::errors::MajError;
use crate::events::TransactionCancelled;
use crate::state::{AdminRecord, MajInstance, MajTransaction};

// ── Accounts context ─────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct CancelTransaction<'info> {
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
        constraint = maj_transaction.proposed_by == proposer.key()
            @ MajError::OnlyProposerCanCancel,
        constraint = maj_transaction.maj_instance == maj_instance.key() @ MajError::OnlyAdmin,
    )]
    pub maj_transaction: Account<'info, MajTransaction>,

    /// The proposer must be an admin to cancel.
    #[account(
        seeds = [b"admin_record", proposer.key().as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    #[account(mut)]
    pub proposer: Signer<'info>,
}

// ── Handler ──────────────────────────────────────────────────────────────────

pub fn handler(ctx: Context<CancelTransaction>) -> Result<()> {
    ctx.accounts.maj_transaction.active = false;

    emit!(TransactionCancelled {
        tx_index: ctx.accounts.maj_transaction.tx_index,
    });

    Ok(())
}
