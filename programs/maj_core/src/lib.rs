use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;
use state::SerializedAccountMeta;

// Replace this placeholder with the real program ID after running:
//   anchor build && anchor keys list
// Then update Anchor.toml [programs.localnet] and [programs.devnet] as well.
declare_id!("Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS");

#[program]
pub mod maj_core {
    use super::*;

    /// Initialise the global registry singleton.  Must be called once before
    /// any `create_maj` call.  Subsequent calls fail because the PDA already
    /// exists (Anchor `init` constraint).
    pub fn initialize_registry(ctx: Context<InitializeRegistry>) -> Result<()> {
        instructions::initialize_registry::handler(ctx)
    }

    /// Factory entry-point.  Creates a new named multisig instance and
    /// registers one AdminRecord PDA per initial admin (passed via
    /// `remaining_accounts`).
    pub fn create_maj<'a>(
        ctx: Context<'a, 'a, 'a, 'a, CreateMaj<'a>>,
        name: String,
        admins: Vec<Pubkey>,
        sigs_required: u64,
    ) -> Result<()> {
        instructions::create_maj::handler(ctx, name, admins, sigs_required)
    }

    /// Propose a transaction.  Any non-blacklisted address may call this.
    /// If the proposer is an admin they auto-sign and auto-execute when the
    /// threshold is already met.
    pub fn propose_transaction<'a>(
        ctx: Context<'a, 'a, 'a, 'a, ProposeTransaction<'a>>,
        to: Pubkey,
        value: u64,
        data: Vec<u8>,
        program_id: Option<Pubkey>,
        account_metas: Vec<SerializedAccountMeta>,
    ) -> Result<()> {
        instructions::propose_transaction::handler(ctx, to, value, data, program_id, account_metas)
    }

    /// Admin signs an active transaction.  Auto-executes if the threshold is
    /// reached.
    pub fn sign_transaction<'a>(ctx: Context<'a, 'a, 'a, 'a, SignTransaction<'a>>) -> Result<()> {
        instructions::sign_transaction::handler(ctx)
    }

    /// Manually execute a transaction that has already reached the threshold.
    /// All destination/CPI accounts must be supplied in `remaining_accounts`.
    pub fn execute_transaction<'a>(ctx: Context<'a, 'a, 'a, 'a, ExecuteTransaction<'a>>) -> Result<()> {
        instructions::execute_transaction::handler(ctx)
    }

    /// Admin revokes their signature from an active transaction.  Closes the
    /// SignatureRecord PDA and returns rent to the admin.
    pub fn revoke_signature(ctx: Context<RevokeSignature>) -> Result<()> {
        instructions::revoke_signature::handler(ctx)
    }

    /// The original proposer cancels an active transaction.  Sets it inactive
    /// without deleting the account (history is preserved).
    pub fn cancel_transaction(ctx: Context<CancelTransaction>) -> Result<()> {
        instructions::cancel_transaction::handler(ctx)
    }

    // ── Governance instructions (onlyMaj — only reachable via execute_transaction CPI) ──

    /// Add a new admin to the multisig.
    pub fn add_admin(ctx: Context<AddAdmin>, new_admin: Pubkey) -> Result<()> {
        instructions::governance::add_admin_handler(ctx, new_admin)
    }

    /// Remove an existing admin (requires > 2 admins remain).
    pub fn remove_admin(ctx: Context<RemoveAdmin>, admin_to_remove: Pubkey) -> Result<()> {
        instructions::governance::remove_admin_handler(ctx, admin_to_remove)
    }

    /// Change the signature threshold (must remain within [2, admins.len()]).
    pub fn change_sigs_required(
        ctx: Context<ChangeSigsRequired>,
        new_sigs_required: u64,
    ) -> Result<()> {
        instructions::governance::change_sigs_required_handler(ctx, new_sigs_required)
    }

    /// Blacklist an address so it cannot propose new transactions.
    pub fn add_blacklist(ctx: Context<AddBlacklist>, target: Pubkey) -> Result<()> {
        instructions::governance::add_blacklist_handler(ctx, target)
    }

    /// Remove an address from the blacklist.
    pub fn remove_blacklist(ctx: Context<RemoveBlacklist>, target: Pubkey) -> Result<()> {
        instructions::governance::remove_blacklist_handler(ctx, target)
    }
}
