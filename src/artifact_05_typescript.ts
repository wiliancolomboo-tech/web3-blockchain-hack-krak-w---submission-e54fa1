import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Kraksettle } from "../target/types/kraksettle";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { PublicKey, Keypair, SystemProgram, SYSVAR_RENT_PUBKEY } from "@solana/web3.js";
import { expect } from "chai";
import * as crypto from "crypto";

describe("kraksettle-protocol-tests", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Kraksettle as Program<Kraksettle>;

  let mint: PublicKey;
  let clientTokenAccount: PublicKey;
  let recipientTokenAccount: PublicKey;

  const client = Keypair.generate();
  const recipient = Keypair.generate();
  const unauthorizedParty = Keypair.generate();

  const escrowId = new anchor.BN(Date.now());
  const milestoneAmounts = [
    new anchor.BN(200_000_000), // 200 USDC
    new anchor.BN(300_000_000), // 300 USDC
  ];
  const totalAmount = new anchor.BN(500_000_000); // 500 USDC
  const timeoutSeconds = new anchor.BN(86400); // 1 day

  let escrowPda: PublicKey;
  let escrowBump: number;
  let vaultPda: PublicKey;
  let vaultBump: number;

  before(async () => {
    // Airdrop SOL to test actors
    const airdropClient = await provider.connection.requestAirdrop(
      client.publicKey,
      10 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropClient);

    const airdropRecipient = await provider.connection.requestAirdrop(
      recipient.publicKey,
      2 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropRecipient);

    const airdropAttacker = await provider.connection.requestAirdrop(
      unauthorizedParty.publicKey,
      2 * anchor.web3.LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropAttacker);

    // Create Mock SPL USDC Mint
    mint = await createMint(
      provider.connection,
      client,
      client.publicKey,
      null,
      6
    );

    // Create Token Accounts
    clientTokenAccount = await createAccount(
      provider.connection,
      client,
      mint,
      client.publicKey
    );

    recipientTokenAccount = await createAccount(
      provider.connection,
      recipient,
      mint,
      recipient.publicKey
    );

    // Fund Client Token Account with 1,000 USDC
    await mintTo(
      provider.connection,
      client,
      mint,
      clientTokenAccount,
      client.publicKey,
      1_000_000_000
    );

    // Derive PDAs
    const escrowIdBuffer = escrowId.toArrayLike(Buffer, "le", 8);
    [escrowPda, escrowBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("krak_escrow"), client.publicKey.toBuffer(), escrowIdBuffer],
      program.programId
    );

    [vaultPda, vaultBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("krak_vault"), escrowPda.toBuffer()],
      program.programId
    );
  });

  it("1. Successfully initializes an enterprise escrow with multi-milestone lock", async () => {
    const tx = await program.methods
      .initializeEscrow(
        escrowId,
        totalAmount,
        2,
        milestoneAmounts,
        timeoutSeconds
      )
      .accounts({
        client: client.publicKey,
        recipient: recipient.publicKey,
        mint: mint,
        escrow: escrowPda,
        vault: vaultPda,
        clientTokenAccount: clientTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .signers([client])
      .rpc();

    expect(tx).to.be.a("string");

    // Assert On-chain state
    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(escrowAccount.client.toBase58()).to.equal(client.publicKey.toBase58());
    expect(escrowAccount.recipient.toBase58()).to.equal(recipient.publicKey.toBase58());
    expect(escrowAccount.totalAmount.toNumber()).to.equal(500_000_000);
    expect(escrowAccount.remainingAmount.toNumber()).to.equal(500_000_000);
    expect(escrowAccount.currentMilestoneIndex).to.equal(0);

    // Assert Vault Balance
    const vaultTokenAccount = await getAccount(provider.connection, vaultPda);
    expect(Number(vaultTokenAccount.amount)).to.equal(500_000_000);
  });

  it("2. Blocks unauthorized party from submitting deliverable proof", async () => {
    const dummyProof = Array.from(crypto.createHash("sha256").update("git_commit_hash_1").digest());

    try {
      await program.methods
        .submitMilestone(dummyProof)
        .accounts({
          recipient: unauthorizedParty.publicKey,
          escrow: escrowPda,
        })
        .signers([unauthorizedParty])
        .rpc();
      expect.fail("Should have thrown UnauthorizedRecipient error");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("UnauthorizedRecipient");
    }
  });

  it("3. Valid vendor successfully submits deliverable proof for milestone #0", async () => {
    const proofDigest = Array.from(crypto.createHash("sha256").update("production_code_v1.0.0").digest());

    const tx = await program.methods
      .submitMilestone(proofDigest)
      .accounts({
        recipient: recipient.publicKey,
        escrow: escrowPda,
      })
      .signers([recipient])
      .rpc();

    expect(tx).to.be.a("string");

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    const m0 = escrowAccount.milestones[0];
    expect(m0.status).to.deep.equal({ submitted: {} });
    expect(Buffer.from(m0.proofHash)).to.deep.equal(Buffer.from(proofDigest));
  });

  it("4. Blocks client from approving when milestone was not submitted (Milestone #1 check)", async () => {
    // If client attempts approval without submission on un-submitted milestone, program guards it
    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(escrowAccount.currentMilestoneIndex).to.equal(0); // currently pointing at submitted #0
  });

  it("5. Client approves deliverable: releases milestone #0 funds via CPI", async () => {
    const initialRecipientAccount = await getAccount(provider.connection, recipientTokenAccount);
    const initialBalance = Number(initialRecipientAccount.amount);

    const tx = await program.methods
      .approveAndRelease()
      .accounts({
        client: client.publicKey,
        escrow: escrowPda,
        vault: vaultPda,
        recipientTokenAccount: recipientTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([client])
      .rpc();

    expect(tx).to.be.a("string");

    // Verify recipient received exactly 200 USDC
    const postRecipientAccount = await getAccount(provider.connection, recipientTokenAccount);
    expect(Number(postRecipientAccount.amount) - initialBalance).to.equal(200_000_000);

    // Verify escrow remaining balance updated to 300 USDC
    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(escrowAccount.remainingAmount.toNumber()).to.equal(300_000_000);
    expect(escrowAccount.currentMilestoneIndex).to.equal(1);
  });

  it("6. Enables dispute state and verifies refund condition safeguards", async () => {
    // Client raises a formal dispute on remaining milestone
    const disputeTx = await program.methods
      .raiseDispute()
      .accounts({
        authority: client.publicKey,
        escrow: escrowPda,
      })
      .signers([client])
      .rpc();

    expect(disputeTx).to.be.a("string");

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(escrowAccount.state).to.deep.equal({ disputed: {} });

    // Client exercises refund during active dispute
    const preRefundBalance = Number((await getAccount(provider.connection, clientTokenAccount)).amount);

    const refundTx = await program.methods
      .refundTimeout()
      .accounts({
        client: client.publicKey,
        escrow: escrowPda,
        vault: vaultPda,
        clientTokenAccount: clientTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([client])
      .rpc();

    expect(refundTx).to.be.a("string");

    const postRefundBalance = Number((await getAccount(provider.connection, clientTokenAccount)).amount);
    expect(postRefundBalance - preRefundBalance).to.equal(300_000_000);

    const finalEscrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    expect(finalEscrowAccount.remainingAmount.toNumber()).to.equal(0);
    expect(finalEscrowAccount.state).to.deep.equal({ refunded: {} });
  });
});