use anchor_lang::prelude::*;

use crate::errors::MajError;
use crate::events::TransactionExecuted;
use crate::state::{AdminRecord, MajInstance, MajTransaction};

// ── Shared execution logic ────────────────────────────────────────────────────
//
// Called inline from:
//   • execute_transaction::handler          (explicit execution)
//   • sign_transaction::handler             (auto-execute at threshold)
//   • propose_transaction::handler          (admin auto-sign + auto-execute)
//
// NOT a CPI-to-self — avoids Anchor's mutable account borrow restrictions.
//
// remaining_accounts layout:
//   SOL transfer : [destination_account]
//   CPI call     : [cpi_program_account, account_0, account_1, ...]
//                  All pubkeys in MajTransaction.account_metas must appear here.
//                  The client may also include maj_instance if it is listed in
//                  account_metas (e.g. governance instructions).
pub fn execute_transaction_logic<'info>(
    maj_instance: &mut Account<'info, MajInstance>,
    tx: &mut Account<'info, MajTransaction>,
    remaining_accounts: &[AccountInfo<'info>],
    _program_id: &Pubkey,
) -> Result<()> {
    require!(tx.active, MajError::TransactionNotActive);
    require!(
        tx.num_signatures >= maj_instance.sigs_required as u32,
        MajError::InsufficientSignatures
    );

    // Mark before executing (checks-effects-interactions).
    tx.active = false;
    tx.executed = true;

    // Capture signing seeds BEFORE any potential mutation of maj_instance.
    let name_bytes = maj_instance.name.as_bytes().to_vec();
    let bump_seed = maj_instance.bump;
    let seeds: &[&[u8]] = &[b"maj_instance", name_bytes.as_slice(), &[bump_seed]];

    if tx.program_id.is_none() {
        // ── SOL Transfer ──────────────────────────────────────────────────────
        let from = maj_instance.to_account_info();
        let to = remaining_accounts
            .iter()
            .find(|a| a.key() == tx.to)
            .ok_or(MajError::TransactionFailed)?;

        // Direct lamport manipulation is valid for program-owned accounts.
        **from.try_borrow_mut_lamports()? -= tx.value;
        **to.try_borrow_mut_lamports()? += tx.value;
    } else {
        // ── CPI Call ──────────────────────────────────────────────────────────
        let cpi_program_id = tx.program_id.unwrap();

        let _cpi_program_info = remaining_accounts
            .iter()
            .find(|a| a.key() == cpi_program_id)
            .ok_or(MajError::TransactionFailed)?;

        // Reconstruct AccountInfos in account_metas order.
        // The maj_instance AccountInfo is included if the client passes it in
        // remaining_accounts; it will also be found via its pubkey.
        let maj_instance_info = maj_instance.to_account_info();

        let mut cpi_account_infos: Vec<AccountInfo<'info>> = Vec::new();
        for meta in tx.account_metas.iter() {
            // Prefer the already-loaded maj_instance over the remaining_accounts
            // copy to ensure we use the same Rc handle (avoids RefCell conflicts).
            let info = if meta.pubkey == maj_instance_info.key() {
                maj_instance_info.clone()
            } else {
                remaining_accounts
                    .iter()
                    .find(|a| a.key() == meta.pubkey)
                    .ok_or(MajError::TransactionFailed)?
                    .clone()
            };
            cpi_account_infos.push(info);
        }

        let account_metas: Vec<anchor_lang::solana_program::instruction::AccountMeta> = tx
            .account_metas
            .iter()
            .map(|m| anchor_lang::solana_program::instruction::AccountMeta {
                pubkey: m.pubkey,
                is_signer: m.is_signer,
                is_writable: m.is_writable,
            })
            .collect();

        let instruction = anchor_lang::solana_program::instruction::Instruction {
            program_id: cpi_program_id,
            accounts: account_metas,
            data: tx.data.clone(),
        };

        anchor_lang::solana_program::program::invoke_signed(
            &instruction,
            &cpi_account_infos[..],
            &[seeds],
        )
        .map_err(|_| MajError::TransactionFailed)?;

        // Reload maj_instance to pick up any mutations made by the governance CPI
        // (e.g. add_admin reallocs and modifies the admins Vec).
        // Without this, Anchor's AccountExit would overwrite the CPI changes.
        maj_instance.reload()?;
    }

    emit!(TransactionExecuted {
        tx_index: tx.tx_index,
        to: tx.to,
        value: tx.value,
        data: tx.data.clone(),
    });

    Ok(())
}

// ── Accounts context ─────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct ExecuteTransaction<'info> {
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
        constraint = maj_transaction.num_signatures >= maj_instance.sigs_required as u32
            @ MajError::InsufficientSignatures,
        constraint = maj_transaction.maj_instance == maj_instance.key() @ MajError::OnlyAdmin,
    )]
    pub maj_transaction: Account<'info, MajTransaction>,

    /// Verifies the caller is a registered admin of this instance.
    #[account(
        seeds = [b"admin_record", admin.key().as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
    // remaining_accounts: destination/CPI accounts (including maj_instance if governance CPI).
}

pub fn handler<'info>(ctx: Context<'info, ExecuteTransaction<'info>>) -> Result<()> {
    // Split borrows so both mutable references can coexist.
    let maj_instance = &mut ctx.accounts.maj_instance;
    let maj_transaction = &mut ctx.accounts.maj_transaction;

    execute_transaction_logic(
        maj_instance,
        maj_transaction,
        ctx.remaining_accounts,
        ctx.program_id,
    )
}
