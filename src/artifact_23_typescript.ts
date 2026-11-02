import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AegisEscrow } from "../target/types/aegis_escrow";
import {
  createMint,
  createAssociatedTokenAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { Keypair, PublicKey } from "@solana/web3.js";
import { assert, expect } from "chai";
import crypto from "crypto";

describe("AegisVault Escrow Protocol Lifecycle Verification", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AegisEscrow as Program<AegisEscrow>;

  let mintKeypair: Keypair;
  let clientKeypair: Keypair;
  let contractorKeypair: Keypair;
  let arbiterKeypair: Keypair;
  let unauthorizedSigner: Keypair;

  let clientAta: PublicKey;
  let contractorAta: PublicKey;

  let agreementId: number[];
  let agreementPda: PublicKey;
  let agreementBump: number;
  let vaultPda: PublicKey;
  let vaultBump: number;
  let vaultAuthorityPda: PublicKey;

  const MILESTONE_ONE_AMOUNT = new anchor.BN(10_000_000); // 10 tokens
  const MILESTONE_TWO_AMOUNT = new anchor.BN(20_000_000); // 20 tokens
  const TOTAL_AMOUNT = MILESTONE_ONE_AMOUNT.add(MILESTONE_TWO_AMOUNT);
  const REVIEW_TIMEOUT = new anchor.BN(2); // 2 seconds for test speed

  before(async () => {
    clientKeypair = Keypair.generate();
    contractorKeypair = Keypair.generate();
    arbiterKeypair = Keypair.generate();
    unauthorizedSigner = Keypair.generate();

    // Airdrop SOL
    for (const kp of [clientKeypair, contractorKeypair, arbiterKeypair, unauthorizedSigner]) {
      const sig = await provider.connection.requestAirdrop(kp.publicKey, 5 * anchor.web3.LAMPORTS_PER_SOL);
      await provider.connection.confirmTransaction(sig);
    }

    // Create Token Mint
    mintKeypair = Keypair.generate();
    await createMint(
      provider.connection,
      clientKeypair,
      clientKeypair.publicKey,
      null,
      6,
      mintKeypair
    );

    // Create ATAs
    clientAta = await createAssociatedTokenAccount(
      provider.connection,
      clientKeypair,
      mintKeypair.publicKey,
      clientKeypair.publicKey
    );

    contractorAta = await createAssociatedTokenAccount(
      provider.connection,
      contractorKeypair,
      mintKeypair.publicKey,
      contractorKeypair.publicKey
    );

    // Mint balance to client
    await mintTo(
      provider.connection,
      clientKeypair,
      mintKeypair.publicKey,
      clientAta,
      clientKeypair,
      TOTAL_AMOUNT.toNumber()
    );

    // Generate Agreement ID (16 bytes)
    agreementId = Array.from(crypto.randomBytes(16));

    // Find PDAs
    [agreementPda, agreementBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("aegis_agreement"), Buffer.from(agreementId)],
      program.programId
    );

    [vaultPda, vaultBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("aegis_vault"), agreementPda.toBuffer()],
      program.programId
    );

    [vaultAuthorityPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("aegis_authority"), agreementPda.toBuffer()],
      program.programId
    );
  });

  it("1. Successfully initializes a multi-milestone agreement PDA", async () => {
    await program.methods
      .initializeAgreement(
        agreementId,
        [MILESTONE_ONE_AMOUNT, MILESTONE_TWO_AMOUNT],
        [REVIEW_TIMEOUT, REVIEW_TIMEOUT]
      )
      .accounts({
        client: clientKeypair.publicKey,
        contractor: contractorKeypair.publicKey,
        arbiter: arbiterKeypair.publicKey,
        tokenMint: mintKeypair.publicKey,
        agreement: agreementPda,
        vault: vaultPda,
        vaultAuthority: vaultAuthorityPda,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([clientKeypair])
      .rpc();

    const state = await program.account.agreementState.fetch(agreementPda);
    expect(state.totalAmount.toString()).to.equal(TOTAL_AMOUNT.toString());
    expect(state.milestoneCount).to.equal(2);
    expect(state.currentMilestoneIndex).to.equal(0);
    expect(state.status).to.deep.equal({ initialized: {} });
  });

  it("2. Client locks tokens into PDA escrow vault via deposit_funds", async () => {
    await program.methods
      .depositFunds(TOTAL_AMOUNT)
      .accounts({
        client: clientKeypair.publicKey,
        agreement: agreementPda,
        clientTokenAccount: clientAta,
        vaultPda: vaultPda,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .signers([clientKeypair])
      .rpc();

    const vaultAccount = await getAccount(provider.connection, vaultPda);
    expect(vaultAccount.amount.toString()).to.equal(TOTAL_AMOUNT.toString());

    const state = await program.account.agreementState.fetch(agreementPda);
    expect(state.status).to.deep.equal({ funded: {} });
  });

  it("3. Rejects milestone deliverable from unauthorized signer", async () => {
    const dummyHash = Array.from(crypto.randomBytes(32));

    try {
      await program.methods
        .submitMilestone(0, dummyHash)
        .accounts({
          contractor: unauthorizedSigner.publicKey,
          agreement: agreementPda,
        })
        .signers([unauthorizedSigner])
        .rpc();
      assert.fail("Should have failed with unauthorized signer.");
    } catch (err: any) {
      expect(err.toString()).to.contain("UnauthorizedSigner");
    }
  });

  it("4. Contractor submits milestone deliverable with SHA-256 hash", async () => {
    const dummyHash = Array.from(crypto.createHash("sha256").update("deliverable-v1").digest());

    await program.methods
      .submitMilestone(0, dummyHash)
      .accounts({
        contractor: contractorKeypair.publicKey,
        agreement: agreementPda,
      })
      .signers([contractorKeypair])
      .rpc();

    const state = await program.account.agreementState.fetch(agreementPda);
    expect(state.milestones[0].status).to.equal(2); // UnderReview
  });

  it("5. Client approves milestone 1: triggers CPI payout to contractor", async () => {
    const initialContractorBalance = (await getAccount(provider.connection, contractorAta)).amount;

    await program.methods
      .approveMilestone(0)
      .accounts({
        client: clientKeypair.publicKey,
        agreement: agreementPda,
        vaultPda: vaultPda,
        vaultAuthority: vaultAuthorityPda,
        contractorTokenAccount: contractorAta,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .signers([clientKeypair])
      .rpc();

    const finalContractorBalance = (await getAccount(provider.connection, contractorAta)).amount;
    expect(finalContractorBalance - initialContractorBalance).to.equal(BigInt(MILESTONE_ONE_AMOUNT.toNumber()));

    const state = await program.account.agreementState.fetch(agreementPda);
    expect(state.currentMilestoneIndex).to.equal(1);
    expect(state.milestones[1].status).to.equal(1); // Active
  });

  it("6. Milestone 2 dispute lifecycle: raise dispute and execute split resolution", async () => {
    // Contractor submits milestone 2
    const dummyHash2 = Array.from(crypto.createHash("sha256").update("deliverable-v2").digest());
    await program.methods
      .submitMilestone(1, dummyHash2)
      .accounts({
        contractor: contractorKeypair.publicKey,
        agreement: agreementPda,
      })
      .signers([contractorKeypair])
      .rpc();

    // Client disputes milestone 2
    const disputeEvidence = Array.from(crypto.createHash("sha256").update("malformed-code").digest());
    await program.methods
      .raiseDispute(1, disputeEvidence)
      .accounts({
        caller: clientKeypair.publicKey,
        agreement: agreementPda,
      })
      .signers([clientKeypair])
      .rpc();

    let state = await program.account.agreementState.fetch(agreementPda);
    expect(state.status).to.deep.equal({ disputed: {} });

    // Arbiter resolves dispute with 50/50 split (5000 bps client, 5000 bps contractor)
    const clientBefore = (await getAccount(provider.connection, clientAta)).amount;
    const contractorBefore = (await getAccount(provider.connection, contractorAta)).amount;

    await program.methods
      .resolveDispute(1, new anchor.BN(5000), new anchor.BN(5000))
      .accounts({
        arbiter: arbiterKeypair.publicKey,
        agreement: agreementPda,
        vaultPda: vaultPda,
        vaultAuthority: vaultAuthorityPda,
        clientTokenAccount: clientAta,
        contractorTokenAccount: contractorAta,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .signers([arbiterKeypair])
      .rpc();

    const clientAfter = (await getAccount(provider.connection, clientAta)).amount;
    const contractorAfter = (await getAccount(provider.connection, contractorAta)).amount;

    const halfAmount = BigInt(MILESTONE_TWO_AMOUNT.toNumber() / 2);
    expect(clientAfter - clientBefore).to.equal(halfAmount);
    expect(contractorAfter - contractorBefore).to.equal(halfAmount);

    state = await program.account.agreementState.fetch(agreementPda);
    expect(state.status).to.deep.equal({ completed: {} });
    expect(state.remainingBalance.toNumber()).to.equal(0);
  });
});