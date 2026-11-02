import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { SolwawelProtocol } from "../target/types/solwawel_protocol";
import { PublicKey, SystemProgram, Keypair, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { expect } from "chai";
import crypto from "crypto";

describe("SolWawel Protocol: Comprehensive Test Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.SolwawelProtocol as Program<SolwawelProtocol>;

  const payer = (provider.wallet as any).payer as Keypair;
  const beneficiary = Keypair.generate();
  const arbiter = Keypair.generate();
  const treasury = Keypair.generate();
  const unauthorizedParty = Keypair.generate();

  const escrowId = new anchor.BN(Date.now());
  const milestone1Amount = new anchor.BN(1 * LAMPORTS_PER_SOL);
  const milestone2Amount = new anchor.BN(2 * LAMPORTS_PER_SOL);
  const totalAmount = milestone1Amount.add(milestone2Amount);

  let escrowPda: PublicKey;
  let escrowBump: number;
  let vaultPda: PublicKey;
  let vaultBump: number;

  before(async () => {
    // Airdrop accounts for gas & collateral verification
    const sig1 = await provider.connection.requestAirdrop(beneficiary.publicKey, 2 * LAMPORTS_PER_SOL);
    const sig2 = await provider.connection.requestAirdrop(arbiter.publicKey, 2 * LAMPORTS_PER_SOL);
    const sig3 = await provider.connection.requestAirdrop(unauthorizedParty.publicKey, 1 * LAMPORTS_PER_SOL);
    
    await provider.connection.confirmTransaction(sig1);
    await provider.connection.confirmTransaction(sig2);
    await provider.connection.confirmTransaction(sig3);

    [escrowPda, escrowBump] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("escrow"),
        payer.publicKey.toBuffer(),
        escrowId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    [vaultPda, vaultBump] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("vault"),
        payer.publicKey.toBuffer(),
        escrowId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );
  });

  it("1. Successfully initializes an Escrow with PDA Vault lockup", async () => {
    const futureDeadline = Math.floor(Date.now() / 1000) + 3600; // +1 hour
    const milestoneAmounts = [milestone1Amount, milestone2Amount];
    const milestoneDeadlines = [new anchor.BN(futureDeadline), new anchor.BN(futureDeadline + 3600)];

    const tx = await program.methods
      .initializeEscrow(
        escrowId,
        totalAmount,
        milestoneAmounts,
        milestoneDeadlines,
        "Krakow Hackathon Demo Settlement"
      )
      .accounts({
        payer: payer.publicKey,
        beneficiary: beneficiary.publicKey,
        arbiter: arbiter.publicKey,
        treasury: treasury.publicKey,
        escrow: escrowPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
      })
      .rpc();

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(escrowAccount.escrowId.toString()).to.equal(escrowId.toString());
    expect(escrowAccount.payer.toBase58()).to.equal(payer.publicKey.toBase58());
    expect(escrowAccount.beneficiary.toBase58()).to.equal(beneficiary.publicKey.toBase58());
    expect(escrowAccount.totalAmount.toNumber()).to.equal(totalAmount.toNumber());
    expect(escrowAccount.remainingAmount.toNumber()).to.equal(totalAmount.toNumber());
    expect(escrowAccount.currentMilestoneIndex).to.equal(0);
    expect(escrowAccount.milestoneCount).to.equal(2);
    expect(escrowAccount.isCompleted).to.be.false;

    // Verify vault balance matches totalAmount
    const vaultBalance = await provider.connection.getBalance(vaultPda);
    expect(vaultBalance).to.equal(totalAmount.toNumber());
  });

  it("2. Rejects unauthorized milestone settlement attempt", async () => {
    const proofHash = Array.from(crypto.createHash("sha256").update("proof1").digest());

    try {
      await program.methods
        .completeMilestone(escrowId, 0, proofHash)
        .accounts({
          signer: unauthorizedParty.publicKey,
          escrow: escrowPda,
          vault: vaultPda,
          beneficiary: beneficiary.publicKey,
          treasury: treasury.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([unauthorizedParty])
        .rpc();
      expect.fail("Should have failed with unauthorized signer error");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("UnauthorizedSigner");
    }
  });

  it("3. Rejects out-of-order milestone completion", async () => {
    const proofHash = Array.from(crypto.createHash("sha256").update("proof-out-of-order").digest());

    try {
      await program.methods
        .completeMilestone(escrowId, 1, proofHash) // Trying index 1 before 0
        .accounts({
          signer: arbiter.publicKey,
          escrow: escrowPda,
          vault: vaultPda,
          beneficiary: beneficiary.publicKey,
          treasury: treasury.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([arbiter])
        .rpc();
      expect.fail("Should have failed with InvalidMilestoneProgression");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("InvalidMilestoneProgression");
    }
  });

  it("4. Successfully settles Milestone 0 with protocol fee deduction", async () => {
    const initialBeneficiaryBalance = await provider.connection.getBalance(beneficiary.publicKey);
    const initialTreasuryBalance = await provider.connection.getBalance(treasury.publicKey);

    const proofHash = Array.from(crypto.createHash("sha256").update("deliverable-phase-1").digest());

    await program.methods
      .completeMilestone(escrowId, 0, proofHash)
      .accounts({
        signer: arbiter.publicKey,
        escrow: escrowPda,
        vault: vaultPda,
        beneficiary: beneficiary.publicKey,
        treasury: treasury.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([arbiter])
      .rpc();

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(escrowAccount.currentMilestoneIndex).to.equal(1);
    expect(escrowAccount.remainingAmount.toNumber()).to.equal(milestone2Amount.toNumber());

    // Protocol Fee: 0.25% of 1 SOL = 2,500,000 lamports
    const expectedFee = 2_500_000;
    const expectedBeneficiaryDelta = 1_000_000_000 - expectedFee;

    const postBeneficiaryBalance = await provider.connection.getBalance(beneficiary.publicKey);
    const postTreasuryBalance = await provider.connection.getBalance(treasury.publicKey);

    expect(postBeneficiaryBalance - initialBeneficiaryBalance).to.equal(expectedBeneficiaryDelta);
    expect(postTreasuryBalance - initialTreasuryBalance).to.equal(expectedFee);
  });

  it("5. Settles final Milestone 1 and transitions Escrow to completed state", async () => {
    const proofHash = Array.from(crypto.createHash("sha256").update("final-deliverable").digest());

    await program.methods
      .completeMilestone(escrowId, 1, proofHash)
      .accounts({
        signer: payer.publicKey,
        escrow: escrowPda,
        vault: vaultPda,
        beneficiary: beneficiary.publicKey,
        treasury: treasury.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .rpc();

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(escrowAccount.currentMilestoneIndex).to.equal(2);
    expect(escrowAccount.remainingAmount.toNumber()).to.equal(0);
    expect(escrowAccount.isCompleted).to.be.true;

    const vaultBalance = await provider.connection.getBalance(vaultPda);
    expect(vaultBalance).to.equal(0);
  });

  it("6. Rejects cancellation or refund on an already completed escrow", async () => {
    try {
      await program.methods
        .refundCancelledEscrow(escrowId)
        .accounts({
          signer: arbiter.publicKey,
          escrow: escrowPda,
          vault: vaultPda,
          payer: payer.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([arbiter])
        .rpc();
      expect.fail("Should have failed with EscrowAlreadyCompleted");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("EscrowAlreadyCompleted");
    }
  });
});