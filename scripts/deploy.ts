/**
 * deploy.ts
 *
 * Deploys the maj_core program and initialises the global registry PDA.
 *
 * Usage:
 *   yarn deploy:localnet
 *   yarn deploy:devnet
 *   yarn deploy:mainnet
 *
 * The CLUSTER env var controls which network to target (set by the npm scripts).
 * ANCHOR_WALLET may be set to override the default ~/.config/solana/id.json.
 */

import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, BN } from "@coral-xyz/anchor";
import {
  Connection,
  PublicKey,
  SystemProgram,
  clusterApiUrl,
  Cluster,
} from "@solana/web3.js";
import { readFileSync } from "fs";
import path from "path";

// ── Config ────────────────────────────────────────────────────────────────────

const CLUSTER = (process.env.CLUSTER ?? "localnet") as string;

const RPC_URLS: Record<string, string> = {
  localnet: "http://127.0.0.1:8899",
  devnet: clusterApiUrl("devnet"),
  mainnet: clusterApiUrl("mainnet-beta"),
};

const rpcUrl = RPC_URLS[CLUSTER];
if (!rpcUrl) {
  console.error(`Unknown CLUSTER "${CLUSTER}". Use localnet, devnet, or mainnet.`);
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
const provider = new AnchorProvider(connection, wallet, {
  commitment: "confirmed",
});
anchor.setProvider(provider);

const program = new Program(idl, provider);
const pid = program.programId;

// ── Helpers ───────────────────────────────────────────────────────────────────

function deriveRegistry(): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [Buffer.from("maj_registry")],
    pid
  );
  return pda;
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main() {
  console.log(`\n=== maj_core deployment ===`);
  console.log(`Cluster  : ${CLUSTER} (${rpcUrl})`);
  console.log(`Program  : ${pid.toBase58()}`);
  console.log(`Payer    : ${wallet.publicKey.toBase58()}\n`);

  const registryPda = deriveRegistry();
  console.log(`Registry PDA: ${registryPda.toBase58()}`);

  // Check if registry is already initialised.
  const existing = await connection.getAccountInfo(registryPda);
  if (existing) {
    console.log("Registry already initialised — skipping initialize_registry.");
  } else {
    console.log("Calling initialize_registry ...");
    const tx = await program.methods
      .initializeRegistry()
      .accounts({
        registry: registryPda,
        payer: wallet.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .rpc();
    console.log(`initialize_registry tx: ${tx}`);
    console.log("Registry initialised successfully.");
  }

  // Fetch and print registry state.
  const registry = await (program.account as any).majRegistry.fetch(registryPda);
  console.log(`\nRegistry state:`);
  console.log(`  total_instances: ${registry.totalInstances.toString()}`);
  console.log(`  bump           : ${registry.bump}`);

  console.log("\nDeployment complete.\n");
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
