use anchor_lang::prelude::*;

/// The core multisig wallet PDA.  Each unique Maj instance is identified
/// by its human-readable name.  This account IS the treasury — it holds SOL.
#[account]
pub struct MajInstance {
    /// Human-readable identifier (max 64 UTF-8 bytes).
    pub name: String,
    /// Ordered list of admin pubkeys (kept in sync with AdminRecord PDAs).
    pub admins: Vec<Pubkey>,
    /// Minimum signatures needed to execute a transaction (always >= 2).
    pub sigs_required: u64,
    /// Monotonically-increasing counter used as the MajTransaction PDA seed.
    pub tx_count: u64,
    /// Canonical bump for the PDA derived from [b"maj_instance", name.as_bytes()].
    pub bump: u8,
}

impl MajInstance {
    /// Initial allocation supports up to 10 admins to avoid an immediate
    /// realloc in typical deployments.
    pub const INITIAL_SPACE: usize = 8      // discriminator
        + 4 + 64                             // name  (4-byte prefix + 64 chars)
        + 4 + (10 * 32)                      // admins vec (initial 10 slots)
        + 8                                  // sigs_required
        + 8                                  // tx_count
        + 1;                                 // bump  →  total: 417

    /// Dynamic space for `n` admins.
    pub fn space_for(admin_count: usize) -> usize {
        8 + 4 + 64 + 4 + (admin_count * 32) + 8 + 8 + 1
    }
}

/// Reserves a name so no two instances share the same identifier.
/// Seeds: [b"maj_name", name.as_bytes()]
#[account]
pub struct MajNameRecord {
    pub maj_instance: Pubkey,
    pub bump: u8,
}

impl MajNameRecord {
    pub const SPACE: usize = 8 + 32 + 1; // 41
}

/// Records that `admin` is a member of `maj_instance`.
/// Seeds: [b"admin_record", admin.as_ref(), maj_instance.as_ref()]
///
/// Existence of this PDA is the O(1) membership check used by instruction
/// account constraints.  It also enables efficient off-chain reverse lookups
/// via getProgramAccounts with a memcmp filter at offset 8.
#[account]
pub struct AdminRecord {
    /// Offset 8..40 — filter target for getProgramAccounts queries.
    pub admin: Pubkey,
    /// Offset 40..72 — secondary filter target.
    pub maj_instance: Pubkey,
    pub bump: u8,
}

impl AdminRecord {
    pub const SPACE: usize = 8 + 32 + 32 + 1; // 73
}

/// Records that `target` is blacklisted from proposing transactions.
/// Seeds: [b"blacklist", maj_instance.as_ref(), target.as_ref()]
/// Existence = blacklisted; closed on remove.
#[account]
pub struct BlacklistRecord {
    pub bump: u8,
}

impl BlacklistRecord {
    pub const SPACE: usize = 8 + 1; // 9
}
