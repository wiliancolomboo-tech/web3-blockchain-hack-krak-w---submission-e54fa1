import { Command } from "commander";
import { Connection, Keypair, PublicKey, LAMPORTS_PER_SOL } from "@solana/web3.js";
import * as anchor from "@coral-xyz/anchor";
import fs from "fs";
import crypto from "crypto";

const program = new Command();
program.version("1.0.0").description("KrakowPay Enterprise Settlement CLI");

function loadKeypair(path: string): Keypair {
  const secretKey = JSON.parse(fs.readFileSync(path, "utf-8"));
  return Keypair.fromSecretKey(new Uint8Array(secretKey));
}

program
  .command("create-hash")
  .description("Generate Blake3 / SHA-256 deliverable payload hash")
  .argument("<data>", "Milestone data string or IPFS CID")
  .action((data: string) => {
    const hash = crypto.createHash("sha256").update(data).digest();
    console.log("32-Byte Milestone Hash (Hex):", hash.toString("hex"));
    console.log("Array representation:", JSON.stringify(Array.from(hash)));
  });

program
  .command("inspect-vault")
  .description("Query on-chain escrow balance and derive PDA accounts")
  .requiredOption("-p, --payer <string>", "Payer Public Key")
  .requiredOption("-i, --id <number>", "Escrow ID")
  .action(async (options) => {
    const connection = new Connection("https://api.devnet.solana.com", "confirmed");
    const programId = new PublicKey("KrkPay1111111111111111111111111111111111111");
    const payerKey = new PublicKey(options.payer);
    const idBuffer = Buffer.alloc(8);
    idBuffer.writeBigUInt64LE(BigInt(options.id));

    const [escrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), payerKey.toBuffer(), idBuffer],
      programId
    );
    const [vaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), escrowPda.toBuffer()],
      programId
    );

    const balance = await connection.getBalance(vaultPda);
    console.log(`\n--- KrakowPay On-Chain Account State ---`);
    console.log(`Escrow PDA: ${escrowPda.toBase58()}`);
    console.log(`Vault PDA:  ${vaultPda.toBase58()}`);
    console.log(`Vault Balance: ${balance / LAMPORTS_PER_SOL} SOL (${balance} Lamports)`);
  });

program.parse(process.argv);