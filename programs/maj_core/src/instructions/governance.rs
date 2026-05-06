use anchor_lang::prelude::*;

use crate::errors::MajError;
use crate::events::{AdminAdded, AdminRemoved, AdminStatusUpdated, SignaturesRequiredChanged};
use crate::state::{AdminRecord, BlacklistRecord, MajInstance};

// ─────────────────────────────────────────────────────────────────────────────
// add_admin
// ─────────────────────────────────────────────────────────────────────────────
//
// Guard: `maj_instance` must be a signer (the `signer` attribute).
// Only `invoke_signed` from inside `execute_transaction_logic` can make a PDA
// sign, so direct wallet calls will always fail this constraint — equivalent to
// Solidity's `onlyMaj` modifier.

#[derive(Accounts)]
#[instruction(new_admin: Pubkey)]
pub struct AddAdmin<'info> {
    #[account(
        mut,
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer, // ← THE onlyMaj GUARD
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(
        init,
        payer = payer,
        space = AdminRecord::SPACE,
        seeds = [b"admin_record", new_admin.as_ref(), maj_instance.key().as_ref()],
        bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn add_admin_handler(ctx: Context<AddAdmin>, new_admin: Pubkey) -> Result<()> {
    require!(new_admin != Pubkey::default(), MajError::ZeroAddress);
    require!(
        !ctx.accounts.maj_instance.admins.contains(&new_admin),
        MajError::DuplicateAdminAddress
    );

    // Realloc MajInstance if the expanded admins Vec needs more space.
    let new_admin_count = ctx.accounts.maj_instance.admins.len() + 1;
    let new_space = MajInstance::space_for(new_admin_count);
    let current_space = ctx.accounts.maj_instance.to_account_info().data_len();

    if new_space > current_space {
        let rent = Rent::get()?;
        let extra_lamports = rent
            .minimum_balance(new_space)
            .saturating_sub(rent.minimum_balance(current_space));

        if extra_lamports > 0 {
            **ctx.accounts.payer.try_borrow_mut_lamports()? -= extra_lamports;
            **ctx.accounts.maj_instance.to_account_info().try_borrow_mut_lamports()? +=
                extra_lamports;
        }

        ctx.accounts
            .maj_instance
            .to_account_info()
            .resize(new_space)?;
    }

    ctx.accounts.maj_instance.admins.push(new_admin);

    let record = &mut ctx.accounts.admin_record;
    record.admin = new_admin;
    record.maj_instance = ctx.accounts.maj_instance.key();
    record.bump = ctx.bumps.admin_record;

    let maj_instance_key = ctx.accounts.maj_instance.key();

    emit!(AdminAdded {
        maj_instance: maj_instance_key,
        new_admin,
    });
    emit!(AdminStatusUpdated {
        admin: new_admin,
        maj_instance: maj_instance_key,
        status: true,
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// remove_admin
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(admin_to_remove: Pubkey)]
pub struct RemoveAdmin<'info> {
    #[account(
        mut,
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer, // ← onlyMaj guard
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(
        mut,
        close = rent_receiver,
        seeds = [b"admin_record", admin_to_remove.as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    /// CHECK: receives the rent from the closed AdminRecord.
    #[account(mut)]
    pub rent_receiver: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

pub fn remove_admin_handler(ctx: Context<RemoveAdmin>, admin_to_remove: Pubkey) -> Result<()> {
    let instance = &mut ctx.accounts.maj_instance;

    require!(
        instance.admins.contains(&admin_to_remove),
        MajError::AddressIsNotAdmin
    );
    // Must keep at least 2 admins after removal.
    require!(instance.admins.len() > 2, MajError::TwoAdminMinimum);

    // swap_remove is O(1); order is not guaranteed — acceptable for multisig.
    if let Some(pos) = instance.admins.iter().position(|a| *a == admin_to_remove) {
        instance.admins.swap_remove(pos);
    }

    // Auto-adjust threshold so the multisig cannot become permanently stuck.
    if instance.sigs_required > instance.admins.len() as u64 {
        instance.sigs_required = instance.admins.len() as u64;
    }

    let maj_instance_key = instance.key();

    emit!(AdminRemoved {
        maj_instance: maj_instance_key,
        admin_removed: admin_to_remove,
    });
    emit!(AdminStatusUpdated {
        admin: admin_to_remove,
        maj_instance: maj_instance_key,
        status: false,
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// change_sigs_required
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct ChangeSigsRequired<'info> {
    #[account(
        mut,
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer, // ← onlyMaj guard
    )]
    pub maj_instance: Account<'info, MajInstance>,
}

pub fn change_sigs_required_handler(
    ctx: Context<ChangeSigsRequired>,
    new_sigs_required: u64,
) -> Result<()> {
    let instance = &mut ctx.accounts.maj_instance;
    require!(new_sigs_required >= 2, MajError::TooFewSignaturesRequired);
    require!(
        new_sigs_required <= instance.admins.len() as u64,
        MajError::TooManySignaturesRequired
    );

    instance.sigs_required = new_sigs_required;

    emit!(SignaturesRequiredChanged {
        maj_instance: instance.key(),
        signatures_required: new_sigs_required,
    });

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// add_blacklist
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(target: Pubkey)]
pub struct AddBlacklist<'info> {
    #[account(
        mut,
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer, // ← onlyMaj guard
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(
        init,
        payer = payer,
        space = BlacklistRecord::SPACE,
        seeds = [b"blacklist", maj_instance.key().as_ref(), target.as_ref()],
        bump,
    )]
    pub blacklist_record: Account<'info, BlacklistRecord>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn add_blacklist_handler(ctx: Context<AddBlacklist>, _target: Pubkey) -> Result<()> {
    ctx.accounts.blacklist_record.bump = ctx.bumps.blacklist_record;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// remove_blacklist
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
#[instruction(target: Pubkey)]
pub struct RemoveBlacklist<'info> {
    #[account(
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer, // ← onlyMaj guard
    )]
    pub maj_instance: Account<'info, MajInstance>,

    /// If this PDA does not exist, Anchor's account resolution fails before
    /// the handler runs — equivalent to `require!(isBlacklisted, NotBlacklisted)`.
    #[account(
        mut,
        close = rent_receiver,
        seeds = [b"blacklist", maj_instance.key().as_ref(), target.as_ref()],
        bump = blacklist_record.bump,
    )]
    pub blacklist_record: Account<'info, BlacklistRecord>,

    /// CHECK: receives the rent from the closed BlacklistRecord.
    #[account(mut)]
    pub rent_receiver: UncheckedAccount<'info>,
}

pub fn remove_blacklist_handler(ctx: Context<RemoveBlacklist>, _target: Pubkey) -> Result<()> {
    // The `close = rent_receiver` constraint handles everything.
    let _ = &ctx.accounts.blacklist_record; // suppress unused-variable warning
    Ok(())
}
