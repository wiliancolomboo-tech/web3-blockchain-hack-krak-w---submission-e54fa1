#!/usr/bin/env node
import { Command } from "commander";
import { Connection, Keypair, PublicKey, LAMPORTS_PER_SOL } from "@solana/web3.js";
import * as anchor from "@coral-xyz/anchor";
import { BN } from "@coral-xyz/anchor";
import * as fs from "fs";
import * as path from "path";
import dotenv from "dotenv";
import { AegisClient } from "./client";

dotenv.config();

const program = new Command();
program
  .name("aegis")
  .description("AegisKraków Enterprise CLI for Multi-Milestone Solana Escrows")
  .version("1.0.0");

function loadKeypair(keypairPath: string): Keypair {
  const resolvedPath = path.resolve(keypairPath);
  const secretKey = JSON.parse(fs.readFileSync(resolvedPath, "utf-8"));
  return Keypair.fromSecretKey(new Uint8Array(secretKey));
}

function getClient(keypairPath?: string): AegisClient {
  const rpcUrl = process.env.SOLANA_RPC_URL || "http://127.0.0.1:8899";
  const connection = new Connection(rpcUrl, "confirmed");
  const kp = keypairPath
    ? loadKeypair(keypairPath)
    : Keypair.generate();
  const wallet = new anchor.Wallet(kp);
  return new AegisClient(connection, wallet);
}

program
  .command("init")
  .description("Initialize a new multi-milestone escrow")
  .requiredOption("-k, --keypair <path>", "Path to payer keypair JSON")
  .requiredOption("-i, --id <number>", "Unique escrow identifier")
  .requiredOption("-r, --recipient <pubkey>", "Recipient wallet public key")
  .requiredOption("-a, --arbiter <pubkey>", "Arbiter wallet public key")
  .requiredOption("-m, --milestones <amounts>", "Comma-separated list of milestone amounts in SOL")
  .option("-t, --timeout <seconds>", "Escrow timeout in seconds", "86400")
  .action(async (opts) => {
    const client = getClient(opts.keypair);
    const escrowId = new BN(opts.id);
    const recipient = new PublicKey(opts.recipient);
    const arbiter = new PublicKey(opts.arbiter);
    const milestonesLamports = opts.milestones
      .split(",")
      .map((s: string) => new BN(parseFloat(s.trim()) * LAMPORTS_PER_SOL));
    const timeout = new BN(parseInt(opts.timeout));

    console.log(`Initializing Escrow #${opts.id}...`);
    const tx = await client.initializeEscrow(
      escrowId,
      recipient,
      arbiter,
      milestonesLamports,
      timeout
    );
    const [escrowPda] = client.getEscrowPda(client.provider.wallet.publicKey, escrowId);
    console.log(`✔ Success! Escrow Address: ${escrowPda.toBase58()}`);
    console.log(`✔ Transaction Signature: ${tx}`);
  });

program
  .command("release")
  .description("Release the next sequential milestone")
  .requiredOption("-k, --keypair <path>", "Path to payer keypair JSON")
  .requiredOption("-e, --escrow <pubkey>", "Escrow PDA public key")
  .requiredOption("-r, --recipient <pubkey>", "Recipient destination address")
  .action(async (opts) => {
    const client = getClient(opts.keypair);
    const escrowPda = new PublicKey(opts.escrow);
    const recipient = new PublicKey(opts.recipient);

    console.log(`Releasing milestone for escrow ${escrowPda.toBase58()}...`);
    const tx = await client.releaseMilestone(escrowPda, recipient);
    console.log(`✔ Success! Milestone funds disbursed.`);
    console.log(`✔ Tx Signature: ${tx}`);
  });

program
  .command("dispute")
  .description("Flag and freeze an escrow into disputed state")
  .requiredOption("-k, --keypair <path>", "Path to initiator keypair JSON")
  .requiredOption("-e, --escrow <pubkey>", "Escrow PDA public key")
  .requiredOption("-s, --reason <text>", "Reason description for dispute")
  .action(async (opts) => {
    const client = getClient(opts.keypair);
    const escrowPda = new PublicKey(opts.escrow);

    console.log(`Opening dispute for ${escrowPda.toBase58()}...`);
    const tx = await client.openDispute(escrowPda, opts.reason);
    console.log(`✔ Dispute filed on-chain.`);
    console.log(`✔ Tx Signature: ${tx}`);
  });

program
  .command("resolve")
  .description("Resolve a dispute as authorized arbiter")
  .requiredOption("-k, --keypair <path>", "Path to arbiter keypair JSON")
  .requiredOption("-e, --escrow <pubkey>", "Escrow PDA public key")
  .requiredOption("-p, --payer <pubkey>", "Payer public key")
  .requiredOption("-r, --recipient <pubkey>", "Recipient public key")
  .requiredOption("--payer-bps <number>", "Payer split basis points (0-10000)")
  .requiredOption("--recipient-bps <number>", "Recipient split basis points (0-10000)")
  .action(async (opts) => {
    const client = getClient(opts.keypair);
    const escrowPda = new PublicKey(opts.escrow);
    const payer = new PublicKey(opts.payer);
    const recipient = new PublicKey(opts.recipient);
    const payerBps = parseInt(opts.payerBps);
    const recipientBps = parseInt(opts.recipientBps);

    console.log(`Executing arbitration ruling on ${escrowPda.toBase58()}...`);
    const tx = await client.resolveDispute(escrowPda, payer, recipient, payerBps, recipientBps);
    console.log(`✔ Arbitration settlement confirmed.`);
    console.log(`✔ Tx Signature: ${tx}`);
  });

program.parse(process.argv);