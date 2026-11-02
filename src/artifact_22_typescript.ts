import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { KrakowVms } from "../target/types/krakow_vms";
import { PublicKey, Keypair, SystemProgram, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { expect } from "chai";
import crypto from "crypto";

describe("KrakowVMS Protocol End-to-End Suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.KrakowVms as Program<KrakowVms>;

  const creator = Keypair.generate();
  const contractor = Keypair.generate();
  const arbitrator = Keypair.generate();
  const maliciousActor = Keypair.generate();

  const bountyId = new anchor.BN(9942);
  const bountyAmount = new anchor.BN(1.5 * LAMPORTS_PER_SOL);

  let bountyPda: PublicKey;
  let bountyBump: number;
  let vaultPda: PublicKey;
  let vaultBump: number;

  before(async () => {
    // Airdrop funds to all actors
    for (const actor of [creator, contractor, arbitrator, maliciousActor]) {
      const sig = await provider.connection.requestAirdrop(actor.publicKey, 5 * LAMPORTS_PER_SOL);
      await provider.connection.confirmTransaction(sig, "confirmed");
    }

    [bountyPda, bountyBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("bounty"), creator.publicKey.toBuffer(), bountyId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    [vaultPda, vaultBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), bountyPda.toBuffer()],
      program.programId
    );
  });

  it("1. Successfully initializes a bounty and escrows 1.5 SOL in PDA vault", async () => {
    const currentClock = Math.floor(Date.now() / 1000);
    const deadline = new anchor.BN(currentClock + 86400); // 24 hours out

    const initialVaultBal = await provider.connection.getBalance(vaultPda);
    expect(initialVaultBal).to.equal(0);

    await program.methods
      .initializeBounty(bountyId, bountyAmount, deadline)
      .accounts({
        creator: creator.publicKey,
        contractor: contractor.publicKey,
        arbitrator: arbitrator.publicKey,
        bounty: bountyPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([creator])
      .rpc();

    const bountyAccount = await program.account.bounty.fetch(bountyPda);
    const vaultBalAfter = await provider.connection.getBalance(vaultPda);

    expect(bountyAccount.creator.toBase58()).to.equal(creator.publicKey.toBase58());
    expect(bountyAccount.contractor.toBase58()).to.equal(contractor.publicKey.toBase58());
    expect(bountyAccount.arbitrator.toBase58()).to.equal(arbitrator.publicKey.toBase58());
    expect(bountyAccount.amount.toNumber()).to.equal(bountyAmount.toNumber());
    expect(JSON.stringify(bountyAccount.status)).to.include("created");
    expect(vaultBalAfter).to.equal(bountyAmount.toNumber());
  });

  it("2. Fails when unauthorized caller attempts to submit milestone", async () => {
    const fakeHash = Array.from(crypto.randomBytes(32));

    try {
      await program.methods
        .submitMilestone(fakeHash)
        .accounts({
          contractor: maliciousActor.publicKey,
          bounty: bountyPda,
        })
        .signers([maliciousActor])
        .rpc();
      expect.fail("Expected transaction to fail on unauthorized contractor");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("Unauthorized");
    }
  });

  it("3. Contractor submits valid deliverable hash successfully", async () => {
    const testDeliverableString = "commit:a9f24b8d|ipfs:QmZtmD2VD";
    const deliverableHash = Array.from(crypto.createHash("sha256").update(testDeliverableString).digest());

    await program.methods
      .submitMilestone(deliverableHash)
      .accounts({
        contractor: contractor.publicKey,
        bounty: bountyPda,
      })
      .signers([contractor])
      .rpc();

    const bountyAccount = await program.account.bounty.fetch(bountyPda);
    expect(JSON.stringify(bountyAccount.status)).to.include("milestoneSubmitted");
    expect(Buffer.from(bountyAccount.deliverableHash).toString("hex")).to.equal(
      Buffer.from(deliverableHash).toString("hex")
    );
  });

  it("4. Fails when contractor attempts to approve own milestone", async () => {
    try {
      await program.methods
        .approveMilestone()
        .accounts({
          creator: contractor.publicKey,
          contractor: contractor.publicKey,
          bounty: bountyPda,
          vault: vaultPda,
          systemProgram: SystemProgram.programId,
        })
        .signers([contractor])
        .rpc();
      expect.fail("Expected failure when contractor acts as creator");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("Unauthorized");
    }
  });

  it("5. Creator approves milestone: Releases full vault balance to contractor", async () => {
    const contractorBalBefore = await provider.connection.getBalance(contractor.publicKey);

    await program.methods
      .approveMilestone()
      .accounts({
        creator: creator.publicKey,
        contractor: contractor.publicKey,
        bounty: bountyPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([creator])
      .rpc();

    const contractorBalAfter = await provider.connection.getBalance(contractor.publicKey);
    const vaultBalAfter = await provider.connection.getBalance(vaultPda);
    const bountyAccount = await program.account.bounty.fetch(bountyPda);

    expect(JSON.stringify(bountyAccount.status)).to.include("settled");
    expect(vaultBalAfter).to.equal(0);
    expect(contractorBalAfter - contractorBalBefore).to.equal(bountyAmount.toNumber());
  });

  it("6. Edge Case: Dispute & Arbitrator Settlement Lifecycle", async () => {
    // Initialize a second bounty to test dispute paths
    const secondBountyId = new anchor.BN(9943);
    const [secondBountyPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("bounty"), creator.publicKey.toBuffer(), secondBountyId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );
    const [secondVaultPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), secondBountyPda.toBuffer()],
      program.programId
    );

    const deadline = new anchor.BN(Math.floor(Date.now() / 1000) + 3600);
    await program.methods
      .initializeBounty(secondBountyId, bountyAmount, deadline)
      .accounts({
        creator: creator.publicKey,
        contractor: contractor.publicKey,
        arbitrator: arbitrator.publicKey,
        bounty: secondBountyPda,
        vault: secondVaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([creator])
      .rpc();

    // Trigger dispute from Creator
    await program.methods
      .disputeBounty()
      .accounts({
        signer: creator.publicKey,
        bounty: secondBountyPda,
      })
      .signers([creator])
      .rpc();

    let account = await program.account.bounty.fetch(secondBountyPda);
    expect(JSON.stringify(account.status)).to.include("disputed");

    // Arbitrator resolves in favor of Creator (Refund)
    const creatorBalBefore = await provider.connection.getBalance(creator.publicKey);

    await program.methods
      .resolveDispute(false) // false = refund creator
      .accounts({
        arbitrator: arbitrator.publicKey,
        recipient: creator.publicKey,
        bounty: secondBountyPda,
        vault: secondVaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([arbitrator])
      .rpc();

    const creatorBalAfter = await provider.connection.getBalance(creator.publicKey);
    account = await program.account.bounty.fetch(secondBountyPda);

    expect(JSON.stringify(account.status)).to.include("refunded");
    expect(creatorBalAfter - creatorBalBefore).to.equal(bountyAmount.toNumber());
  });
});