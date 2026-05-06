use anchor_lang::prelude::*;

use crate::errors::MajError;
use crate::events::TransactionSigned;
use crate::instructions::execute_transaction::execute_transaction_logic;
use crate::state::{AdminRecord, MajInstance, MajTransaction, SignatureRecord};

// ── Accounts context ─────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct SignTransaction<'info> {
    #[account(
        mut,
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

    /// Creating this PDA atomically enforces the DuplicateSignature guard:
    /// if the admin has already signed, the PDA exists and `init` fails.
    #[account(
        init,
        payer = admin,
        space = SignatureRecord::SPACE,
        seeds = [b"sig", maj_transaction.key().as_ref(), admin.key().as_ref()],
        bump,
    )]
    pub signature_record: Account<'info, SignatureRecord>,

    /// Verifies the signer is a registered admin of this instance.
    #[account(
        seeds = [b"admin_record", admin.key().as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
    // remaining_accounts: destination/CPI accounts required if auto-execute fires.
}

// ── Handler ──────────────────────────────────────────────────────────────────

pub fn handler<'a>(ctx: Context<'a, 'a, 'a, 'a, SignTransaction<'a>>) -> Result<()> {
    ctx.accounts.signature_record.bump = ctx.bumps.signature_record;

    let admin_key = ctx.accounts.admin.key();
    let tx_index = ctx.accounts.maj_transaction.tx_index;

    ctx.accounts.maj_transaction.num_signatures += 1;
    let num_sigs = ctx.accounts.maj_transaction.num_signatures;
    let sigs_required = ctx.accounts.maj_instance.sigs_required;

    emit!(TransactionSigned {
        tx_index,
        admin: admin_key,
        num_signatures: num_sigs,
    });

    // Auto-execute inline when threshold is reached.
    if num_sigs >= sigs_required as u32 {
        let maj_instance = &mut ctx.accounts.maj_instance;
        let maj_transaction = &mut ctx.accounts.maj_transaction;
        execute_transaction_logic(
            maj_instance,
            maj_transaction,
            ctx.remaining_accounts,
            ctx.program_id,
        )?;
    }

    Ok(())
}
