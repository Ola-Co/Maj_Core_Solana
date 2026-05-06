# Maj Core — Anchor Program Implementation Guide

## Table of Contents

1. [Overview](#overview)
2. [Source Contract Reference](#source-contract-reference)
3. [Architecture](#architecture)
4. [Account Structures & Space Calculations](#account-structures--space-calculations)
5. [File & Module Structure](#file--module-structure)
6. [Error Types](#error-types)
7. [Events](#events)
8. [State Definitions](#state-definitions)
9. [Instruction Specifications](#instruction-specifications)
10. [CPI Payload & Execution Model](#cpi-payload--execution-model)
11. [PDA Signer Pattern (onlyMaj Equivalent)](#pda-signer-pattern-onlymaj-equivalent)
12. [Realloc Pattern](#realloc-pattern)
13. [Testing Requirements](#testing-requirements)
14. [Cargo.toml & Anchor.toml](#cargotoml--anchortom)
15. [Deployment](#deployment)
16. [Important Implementation Notes](#important-implementation-notes)

---

## Overview

This document is the canonical implementation guide for porting the Maj multisig system from Ethereum/Solidity to Solana/Anchor. The guide is exhaustive and self-contained. The implementing agent should follow it top-to-bottom without requiring any external context.

**What is being built:** A single Anchor program (`maj_core`) that replicates:
- `MajFactory.sol` — A factory/registry that creates named multisig instances and tracks admin membership across instances.
- `Maj.sol` — A multisig wallet where a configurable threshold of admins must approve transactions before execution. Governance changes (adding/removing admins, changing thresholds, blacklisting) are themselves subject to multisig approval.

**Key translation decisions:**
- EVM contract instances → Solana PDAs (each "Maj" is a PDA account, not a deployed program).
- Solidity `mapping` → PDA existence checks (O(1) via account lookup).
- `onlyMaj` modifier → PDA signer via `invoke_signed` from `execute_transaction`.
- `MajFactory.adminMajContracts` reverse lookup → `AdminRecord` PDAs queried client-side via `getProgramAccounts`.
- Dynamic arrays → Anchor `realloc` for unbounded growth.

---

## Source Contract Reference

### Maj.sol — Behaviour Summary

```
State:
  string majName
  uint256 signaturesRequired           // minimum 2
  address[] admins
  mapping(address => bool) isAdmin
  mapping(address => bool) isBlacklisted
  Transaction[] transactions
  mapping(uint256 => mapping(address => bool)) hasSigned

Transaction struct:
  address to
  uint256 value
  bytes data
  address proposedBy
  bool active
  bool executed
  uint8 numSignatures

Modifiers:
  onlyAdmin         — msg.sender must be in isAdmin
  onlyActive(idx)   — transactions[idx].active must be true
  onlyMaj           — msg.sender must == address(this)   ← governance guard
  onlyAllowed       — msg.sender must NOT be blacklisted

Key behaviours:
  proposeTransaction:
    - Non-blacklisted address may call
    - If caller is admin → auto-signs (calls signTransaction internally)
    - If caller is non-admin → proposes without signing
    - Admin auto-sign triggers auto-execute if threshold reached

  signTransaction:
    - Admin only, tx must be active
    - Duplicate signature reverts
    - Auto-executes if numSignatures >= signaturesRequired

  executeTransaction:
    - Admin only, tx must be active
    - Requires numSignatures >= signaturesRequired
    - Calls target.call{value}(data)
    - Sets executed=true, active=false

  revokeSignature:
    - Admin only, tx active
    - Must have signed previously
    - Decrements numSignatures

  cancelTransaction:
    - Admin only, tx active
    - Only the original proposer can cancel
    - Sets active=false

  Governance (all require onlyMaj — must come through execute_transaction CPI):
    addAdmin(address)
    removeAdmin(address)        — enforces admins.length > 2
    changeSignaturesRequired(uint256)
    addBlacklist(address)
    removeBlacklist(address)
```

### MajFactory.sol — Behaviour Summary

```
State:
  Maj[] deployedContracts
  mapping(string => address) majName
  mapping(string => bool) isMajNameNotAvailable
  mapping(address => address[]) adminMajContracts

Key behaviours:
  createMajContract(name, admins[], sigsRequired):
    - Reverts if name taken
    - Deploys new Maj, registers name
    - Pushes Maj address into each admin's adminMajContracts list

  updateAdminStatus(admin, majContract, status):
    - Called BY Maj on addAdmin/removeAdmin
    - Pushes or removes majContract from admin's list

  getAdminContracts(admin) → address[]   ← on Solana: done client-side via getProgramAccounts
  getDeployedMajNames(name) → address    ← on Solana: MajNameRecord PDA lookup
```

---

## Architecture

### Design Principles

1. **Single Program** — All logic (factory + multisig) lives in one Anchor program (`maj_core`). There is no factory program deploying child programs. Instead, each multisig instance is a PDA owned by this single program.

2. **PDAs as Identity** — Each unique Maj instance is identified by its name. The PDA `[b"maj_instance", name.as_bytes()]` IS the multisig wallet. It holds lamports for SOL transfers. It signs CPIs as a PDA signer when governance instructions execute.

3. **Existence-Based Checks** — Membership, signatures, and blacklist status are all determined by whether a specific PDA account exists (non-zero lamports / has data). This is O(1) and gas-equivalent to Solidity mappings.

4. **Admin Reverse Lookup is Off-Chain** — There is no on-chain `Vec<Pubkey>` mapping admins to their instances. Instead, `AdminRecord` PDAs (seeded by `[admin_pubkey, instance_pubkey]`) are created per membership. Clients query `getProgramAccounts` with a `memcmp` filter on the admin pubkey to enumerate all instances a wallet belongs to.

5. **Governance via CPI** — The `onlyMaj` pattern is enforced by requiring the `MajInstance` PDA to be a signer. Since a PDA can only sign via `invoke_signed`, governance instructions can only be triggered from within `execute_transaction` — which provides the seeds. Direct calls without CPI will fail the `#[account(signer)]` constraint.

6. **Unbounded Growth via Realloc** — Anchor's `realloc` allows accounts to grow beyond their initial allocation. `MajInstance` reallocs when admins are added. `MajTransaction` reallocs to fit arbitrary `data` and `account_metas`.

### PDA Map

```
Program: maj_core (declare_id! pubkey)

[b"maj_registry"]
  └─ MajRegistry { total_instances: u64 }

[b"maj_name", name_bytes]
  └─ MajNameRecord { maj_instance: Pubkey, bump: u8 }

[b"maj_instance", name_bytes]
  └─ MajInstance { name, admins: Vec<Pubkey>, sigs_required, tx_count, bump }
     └─ holds SOL lamports (is the multisig treasury)

[b"maj_tx", maj_instance_key, tx_index_le_bytes]
  └─ MajTransaction { to, value, proposed_by, active, executed, num_signatures,
                      program_id?, data, account_metas, bump }

[b"sig", maj_tx_key, signer_key]
  └─ SignatureRecord { bump }  (existence = signed; closed on revoke)

[b"admin_record", admin_key, maj_instance_key]
  └─ AdminRecord { admin, maj_instance, bump }

[b"blacklist", maj_instance_key, target_key]
  └─ BlacklistRecord { bump }  (existence = blacklisted; closed on remove)
```

---

## Account Structures & Space Calculations

All Anchor accounts have an **8-byte discriminator** prefix (automatically handled by Anchor, but included in all space calculations below).

### MajRegistry

```rust
pub struct MajRegistry {
    pub total_instances: u64,   // 8 bytes
    pub bump: u8,               // 1 byte
}
```

Space: `8 (disc) + 8 (u64) + 1 (u8)` = **17 bytes**

Use `init` space `17`.

### MajNameRecord

```rust
pub struct MajNameRecord {
    pub maj_instance: Pubkey,   // 32 bytes
    pub bump: u8,               // 1 byte
}
```

Space: `8 (disc) + 32 (Pubkey) + 1 (u8)` = **41 bytes**

### MajInstance

```rust
pub struct MajInstance {
    pub name: String,             // 4 + up to 64 bytes = 68
    pub admins: Vec<Pubkey>,      // 4 + (n * 32) — grows via realloc
    pub sigs_required: u64,       // 8
    pub tx_count: u64,            // 8
    pub bump: u8,                 // 1
}
```

**Initial allocation** (supports up to 10 admins to avoid immediate realloc in typical cases):

```
8   (disc)
+ 68  (name: 4 prefix + 64 max chars)
+ 4   (Vec prefix)
+ (10 * 32)  = 320  (initial admin slots)
+ 8   (sigs_required)
+ 8   (tx_count)
+ 1   (bump)
= 417 bytes
```

Use `init` space `417`. When admins exceed 10, `realloc` adds `32` bytes per additional admin.

**Realloc formula:**
```
new_space = 8 + 68 + 4 + (admins.len() * 32) + 8 + 8 + 1
```

### MajTransaction

```rust
pub struct MajTransaction {
    pub maj_instance: Pubkey,                      // 32
    pub tx_index: u64,                             // 8
    pub to: Pubkey,                                // 32
    pub value: u64,                                // 8 (lamports)
    pub proposed_by: Pubkey,                       // 32
    pub active: bool,                              // 1
    pub executed: bool,                            // 1
    pub num_signatures: u32,                       // 4 (u32 — not u8, supports large admin sets)
    pub program_id: Option<Pubkey>,                // 1 + 32 = 33
    pub data: Vec<u8>,                             // 4 + n bytes — grows via realloc
    pub account_metas: Vec<SerializedAccountMeta>, // 4 + (n * 34) — grows via realloc
    pub bump: u8,                                  // 1
}
```

`SerializedAccountMeta` is 34 bytes:
```
pub pubkey: Pubkey,      // 32
pub is_signer: bool,     // 1
pub is_writable: bool,   // 1
```

**Initial allocation** (empty data + empty account_metas):

```
8   (disc)
+ 32  (maj_instance)
+ 8   (tx_index)
+ 32  (to)
+ 8   (value)
+ 32  (proposed_by)
+ 1   (active)
+ 1   (executed)
+ 4   (num_signatures)
+ 33  (Option<Pubkey>)
+ 4   (data Vec prefix)
+ 0   (initial data)
+ 4   (account_metas Vec prefix)
+ 0   (initial account_metas)
+ 1   (bump)
= 170 bytes
```

Use `init` space `170`. Realloc when `data` or `account_metas` are provided.

**Realloc formula:**
```
new_space = 170 + data.len() + (account_metas.len() * 34)
```

### SignatureRecord

```rust
pub struct SignatureRecord {
    pub bump: u8,   // 1
}
```

Space: `8 (disc) + 1 (u8)` = **9 bytes**

This account is created by `sign_transaction` and closed (lamports returned to admin) by `revoke_signature`.

### AdminRecord

```rust
pub struct AdminRecord {
    pub admin: Pubkey,         // 32
    pub maj_instance: Pubkey,  // 32
    pub bump: u8,              // 1
}
```

Space: `8 (disc) + 32 + 32 + 1` = **73 bytes**

The `admin` field (offset 8) and `maj_instance` field (offset 40) are the memcmp filter targets for `getProgramAccounts` queries.

### BlacklistRecord

```rust
pub struct BlacklistRecord {
    pub bump: u8,   // 1
}
```

Space: `8 (disc) + 1 (u8)` = **9 bytes**

Created by `add_blacklist`, closed by `remove_blacklist`.

---

## File & Module Structure

```
programs/
  maj_core/
    src/
      lib.rs                        ← declare_id!, mod declarations, #[program] block
      state/
        mod.rs                      ← pub mod re-exports
        registry.rs                 ← MajRegistry
        instance.rs                 ← MajInstance
        transaction.rs              ← MajTransaction + SerializedAccountMeta
      instructions/
        mod.rs                      ← pub mod re-exports
        initialize_registry.rs      ← InitializeRegistry ctx + handler
        create_maj.rs               ← CreateMaj ctx + handler
        propose_transaction.rs      ← ProposeTransaction ctx + handler
        sign_transaction.rs         ← SignTransaction ctx + handler
        execute_transaction.rs      ← ExecuteTransaction ctx + handler
        revoke_signature.rs         ← RevokeSignature ctx + handler
        cancel_transaction.rs       ← CancelTransaction ctx + handler
        governance.rs               ← add_admin, remove_admin, change_sigs_required,
                                       add_blacklist, remove_blacklist — all ctx + handlers
      errors.rs                     ← MajError enum
      events.rs                     ← all Anchor event structs
    Cargo.toml
tests/
  maj_core.ts                       ← Anchor mocha TypeScript tests
Anchor.toml
```

### lib.rs skeleton

```rust
use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("<PROGRAM_ID>");  // Replace after `anchor keys list`

#[program]
pub mod maj_core {
    use super::*;

    pub fn initialize_registry(ctx: Context<InitializeRegistry>) -> Result<()> {
        instructions::initialize_registry::handler(ctx)
    }

    pub fn create_maj(
        ctx: Context<CreateMaj>,
        name: String,
        admins: Vec<Pubkey>,
        sigs_required: u64,
    ) -> Result<()> {
        instructions::create_maj::handler(ctx, name, admins, sigs_required)
    }

    pub fn propose_transaction(
        ctx: Context<ProposeTransaction>,
        to: Pubkey,
        value: u64,
        data: Vec<u8>,
        program_id: Option<Pubkey>,
        account_metas: Vec<SerializedAccountMeta>,
    ) -> Result<()> {
        instructions::propose_transaction::handler(ctx, to, value, data, program_id, account_metas)
    }

    pub fn sign_transaction(ctx: Context<SignTransaction>) -> Result<()> {
        instructions::sign_transaction::handler(ctx)
    }

    pub fn execute_transaction(ctx: Context<ExecuteTransaction>) -> Result<()> {
        instructions::execute_transaction::handler(ctx)
    }

    pub fn revoke_signature(ctx: Context<RevokeSignature>) -> Result<()> {
        instructions::revoke_signature::handler(ctx)
    }

    pub fn cancel_transaction(ctx: Context<CancelTransaction>) -> Result<()> {
        instructions::cancel_transaction::handler(ctx)
    }

    // Governance — only callable via CPI from execute_transaction
    pub fn add_admin(ctx: Context<AddAdmin>, new_admin: Pubkey) -> Result<()> {
        instructions::governance::add_admin_handler(ctx, new_admin)
    }

    pub fn remove_admin(ctx: Context<RemoveAdmin>, admin_to_remove: Pubkey) -> Result<()> {
        instructions::governance::remove_admin_handler(ctx, admin_to_remove)
    }

    pub fn change_sigs_required(
        ctx: Context<ChangeSigsRequired>,
        new_sigs_required: u64,
    ) -> Result<()> {
        instructions::governance::change_sigs_required_handler(ctx, new_sigs_required)
    }

    pub fn add_blacklist(ctx: Context<AddBlacklist>, target: Pubkey) -> Result<()> {
        instructions::governance::add_blacklist_handler(ctx, target)
    }

    pub fn remove_blacklist(ctx: Context<RemoveBlacklist>, target: Pubkey) -> Result<()> {
        instructions::governance::remove_blacklist_handler(ctx, target)
    }
}
```

---

## Error Types

All errors map directly from the Solidity source.

```rust
// errors.rs
use anchor_lang::prelude::*;

#[error_code]
pub enum MajError {
    #[msg("Duplicate admin address")]
    DuplicateAdminAddress,

    #[msg("Zero address not allowed")]
    ZeroAddress,

    #[msg("Too many signatures required")]
    TooManySignaturesRequired,

    #[msg("Too few signatures required (minimum 2)")]
    TooFewSignaturesRequired,

    #[msg("Caller is not an admin")]
    OnlyAdmin,

    #[msg("Transaction is not active")]
    TransactionNotActive,

    #[msg("Admin has already signed this transaction")]
    DuplicateSignature,

    #[msg("Insufficient signatures to execute")]
    InsufficientSignatures,

    #[msg("CPI call failed")]
    TransactionFailed,

    #[msg("Admin has not signed this transaction")]
    UserHasNotSigned,

    #[msg("Only the proposer can cancel this transaction")]
    OnlyProposerCanCancel,

    #[msg("Instruction must be called via multisig execution (onlyMaj)")]
    OnlyMaj,

    #[msg("Address is not an admin")]
    AddressIsNotAdmin,

    #[msg("Minimum two admins required")]
    TwoAdminMinimum,

    #[msg("A Maj with this name already exists")]
    NameTaken,

    #[msg("No Maj found with this name")]
    NameNotFound,

    #[msg("Address is not blacklisted")]
    NotBlacklisted,

    #[msg("Address is blacklisted")]
    AddressIsBlacklisted,

    #[msg("Name exceeds maximum length (64 chars)")]
    NameTooLong,
}
```

---

## Events

```rust
// events.rs
use anchor_lang::prelude::*;

#[event]
pub struct TransactionProposed {
    pub tx_index: u64,
    pub to: Pubkey,
    pub value: u64,
    pub data: Vec<u8>,
    pub proposed_by: Pubkey,
}

#[event]
pub struct TransactionSigned {
    pub tx_index: u64,
    pub admin: Pubkey,
    pub num_signatures: u32,
}

#[event]
pub struct TransactionExecuted {
    pub tx_index: u64,
    pub to: Pubkey,
    pub value: u64,
    pub data: Vec<u8>,
}

#[event]
pub struct SignatureRevoked {
    pub tx_index: u64,
    pub admin: Pubkey,
    pub num_signatures: u32,
}

#[event]
pub struct TransactionCancelled {
    pub tx_index: u64,
}

#[event]
pub struct AdminAdded {
    pub maj_instance: Pubkey,
    pub new_admin: Pubkey,
}

#[event]
pub struct AdminRemoved {
    pub maj_instance: Pubkey,
    pub admin_removed: Pubkey,
}

#[event]
pub struct SignaturesRequiredChanged {
    pub maj_instance: Pubkey,
    pub signatures_required: u64,
}

#[event]
pub struct MajDeployed {
    pub maj_instance: Pubkey,
    pub admins: Vec<Pubkey>,
    pub sigs_required: u64,
}

#[event]
pub struct AdminStatusUpdated {
    pub admin: Pubkey,
    pub maj_instance: Pubkey,
    pub status: bool,
}
```

---

## State Definitions

### state/registry.rs

```rust
use anchor_lang::prelude::*;

#[account]
pub struct MajRegistry {
    pub total_instances: u64,
    pub bump: u8,
}

impl MajRegistry {
    pub const SPACE: usize = 8 + 8 + 1; // 17
}
```

### state/instance.rs

```rust
use anchor_lang::prelude::*;

#[account]
pub struct MajInstance {
    pub name: String,            // stored with 4-byte length prefix
    pub admins: Vec<Pubkey>,
    pub sigs_required: u64,
    pub tx_count: u64,
    pub bump: u8,
}

impl MajInstance {
    /// Initial space for up to 10 admins. Use realloc to grow.
    pub const INITIAL_SPACE: usize = 8      // discriminator
        + 4 + 64                             // name (max 64 chars)
        + 4 + (10 * 32)                      // admins vec (initial 10 slots)
        + 8                                  // sigs_required
        + 8                                  // tx_count
        + 1;                                 // bump
    // = 417

    /// Compute required space for given admin count.
    pub fn space_for(admin_count: usize) -> usize {
        8 + 4 + 64 + 4 + (admin_count * 32) + 8 + 8 + 1
    }
}

#[account]
pub struct MajNameRecord {
    pub maj_instance: Pubkey,
    pub bump: u8,
}

impl MajNameRecord {
    pub const SPACE: usize = 8 + 32 + 1; // 41
}

#[account]
pub struct AdminRecord {
    pub admin: Pubkey,
    pub maj_instance: Pubkey,
    pub bump: u8,
}

impl AdminRecord {
    pub const SPACE: usize = 8 + 32 + 32 + 1; // 73
}

#[account]
pub struct BlacklistRecord {
    pub bump: u8,
}

impl BlacklistRecord {
    pub const SPACE: usize = 8 + 1; // 9
}
```

### state/transaction.rs

```rust
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SerializedAccountMeta {
    pub pubkey: Pubkey,       // 32
    pub is_signer: bool,      // 1
    pub is_writable: bool,    // 1
}                             // total: 34 bytes each

impl SerializedAccountMeta {
    pub const SIZE: usize = 32 + 1 + 1; // 34
}

#[account]
pub struct MajTransaction {
    pub maj_instance: Pubkey,
    pub tx_index: u64,
    pub to: Pubkey,
    pub value: u64,
    pub proposed_by: Pubkey,
    pub active: bool,
    pub executed: bool,
    pub num_signatures: u32,        // u32, NOT u8 — supports large admin sets
    pub program_id: Option<Pubkey>, // None = SOL transfer; Some = CPI call
    pub data: Vec<u8>,
    pub account_metas: Vec<SerializedAccountMeta>,
    pub bump: u8,
}

impl MajTransaction {
    /// Minimum initial space (empty data and account_metas).
    pub const INITIAL_SPACE: usize = 8   // discriminator
        + 32    // maj_instance
        + 8     // tx_index
        + 32    // to
        + 8     // value
        + 32    // proposed_by
        + 1     // active
        + 1     // executed
        + 4     // num_signatures (u32)
        + 33    // Option<Pubkey>
        + 4     // data vec prefix
        + 4     // account_metas vec prefix
        + 1;    // bump
    // = 170

    pub fn space_for(data_len: usize, meta_count: usize) -> usize {
        8 + 32 + 8 + 32 + 8 + 32 + 1 + 1 + 4 + 33 + 4 + data_len + 4
            + (meta_count * SerializedAccountMeta::SIZE) + 1
    }
}

#[account]
pub struct SignatureRecord {
    pub bump: u8,
}

impl SignatureRecord {
    pub const SPACE: usize = 8 + 1; // 9
}
```

### state/mod.rs

```rust
pub mod instance;
pub mod registry;
pub mod transaction;

pub use instance::*;
pub use registry::*;
pub use transaction::*;
```

---

## Instruction Specifications

Each subsection details: purpose, accounts context struct, instruction parameters, validation logic, state mutations, events emitted, and Rust pseudocode.

---

### 1. `initialize_registry`

**Purpose:** Create the global `MajRegistry` singleton. Must be called exactly once before any `create_maj` call. Subsequent calls must fail because the PDA already exists (Anchor's `init` constraint handles this automatically).

**Parameters:** None

**Accounts:**

```rust
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
```

**Handler:**

```rust
pub fn handler(ctx: Context<InitializeRegistry>) -> Result<()> {
    let registry = &mut ctx.accounts.registry;
    registry.total_instances = 0;
    registry.bump = ctx.bumps.registry;
    Ok(())
}
```

**Notes:** No event emitted. Second call fails automatically with Anchor's "already in use" error because `init` requires the account not to exist.

---

### 2. `create_maj`

**Purpose:** Factory entrypoint. Creates a new `MajInstance` PDA and `MajNameRecord` PDA (name reservation). Creates one `AdminRecord` PDA per admin. Increments `MajRegistry.total_instances`. Mirrors `MajFactory.createMajContract`.

**Parameters:**
- `name: String` — unique human-readable identifier (max 64 bytes)
- `admins: Vec<Pubkey>` — initial admin list (minimum 2)
- `sigs_required: u64` — threshold (must be `>= 2` and `<= admins.len()`)

**Accounts:**

```rust
#[derive(Accounts)]
#[instruction(name: String, admins: Vec<Pubkey>, sigs_required: u64)]
pub struct CreateMaj<'info> {
    #[account(
        mut,
        seeds = [b"maj_registry"],
        bump = registry.bump,
    )]
    pub registry: Account<'info, MajRegistry>,

    // Name reservation — init fails if already exists (NameTaken guard)
    #[account(
        init,
        payer = payer,
        space = MajNameRecord::SPACE,
        seeds = [b"maj_name", name.as_bytes()],
        bump,
    )]
    pub name_record: Account<'info, MajNameRecord>,

    // The multisig instance PDA
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

    // NOTE: AdminRecord PDAs for each initial admin are created using
    // remaining_accounts + CPI to system_program. See handler notes below.
}
```

> **Important — AdminRecord creation:** Since the number of admins is variable, `AdminRecord` PDAs cannot be statically declared in the `Accounts` struct. They must be created in the handler using `remaining_accounts`. Pass all `AdminRecord` PDAs in `remaining_accounts` in the same order as `admins`. The handler iterates and creates each one via `anchor_lang::system_program::create_account` CPI with the correct seeds.

**Validation:**
- `name.len() <= 64` → error `NameTooLong`
- `admins.len() >= 2` → error `TwoAdminMinimum`
- No duplicate pubkeys in `admins` → error `DuplicateAdminAddress`
- No zero pubkeys → error `ZeroAddress`
- `sigs_required >= 2 && sigs_required <= admins.len()` → errors `TooFewSignaturesRequired` / `TooManySignaturesRequired`

**Note:** The `init` constraint on `name_record` naturally enforces name uniqueness — if the PDA already exists, Anchor returns an error. Map this to `NameTaken` with a custom error or let Anchor's native error propagate (acceptable).

**Handler:**

```rust
pub fn handler(
    ctx: Context<CreateMaj>,
    name: String,
    admins: Vec<Pubkey>,
    sigs_required: u64,
) -> Result<()> {
    require!(name.len() <= 64, MajError::NameTooLong);
    require!(admins.len() >= 2, MajError::TwoAdminMinimum);
    require!(sigs_required >= 2, MajError::TooFewSignaturesRequired);
    require!(sigs_required <= admins.len() as u64, MajError::TooManySignaturesRequired);

    // Check for duplicates and zero addresses
    let mut seen = std::collections::HashSet::new();
    for admin in &admins {
        require!(admin != &Pubkey::default(), MajError::ZeroAddress);
        require!(seen.insert(admin), MajError::DuplicateAdminAddress);
    }

    // Realloc MajInstance if admins.len() > 10
    if admins.len() > 10 {
        let new_space = MajInstance::space_for(admins.len());
        ctx.accounts.maj_instance.to_account_info().realloc(new_space, false)?;
    }

    // Init MajInstance
    let instance = &mut ctx.accounts.maj_instance;
    instance.name = name.clone();
    instance.admins = admins.clone();
    instance.sigs_required = sigs_required;
    instance.tx_count = 0;
    instance.bump = ctx.bumps.maj_instance;

    // Init MajNameRecord
    let name_record = &mut ctx.accounts.name_record;
    name_record.maj_instance = ctx.accounts.maj_instance.key();
    name_record.bump = ctx.bumps.name_record;

    // Increment registry
    ctx.accounts.registry.total_instances += 1;

    // Create AdminRecord PDAs via remaining_accounts
    // remaining_accounts must be provided in order: [admin_record_0, admin_record_1, ...]
    // Each account is the PDA for seeds [b"admin_record", admins[i], maj_instance.key()]
    let maj_instance_key = ctx.accounts.maj_instance.key();
    let payer = &ctx.accounts.payer;
    let system_program = &ctx.accounts.system_program;

    for (i, admin) in admins.iter().enumerate() {
        let admin_record_info = &ctx.remaining_accounts[i];

        let (expected_pda, bump) = Pubkey::find_program_address(
            &[b"admin_record", admin.as_ref(), maj_instance_key.as_ref()],
            ctx.program_id,
        );
        require!(admin_record_info.key() == expected_pda, MajError::ZeroAddress); // reuse as sanity

        let space = AdminRecord::SPACE;
        let rent = Rent::get()?.minimum_balance(space);

        anchor_lang::system_program::create_account(
            CpiContext::new(
                system_program.to_account_info(),
                anchor_lang::system_program::CreateAccount {
                    from: payer.to_account_info(),
                    to: admin_record_info.clone(),
                },
            ).with_signer(&[&[
                b"admin_record",
                admin.as_ref(),
                maj_instance_key.as_ref(),
                &[bump],
            ]]),
            rent,
            space as u64,
            ctx.program_id,
        )?;

        // Write AdminRecord data manually using borsh
        let mut data = admin_record_info.try_borrow_mut_data()?;
        let disc = AdminRecord::discriminator();
        data[..8].copy_from_slice(&disc);
        let record = AdminRecord {
            admin: *admin,
            maj_instance: maj_instance_key,
            bump,
        };
        record.try_serialize(&mut &mut data[8..])?;

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
```

---

### 3. `propose_transaction`

**Purpose:** Any non-blacklisted address may propose a transaction. If the proposer is an admin, they automatically sign (creating a `SignatureRecord`). If `num_signatures >= sigs_required` after the auto-sign, execute inline immediately. Non-admins propose without signing. Mirrors `Maj.proposeTransaction`.

**Parameters:**
- `to: Pubkey` — destination (for SOL transfer or CPI target)
- `value: u64` — lamports to transfer (0 for pure CPI)
- `data: Vec<u8>` — calldata for CPI (empty for SOL transfer)
- `program_id: Option<Pubkey>` — `None` = SOL transfer; `Some(pid)` = CPI call
- `account_metas: Vec<SerializedAccountMeta>` — accounts for CPI (empty for SOL transfer)

**Accounts:**

```rust
#[derive(Accounts)]
#[instruction(to: Pubkey, value: u64, data: Vec<u8>, program_id: Option<Pubkey>, account_metas: Vec<SerializedAccountMeta>)]
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
        seeds = [b"maj_tx", maj_instance.key().as_ref(), &maj_instance.tx_count.to_le_bytes()],
        bump,
    )]
    pub maj_transaction: Account<'info, MajTransaction>,

    // Optional: if proposer is admin, we also init a SignatureRecord.
    // This CANNOT be conditionally init'd in the Accounts struct.
    // Handle in remaining_accounts: the client passes the SignatureRecord PDA
    // in remaining_accounts[0] if the proposer is an admin.

    /// CHECK: verified against blacklist PDA derivation in handler
    #[account(mut)]
    pub proposer: Signer<'info>,

    pub system_program: Program<'info, System>,
}
```

> **Admin branch and SignatureRecord:** Since Anchor cannot conditionally `init` accounts, the admin's `SignatureRecord` PDA is passed in `remaining_accounts[0]` when the proposer is an admin. The handler derives the expected PDA, verifies it matches, then creates it via CPI. The client is responsible for passing it; if the proposer is not an admin the handler skips this.

**Validation:**
1. Derive `BlacklistRecord` PDA: `[b"blacklist", maj_instance.key(), proposer.key()]`. Check if account exists (non-zero lamports). If yes → error `AddressIsBlacklisted`.
2. Derive `AdminRecord` PDA: `[b"admin_record", proposer.key(), maj_instance.key()]`. Check if account exists → proposer is admin.

**Handler:**

```rust
pub fn handler(
    ctx: Context<ProposeTransaction>,
    to: Pubkey,
    value: u64,
    data: Vec<u8>,
    program_id: Option<Pubkey>,
    account_metas: Vec<SerializedAccountMeta>,
) -> Result<()> {
    let maj_instance_key = ctx.accounts.maj_instance.key();
    let proposer_key = ctx.accounts.proposer.key();

    // --- Blacklist check ---
    let (blacklist_pda, _) = Pubkey::find_program_address(
        &[b"blacklist", maj_instance_key.as_ref(), proposer_key.as_ref()],
        ctx.program_id,
    );
    // Check remaining_accounts or derive via get_account_info. Use account_info lookup:
    let blacklisted = ctx.remaining_accounts.iter().any(|a| {
        a.key() == blacklist_pda && a.lamports() > 0
    });
    // NOTE: The client must pass the BlacklistRecord account info in remaining_accounts
    // so the runtime loads it. If the account doesn't exist, its lamports == 0.
    require!(!blacklisted, MajError::AddressIsBlacklisted);

    // --- Admin check ---
    let (admin_record_pda, _) = Pubkey::find_program_address(
        &[b"admin_record", proposer_key.as_ref(), maj_instance_key.as_ref()],
        ctx.program_id,
    );
    let is_admin = ctx.remaining_accounts.iter().any(|a| {
        a.key() == admin_record_pda && a.lamports() > 0
    });

    // --- Init MajTransaction ---
    let tx_index = ctx.accounts.maj_instance.tx_count;
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

    ctx.accounts.maj_instance.tx_count += 1;

    emit!(TransactionProposed {
        tx_index,
        to,
        value,
        data: data.clone(),
        proposed_by: proposer_key,
    });

    if is_admin {
        // Auto-sign: create SignatureRecord PDA
        // remaining_accounts must contain the SignatureRecord PDA account info
        // The client passes it as the LAST item in remaining_accounts
        let sig_record_info = ctx.remaining_accounts
            .iter()
            .find(|a| {
                let (expected, _) = Pubkey::find_program_address(
                    &[b"sig", ctx.accounts.maj_transaction.key().as_ref(), proposer_key.as_ref()],
                    ctx.program_id,
                );
                a.key() == expected
            })
            .ok_or(MajError::OnlyAdmin)?; // should always be present for admin branch

        let (_, sig_bump) = Pubkey::find_program_address(
            &[b"sig", ctx.accounts.maj_transaction.key().as_ref(), proposer_key.as_ref()],
            ctx.program_id,
        );

        let rent = Rent::get()?.minimum_balance(SignatureRecord::SPACE);
        anchor_lang::system_program::create_account(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::CreateAccount {
                    from: ctx.accounts.proposer.to_account_info(),
                    to: sig_record_info.clone(),
                },
            ).with_signer(&[&[
                b"sig",
                ctx.accounts.maj_transaction.key().as_ref(),
                proposer_key.as_ref(),
                &[sig_bump],
            ]]),
            rent,
            SignatureRecord::SPACE as u64,
            ctx.program_id,
        )?;

        // Write discriminator + data
        let mut sig_data = sig_record_info.try_borrow_mut_data()?;
        sig_data[..8].copy_from_slice(&SignatureRecord::discriminator());
        sig_data[8] = sig_bump;

        let tx = &mut ctx.accounts.maj_transaction;
        tx.num_signatures += 1;

        emit!(TransactionSigned {
            tx_index,
            admin: proposer_key,
            num_signatures: tx.num_signatures,
        });

        // Auto-execute if threshold reached
        if tx.num_signatures >= ctx.accounts.maj_instance.sigs_required as u32 {
            execute_transaction_logic(
                &mut ctx.accounts.maj_instance,
                tx,
                ctx.remaining_accounts,
                ctx.program_id,
            )?;
        }
    }

    Ok(())
}
```

---

### 4. `sign_transaction`

**Purpose:** An admin signs an existing active transaction. Creates `SignatureRecord` PDA. If `num_signatures >= sigs_required`, calls execute logic inline (no CPI-to-self — see Important Notes).

**Parameters:** None (tx identified by the `MajTransaction` account passed)

**Accounts:**

```rust
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
        seeds = [b"maj_tx", maj_instance.key().as_ref(), &maj_transaction.tx_index.to_le_bytes()],
        bump = maj_transaction.bump,
        constraint = maj_transaction.active @ MajError::TransactionNotActive,
        constraint = maj_transaction.maj_instance == maj_instance.key() @ MajError::OnlyAdmin,
    )]
    pub maj_transaction: Account<'info, MajTransaction>,

    #[account(
        init,
        payer = admin,
        space = SignatureRecord::SPACE,
        seeds = [b"sig", maj_transaction.key().as_ref(), admin.key().as_ref()],
        bump,
        // init fails if PDA already exists → DuplicateSignature
    )]
    pub signature_record: Account<'info, SignatureRecord>,

    // Admin membership check via AdminRecord PDA
    #[account(
        seeds = [b"admin_record", admin.key().as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}
```

**Notes on duplicate check:** The `init` constraint on `signature_record` will fail if the PDA already exists. This is the `DuplicateSignature` guard. You may optionally map the Anchor error to `MajError::DuplicateSignature` using a custom error handler, or document that the Anchor error is sufficient.

**Handler:**

```rust
pub fn handler(ctx: Context<SignTransaction>) -> Result<()> {
    let sig_record = &mut ctx.accounts.signature_record;
    sig_record.bump = ctx.bumps.signature_record;

    let tx = &mut ctx.accounts.maj_transaction;
    tx.num_signatures += 1;

    let admin_key = ctx.accounts.admin.key();
    let tx_index = tx.tx_index;
    let num_sigs = tx.num_signatures;

    emit!(TransactionSigned {
        tx_index,
        admin: admin_key,
        num_signatures: num_sigs,
    });

    // Auto-execute inline if threshold reached
    if tx.num_signatures >= ctx.accounts.maj_instance.sigs_required as u32 {
        execute_transaction_logic(
            &mut ctx.accounts.maj_instance,
            tx,
            ctx.remaining_accounts,
            ctx.program_id,
        )?;
    }

    Ok(())
}
```

---

### 5. `execute_transaction`

**Purpose:** Manually trigger execution of a threshold-met transaction. Admin only. Executes SOL transfer or CPI. Marks transaction as `executed = true, active = false`.

**Accounts:**

```rust
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
        seeds = [b"maj_tx", maj_instance.key().as_ref(), &maj_transaction.tx_index.to_le_bytes()],
        bump = maj_transaction.bump,
        constraint = maj_transaction.active @ MajError::TransactionNotActive,
        constraint = maj_transaction.num_signatures >= maj_instance.sigs_required as u32 @ MajError::InsufficientSignatures,
    )]
    pub maj_transaction: Account<'info, MajTransaction>,

    #[account(
        seeds = [b"admin_record", admin.key().as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    #[account(mut)]
    pub admin: Signer<'info>,

    /// CHECK: destination for SOL transfer; validated by instruction logic
    #[account(mut)]
    pub to: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}
```

> **CPI Accounts:** For CPI transactions, all accounts referenced in `account_metas` must be passed in `remaining_accounts`. The handler maps them via `account_metas` order. The client is responsible for passing them in the correct order.

**Handler — shared execute logic:**

Factor execution into a standalone function `execute_transaction_logic` so it can be called from both `execute_transaction` and the auto-execute paths in `sign_transaction` and `propose_transaction`. This avoids CPI-to-self.

```rust
/// Shared execution logic called from execute_transaction handler
/// and auto-execute branches in sign_transaction/propose_transaction.
pub fn execute_transaction_logic<'info>(
    maj_instance: &mut Account<'info, MajInstance>,
    tx: &mut Account<'info, MajTransaction>,
    remaining_accounts: &[AccountInfo<'info>],
    program_id: &Pubkey,
) -> Result<()> {
    require!(tx.active, MajError::TransactionNotActive);
    require!(
        tx.num_signatures >= maj_instance.sigs_required as u32,
        MajError::InsufficientSignatures
    );

    tx.active = false;
    tx.executed = true;

    let name_bytes = maj_instance.name.as_bytes().to_vec();
    let bump = maj_instance.bump;
    let seeds: &[&[u8]] = &[b"maj_instance", &name_bytes, &[bump]];

    if tx.program_id.is_none() {
        // --- SOL transfer ---
        // Transfer lamports from the MajInstance PDA to tx.to
        let from = maj_instance.to_account_info();
        let to = remaining_accounts
            .iter()
            .find(|a| a.key() == tx.to)
            .ok_or(MajError::TransactionFailed)?;

        **from.try_borrow_mut_lamports()? -= tx.value;
        **to.try_borrow_mut_lamports()? += tx.value;
    } else {
        // --- CPI call ---
        let cpi_program_id = tx.program_id.unwrap();
        let cpi_program_info = remaining_accounts
            .iter()
            .find(|a| a.key() == cpi_program_id)
            .ok_or(MajError::TransactionFailed)?;

        // Reconstruct account_infos from remaining_accounts in the order of account_metas
        let mut cpi_accounts: Vec<AccountInfo<'info>> = Vec::new();
        for meta in &tx.account_metas {
            let info = remaining_accounts
                .iter()
                .find(|a| a.key() == meta.pubkey)
                .ok_or(MajError::TransactionFailed)?;
            cpi_accounts.push(info.clone());
        }

        // Reconstruct AccountMeta list
        let account_metas: Vec<solana_program::instruction::AccountMeta> = tx
            .account_metas
            .iter()
            .map(|m| solana_program::instruction::AccountMeta {
                pubkey: m.pubkey,
                is_signer: m.is_signer,
                is_writable: m.is_writable,
            })
            .collect();

        let instruction = solana_program::instruction::Instruction {
            program_id: cpi_program_id,
            accounts: account_metas,
            data: tx.data.clone(),
        };

        solana_program::program::invoke_signed(
            &instruction,
            &cpi_accounts,
            &[seeds],
        ).map_err(|_| MajError::TransactionFailed)?;
    }

    emit!(TransactionExecuted {
        tx_index: tx.tx_index,
        to: tx.to,
        value: tx.value,
        data: tx.data.clone(),
    });

    Ok(())
}

pub fn handler(ctx: Context<ExecuteTransaction>) -> Result<()> {
    execute_transaction_logic(
        &mut ctx.accounts.maj_instance,
        &mut ctx.accounts.maj_transaction,
        ctx.remaining_accounts,
        ctx.program_id,
    )
}
```

---

### 6. `revoke_signature`

**Purpose:** Admin removes their signature from an active transaction. Closes the `SignatureRecord` PDA, returning rent to the admin. Decrements `num_signatures` on the transaction.

**Accounts:**

```rust
#[derive(Accounts)]
pub struct RevokeSignature<'info> {
    #[account(
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(
        mut,
        seeds = [b"maj_tx", maj_instance.key().as_ref(), &maj_transaction.tx_index.to_le_bytes()],
        bump = maj_transaction.bump,
        constraint = maj_transaction.active @ MajError::TransactionNotActive,
    )]
    pub maj_transaction: Account<'info, MajTransaction>,

    #[account(
        mut,
        close = admin,  // return rent to admin
        seeds = [b"sig", maj_transaction.key().as_ref(), admin.key().as_ref()],
        bump = signature_record.bump,
        // If PDA doesn't exist, this constraint fails → UserHasNotSigned
    )]
    pub signature_record: Account<'info, SignatureRecord>,

    #[account(
        seeds = [b"admin_record", admin.key().as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    #[account(mut)]
    pub admin: Signer<'info>,
}
```

**Handler:**

```rust
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
```

> **Note:** If the admin has not signed (SignatureRecord doesn't exist), Anchor's constraint resolution for the `close`-tagged account will fail before the handler runs. This is the `UserHasNotSigned` guard.

---

### 7. `cancel_transaction`

**Purpose:** The original proposer cancels an active transaction. Sets `active = false`. Does NOT close the `MajTransaction` account (keep for historical record; set inactive).

**Accounts:**

```rust
#[derive(Accounts)]
pub struct CancelTransaction<'info> {
    #[account(
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(
        mut,
        seeds = [b"maj_tx", maj_instance.key().as_ref(), &maj_transaction.tx_index.to_le_bytes()],
        bump = maj_transaction.bump,
        constraint = maj_transaction.active @ MajError::TransactionNotActive,
        constraint = maj_transaction.proposed_by == proposer.key() @ MajError::OnlyProposerCanCancel,
    )]
    pub maj_transaction: Account<'info, MajTransaction>,

    #[account(
        seeds = [b"admin_record", proposer.key().as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    #[account(mut)]
    pub proposer: Signer<'info>,
}
```

**Handler:**

```rust
pub fn handler(ctx: Context<CancelTransaction>) -> Result<()> {
    ctx.accounts.maj_transaction.active = false;

    emit!(TransactionCancelled {
        tx_index: ctx.accounts.maj_transaction.tx_index,
    });

    Ok(())
}
```

> **Note:** The Solidity source only enforces that the caller is an admin AND the proposer. The admin check is enforced by the `admin_record` account constraint (the PDA must exist and match seeds).

---

### 8. `add_admin`

**Purpose:** Governance instruction. Adds a new admin to the `MajInstance`. Creates an `AdminRecord` PDA for the new admin. Reallocs `MajInstance` if necessary. **Can only be invoked via CPI from `execute_transaction`** — enforced by requiring `MajInstance` as a signer.

**Parameters:**
- `new_admin: Pubkey`

**Accounts:**

```rust
#[derive(Accounts)]
#[instruction(new_admin: Pubkey)]
pub struct AddAdmin<'info> {
    #[account(
        mut,
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer,  // ← THE onlyMaj GUARD — PDA must be signer via invoke_signed
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

    // The new admin's pubkey (passed as instruction param, not account)
    // We also need a system_program and a payer for AdminRecord init.
    // The payer here is the fee payer of the original tx, passed via remaining_accounts
    // or as a writable signer in the CPI account_metas.

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}
```

**Handler:**

```rust
pub fn add_admin_handler(ctx: Context<AddAdmin>, new_admin: Pubkey) -> Result<()> {
    require!(new_admin != Pubkey::default(), MajError::ZeroAddress);

    let instance = &mut ctx.accounts.maj_instance;

    // Duplicate check: AdminRecord init would fail, but check explicitly for clarity
    require!(
        !instance.admins.contains(&new_admin),
        MajError::DuplicateAdminAddress
    );

    // Realloc if needed
    let new_admin_count = instance.admins.len() + 1;
    let new_space = MajInstance::space_for(new_admin_count);
    let current_space = instance.to_account_info().data_len();
    if new_space > current_space {
        instance.to_account_info().realloc(new_space, false)?;
        // Top up lamports for the extra rent
        let rent = Rent::get()?;
        let extra_lamports = rent.minimum_balance(new_space)
            .saturating_sub(rent.minimum_balance(current_space));
        if extra_lamports > 0 {
            **ctx.accounts.payer.try_borrow_mut_lamports()? -= extra_lamports;
            **instance.to_account_info().try_borrow_mut_lamports()? += extra_lamports;
        }
    }

    instance.admins.push(new_admin);

    // Init AdminRecord
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
```

---

### 9. `remove_admin`

**Purpose:** Governance instruction. Removes an admin. Closes the `AdminRecord` PDA. Removes from `MajInstance.admins` Vec. Enforces minimum 2 admins. **PDA signer required.**

**Parameters:**
- `admin_to_remove: Pubkey`

**Accounts:**

```rust
#[derive(Accounts)]
#[instruction(admin_to_remove: Pubkey)]
pub struct RemoveAdmin<'info> {
    #[account(
        mut,
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer,  // onlyMaj guard
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(
        mut,
        close = rent_receiver,
        seeds = [b"admin_record", admin_to_remove.as_ref(), maj_instance.key().as_ref()],
        bump = admin_record.bump,
    )]
    pub admin_record: Account<'info, AdminRecord>,

    /// CHECK: receives the closed AdminRecord rent
    #[account(mut)]
    pub rent_receiver: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}
```

**Handler:**

```rust
pub fn remove_admin_handler(ctx: Context<RemoveAdmin>, admin_to_remove: Pubkey) -> Result<()> {
    let instance = &mut ctx.accounts.maj_instance;

    require!(instance.admins.contains(&admin_to_remove), MajError::AddressIsNotAdmin);
    require!(instance.admins.len() > 2, MajError::TwoAdminMinimum);

    // Swap-remove (O(1), order not guaranteed — acceptable for multisig)
    if let Some(pos) = instance.admins.iter().position(|a| a == &admin_to_remove) {
        instance.admins.swap_remove(pos);
    }

    // Also enforce sigs_required <= new admin count
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
```

> **Note on sigs_required auto-adjustment:** The Solidity source would revert if removing an admin drops `admins.length` below `signaturesRequired`. On Solana, auto-adjusting is more user-friendly. Choose either approach and document it. The above auto-adjusts to avoid a stuck multisig where sigs_required can never be met.

---

### 10. `change_sigs_required`

**Purpose:** Governance instruction. Changes the signature threshold. Must satisfy `2 <= new_sigs <= admins.len()`. **PDA signer required.**

**Parameters:**
- `new_sigs_required: u64`

**Accounts:**

```rust
#[derive(Accounts)]
pub struct ChangeSigsRequired<'info> {
    #[account(
        mut,
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer,  // onlyMaj guard
    )]
    pub maj_instance: Account<'info, MajInstance>,
}
```

**Handler:**

```rust
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
```

---

### 11. `add_blacklist`

**Purpose:** Governance instruction. Creates a `BlacklistRecord` PDA for the target address. **PDA signer required.**

**Parameters:**
- `target: Pubkey`

**Accounts:**

```rust
#[derive(Accounts)]
#[instruction(target: Pubkey)]
pub struct AddBlacklist<'info> {
    #[account(
        mut,
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer,  // onlyMaj guard
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
```

**Handler:**

```rust
pub fn add_blacklist_handler(ctx: Context<AddBlacklist>, _target: Pubkey) -> Result<()> {
    ctx.accounts.blacklist_record.bump = ctx.bumps.blacklist_record;
    Ok(())
}
```

---

### 12. `remove_blacklist`

**Purpose:** Governance instruction. Closes `BlacklistRecord` PDA. Reverts if PDA does not exist (`NotBlacklisted`). **PDA signer required.**

**Parameters:**
- `target: Pubkey`

**Accounts:**

```rust
#[derive(Accounts)]
#[instruction(target: Pubkey)]
pub struct RemoveBlacklist<'info> {
    #[account(
        seeds = [b"maj_instance", maj_instance.name.as_bytes()],
        bump = maj_instance.bump,
        signer,  // onlyMaj guard
    )]
    pub maj_instance: Account<'info, MajInstance>,

    #[account(
        mut,
        close = rent_receiver,
        seeds = [b"blacklist", maj_instance.key().as_ref(), target.as_ref()],
        bump = blacklist_record.bump,
        // Constraint: PDA must exist. If not, Anchor errors → NotBlacklisted.
    )]
    pub blacklist_record: Account<'info, BlacklistRecord>,

    /// CHECK: receives blacklist_record rent
    #[account(mut)]
    pub rent_receiver: UncheckedAccount<'info>,
}
```

**Handler:**

```rust
pub fn remove_blacklist_handler(ctx: Context<RemoveBlacklist>, _target: Pubkey) -> Result<()> {
    // Account is closed by the `close = rent_receiver` constraint.
    Ok(())
}
```

---

### 13. `get_admin_contracts` — Client-Side Only

This is **not an on-chain instruction**. In Solidity, `MajFactory.getAdminContracts(admin)` returns the list of Maj instances an admin belongs to. In Anchor, this is queried client-side using `getProgramAccounts` with a `memcmp` discriminator filter.

**TypeScript client query:**

```typescript
import { Program, utils } from "@coral-xyz/anchor";

async function getAdminContracts(
  program: Program,
  adminPubkey: PublicKey
): Promise<PublicKey[]> {
  // AdminRecord layout:
  //   0..8   = discriminator
  //   8..40  = admin: Pubkey
  //   40..72 = maj_instance: Pubkey
  //   72     = bump: u8

  const adminRecordDiscriminator = Buffer.from(
    utils.sha256.hash("account:AdminRecord").slice(0, 8)
  );

  const accounts = await program.provider.connection.getProgramAccounts(
    program.programId,
    {
      filters: [
        { memcmp: { offset: 0, bytes: adminRecordDiscriminator.toString("base64") } },
        { memcmp: { offset: 8, bytes: adminPubkey.toBase58() } },
      ],
    }
  );

  return accounts.map((a) => {
    // maj_instance pubkey is at bytes 40..72
    return new PublicKey(a.account.data.slice(40, 72));
  });
}
```

> **Note:** The Anchor IDL discriminator for `AdminRecord` is the first 8 bytes of `SHA256("account:AdminRecord")`. Use `program.coder.accounts.accountDiscriminator("AdminRecord")` for a reliable way to get this in Anchor v0.30+.

---

## CPI Payload & Execution Model

### Transaction Types

```
SOL Transfer:
  program_id = None
  data       = [] (empty)
  account_metas = [] (empty)
  to         = recipient pubkey
  value      = lamports to transfer

CPI Call (e.g., governance instruction):
  program_id = Some(<program_to_call>)
  data       = borsh-encoded instruction discriminator + args
  account_metas = [
    SerializedAccountMeta { pubkey: maj_instance, is_signer: true, is_writable: true },
    SerializedAccountMeta { pubkey: payer, is_signer: true, is_writable: true },
    ... other accounts the target instruction needs
  ]
  to         = program_id (can be the same as program_id field)
  value      = 0
```

### Encoding Governance Instructions as CPI Payloads

When a client wants to propose a governance action (e.g., `add_admin`), it must:

1. Compute the Anchor instruction discriminator: first 8 bytes of `SHA256("global:add_admin")`.
2. Borsh-encode the instruction arguments (e.g., `new_admin: Pubkey`).
3. Concatenate: `data = discriminator ++ borsh_args`.
4. Build `account_metas` listing all accounts the instruction needs, with `MajInstance` marked as `is_signer: true`.
5. Set `program_id = Some(maj_core_program_id)`.

**TypeScript example for proposing `add_admin`:**

```typescript
import * as anchor from "@coral-xyz/anchor";
import { BorshCoder, utils } from "@coral-xyz/anchor";

async function proposeAddAdmin(
  program: Program,
  majInstance: PublicKey,
  majInstanceName: string,
  proposer: Keypair,
  newAdmin: PublicKey,
  payer: PublicKey
) {
  // 1. Encode the add_admin instruction
  const discriminator = Buffer.from(
    utils.sha256.hash("global:add_admin").slice(0, 16), // 8 bytes
    "hex"
  ).slice(0, 8);

  const newAdminBytes = newAdmin.toBytes();
  const data = Buffer.concat([discriminator, Buffer.from(newAdminBytes)]);

  // 2. Compute AdminRecord PDA for the new admin
  const [adminRecordPda] = PublicKey.findProgramAddressSync(
    [
      Buffer.from("admin_record"),
      newAdmin.toBytes(),
      majInstance.toBytes(),
    ],
    program.programId
  );

  // 3. Build account_metas
  const accountMetas = [
    { pubkey: majInstance, isSigner: true, isWritable: true },   // MajInstance PDA (signs via invoke_signed)
    { pubkey: adminRecordPda, isSigner: false, isWritable: true }, // new AdminRecord
    { pubkey: payer, isSigner: true, isWritable: true },          // payer for rent
    { pubkey: anchor.web3.SystemProgram.programId, isSigner: false, isWritable: false },
  ];

  // 4. Propose the transaction
  await program.methods
    .proposeTransaction(
      program.programId,   // to (same as program_id for self-CPI)
      new anchor.BN(0),    // value
      Buffer.from(data),   // instruction data
      program.programId,   // program_id (Some variant)
      accountMetas.map(m => ({
        pubkey: m.pubkey,
        isSigner: m.isSigner,
        isWritable: m.isWritable,
      }))
    )
    .accounts({
      majInstance,
      proposer: proposer.publicKey,
      // ... other required accounts
    })
    .signers([proposer])
    .rpc();
}
```

---

## PDA Signer Pattern (onlyMaj Equivalent)

### How It Works

In Solidity, `onlyMaj` checks `msg.sender == address(this)`. On Solana, a PDA cannot sign transactions directly — only programs can authorize PDAs as signers via `invoke_signed`. This creates an equivalent pattern:

1. **Governance instruction accounts** include `maj_instance` with the `signer` constraint (`#[account(signer)]` or `signer` keyword in Accounts macro).
2. **Direct calls** from any wallet will fail: a wallet cannot sign on behalf of a PDA.
3. **Via `execute_transaction`:** when the handler calls `invoke_signed` with seeds `[b"maj_instance", name.as_bytes(), &[bump]]`, the Solana runtime grants that PDA signer authority for the duration of the CPI. The governance instruction then receives `maj_instance` as a legitimate signer.

### Security Guarantee

The only path that grants signer authority to the `MajInstance` PDA is `invoke_signed` called from within this program. The program only calls `invoke_signed` inside `execute_transaction_logic`, which requires:
- `num_signatures >= sigs_required`
- The transaction is still `active`
- The caller is an admin

Therefore, governance instructions are only reachable through a properly approved multisig vote.

### Code Pattern in execute_transaction_logic

```rust
let name_bytes = maj_instance.name.as_bytes().to_vec();
let bump = maj_instance.bump;

solana_program::program::invoke_signed(
    &instruction,
    &cpi_account_infos,
    &[&[
        b"maj_instance",
        name_bytes.as_slice(),
        &[bump],
    ]],
)?;
```

---

## Realloc Pattern

### When to Realloc

| Account | Trigger |
|---|---|
| `MajInstance` | `add_admin` when `admins.len()` would exceed current space |
| `MajTransaction` | `propose_transaction` when `data.len() + account_metas.len()` exceeds initial 170 bytes |

### Realloc Requirements

To use `realloc`, the instruction's `Accounts` struct must include:
- A mutable reference to the account being realloced.
- `system_program: Program<'info, System>` — required for lamport transfer.
- A mutable `payer: Signer<'info>` — funds the extra rent.

### Anchor Realloc Constraint

```rust
#[account(
    mut,
    realloc = MajInstance::space_for(new_admin_count),
    realloc::payer = payer,
    realloc::zero = false,  // false = preserve existing data
)]
pub maj_instance: Account<'info, MajInstance>,
```

`realloc::zero = false` preserves existing account data. Only use `true` if you want to zero-initialize new bytes (slight extra cost but safer for uninitialized memory).

### Manual Realloc (if Anchor macro realloc not available in your version)

```rust
let new_size = MajInstance::space_for(new_admin_count);
let account_info = ctx.accounts.maj_instance.to_account_info();
account_info.realloc(new_size, false)?;

// Fund additional rent
let rent = Rent::get()?;
let current_lamports = account_info.lamports();
let required_lamports = rent.minimum_balance(new_size);
if required_lamports > current_lamports {
    let diff = required_lamports - current_lamports;
    **ctx.accounts.payer.try_borrow_mut_lamports()? -= diff;
    **account_info.try_borrow_mut_lamports()? += diff;
}
```

> **Warning:** Always realloc before mutating the account's data. Writing beyond the current data length without realloc first will panic.

---

## Testing Requirements

All tests live in `tests/maj_core.ts`. Use Anchor's TypeScript test framework (Mocha + Chai). Configure `Anchor.toml` to point at `localnet` for testing.

### Test Setup

```typescript
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { MajCore } from "../target/types/maj_core";
import { Keypair, PublicKey, SystemProgram, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { expect } from "chai";

describe("maj_core", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.MajCore as Program<MajCore>;

  // Helper: derive PDAs
  const deriveRegistry = () =>
    PublicKey.findProgramAddressSync([Buffer.from("maj_registry")], program.programId);

  const deriveMajInstance = (name: string) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("maj_instance"), Buffer.from(name)],
      program.programId
    );

  const deriveNameRecord = (name: string) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("maj_name"), Buffer.from(name)],
      program.programId
    );

  const deriveAdminRecord = (admin: PublicKey, instance: PublicKey) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("admin_record"), admin.toBytes(), instance.toBytes()],
      program.programId
    );

  const deriveMajTx = (instance: PublicKey, txIndex: bigint) =>
    PublicKey.findProgramAddressSync(
      [
        Buffer.from("maj_tx"),
        instance.toBytes(),
        Buffer.from(new anchor.BN(txIndex.toString()).toArrayLike(Buffer, "le", 8)),
      ],
      program.programId
    );

  const deriveSignatureRecord = (majTx: PublicKey, signer: PublicKey) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("sig"), majTx.toBytes(), signer.toBytes()],
      program.programId
    );

  const deriveBlacklist = (instance: PublicKey, target: PublicKey) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("blacklist"), instance.toBytes(), target.toBytes()],
      program.programId
    );
```

### Test Suite 1: Registry Initialization

```typescript
  describe("initialize_registry", () => {
    it("initializes the registry successfully", async () => {
      const [registryPda] = deriveRegistry();
      await program.methods
        .initializeRegistry()
        .accounts({ registry: registryPda, payer: provider.wallet.publicKey, systemProgram: SystemProgram.programId })
        .rpc();

      const registry = await program.account.majRegistry.fetch(registryPda);
      expect(registry.totalInstances.toNumber()).to.equal(0);
    });

    it("fails on second call (idempotency — account already exists)", async () => {
      const [registryPda] = deriveRegistry();
      try {
        await program.methods
          .initializeRegistry()
          .accounts({ registry: registryPda, payer: provider.wallet.publicKey, systemProgram: SystemProgram.programId })
          .rpc();
        expect.fail("Should have thrown");
      } catch (e) {
        // Anchor throws when account already exists under `init`
        expect(e.message).to.include("already in use");
      }
    });
  });
```

### Test Suite 2: create_maj

```typescript
  describe("create_maj", () => {
    const admin1 = Keypair.generate();
    const admin2 = Keypair.generate();
    const name = "test-multisig";

    before(async () => {
      // Airdrop to admins
      await provider.connection.requestAirdrop(admin1.publicKey, 2 * LAMPORTS_PER_SOL);
      await provider.connection.requestAirdrop(admin2.publicKey, 2 * LAMPORTS_PER_SOL);
    });

    it("creates a Maj instance successfully", async () => {
      const [instancePda] = deriveMajInstance(name);
      const [nameRecordPda] = deriveNameRecord(name);
      const [adminRecord1Pda] = deriveAdminRecord(admin1.publicKey, instancePda);
      const [adminRecord2Pda] = deriveAdminRecord(admin2.publicKey, instancePda);

      await program.methods
        .createMaj(name, [admin1.publicKey, admin2.publicKey], new anchor.BN(2))
        .accounts({
          registry: deriveRegistry()[0],
          nameRecord: nameRecordPda,
          majInstance: instancePda,
          payer: provider.wallet.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .remainingAccounts([
          { pubkey: adminRecord1Pda, isWritable: true, isSigner: false },
          { pubkey: adminRecord2Pda, isWritable: true, isSigner: false },
        ])
        .rpc();

      const instance = await program.account.majInstance.fetch(instancePda);
      expect(instance.name).to.equal(name);
      expect(instance.sigsRequired.toNumber()).to.equal(2);
      expect(instance.admins).to.have.length(2);
      expect(instance.txCount.toNumber()).to.equal(0);
    });

    it("reverts on duplicate name", async () => {
      const [instancePda] = deriveMajInstance(name);
      const [nameRecordPda] = deriveNameRecord(name);
      try {
        await program.methods
          .createMaj(name, [admin1.publicKey, admin2.publicKey], new anchor.BN(2))
          .accounts({ /* ... */ })
          .rpc();
        expect.fail("Should have thrown");
      } catch (e) {
        expect(e.message).to.include("already in use"); // NameTaken via Anchor init guard
      }
    });

    it("reverts if sigs_required < 2", async () => {
      const name2 = "test-multisig-2";
      const [instancePda] = deriveMajInstance(name2);
      try {
        await program.methods
          .createMaj(name2, [admin1.publicKey, admin2.publicKey], new anchor.BN(1))
          .accounts({ /* ... */ })
          .rpc();
        expect.fail("Should have thrown");
      } catch (e) {
        expect(e.message).to.include("TooFewSignaturesRequired");
      }
    });

    it("reverts if sigs_required > admins.length", async () => {
      const name3 = "test-multisig-3";
      try {
        await program.methods
          .createMaj(name3, [admin1.publicKey, admin2.publicKey], new anchor.BN(3))
          .accounts({ /* ... */ })
          .rpc();
        expect.fail("Should have thrown");
      } catch (e) {
        expect(e.message).to.include("TooManySignaturesRequired");
      }
    });
  });
```

### Test Suite 3: propose_transaction

```typescript
  describe("propose_transaction", () => {
    const name = "test-multisig";
    let instancePda: PublicKey;

    before(() => {
      [instancePda] = deriveMajInstance(name);
    });

    it("admin branch: auto-signs and auto-executes at threshold=2", async () => {
      // Admin 1 proposes — auto-sign gives 1 sig. Still needs 1 more.
      // Then admin 2 signs → reaches threshold → auto-executes.
      const recipient = Keypair.generate();
      const transferAmount = 0.1 * LAMPORTS_PER_SOL;

      // Fund the instance PDA
      await provider.connection.requestAirdrop(instancePda, LAMPORTS_PER_SOL);

      const [txPda] = deriveMajTx(instancePda, 0n);
      const [sigRecord1Pda] = deriveSignatureRecord(txPda, admin1.publicKey);
      const [adminRecord1Pda] = deriveAdminRecord(admin1.publicKey, instancePda);

      // Admin1 proposes (auto-signs)
      await program.methods
        .proposeTransaction(
          recipient.publicKey,
          new anchor.BN(transferAmount),
          Buffer.from([]),
          null,
          []
        )
        .accounts({
          majInstance: instancePda,
          majTransaction: txPda,
          proposer: admin1.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .remainingAccounts([
          // blacklist record PDA (doesn't exist, just needs to be passed for lamport check)
          { pubkey: deriveBlacklist(instancePda, admin1.publicKey)[0], isWritable: false, isSigner: false },
          // admin record PDA (exists → is admin)
          { pubkey: adminRecord1Pda, isWritable: false, isSigner: false },
          // sig record PDA (for auto-sign)
          { pubkey: sigRecord1Pda, isWritable: true, isSigner: false },
        ])
        .signers([admin1])
        .rpc();

      const tx = await program.account.majTransaction.fetch(txPda);
      expect(tx.numSignatures).to.equal(1);
      expect(tx.active).to.be.true;
    });

    it("non-admin branch: proposes without signing", async () => {
      const nonAdmin = Keypair.generate();
      await provider.connection.requestAirdrop(nonAdmin.publicKey, LAMPORTS_PER_SOL);

      const instance = await program.account.majInstance.fetch(instancePda);
      const txIndex = instance.txCount.toNumber();
      const [txPda] = deriveMajTx(instancePda, BigInt(txIndex));

      await program.methods
        .proposeTransaction(Keypair.generate().publicKey, new anchor.BN(0), Buffer.from([]), null, [])
        .accounts({
          majInstance: instancePda,
          majTransaction: txPda,
          proposer: nonAdmin.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .remainingAccounts([
          { pubkey: deriveBlacklist(instancePda, nonAdmin.publicKey)[0], isWritable: false, isSigner: false },
          { pubkey: deriveAdminRecord(nonAdmin.publicKey, instancePda)[0], isWritable: false, isSigner: false },
        ])
        .signers([nonAdmin])
        .rpc();

      const tx = await program.account.majTransaction.fetch(txPda);
      expect(tx.numSignatures).to.equal(0);
    });

    it("reverts for blacklisted address", async () => {
      // First, add address to blacklist via multisig... (see governance tests)
      // Then attempt propose — should fail with AddressIsBlacklisted
    });
  });
```

### Test Suite 4: sign_transaction

```typescript
  describe("sign_transaction", () => {
    it("reverts on duplicate signature", async () => {
      // Admin1 already signed in propose. Attempt sign again → DuplicateSignature.
    });

    it("auto-executes when threshold is reached", async () => {
      // Admin2 signs a tx that admin1 already signed → threshold met → auto-execute.
    });

    it("reverts for non-admin", async () => {
      // Non-admin attempts to sign → AdminRecord PDA constraint fails → OnlyAdmin.
    });
  });
```

### Test Suite 5: execute_transaction

```typescript
  describe("execute_transaction", () => {
    it("executes a SOL transfer successfully", async () => {
      // Propose + sign to threshold, then call execute_transaction explicitly.
      // Verify recipient balance increased.
    });

    it("executes a CPI call (e.g., governance instruction)", async () => {
      // Propose add_admin as CPI payload, sign to threshold, execute.
      // Verify new admin's AdminRecord PDA was created.
    });

    it("reverts with insufficient signatures", async () => {
      // Propose tx, do NOT reach threshold, call execute_transaction → InsufficientSignatures.
    });
  });
```

### Test Suite 6: revoke_signature

```typescript
  describe("revoke_signature", () => {
    it("reverts if admin has not signed", async () => {
      // Admin2 tries to revoke on a tx they never signed → UserHasNotSigned.
    });

    it("successfully revokes and decrements num_signatures", async () => {
      // Admin1 signs, then revokes. Verify num_signatures decremented.
      // Verify SignatureRecord PDA is closed.
    });
  });
```

### Test Suite 7: cancel_transaction

```typescript
  describe("cancel_transaction", () => {
    it("reverts for non-proposer admin", async () => {
      // Admin1 proposes. Admin2 tries to cancel → OnlyProposerCanCancel.
    });

    it("cancels successfully for proposer", async () => {
      // Admin1 proposes and cancels. Verify active = false.
    });

    it("reverts on already inactive transaction", async () => {
      // Try to cancel an already-cancelled tx → TransactionNotActive.
    });
  });
```

### Test Suite 8: add_admin (governance)

```typescript
  describe("add_admin (governance)", () => {
    it("fails if called directly (not via multisig CPI)", async () => {
      const newAdmin = Keypair.generate();
      const [instancePda] = deriveMajInstance("test-multisig");
      const [adminRecordPda] = deriveAdminRecord(newAdmin.publicKey, instancePda);

      try {
        // Direct call — maj_instance is not a signer, this should fail
        await program.methods
          .addAdmin(newAdmin.publicKey)
          .accounts({
            majInstance: instancePda,
            adminRecord: adminRecordPda,
            payer: provider.wallet.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
        expect.fail("Should have thrown — OnlyMaj");
      } catch (e) {
        // Signer constraint failure or OnlyMaj error
        expect(e.message).to.match(/signer|OnlyMaj/i);
      }
    });

    it("succeeds when triggered via an executed multisig transaction", async () => {
      // 1. Encode add_admin instruction data.
      // 2. Propose it as a CPI transaction with maj_instance as signer.
      // 3. Admin1 + admin2 sign → threshold met → auto-execute.
      // 4. Verify new AdminRecord PDA was created.
      // 5. Verify MajInstance.admins includes new admin.
    });

    it("reverts on duplicate admin via multisig", async () => {
      // Propose add_admin for an existing admin → DuplicateAdminAddress.
    });
  });
```

### Test Suite 9: remove_admin

```typescript
  describe("remove_admin (governance)", () => {
    it("reverts at two-admin minimum", async () => {
      // Instance has exactly 2 admins. Propose remove_admin → TwoAdminMinimum.
    });

    it("reverts for non-admin address", async () => {
      // Propose remove_admin for an address not in admins → AddressIsNotAdmin.
    });

    it("removes admin successfully with 3+ admins", async () => {
      // Add a third admin first, then remove one. Verify AdminRecord closed.
    });
  });
```

### Test Suite 10: change_sigs_required

```typescript
  describe("change_sigs_required (governance)", () => {
    it("reverts if new value < 2", async () => {
      // Propose change to 1 → TooFewSignaturesRequired.
    });

    it("reverts if new value > admins.length", async () => {
      // With 2 admins, propose change to 3 → TooManySignaturesRequired.
    });

    it("changes successfully within valid range", async () => {
      // With 3 admins, propose change to 3 → success. Verify sigs_required updated.
    });
  });
```

### Test Suite 11: blacklist

```typescript
  describe("add_blacklist / remove_blacklist (governance)", () => {
    it("blacklists an address via multisig", async () => {
      // Propose add_blacklist for a target.
      // Sign to threshold. Execute.
      // Verify BlacklistRecord PDA exists.
    });

    it("reverts if blacklisted address tries to propose", async () => {
      // Blacklisted address calls propose_transaction → AddressIsBlacklisted.
    });

    it("removes blacklist via multisig", async () => {
      // Propose remove_blacklist.
      // Execute. Verify BlacklistRecord PDA is closed.
      // Verify previously blacklisted address can now propose.
    });

    it("reverts remove_blacklist if address not blacklisted", async () => {
      // Propose remove_blacklist for non-blacklisted address → NotBlacklisted.
    });
  });
```

### Test Suite 12: getProgramAccounts — AdminRecord Query

```typescript
  describe("getProgramAccounts — admin reverse lookup", () => {
    it("returns all MajInstance PDAs an admin belongs to", async () => {
      const admin = admin1.publicKey;

      // Create two Maj instances with admin1
      // Then query:
      const accounts = await provider.connection.getProgramAccounts(
        program.programId,
        {
          filters: [
            {
              memcmp: {
                offset: 0,
                bytes: anchor.utils.bytes.bs58.encode(
                  program.coder.accounts.accountDiscriminator("AdminRecord")
                ),
              },
            },
            {
              memcmp: {
                offset: 8,
                bytes: admin.toBase58(),
              },
            },
          ],
        }
      );

      expect(accounts.length).to.be.greaterThanOrEqual(2);

      const majInstanceKeys = accounts.map(({ account }) =>
        new PublicKey(account.data.slice(40, 72))
      );

      // Verify both expected instance PDAs are in the result
      expect(majInstanceKeys.map(k => k.toBase58())).to.include(instancePda.toBase58());
    });
  });
```

---

## Cargo.toml & Anchor.toml

### programs/maj_core/Cargo.toml

```toml
[package]
name = "maj_core"
version = "0.1.0"
description = "Maj multisig program for Solana"
edition = "2021"

[lib]
crate-type = ["cdylib", "lib"]
name = "maj_core"

[features]
no-entrypoint = []
no-idl = []
no-log-ix-name = []
cpi = ["no-entrypoint"]
default = []
idl-build = ["anchor-lang/idl-build"]

[dependencies]
anchor-lang = { version = "0.30.1", features = ["init-if-needed"] }
solana-program = "1.18"

[dev-dependencies]
```

### Anchor.toml

```toml
[toolchain]
anchor_version = "0.30.1"

[features]
seeds = true
skip-lint = false

[programs.localnet]
maj_core = "<PROGRAM_ID>"

[programs.devnet]
maj_core = "<PROGRAM_ID>"

[programs.mainnet]
maj_core = "<PROGRAM_ID>"

[registry]
url = "https://api.apr.dev"

[provider]
cluster = "Localnet"
wallet = "~/.config/solana/id.json"

[scripts]
test = "yarn run ts-mocha -p ./tsconfig.json -t 1000000 tests/**/*.ts"
```

### tsconfig.json

```json
{
  "compilerOptions": {
    "types": ["mocha", "chai", "node"],
    "typeRoots": ["./node_modules/@types"],
    "lib": ["es2015", "dom"],
    "module": "commonjs",
    "target": "es6",
    "esModuleInterop": true,
    "strict": true
  }
}
```

### package.json (test dependencies)

```json
{
  "scripts": {
    "test": "yarn run ts-mocha -p ./tsconfig.json -t 1000000 tests/**/*.ts"
  },
  "dependencies": {
    "@coral-xyz/anchor": "^0.30.1"
  },
  "devDependencies": {
    "@types/bn.js": "^5.1.5",
    "@types/chai": "^4.3.12",
    "@types/mocha": "^10.0.6",
    "@types/node": "^20.14.0",
    "chai": "^4.3.4",
    "mocha": "^10.4.0",
    "ts-mocha": "^10.0.0",
    "typescript": "^5.4.5"
  }
}
```

---

## Deployment

### Prerequisites

```bash
# Install Anchor CLI
cargo install --git https://github.com/coral-xyz/anchor anchor-cli --tag v0.30.1 --locked

# Install Solana CLI
sh -c "$(curl -sSfL https://release.solana.com/stable/install)"

# Generate/set keypair
solana-keygen new --outfile ~/.config/solana/id.json
solana config set --url devnet
```

### Build

```bash
# From workspace root
anchor build
```

This compiles the program and generates:
- `target/deploy/maj_core.so` — deployable binary
- `target/idl/maj_core.json` — IDL for client SDK
- `target/types/maj_core.ts` — TypeScript type definitions

### Get Program ID

```bash
anchor keys list
# maj_core: <PUBKEY>
```

Copy the output pubkey into:
- `programs/maj_core/src/lib.rs` → `declare_id!("<PUBKEY>")`
- `Anchor.toml` → `[programs.localnet]` and `[programs.devnet]`

Then rebuild: `anchor build`

### Local Testing

```bash
# Start local validator
solana-test-validator

# Or use Anchor's integrated test runner (starts validator automatically)
anchor test
```

### Deploy to Devnet

```bash
# Fund your wallet on devnet
solana airdrop 2 --url devnet

# Deploy
anchor deploy --provider.cluster devnet

# Verify deployment
solana program show <PROGRAM_ID> --url devnet
```

### Deploy to Mainnet

```bash
# Ensure wallet has sufficient SOL for deployment (~2-3 SOL for this program size)
anchor deploy --provider.cluster mainnet

# Or with explicit keypair
anchor deploy --provider.cluster mainnet --provider.wallet /path/to/keypair.json
```

> **Important:** On Solana mainnet-beta, there is no direct equivalent of "Base chain-8453". The original contracts deployed to EVM Base (an L2). The Anchor program deploys to Solana mainnet-beta or devnet. Adjust your deployment targets in `Anchor.toml` accordingly.

### Exporting the IDL

The IDL is automatically generated at `target/idl/maj_core.json` after `anchor build`. To make it available to client SDKs:

```bash
# Publish IDL to chain (optional — allows clients to fetch IDL from chain)
anchor idl init --filepath target/idl/maj_core.json <PROGRAM_ID> --provider.cluster devnet
```

---

## Important Implementation Notes

### 1. admins Vec vs AdminRecord PDAs — Two Sources of Truth

The `MajInstance.admins: Vec<Pubkey>` and the `AdminRecord` PDAs serve different purposes:

- **`admins` Vec** — Authoritative ordered list for UI display, iteration (e.g., enumerating admins), and computing admin count for validation.
- **`AdminRecord` PDAs** — Authoritative membership check at instruction time. O(1). Used in `#[account(seeds = ...)]` constraints to verify an admin without iterating the Vec.

Both must be kept in sync. Whenever an admin is added or removed, update BOTH the Vec AND create/close the corresponding AdminRecord PDA.

### 2. No CPI-to-Self for Auto-Execute

When `sign_transaction` or `propose_transaction` (admin branch) triggers auto-execution upon reaching the threshold, do NOT do a CPI back into your own program. Anchor has complex account borrow rules that make CPI-to-self with mutable accounts difficult or impossible without `UncheckedAccount`. Instead:

**Extract `execute_transaction_logic` as a regular Rust function** (not an instruction handler) that takes mutable references to the required accounts and operates inline. Call this function from both `execute_transaction::handler` and the auto-execute branches.

### 3. Realloc and System Program

Any instruction that may `realloc` an account MUST include `system_program` and a mutable `payer` in its accounts context, even if the instruction doesn't always realloc. The Anchor `realloc` constraint (or manual realloc + rent top-up) requires these accounts to be available.

For governance instructions invoked via CPI, the payer passed in the CPI's `account_metas` must be a signer in the original transaction. The client must include it with `is_signer: true` in the `SerializedAccountMeta`.

### 4. Blacklist Check in propose_transaction

The `BlacklistRecord` for a non-existing blacklist entry will not be a real initialized account. The check must be:

```rust
// Account is passed in remaining_accounts.
// If not blacklisted, the account either doesn't exist or has 0 lamports.
let is_blacklisted = ctx.remaining_accounts
    .iter()
    .find(|a| a.key() == expected_blacklist_pda)
    .map(|a| a.lamports() > 0)
    .unwrap_or(false);

require!(!is_blacklisted, MajError::AddressIsBlacklisted);
```

The client must always pass the `BlacklistRecord` account (even if it doesn't exist) so the runtime can provide its lamport count. Pass it as `isWritable: false, isSigner: false`.

### 5. Minimum Admin Count = 2

Enforced at two points:
- `create_maj`: `admins.len() >= 2`
- `remove_admin`: `instance.admins.len() > 2` (must have more than 2 BEFORE removal, ensuring at least 2 remain)

The Solidity source uses `admins.length > 2` which enforces the same: you can only remove if there will still be at least 2 left.

### 6. num_signatures Must Be u32, Not u8

The Solidity source uses `uint8 numSignatures`, which caps at 255. Since Anchor's `realloc` allows unbounded admins, use `u32` for `MajTransaction.num_signatures`. This avoids overflow for large admin sets.

### 7. MajTransaction Space for propose_transaction

The `space` in the `init` constraint for `MajTransaction` must be computed from the instruction parameters. Anchor allows calling functions in the `#[instruction(...)]` attribute and using them in `space`. Pattern:

```rust
#[account(
    init,
    payer = proposer,
    space = MajTransaction::space_for(data.len(), account_metas.len()),
    seeds = [...],
    bump,
)]
pub maj_transaction: Account<'info, MajTransaction>,
```

This works because `data` and `account_metas` are bound in `#[instruction(to, value, data, program_id, account_metas)]`.

### 8. SOL Transfer from PDA

To transfer SOL out of a PDA (the `MajInstance` account is the treasury), you cannot use `system_program::transfer` with a PDA as the `from` (unless the PDA is a program-owned account — which it is, but `system_program::transfer` requires a signer, and PDAs sign via `invoke_signed`). The correct approach for lamport transfer from a PDA is direct lamport manipulation:

```rust
**maj_instance.to_account_info().try_borrow_mut_lamports()? -= lamports;
**recipient.try_borrow_mut_lamports()? += lamports;
```

This bypasses the `system_program` and directly adjusts lamports, which is valid for program-owned accounts. Ensure the recipient account is writable.

### 9. Seeds Must Be Stable for PDA Signing

When calling `invoke_signed` for governance CPIs, the seeds are:
```rust
&[b"maj_instance", name_bytes.as_slice(), &[bump]]
```

`name_bytes` must be a stable `Vec<u8>` derived from `maj_instance.name.as_bytes()`. Ensure you call `.to_vec()` on it BEFORE mutating `maj_instance` in any way, since the name is needed in the seeds.

### 10. Anchor Version Compatibility

This guide targets **Anchor 0.30.x**. Key API notes:
- Use `anchor_lang::system_program::create_account` for manual PDA creation via CPI.
- The `realloc` constraint is available since Anchor 0.27.
- `#[account(signer)]` enforces the account is a signer — use this for governance PDA signer checks.
- `AccountInfo::lamports()` returns `u64` — use this for existence checks.
- IDL build feature: add `idl-build = ["anchor-lang/idl-build"]` to `Cargo.toml` features.

### 11. Account Layout for getProgramAccounts Filters

The `AdminRecord` account layout (after the 8-byte Anchor discriminator) is:

```
Offset 0..8   : discriminator (8 bytes) — SHA256("account:AdminRecord")[0..8]
Offset 8..40  : admin: Pubkey (32 bytes)
Offset 40..72 : maj_instance: Pubkey (32 bytes)
Offset 72     : bump: u8 (1 byte)
```

When filtering by admin pubkey: `memcmp { offset: 8, bytes: admin.toBase58() }`.
When filtering by instance: `memcmp { offset: 40, bytes: instancePubkey.toBase58() }`.

Both filters combined give "all AdminRecords for a specific admin in a specific instance" — which should return at most 1 account (confirming membership).

### 12. Error Mapping for Anchor Constraint Failures

Some Solidity errors map to Anchor constraint failures rather than explicit `require!` checks:

| Solidity Error | Anchor Equivalent |
|---|---|
| `NameTaken` | `init` constraint failure (account already exists) |
| `DuplicateSignature` | `init` on `SignatureRecord` fails if it already exists |
| `UserHasNotSigned` | Account constraint fails if `SignatureRecord` PDA doesn't exist |
| `AddressIsNotAdmin` | `#[account(seeds = [b"admin_record", ...])]` fails if AdminRecord doesn't exist |

You may choose to intercept Anchor errors and re-emit custom errors using a middleware approach, or accept that the error messages will differ from the Solidity source. Document your choice in the program's README.

---

*End of Implementation Guide*
