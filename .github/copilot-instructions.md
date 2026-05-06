# Copilot Instructions

## Build, test, and lint commands

- `anchor build` builds the Anchor program and regenerates `target/idl/maj_core.json` plus `target/types/maj_core.ts`.
- `anchor test` runs the full integration suite against Anchor's local validator.
- `anchor test --skip-build` reruns the suite after a successful build.
- `yarn ts-mocha -p ./tsconfig.json -t 1000000 tests/maj_core.ts --grep "create_maj"` runs a single test suite or case; replace the grep string with a `describe(...)` or `it(...)` title from `tests/maj_core.ts`.
- There is no dedicated lint script configured in `package.json`.
- After the first successful build, run `anchor keys list` and keep the `maj_core` program ID synchronized between `programs/maj_core/src/lib.rs` (`declare_id!`) and all `[programs.*]` entries in `Anchor.toml`.

## High-level architecture

- `maj_core` is a single Anchor program that combines factory logic and per-instance multisig logic. There is no separate factory program and no child program deployment.
- Each multisig wallet is a PDA derived from `[b"maj_instance", name.as_bytes()]`. That account is both the identity of the multisig and its SOL treasury.
- Other important PDAs are:
  - `[b"maj_registry"]` for the global instance counter
  - `[b"maj_name", name]` for unique name reservation
  - `[b"maj_tx", maj_instance, tx_index]` for proposed transactions
  - `[b"sig", maj_tx, signer]` for per-admin signatures
  - `[b"admin_record", admin, maj_instance]` for admin membership
  - `[b"blacklist", maj_instance, target]` for proposal blacklisting
- Governance instructions (`add_admin`, `remove_admin`, `change_sigs_required`, `add_blacklist`, `remove_blacklist`) implement the Solidity `onlyMaj` pattern by requiring the `MajInstance` PDA itself to be a signer. That only happens inside `execute_transaction_logic` through `invoke_signed`, so direct wallet calls to governance instructions are expected to fail.
- Transactions come in two modes:
  - SOL transfer: `program_id = None`, `data = []`, `account_metas = []`
  - CPI call: `program_id = Some(pid)` with Borsh-encoded instruction data and serialized account metas stored in `MajTransaction`
- Governance proposals are self-CPI calls back into `maj_core`: clients must compute the Anchor instruction discriminator, Borsh-encode the args, and include `MajInstance` in the CPI metas as a signer.
- Reverse lookup of "which multisigs is this wallet an admin of?" is intentionally off-chain. Query `AdminRecord` PDAs with `getProgramAccounts`; there is no on-chain reverse index.
- Auto-execution is shared inline Rust logic (`execute_transaction_logic`), not a CPI-to-self. After governance CPIs mutate or realloc the multisig account, the code reloads `maj_instance` before exiting so Anchor does not overwrite the updated state.

## Key conventions

- Membership, blacklist status, and signature status are modeled by PDA existence rather than boolean fields. Many checks rely on PDA presence or `lamports() > 0`, not on data stored inside `MajInstance`.
- `MajInstance.admins` and `AdminRecord` PDAs are both maintained on purpose: the vector is the display/iteration list, while `AdminRecord` is the O(1) membership proof used by constraints and off-chain reverse lookup.
- `remaining_accounts` is part of the instruction contract, not an optional escape hatch. Important cases:
  - `create_maj` expects one `AdminRecord` PDA per admin, in admin list order
  - `propose_transaction` expects the blacklist PDA, the proposer's admin PDA, the proposer's signature PDA when auto-signing, then any destination/CPI accounts needed for auto-execution
  - `sign_transaction` and `execute_transaction` expect transfer destinations or CPI accounts in `remaining_accounts`
- Account sizing is manual and significant:
  - `MajInstance::INITIAL_SPACE` only preallocates for 10 admins and governance code tops up rent before reallocating beyond that
  - `MajTransaction` space is derived from `data.len()` and `account_metas.len()`
- Removing an admin uses `swap_remove`, so admin ordering is not stable after removals.
- SOL transfers out of the multisig use direct lamport mutation on the program-owned `MajInstance`; they do not call `system_program::transfer`.
- Tests are full Anchor integration tests in `tests/maj_core.ts`, with PDA derivation helpers in the test file itself and generated program types imported from `target/types/maj_core`.
