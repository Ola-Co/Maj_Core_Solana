use anchor_lang::prelude::*;
use anchor_lang::system_program::{create_account, CreateAccount};

use crate::errors::MajError;
use crate::events::{AdminStatusUpdated, MajDeployed};
use crate::state::{AdminRecord, MajInstance, MajNameRecord, MajRegistry};

#[derive(Accounts)]
#[instruction(name: String, admins: Vec<Pubkey>, sigs_required: u64)]
pub struct CreateMaj<'info> {
    #[account(
        mut,
        seeds = [b"maj_registry"],
        bump = registry.bump,
    )]
    pub registry: Account<'info, MajRegistry>,

    /// Name reservation — init fails with "already in use" if the name is taken.
    #[account(
        init,
        payer = payer,
        space = MajNameRecord::SPACE,
        seeds = [b"maj_name", name.as_bytes()],
        bump,
    )]
    pub name_record: Account<'info, MajNameRecord>,

    /// The multisig wallet PDA — this account IS the SOL treasury.
    #[account(
        init,
        payer = payer,
        space = MajInstance::INITIAL_SPACE,
        seeds = [b"maj_instance", name.as_bytes()],
        bump,
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
    // remaining_accounts: one AdminRecord PDA per admin in order.
}

pub fn handler<'info>(
    ctx: Context<'info, CreateMaj<'info>>,
    name: String,
    admins: Vec<Pubkey>,
    sigs_required: u64,
) -> Result<()> {
    // ── Validation ──────────────────────────────────────────────────────────
    require!(name.len() <= 64, MajError::NameTooLong);
    require!(admins.len() >= 2, MajError::TwoAdminMinimum);
    require!(sigs_required >= 2, MajError::TooFewSignaturesRequired);
    require!(
        sigs_required <= admins.len() as u64,
        MajError::TooManySignaturesRequired
    );

    let mut seen = std::collections::HashSet::new();
    for admin in &admins {
        require!(*admin != Pubkey::default(), MajError::ZeroAddress);
        require!(seen.insert(*admin), MajError::DuplicateAdminAddress);
    }

    require!(
        ctx.remaining_accounts.len() >= admins.len(),
        MajError::ZeroAddress // used as sanity guard; client must pass all AdminRecord PDAs
    );

    // ── Realloc MajInstance if admins > 10 ──────────────────────────────────
    if admins.len() > 10 {
        let new_space = MajInstance::space_for(admins.len());
        let rent = Rent::get()?;
        let current_lamports = ctx.accounts.maj_instance.to_account_info().lamports();
        let required_lamports = rent.minimum_balance(new_space);

        if required_lamports > current_lamports {
            let extra = required_lamports - current_lamports;
            **ctx.accounts.payer.try_borrow_mut_lamports()? -= extra;
            **ctx.accounts.maj_instance.to_account_info().try_borrow_mut_lamports()? += extra;
        }

        ctx.accounts
            .maj_instance
            .to_account_info()
            .resize(new_space)?;;
    }

    // ── Populate MajInstance ─────────────────────────────────────────────────
    let maj_instance_key = ctx.accounts.maj_instance.key();
    {
        let instance = &mut ctx.accounts.maj_instance;
        instance.name = name.clone();
        instance.admins = admins.clone();
        instance.sigs_required = sigs_required;
        instance.tx_count = 0;
        instance.bump = ctx.bumps.maj_instance;
    }

    // ── Populate MajNameRecord ───────────────────────────────────────────────
    {
        let name_record = &mut ctx.accounts.name_record;
        name_record.maj_instance = maj_instance_key;
        name_record.bump = ctx.bumps.name_record;
    }

    // ── Increment registry ───────────────────────────────────────────────────
    ctx.accounts.registry.total_instances += 1;

    // ── Create one AdminRecord PDA per admin via remaining_accounts ──────────
    for (i, admin) in admins.iter().enumerate() {
        let admin_record_info = &ctx.remaining_accounts[i];

        let (expected_pda, bump) = Pubkey::find_program_address(
            &[b"admin_record", admin.as_ref(), maj_instance_key.as_ref()],
            ctx.program_id,
        );
        require!(
            admin_record_info.key() == expected_pda,
            MajError::ZeroAddress // wrong PDA provided
        );

        let space = AdminRecord::SPACE;
        let rent = Rent::get()?.minimum_balance(space);

        create_account(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.key(),
                CreateAccount {
                    from: ctx.accounts.payer.to_account_info(),
                    to: admin_record_info.clone(),
                },
                &[&[
                    b"admin_record",
                    admin.as_ref(),
                    maj_instance_key.as_ref(),
                    &[bump],
                ]],
            ),
            rent,
            space as u64,
            ctx.program_id,
        )?;

        // Write discriminator + borsh-encoded AdminRecord data.
        let record = AdminRecord {
            admin: *admin,
            maj_instance: maj_instance_key,
            bump,
        };
        let mut data = admin_record_info.try_borrow_mut_data()?;
        record.try_serialize(&mut &mut data[..])?;

        emit!(AdminStatusUpdated {
            admin: *admin,
            maj_instance: maj_instance_key,
            status: true,
        });
    }

    emit!(MajDeployed {
        maj_instance: maj_instance_key,
        admins,
        sigs_required,
    });

    Ok(())
}
