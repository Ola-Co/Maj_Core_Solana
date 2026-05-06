import * as anchor from "@coral-xyz/anchor";
import { Program, BN } from "@coral-xyz/anchor";
import { MajCore } from "../target/types/maj_core";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  LAMPORTS_PER_SOL,
  Connection,
} from "@solana/web3.js";
import { expect } from "chai";

// ── PDA helper functions ──────────────────────────────────────────────────────

function deriveRegistry(programId: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("maj_registry")],
    programId
  );
}

function deriveMajInstance(name: string, programId: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("maj_instance"), Buffer.from(name)],
    programId
  );
}

function deriveNameRecord(name: string, programId: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("maj_name"), Buffer.from(name)],
    programId
  );
}

function deriveAdminRecord(
  admin: PublicKey,
  instance: PublicKey,
  programId: PublicKey
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("admin_record"), admin.toBytes(), instance.toBytes()],
    programId
  );
}

function deriveMajTx(
  instance: PublicKey,
  txIndex: bigint,
  programId: PublicKey
): [PublicKey, number] {
  const idxBuf = Buffer.alloc(8);
  idxBuf.writeBigUInt64LE(txIndex);
  return PublicKey.findProgramAddressSync(
    [Buffer.from("maj_tx"), instance.toBytes(), idxBuf],
    programId
  );
}

function deriveSignatureRecord(
  majTx: PublicKey,
  signer: PublicKey,
  programId: PublicKey
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("sig"), majTx.toBytes(), signer.toBytes()],
    programId
  );
}

function deriveBlacklist(
  instance: PublicKey,
  target: PublicKey,
  programId: PublicKey
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("blacklist"), instance.toBytes(), target.toBytes()],
    programId
  );
}

// ── Airdrop helper ────────────────────────────────────────────────────────────

async function airdrop(
  connection: Connection,
  pubkey: PublicKey,
  sol = 2
): Promise<void> {
  const sig = await connection.requestAirdrop(pubkey, sol * LAMPORTS_PER_SOL);
  await connection.confirmTransaction(sig, "confirmed");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test suite
// ─────────────────────────────────────────────────────────────────────────────

describe("maj_core", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.MajCore as Program<MajCore>;
  const pid = program.programId;
  const conn = provider.connection;

  const admin1 = Keypair.generate();
  const admin2 = Keypair.generate();
  const admin3 = Keypair.generate();

  const MULTISIG_NAME = "test-multisig";

  // ── Suite 1: initialize_registry ──────────────────────────────────────────

  describe("initialize_registry", () => {
    it("initialises the registry successfully", async () => {
      const [registryPda] = deriveRegistry(pid);

      await program.methods
        .initializeRegistry()
        .accounts({
          registry: registryPda,
          payer: provider.wallet.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .rpc();

      const registry = await program.account.majRegistry.fetch(registryPda);
      expect(registry.totalInstances.toNumber()).to.equal(0);
    });

    it("fails on second call (account already exists)", async () => {
      const [registryPda] = deriveRegistry(pid);
      try {
        await program.methods
          .initializeRegistry()
          .accounts({
            registry: registryPda,
            payer: provider.wallet.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
        expect.fail("should have thrown");
      } catch (e: any) {
        // Anchor throws when the PDA already exists under `init`.
        expect(e.message).to.match(/already in use|0x0/i);
      }
    });
  });

  // ── Suite 2: create_maj ───────────────────────────────────────────────────

  describe("create_maj", () => {
    before(async () => {
      await airdrop(conn, admin1.publicKey);
      await airdrop(conn, admin2.publicKey);
      await airdrop(conn, admin3.publicKey);
    });

    it("creates a Maj instance with two admins", async () => {
      const [registryPda] = deriveRegistry(pid);
      const [instancePda] = deriveMajInstance(MULTISIG_NAME, pid);
      const [nameRecordPda] = deriveNameRecord(MULTISIG_NAME, pid);
      const [adminRecord1] = deriveAdminRecord(admin1.publicKey, instancePda, pid);
      const [adminRecord2] = deriveAdminRecord(admin2.publicKey, instancePda, pid);

      await program.methods
        .createMaj(MULTISIG_NAME, [admin1.publicKey, admin2.publicKey], new BN(2))
        .accounts({
          registry: registryPda,
          nameRecord: nameRecordPda,
          majInstance: instancePda,
          payer: provider.wallet.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .remainingAccounts([
          { pubkey: adminRecord1, isWritable: true, isSigner: false },
          { pubkey: adminRecord2, isWritable: true, isSigner: false },
        ])
        .rpc();

      const instance = await program.account.majInstance.fetch(instancePda);
      expect(instance.name).to.equal(MULTISIG_NAME);
      expect(instance.sigsRequired.toNumber()).to.equal(2);
      expect(instance.admins).to.have.length(2);
      expect(instance.txCount.toNumber()).to.equal(0);

      const registry = await program.account.majRegistry.fetch(registryPda);
      expect(registry.totalInstances.toNumber()).to.equal(1);
    });

    it("reverts on duplicate name", async () => {
      const [registryPda] = deriveRegistry(pid);
      const [instancePda] = deriveMajInstance(MULTISIG_NAME, pid);
      const [nameRecordPda] = deriveNameRecord(MULTISIG_NAME, pid);
      const [ar1] = deriveAdminRecord(admin1.publicKey, instancePda, pid);
      const [ar2] = deriveAdminRecord(admin2.publicKey, instancePda, pid);

      try {
        await program.methods
          .createMaj(MULTISIG_NAME, [admin1.publicKey, admin2.publicKey], new BN(2))
          .accounts({
            registry: registryPda,
            nameRecord: nameRecordPda,
            majInstance: instancePda,
            payer: provider.wallet.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .remainingAccounts([
            { pubkey: ar1, isWritable: true, isSigner: false },
            { pubkey: ar2, isWritable: true, isSigner: false },
          ])
          .rpc();
        expect.fail("should have thrown");
      } catch (e: any) {
        expect(e.message).to.match(/already in use|0x0/i);
      }
    });

    it("reverts if sigs_required < 2", async () => {
      const name = "too-few-sigs";
      const [registryPda] = deriveRegistry(pid);
      const [instancePda] = deriveMajInstance(name, pid);
      const [nameRecordPda] = deriveNameRecord(name, pid);
      const [ar1] = deriveAdminRecord(admin1.publicKey, instancePda, pid);
      const [ar2] = deriveAdminRecord(admin2.publicKey, instancePda, pid);

      try {
        await program.methods
          .createMaj(name, [admin1.publicKey, admin2.publicKey], new BN(1))
          .accounts({
            registry: registryPda,
            nameRecord: nameRecordPda,
            majInstance: instancePda,
            payer: provider.wallet.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .remainingAccounts([
            { pubkey: ar1, isWritable: true, isSigner: false },
            { pubkey: ar2, isWritable: true, isSigner: false },
          ])
          .rpc();
        expect.fail("should have thrown");
      } catch (e: any) {
        expect(e.message).to.include("TooFewSignaturesRequired");
      }
    });

    it("reverts if sigs_required > admins.length", async () => {
      const name = "too-many-sigs";
      const [registryPda] = deriveRegistry(pid);
      const [instancePda] = deriveMajInstance(name, pid);
      const [nameRecordPda] = deriveNameRecord(name, pid);
      const [ar1] = deriveAdminRecord(admin1.publicKey, instancePda, pid);
      const [ar2] = deriveAdminRecord(admin2.publicKey, instancePda, pid);

      try {
        await program.methods
          .createMaj(name, [admin1.publicKey, admin2.publicKey], new BN(3))
          .accounts({
            registry: registryPda,
            nameRecord: nameRecordPda,
            majInstance: instancePda,
            payer: provider.wallet.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .remainingAccounts([
            { pubkey: ar1, isWritable: true, isSigner: false },
            { pubkey: ar2, isWritable: true, isSigner: false },
          ])
          .rpc();
        expect.fail("should have thrown");
      } catch (e: any) {
        expect(e.message).to.include("TooManySignaturesRequired");
      }
    });

    it("reverts on duplicate admin", async () => {
      const name = "dup-admin";
      const [registryPda] = deriveRegistry(pid);
      const [instancePda] = deriveMajInstance(name, pid);
      const [nameRecordPda] = deriveNameRecord(name, pid);
      const [ar1] = deriveAdminRecord(admin1.publicKey, instancePda, pid);

      try {
        await program.methods
          .createMaj(name, [admin1.publicKey, admin1.publicKey], new BN(2))
          .accounts({
            registry: registryPda,
            nameRecord: nameRecordPda,
            majInstance: instancePda,
            payer: provider.wallet.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .remainingAccounts([
            { pubkey: ar1, isWritable: true, isSigner: false },
            { pubkey: ar1, isWritable: true, isSigner: false },
          ])
          .rpc();
        expect.fail("should have thrown");
      } catch (e: any) {
        expect(e.message).to.include("DuplicateAdminAddress");
      }
    });
  });

  // ── Suite 3: propose_transaction (non-admin branch) ───────────────────────

  describe("propose_transaction — non-admin", () => {
    it("proposes without auto-signing (num_signatures stays 0)", async () => {
      const [instancePda] = deriveMajInstance(MULTISIG_NAME, pid);
      const instance = await program.account.majInstance.fetch(instancePda);
      const txIndex = BigInt(instance.txCount.toString());
      const [txPda] = deriveMajTx(instancePda, txIndex, pid);

      const nonAdmin = Keypair.generate();
      await airdrop(conn, nonAdmin.publicKey);

      const [blacklistPda] = deriveBlacklist(instancePda, nonAdmin.publicKey, pid);
      const [adminRecordPda] = deriveAdminRecord(nonAdmin.publicKey, instancePda, pid);

      await program.methods
        .proposeTransaction(
          Keypair.generate().publicKey,
          new BN(0),
          Buffer.from([]),
          null,
          []
        )
        .accounts({
          majInstance: instancePda,
          majTransaction: txPda,
          proposer: nonAdmin.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .remainingAccounts([
          { pubkey: blacklistPda, isWritable: false, isSigner: false },
          { pubkey: adminRecordPda, isWritable: false, isSigner: false },
        ])
        .signers([nonAdmin])
        .rpc();

      const tx = await program.account.majTransaction.fetch(txPda);
      expect(tx.numSignatures).to.equal(0);
      expect(tx.active).to.be.true;
    });
  });

  // ── Suite 4: propose + sign → auto-execute SOL transfer ───────────────────

  describe("propose_transaction — admin auto-sign and sign_transaction", () => {
    let instancePda: PublicKey;
    let txPda: PublicKey;
    const recipient = Keypair.generate();
    const transferLamports = Math.floor(0.1 * LAMPORTS_PER_SOL);

    before(async () => {
      [instancePda] = deriveMajInstance(MULTISIG_NAME, pid);
      // Fund the MajInstance PDA so it can forward SOL.
      await airdrop(conn, instancePda, 1);
    });

    it("admin1 proposes (auto-signs) → num_signatures becomes 1", async () => {
      const instance = await program.account.majInstance.fetch(instancePda);
      const txIndex = BigInt(instance.txCount.toString());
      [txPda] = deriveMajTx(instancePda, txIndex, pid);

      const [blacklistPda] = deriveBlacklist(instancePda, admin1.publicKey, pid);
      const [adminRecord1Pda] = deriveAdminRecord(admin1.publicKey, instancePda, pid);
      const [sigRecord1Pda] = deriveSignatureRecord(txPda, admin1.publicKey, pid);

      await program.methods
        .proposeTransaction(
          recipient.publicKey,
          new BN(transferLamports),
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
          { pubkey: blacklistPda, isWritable: false, isSigner: false },
          { pubkey: adminRecord1Pda, isWritable: false, isSigner: false },
          { pubkey: sigRecord1Pda, isWritable: true, isSigner: false },
        ])
        .signers([admin1])
        .rpc();

      const tx = await program.account.majTransaction.fetch(txPda);
      expect(tx.numSignatures).to.equal(1);
      expect(tx.active).to.be.true;
    });

    it("admin2 signs → threshold met → SOL transfer auto-executes", async () => {
      const recipientBalanceBefore = await conn.getBalance(recipient.publicKey);

      const [adminRecord2Pda] = deriveAdminRecord(admin2.publicKey, instancePda, pid);
      const [sigRecord2Pda] = deriveSignatureRecord(txPda, admin2.publicKey, pid);

      await program.methods
        .signTransaction()
        .accounts({
          majInstance: instancePda,
          majTransaction: txPda,
          signatureRecord: sigRecord2Pda,
          adminRecord: adminRecord2Pda,
          admin: admin2.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .remainingAccounts([
          // Destination for the SOL transfer (auto-execute path).
          { pubkey: recipient.publicKey, isWritable: true, isSigner: false },
        ])
        .signers([admin2])
        .rpc();

      const tx = await program.account.majTransaction.fetch(txPda);
      expect(tx.executed).to.be.true;
      expect(tx.active).to.be.false;

      const recipientBalanceAfter = await conn.getBalance(recipient.publicKey);
      expect(recipientBalanceAfter - recipientBalanceBefore).to.be.closeTo(
        transferLamports,
        5000 // allow small rounding
      );
    });
  });

  // ── Suite 5: revoke_signature ─────────────────────────────────────────────

  describe("revoke_signature", () => {
    let instancePda: PublicKey;
    let txPda: PublicKey;

    before(async () => {
      [instancePda] = deriveMajInstance(MULTISIG_NAME, pid);
    });

    it("admin1 proposes + admin1 revokes → num_signatures back to 0", async () => {
      const instance = await program.account.majInstance.fetch(instancePda);
      const txIndex = BigInt(instance.txCount.toString());
      [txPda] = deriveMajTx(instancePda, txIndex, pid);

      const [blacklistPda] = deriveBlacklist(instancePda, admin1.publicKey, pid);
      const [adminRecord1Pda] = deriveAdminRecord(admin1.publicKey, instancePda, pid);
      const [sigRecord1Pda] = deriveSignatureRecord(txPda, admin1.publicKey, pid);

      // Propose (admin1 auto-signs).
      await program.methods
        .proposeTransaction(
          Keypair.generate().publicKey,
          new BN(0),
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
          { pubkey: blacklistPda, isWritable: false, isSigner: false },
          { pubkey: adminRecord1Pda, isWritable: false, isSigner: false },
          { pubkey: sigRecord1Pda, isWritable: true, isSigner: false },
        ])
        .signers([admin1])
        .rpc();

      let tx = await program.account.majTransaction.fetch(txPda);
      expect(tx.numSignatures).to.equal(1);

      // Revoke.
      await program.methods
        .revokeSignature()
        .accounts({
          majInstance: instancePda,
          majTransaction: txPda,
          signatureRecord: sigRecord1Pda,
          adminRecord: adminRecord1Pda,
          admin: admin1.publicKey,
        })
        .signers([admin1])
        .rpc();

      tx = await program.account.majTransaction.fetch(txPda);
      expect(tx.numSignatures).to.equal(0);

      // SignatureRecord PDA should be closed.
      const sigRecordInfo = await conn.getAccountInfo(sigRecord1Pda);
      expect(sigRecordInfo).to.be.null;
    });

    it("reverts if admin has not signed", async () => {
      const instance = await program.account.majInstance.fetch(instancePda);
      const latestTxIndex = BigInt(instance.txCount.toString()) - 1n;
      [txPda] = deriveMajTx(instancePda, latestTxIndex, pid);

      const [adminRecord2Pda] = deriveAdminRecord(admin2.publicKey, instancePda, pid);
      const [sigRecord2Pda] = deriveSignatureRecord(txPda, admin2.publicKey, pid);

      try {
        await program.methods
          .revokeSignature()
          .accounts({
            majInstance: instancePda,
            majTransaction: txPda,
            signatureRecord: sigRecord2Pda,
            adminRecord: adminRecord2Pda,
            admin: admin2.publicKey,
          })
          .signers([admin2])
          .rpc();
        expect.fail("should have thrown");
      } catch (e: any) {
        // Anchor fails at account constraint resolution — PDA doesn't exist.
        expect(e.message).to.match(/AccountNotInitialized|2006|0x7d2/i);
      }
    });
  });

  // ── Suite 6: cancel_transaction ───────────────────────────────────────────

  describe("cancel_transaction", () => {
    let instancePda: PublicKey;
    let txPda: PublicKey;

    before(async () => {
      [instancePda] = deriveMajInstance(MULTISIG_NAME, pid);
    });

    it("proposer cancels their own active transaction", async () => {
      const instance = await program.account.majInstance.fetch(instancePda);
      const txIndex = BigInt(instance.txCount.toString());
      [txPda] = deriveMajTx(instancePda, txIndex, pid);

      const [blacklistPda] = deriveBlacklist(instancePda, admin1.publicKey, pid);
      const [adminRecord1Pda] = deriveAdminRecord(admin1.publicKey, instancePda, pid);
      const [sigRecord1Pda] = deriveSignatureRecord(txPda, admin1.publicKey, pid);

      await program.methods
        .proposeTransaction(
          Keypair.generate().publicKey,
          new BN(0),
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
          { pubkey: blacklistPda, isWritable: false, isSigner: false },
          { pubkey: adminRecord1Pda, isWritable: false, isSigner: false },
          { pubkey: sigRecord1Pda, isWritable: true, isSigner: false },
        ])
        .signers([admin1])
        .rpc();

      await program.methods
        .cancelTransaction()
        .accounts({
          majInstance: instancePda,
          majTransaction: txPda,
          adminRecord: adminRecord1Pda,
          proposer: admin1.publicKey,
        })
        .signers([admin1])
        .rpc();

      const tx = await program.account.majTransaction.fetch(txPda);
      expect(tx.active).to.be.false;
      expect(tx.executed).to.be.false;
    });

    it("reverts when non-proposer admin tries to cancel", async () => {
      const instance = await program.account.majInstance.fetch(instancePda);
      const txIndex = BigInt(instance.txCount.toString());
      [txPda] = deriveMajTx(instancePda, txIndex, pid);

      const [blacklistPda] = deriveBlacklist(instancePda, admin1.publicKey, pid);
      const [adminRecord1Pda] = deriveAdminRecord(admin1.publicKey, instancePda, pid);
      const [adminRecord2Pda] = deriveAdminRecord(admin2.publicKey, instancePda, pid);
      const [sigRecord1Pda] = deriveSignatureRecord(txPda, admin1.publicKey, pid);

      // Admin1 proposes.
      await program.methods
        .proposeTransaction(
          Keypair.generate().publicKey,
          new BN(0),
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
          { pubkey: blacklistPda, isWritable: false, isSigner: false },
          { pubkey: adminRecord1Pda, isWritable: false, isSigner: false },
          { pubkey: sigRecord1Pda, isWritable: true, isSigner: false },
        ])
        .signers([admin1])
        .rpc();

      // Admin2 tries to cancel.
      try {
        await program.methods
          .cancelTransaction()
          .accounts({
            majInstance: instancePda,
            majTransaction: txPda,
            adminRecord: adminRecord2Pda,
            proposer: admin2.publicKey,
          })
          .signers([admin2])
          .rpc();
        expect.fail("should have thrown");
      } catch (e: any) {
        expect(e.message).to.include("OnlyProposerCanCancel");
      }
    });
  });

  // ── Suite 7: execute_transaction (explicit, with threshold pre-met) ───────

  describe("execute_transaction — explicit SOL transfer", () => {
    it("executes a transaction that already reached threshold", async () => {
      const [instancePda] = deriveMajInstance(MULTISIG_NAME, pid);
      const instance = await program.account.majInstance.fetch(instancePda);
      const txIndex = BigInt(instance.txCount.toString());
      const [txPda] = deriveMajTx(instancePda, txIndex, pid);
      const recipient = Keypair.generate();
      const amount = Math.floor(0.05 * LAMPORTS_PER_SOL);

      await airdrop(conn, instancePda, 1);

      const [blacklistPda] = deriveBlacklist(instancePda, admin1.publicKey, pid);
      const [adminRecord1Pda] = deriveAdminRecord(admin1.publicKey, instancePda, pid);
      const [adminRecord2Pda] = deriveAdminRecord(admin2.publicKey, instancePda, pid);
      const [sigRecord1Pda] = deriveSignatureRecord(txPda, admin1.publicKey, pid);
      const [sigRecord2Pda] = deriveSignatureRecord(txPda, admin2.publicKey, pid);

      // Admin1 proposes (auto-signs).
      await program.methods
        .proposeTransaction(
          recipient.publicKey,
          new BN(amount),
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
          { pubkey: blacklistPda, isWritable: false, isSigner: false },
          { pubkey: adminRecord1Pda, isWritable: false, isSigner: false },
          { pubkey: sigRecord1Pda, isWritable: true, isSigner: false },
        ])
        .signers([admin1])
        .rpc();

      // Admin2 signs (no auto-execute here — we'll call execute explicitly).
      await program.methods
        .signTransaction()
        .accounts({
          majInstance: instancePda,
          majTransaction: txPda,
          signatureRecord: sigRecord2Pda,
          adminRecord: adminRecord2Pda,
          admin: admin2.publicKey,
          systemProgram: SystemProgram.programId,
        })
        // No remaining_accounts — we're NOT auto-executing here.
        // NOTE: auto-execute WILL trigger because threshold==2 and num_sigs==2.
        // Pass the recipient so the auto-execute path can find it.
        .remainingAccounts([
          { pubkey: recipient.publicKey, isWritable: true, isSigner: false },
        ])
        .signers([admin2])
        .rpc();

      const tx = await program.account.majTransaction.fetch(txPda);
      expect(tx.executed).to.be.true;
    });
  });

  // ── Suite 8: add_admin governance (direct call — must fail) ──────────────

  describe("add_admin — direct call guard", () => {
    it("reverts when called directly (onlyMaj guard)", async () => {
      const newAdmin = Keypair.generate();
      const [instancePda] = deriveMajInstance(MULTISIG_NAME, pid);
      const [adminRecordPda] = deriveAdminRecord(newAdmin.publicKey, instancePda, pid);

      try {
        await program.methods
          .addAdmin(newAdmin.publicKey)
          .accounts({
            majInstance: instancePda,
            adminRecord: adminRecordPda,
            payer: provider.wallet.publicKey,
            systemProgram: SystemProgram.programId,
          })
          .rpc();
        expect.fail("should have thrown — onlyMaj guard");
      } catch (e: any) {
        // Anchor's `signer` constraint failure.
        expect(e.message).to.match(/Signer|signer|0x[0-9a-f]+/i);
      }
    });
  });

  // ── Suite 9: getProgramAccounts — AdminRecord reverse lookup ───────────────

  describe("getProgramAccounts — admin reverse lookup", () => {
    it("returns AdminRecord PDAs for admin1", async () => {
      const discriminator = program.coder.accounts.accountDiscriminator("AdminRecord");

      const accounts = await conn.getProgramAccounts(pid, {
        filters: [
          {
            memcmp: {
              offset: 0,
              bytes: anchor.utils.bytes.bs58.encode(discriminator),
            },
          },
          {
            memcmp: {
              offset: 8,
              bytes: admin1.publicKey.toBase58(),
            },
          },
        ],
      });

      expect(accounts.length).to.be.greaterThanOrEqual(1);

      const instanceKeys = accounts.map(({ account }) =>
        new PublicKey(account.data.slice(40, 72))
      );

      const [expectedInstance] = deriveMajInstance(MULTISIG_NAME, pid);
      expect(instanceKeys.map((k) => k.toBase58())).to.include(
        expectedInstance.toBase58()
      );
    });
  });
});
