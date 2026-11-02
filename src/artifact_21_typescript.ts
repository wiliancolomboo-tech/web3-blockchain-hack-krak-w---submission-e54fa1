import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { KrakowEscrow } from "../target/types/krakow_escrow";
import { Keypair, LAMPORTS_PER_SOL, PublicKey, SystemProgram } from "@solana/web3.js";
import { expect } from "chai";
import crypto from "crypto";

describe("krakow_escrow end-to-end QA matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.KrakowEscrow as Program<KrakowEscrow>;

  const payer = Keypair.generate();
  const payee = Keypair.generate();
  const arbiter = Keypair.generate();
  const unauthorizedUser = Keypair.generate();

  const escrowId = new anchor.BN(Date.now());
  const depositAmount = new anchor.BN(1 * LAMPORTS_PER_SOL);
  const milestoneString = "Complete frontend + backend PR: Krakow Hackathon #1";
  const expectedHash = Array.from(crypto.createHash("sha256").update(milestoneString).digest());

  let escrowPda: PublicKey;
  let vaultPda: PublicKey;
  let bump: number;
  let vaultBump: number;

  before(async () => {
    // Airdrop funds to actors
    for (const actor of [payer, payee, arbiter, unauthorizedUser]) {
      const sig = await provider.connection.requestAirdrop(actor.publicKey, 5 * LAMPORTS_PER_SOL);
      const latestBlockhash = await provider.connection.getLatestBlockhash();
      await provider.connection.confirmTransaction({
        signature: sig,
        ...latestBlockhash,
      });
    }

    const idBuffer = escrowId.toArrayLike(Buffer, "le", 8);
    [escrowPda, bump] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), payer.publicKey.toBuffer(), idBuffer],
      program.programId
    );

    [vaultPda, vaultBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), escrowPda.toBuffer()],
      program.programId
    );
  });

  it("1. Initializes the escrow agreement with valid constraints", async () => {
    const futureDeadline = new anchor.BN(Math.floor(Date.now() / 1000) + 3600); // 1 hour future

    await program.methods
      .initializeEscrow(escrowId, depositAmount, futureDeadline, expectedHash)
      .accounts({
        payer: payer.publicKey,
        payee: payee.publicKey,
        arbiter: arbiter.publicKey,
        escrowAccount: escrowPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([payer])
      .rpc();

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(escrowAccount.payer.toBase58()).to.equal(payer.publicKey.toBase58());
    expect(escrowAccount.payee.toBase58()).to.equal(payee.publicKey.toBase58());
    expect(escrowAccount.amount.toNumber()).to.equal(depositAmount.toNumber());
    expect(JSON.stringify(escrowAccount.status)).to.include("initialized");
  });

  it("2. Funds the escrow PDA vault via CPI transfer", async () => {
    const payerInitialBal = await provider.connection.getBalance(payer.publicKey);

    await program.methods
      .fundEscrow()
      .accounts({
        payer: payer.publicKey,
        escrowAccount: escrowPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([payer])
      .rpc();

    const vaultBalance = await provider.connection.getBalance(vaultPda);
    expect(vaultBalance).to.equal(depositAmount.toNumber());

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(JSON.stringify(escrowAccount.status)).to.include("funded");
  });

  it("3. Rejects milestone submission from an unauthorized actor", async () => {
    try {
      await program.methods
        .submitMilestone(expectedHash)
        .accounts({
          payee: unauthorizedUser.publicKey,
          escrowAccount: escrowPda,
        })
        .signers([unauthorizedUser])
        .rpc();
      expect.fail("Should have thrown unauthorized error");
    } catch (err: any) {
      expect(err.toString()).to.include("UnauthorizedParty");
    }
  });

  it("4. Payee submits valid cryptographic milestone hash", async () => {
    await program.methods
      .submitMilestone(expectedHash)
      .accounts({
        payee: payee.publicKey,
        escrowAccount: escrowPda,
      })
      .signers([payee])
      .rpc();

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(JSON.stringify(escrowAccount.status)).to.include("submitted");
    expect(Buffer.from(escrowAccount.submittedMilestoneHash).toString("hex")).to.equal(
      Buffer.from(expectedHash).toString("hex")
    );
  });

  it("5. Approves milestone and releases funds accurately to Payee", async () => {
    const payeePreBalance = await provider.connection.getBalance(payee.publicKey);

    await program.methods
      .approveMilestone()
      .accounts({
        payer: payer.publicKey,
        payee: payee.publicKey,
        escrowAccount: escrowPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([payer])
      .rpc();

    const payeePostBalance = await provider.connection.getBalance(payee.publicKey);
    expect(payeePostBalance - payeePreBalance).to.equal(depositAmount.toNumber());

    const vaultPostBalance = await provider.connection.getBalance(vaultPda);
    expect(vaultPostBalance).to.equal(0);

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(JSON.stringify(escrowAccount.status)).to.include("settled");
  });

  it("6. Handles Dispute & Arbitrated 50/50 Split Scenario", async () => {
    // Initialize a 2nd escrow to test arbitration resolution path
    const disputeId = new anchor.BN(Date.now() + 100);
    const [disputeEscrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), payer.publicKey.toBuffer(), disputeId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );
    const [disputeVaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), disputeEscrowPda.toBuffer()],
      program.programId
    );

    const futureDeadline = new anchor.BN(Math.floor(Date.now() / 1000) + 3600);

    await program.methods
      .initializeEscrow(disputeId, depositAmount, futureDeadline, expectedHash)
      .accounts({
        payer: payer.publicKey,
        payee: payee.publicKey,
        arbiter: arbiter.publicKey,
        escrowAccount: disputeEscrowPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([payer])
      .rpc();

    await program.methods
      .fundEscrow()
      .accounts({
        payer: payer.publicKey,
        escrowAccount: disputeEscrowPda,
        vault: disputeVaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([payer])
      .rpc();

    // Payer raises dispute
    await program.methods
      .raiseDispute()
      .accounts({
        signer: payer.publicKey,
        escrowAccount: disputeEscrowPda,
      })
      .signers([payer])
      .rpc();

    const payeePreBalance = await provider.connection.getBalance(payee.publicKey);
    const payerPreBalance = await provider.connection.getBalance(payer.publicKey);

    // Arbiter resolves: 50% (5,000 basis points)
    await program.methods
      .resolveDispute(5000)
      .accounts({
        arbiter: arbiter.publicKey,
        payer: payer.publicKey,
        payee: payee.publicKey,
        escrowAccount: disputeEscrowPda,
        vault: disputeVaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([arbiter])
      .rpc();

    const payeePostBalance = await provider.connection.getBalance(payee.publicKey);
    const payerPostBalance = await provider.connection.getBalance(payer.publicKey);

    const half = depositAmount.toNumber() / 2;
    expect(payeePostBalance - payeePreBalance).to.equal(half);
    expect(payerPostBalance - payerPreBalance).to.equal(half);
  });
});