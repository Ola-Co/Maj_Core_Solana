use anchor_lang::prelude::*;

use crate::state::MajRegistry;

#[derive(Accounts)]
pub struct InitializeRegistry<'info> {
    #[account(
        init,
        payer = payer,
        space = MajRegistry::SPACE,
        seeds = [b"maj_registry"],
        bump,
    )]
    pub registry: Account<'info, MajRegistry>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitializeRegistry>) -> Result<()> {
    let registry = &mut ctx.accounts.registry;
    registry.total_instances = 0;
    registry.bump = ctx.bumps.registry;
    Ok(())
}
