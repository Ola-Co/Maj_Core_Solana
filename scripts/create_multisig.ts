/**
 * create_multisig.ts
 *
 * Creates a new named multisig instance on-chain.
 *
 * Usage:
 *   yarn create-multisig --name <name> --admins <addr1,addr2,...> --sigs-required <n> [--cluster localnet|devnet|mainnet]
 *
 * Examples:
 *   yarn create-multisig --name my-safe --admins 2Abc...,3Def... --sigs-required 2
 *   CLUSTER=devnet yarn create-multisig --name my-safe --admins 2Abc...,3Def... --sigs-required 2
 */

import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, BN } from "@coral-xyz/anchor";
import {
  Connection,
  PublicKey,
  SystemProgram,
  clusterApiUrl,
} from "@solana/web3.js";
import { readFileSync } from "fs";
import path from "path";

// ── CLI args ──────────────────────────────────────────────────────────────────

function arg(flag: string): string | undefined {
  const idx = process.argv.indexOf(flag);
  return idx !== -1 ? process.argv[idx + 1] : undefined;
}

const nameArg = arg("--name");
const adminsArg = arg("--admins");
const sigsArg = arg("--sigs-required");

if (!nameArg || !adminsArg || !sigsArg) {
  console.error(
    "Usage: yarn create-multisig --name <name> --admins <addr1,addr2,...> --sigs-required <n>"
  );
  process.exit(1);
}

const MULTISIG_NAME = nameArg;
const ADMIN_PUBKEYS = adminsArg.split(",").map((s) => new PublicKey(s.trim()));
const SIGS_REQUIRED = new BN(sigsArg);

// ── Config ────────────────────────────────────────────────────────────────────

const CLUSTER = process.env.CLUSTER ?? "localnet";

const RPC_URLS: Record<string, string> = {
  localnet: "http://127.0.0.1:8899",
  devnet: clusterApiUrl("devnet"),
  mainnet: clusterApiUrl("mainnet-beta"),
};

const rpcUrl = RPC_URLS[CLUSTER];
if (!rpcUrl) {
  console.error(`Unknown CLUSTER "${CLUSTER}".`);
  process.exit(1);
}

// ── Load IDL ──────────────────────────────────────────────────────────────────

const idlPath = path.resolve(__dirname, "../target/idl/maj_core.json");
const idl = JSON.parse(readFileSync(idlPath, "utf8"));

// ── Provider ─────────────────────────────────────────────────────────────────

const walletPath =
  process.env.ANCHOR_WALLET ??
  `${process.env.HOME}/.config/solana/id.json`;

const keypairBytes = JSON.parse(readFileSync(walletPath, "utf8"));
const keypair = anchor.web3.Keypair.fromSecretKey(Uint8Array.from(keypairBytes));
const wallet = new anchor.Wallet(keypair);

const connection = new Connection(rpcUrl, "confirmed");
const provider = new AnchorProvider(connection, wallet, { commitment: "confirmed" });
anchor.setProvider(provider);

const program = new Program(idl, provider);
const pid = program.programId;

// ── PDA helpers ───────────────────────────────────────────────────────────────

function deriveRegistry(): PublicKey {
  return PublicKey.findProgramAddressSync([Buffer.from("maj_registry")], pid)[0];
}

function deriveMajInstance(name: string): PublicKey {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("maj_instance"), Buffer.from(name)],
    pid
  )[0];
}

function deriveNameRecord(name: string): PublicKey {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("maj_name"), Buffer.from(name)],
    pid
  )[0];
}

function deriveAdminRecord(admin: PublicKey, instance: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("admin_record"), admin.toBytes(), instance.toBytes()],
    pid
  )[0];
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main() {
  console.log(`\n=== create_maj ===`);
  console.log(`Cluster         : ${CLUSTER}`);
  console.log(`Program         : ${pid.toBase58()}`);
  console.log(`Payer           : ${wallet.publicKey.toBase58()}`);
  console.log(`Multisig name   : ${MULTISIG_NAME}`);
  console.log(`Admins          : ${ADMIN_PUBKEYS.map((k) => k.toBase58()).join(", ")}`);
  console.log(`Sigs required   : ${SIGS_REQUIRED.toString()}\n`);

  // Guard: registry must already exist.
  const registryPda = deriveRegistry();
  const registryInfo = await connection.getAccountInfo(registryPda);
  if (!registryInfo) {
    console.error(
      "Registry not initialised. Run `yarn deploy:localnet` (or devnet/mainnet) first."
    );
    process.exit(1);
  }

  const instancePda = deriveMajInstance(MULTISIG_NAME);
  const nameRecordPda = deriveNameRecord(MULTISIG_NAME);
  const adminRecordPdas = ADMIN_PUBKEYS.map((admin) =>
    deriveAdminRecord(admin, instancePda)
  );

  console.log(`Instance PDA    : ${instancePda.toBase58()}`);
  console.log(`Name record PDA : ${nameRecordPda.toBase58()}`);
  adminRecordPdas.forEach((pda, i) =>
    console.log(`AdminRecord[${i}]  : ${pda.toBase58()}`)
  );
  console.log();

  const tx = await program.methods
    .createMaj(MULTISIG_NAME, ADMIN_PUBKEYS, SIGS_REQUIRED)
    .accounts({
      registry: registryPda,
      nameRecord: nameRecordPda,
      majInstance: instancePda,
      payer: wallet.publicKey,
      systemProgram: SystemProgram.programId,
    })
    .remainingAccounts(
      adminRecordPdas.map((pda) => ({
        pubkey: pda,
        isSigner: false,
        isWritable: true,
      }))
    )
    .rpc();

  console.log(`create_maj tx: ${tx}`);

  const instance = await (program.account as any).majInstance.fetch(instancePda);
  console.log(`\nMultisig created:`);
  console.log(`  name          : ${instance.name}`);
  console.log(`  sigs_required : ${instance.sigsRequired.toString()}`);
  console.log(
    `  admins        : ${instance.admins.map((a: PublicKey) => a.toBase58()).join(", ")}`
  );
  console.log(`  tx_count      : ${instance.txCount.toString()}`);
  console.log();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
