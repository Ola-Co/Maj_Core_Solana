# Maj Core — Solana / Anchor

> A faithful port of the **Maj multisig system** (originally `MajFactory.sol` + `Maj.sol` on EVM) to Solana using the [Anchor](https://www.anchor-lang.com/) framework.

---

## What Was Built

`maj_core` is a single Anchor program that replicates the full behaviour of the Ethereum Maj multisig contracts on Solana.  It replaces:

| EVM concept | Solana / Anchor equivalent |
|---|---|
| Deployed contract instance | PDA (`maj_instance`) owned by this program |
| `address → bool` mapping | `AdminRecord` / `BlacklistRecord` PDA existence |
| `onlyMaj` modifier | `#[account(signer)]` on the PDA — only reachable via `invoke_signed` |
| `MajFactory.adminMajContracts` | Off-chain `getProgramAccounts` query |
| Dynamic arrays | Anchor `realloc` for unbounded growth |

---

## Architecture

### Single Program, PDA-as-Instance

All factory logic and per-instance multisig logic live in one program (`maj_core`).  Every unique Maj wallet is a PDA keyed by its human-readable name:

```
[b"maj_instance", name.as_bytes()]  →  MajInstance (holds SOL treasury)
```

### PDA Map

```
[b"maj_registry"]                               → MajRegistry (global counter)
[b"maj_name", name_bytes]                       → MajNameRecord (name reservation)
[b"maj_instance", name_bytes]                   → MajInstance   (wallet + treasury)
[b"maj_tx", instance_key, tx_index_le_bytes]    → MajTransaction
[b"sig", maj_tx_key, signer_key]                → SignatureRecord (existence = signed)
[b"admin_record", admin_key, instance_key]      → AdminRecord   (membership proof)
[b"blacklist", instance_key, target_key]        → BlacklistRecord (existence = banned)
```

### onlyMaj Security Pattern

Governance instructions (`add_admin`, `remove_admin`, `change_sigs_required`, `add_blacklist`, `remove_blacklist`) all carry `#[account(signer)]` on the `MajInstance` PDA.  A PDA can only sign via `invoke_signed` — called internally by `execute_transaction_logic` after a multisig vote reaches the required threshold.  Direct wallet calls to any governance instruction will fail the signer constraint at the Anchor constraint layer, before any handler code runs.

---

## File Structure

```
programs/
  maj_core/
    src/
      lib.rs                        ← declare_id!, #[program] dispatch
      errors.rs                     ← MajError enum (20 variants)
      events.rs                     ← Anchor event structs
      state/
        mod.rs
        registry.rs                 ← MajRegistry
        instance.rs                 ← MajInstance, MajNameRecord, AdminRecord, BlacklistRecord
        transaction.rs              ← MajTransaction, SerializedAccountMeta, SignatureRecord
      instructions/
        mod.rs
        initialize_registry.rs      ← InitializeRegistry
        create_maj.rs               ← CreateMaj (factory entry-point)
        propose_transaction.rs      ← ProposeTransaction (with admin auto-sign)
        sign_transaction.rs         ← SignTransaction (with auto-execute)
        execute_transaction.rs      ← ExecuteTransaction + shared execute_transaction_logic
        revoke_signature.rs         ← RevokeSignature
        cancel_transaction.rs       ← CancelTransaction
        governance.rs               ← AddAdmin, RemoveAdmin, ChangeSigsRequired,
                                       AddBlacklist, RemoveBlacklist
    Cargo.toml
tests/
  maj_core.ts                       ← Anchor Mocha tests
Anchor.toml
Cargo.toml                          ← Workspace manifest
package.json
tsconfig.json
```

---

## Instructions

| Instruction | Caller | Description |
|---|---|---|
| `initialize_registry` | Anyone (once) | Creates the global `MajRegistry` singleton |
| `create_maj` | Anyone | Deploys a new named multisig PDA |
| `propose_transaction` | Any non-blacklisted address | Creates a `MajTransaction`; admins auto-sign and auto-execute |
| `sign_transaction` | Admin | Adds a signature; auto-executes at threshold |
| `execute_transaction` | Admin | Manually triggers a threshold-met transaction |
| `revoke_signature` | Admin | Removes a signature; closes SignatureRecord PDA |
| `cancel_transaction` | Admin (proposer only) | Marks transaction inactive without executing |
| `add_admin` | **onlyMaj** — via CPI | Adds an admin; reallocs MajInstance if needed |
| `remove_admin` | **onlyMaj** — via CPI | Removes an admin; enforces ≥ 2 minimum |
| `change_sigs_required` | **onlyMaj** — via CPI | Updates signature threshold |
| `add_blacklist` | **onlyMaj** — via CPI | Blacklists an address |
| `remove_blacklist` | **onlyMaj** — via CPI | Removes a blacklist entry |

### Transaction Types

```
SOL Transfer:  program_id = None,   data = [],   account_metas = [],   value = lamports
CPI Call:      program_id = Some(pid), data = discriminator ++ borsh_args, account_metas = [...]
```

### Proposing Governance via the Multisig

To propose a governance action (e.g. `add_admin`), a client must:

1. Compute the Anchor discriminator: first 8 bytes of `SHA256("global:add_admin")`.
2. Borsh-encode the instruction arguments.
3. Concatenate: `data = discriminator ++ args`.
4. Build `account_metas` with `MajInstance` marked as `is_signer: true`.
5. Call `propose_transaction` with `program_id = Some(maj_core_program_id)`.

---

## Admin Reverse Lookup (off-chain)

There is no on-chain list of "which instances does admin X belong to".  Instead, query `getProgramAccounts` with two `memcmp` filters:

```typescript
const discriminator = program.coder.accounts.accountDiscriminator("AdminRecord");

const accounts = await connection.getProgramAccounts(programId, {
  filters: [
    { memcmp: { offset: 0,  bytes: bs58.encode(discriminator) } },
    { memcmp: { offset: 8,  bytes: adminPubkey.toBase58() } },  // admin field at offset 8
  ],
});

const instanceKeys = accounts.map(({ account }) =>
  new PublicKey(account.data.slice(40, 72))  // maj_instance field at offset 40
);
```

---

## Setup & Build

### Prerequisites

```bash
# 1 — Solana + Anchor CLI (installs both in one step)
curl --proto '=https' --tlsv1.2 -sSfL https://solana-install.solana.workers.dev | bash

# Restart your shell (or source the updated profile), then verify:
solana --version
anchor --version   # should print anchor-cli 0.30.1

# 2 — Node / Yarn
node --version   # >= 18 recommended
yarn --version
```

### Configure Solana keypair

```bash
solana-keygen new --outfile ~/.config/solana/id.json
solana config set --url devnet
```

### Install JS dependencies

```bash
yarn install
```

### Build

```bash
anchor build
```

This generates:
- `target/deploy/maj_core.so`  — deployable BPF binary
- `target/idl/maj_core.json`   — IDL for client SDKs
- `target/types/maj_core.ts`   — TypeScript type definitions

### Set the Program ID

After the first build, run:

```bash
anchor keys list
# maj_core: <PUBKEY>
```

Replace the placeholder in two places:

1. `programs/maj_core/src/lib.rs` → `declare_id!("<PUBKEY>");`
2. `Anchor.toml` → `[programs.localnet]`, `[programs.devnet]`, `[programs.mainnet]`

Then rebuild:

```bash
anchor build
```

---

## Testing

Tests use Anchor's built-in localnet validator.

```bash
anchor test
```

The test suite covers:

| Suite | What is tested |
|---|---|
| `initialize_registry` | Happy path + idempotency guard |
| `create_maj` | Happy path, duplicate name, invalid threshold, duplicate admin |
| `propose_transaction` | Non-admin (no auto-sign), admin auto-sign |
| `sign_transaction` + auto-execute | Threshold reached → SOL transfer fires |
| `revoke_signature` | Decrement + PDA closed; reverts if not signed |
| `cancel_transaction` | Proposer cancels; non-proposer reverts |
| `execute_transaction` | Explicit execution after threshold |
| `add_admin` direct call | Signer constraint rejects it (onlyMaj guard) |
| `getProgramAccounts` | AdminRecord reverse lookup returns correct instance keys |

---

## Deployment

### Devnet

```bash
solana airdrop 2 --url devnet
anchor deploy --provider.cluster devnet
```

### Mainnet

```bash
anchor deploy --provider.cluster mainnet --provider.wallet /path/to/keypair.json
```

### Publish IDL (optional)

```bash
anchor idl init --filepath target/idl/maj_core.json <PROGRAM_ID> \
  --provider.cluster devnet
```

---

## Key Design Notes

### Two Sources of Truth for Admins

`MajInstance.admins` (Vec) holds the canonical ordered list for display and iteration.  `AdminRecord` PDAs hold membership for O(1) on-chain checks via account constraints.  Both are kept in sync — every `add_admin` / `remove_admin` updates the Vec AND creates/closes the corresponding PDA.

### No CPI-to-Self for Auto-Execute

When `sign_transaction` or `propose_transaction` (admin branch) reaches the signature threshold, execution is triggered by calling the shared `execute_transaction_logic` Rust function **inline** — not via a CPI back into the same program.  This avoids Anchor's mutable account borrow restrictions that would arise from a CPI-to-self.

### Realloc & Rent Top-Up

`MajInstance` is allocated with space for 10 admins.  If `add_admin` would exceed this, the instruction manually reallocates (`AccountInfo::realloc`) and tops up rent lamports from the payer before mutating the Vec.  The payer for governance CPIs must be included in `account_metas` with `is_signer: true`.

### SOL Transfer from PDA

Because `MajInstance` is program-owned, lamports are moved using direct `RefCell` manipulation (`try_borrow_mut_lamports`) rather than `system_program::transfer`.  This is valid for program-owned accounts and does not require a PDA signer at the lamport-manipulation layer.

### CPI Reload After Governance Execution

When `execute_transaction_logic` completes a governance CPI (e.g., `add_admin` reallocs `maj_instance`), the handler calls `maj_instance.reload()` to update the in-memory deserialized state from the raw account data.  Without this, Anchor's `AccountExit` serialization would overwrite the CPI's changes on exit.

---

## Error Reference

| Error | Code | Description |
|---|---|---|
| `DuplicateAdminAddress` | 6000 | Same pubkey appears twice in admins |
| `ZeroAddress` | 6001 | Default pubkey (all zeros) not allowed |
| `TooManySignaturesRequired` | 6002 | Threshold > admins.len() |
| `TooFewSignaturesRequired` | 6003 | Threshold < 2 |
| `OnlyAdmin` | 6004 | Caller is not a registered admin |
| `TransactionNotActive` | 6005 | Transaction is already executed or cancelled |
| `DuplicateSignature` | 6006 | Admin already signed this transaction |
| `InsufficientSignatures` | 6007 | Not enough signatures to execute |
| `TransactionFailed` | 6008 | SOL transfer or CPI call failed |
| `UserHasNotSigned` | 6009 | Revoke attempted without having signed |
| `OnlyProposerCanCancel` | 6010 | Only the original proposer may cancel |
| `OnlyMaj` | 6011 | Governance instruction called directly (not via multisig) |
| `AddressIsNotAdmin` | 6012 | Removal target is not an admin |
| `TwoAdminMinimum` | 6013 | Removal would leave fewer than 2 admins |
| `NameTaken` | 6014 | A Maj with this name already exists |
| `NameNotFound` | 6015 | No Maj found with this name |
| `NotBlacklisted` | 6016 | Remove-blacklist target is not blacklisted |
| `AddressIsBlacklisted` | 6017 | Proposer is blacklisted |
| `NameTooLong` | 6018 | Name exceeds 64 UTF-8 bytes |

---

## License

MIT
