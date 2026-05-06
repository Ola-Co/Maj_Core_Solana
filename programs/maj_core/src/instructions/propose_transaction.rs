use anchor_lang::prelude::*;
use anchor_lang::system_program::{create_account, CreateAccount};

use crate::errors::MajError;
use crate::events::{TransactionProposed, TransactionSigned};
use crate::instructions::execute_transaction::execute_transaction_logic;
use crate::state::{MajInstance, MajTransaction, SerializedAccountMeta, SignatureRecord};

// ── Accounts context ─────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(
    to: Pubkey,
    value: u64,
    data: Vec<u8>,
    program_id: Option<Pubkey>,
    account_metas: Vec<SerializedAccountMeta>
)]
pub struct ProposeTransaction<'info> {
    #[account(
        mut,
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(
        init,
        payer = proposer,
        space = MajTransaction::space_for(data.len(), account_metas.len()),
        seeds = [
            b"maj_tx",
            maj_instance.key().as_ref(),
            &maj_instance.tx_count.to_le_bytes(),
        ],
        bump,
    )]
    pub maj_transaction: Account<'info, MajTransaction>,

    /// CHECK: checked against blacklist/admin PDAs in the handler.
    #[account(mut)]
    pub proposer: Signer<'info>,

    pub system_program: Program<'info, System>,
    // remaining_accounts expected layout:
    //   [0] BlacklistRecord PDA  — always required (lamport check)
    //   [1] AdminRecord PDA      — always required (lamport check)
    //   [2] SignatureRecord PDA  — required if proposer is admin
    //   [3..] destination/CPI accounts — required if auto-execute triggers
}

// ── Handler ──────────────────────────────────────────────────────────────────

pub fn handler<'info>(
    ctx: Context<'info, ProposeTransaction<'info>>,
    to: Pubkey,
    value: u64,
    data: Vec<u8>,
    program_id: Option<Pubkey>,
    account_metas: Vec<SerializedAccountMeta>,
) -> Result<()> {
    let maj_instance_key = ctx.accounts.maj_instance.key();
    let proposer_key = ctx.accounts.proposer.key();
    let tx_index = ctx.accounts.maj_instance.tx_count;

    // ── Blacklist check ───────────────────────────────────────────────────────
    let (blacklist_pda, _) = Pubkey::find_program_address(
        &[b"blacklist", maj_instance_key.as_ref(), proposer_key.as_ref()],
        ctx.program_id,
    );
    let is_blacklisted = ctx
        .remaining_accounts
        .iter()
        .find(|a| a.key() == blacklist_pda)
        .map(|a| a.lamports() > 0)
        .unwrap_or(false);
    require!(!is_blacklisted, MajError::AddressIsBlacklisted);

    // ── Admin check ───────────────────────────────────────────────────────────
    let (admin_record_pda, _) = Pubkey::find_program_address(
        &[b"admin_record", proposer_key.as_ref(), maj_instance_key.as_ref()],
        ctx.program_id,
    );
    let is_admin = ctx
        .remaining_accounts
        .iter()
        .find(|a| a.key() == admin_record_pda)
        .map(|a| a.lamports() > 0)
        .unwrap_or(false);

    // ── Initialise MajTransaction ─────────────────────────────────────────────
    {
        let tx = &mut ctx.accounts.maj_transaction;
        tx.maj_instance = maj_instance_key;
        tx.tx_index = tx_index;
        tx.to = to;
        tx.value = value;
        tx.proposed_by = proposer_key;
        tx.active = true;
        tx.executed = false;
        tx.num_signatures = 0;
        tx.program_id = program_id;
        tx.data = data.clone();
        tx.account_metas = account_metas;
        tx.bump = ctx.bumps.maj_transaction;
    }

    ctx.accounts.maj_instance.tx_count += 1;

    emit!(TransactionProposed {
        tx_index,
        to,
        value,
        data: data.clone(),
        proposed_by: proposer_key,
    });

    // ── Admin auto-sign branch ────────────────────────────────────────────────
    if is_admin {
        let maj_tx_key = ctx.accounts.maj_transaction.key();

        // Locate the SignatureRecord PDA in remaining_accounts.
        let (expected_sig_pda, sig_bump) = Pubkey::find_program_address(
            &[b"sig", maj_tx_key.as_ref(), proposer_key.as_ref()],
            ctx.program_id,
        );

        let sig_record_info = ctx
            .remaining_accounts
            .iter()
            .find(|a| a.key() == expected_sig_pda)
            .ok_or(MajError::OnlyAdmin)?; // client must include this for admin proposals

        // Create SignatureRecord PDA.
        let space = SignatureRecord::SPACE;
        let rent = Rent::get()?.minimum_balance(space);

        create_account(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.key(),
                CreateAccount {
                    from: ctx.accounts.proposer.to_account_info(),
                    to: sig_record_info.clone(),
                },
                &[&[
                    b"sig",
                    maj_tx_key.as_ref(),
                    proposer_key.as_ref(),
                    &[sig_bump],
                ]],
            ),
            rent,
            space as u64,
            ctx.program_id,
        )?;

        // Write discriminator + bump.
        {
            let record = SignatureRecord { bump: sig_bump };
            let mut raw = sig_record_info.try_borrow_mut_data()?;
            record.try_serialize(&mut &mut raw[..])?;
        }

        ctx.accounts.maj_transaction.num_signatures += 1;

        emit!(TransactionSigned {
            tx_index,
            admin: proposer_key,
            num_signatures: ctx.accounts.maj_transaction.num_signatures,
        });

        // Auto-execute if threshold is already met after the auto-sign.
        let num_sigs = ctx.accounts.maj_transaction.num_signatures;
        let sigs_required = ctx.accounts.maj_instance.sigs_required;

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
    }

    Ok(())
}
