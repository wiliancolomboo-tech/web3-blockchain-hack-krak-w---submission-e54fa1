import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { AegisSettlement } from "../target/types/aegis_settlement";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { expect } from "chai";
import * as crypto from "crypto";

describe("AegisSettlement Full Lifecycle & Security Invariant QA Matrix", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.AegisSettlement as Program<AegisSettlement>;

  const admin = Keypair.generate();
  const feeRecipient = Keypair.generate();
  const vendor = Keypair.generate();
  const payer = Keypair.generate();
  const factorLp = Keypair.generate();

  let mint: PublicKey;
  let adminTokenAccount: PublicKey;
  let vendorTokenAccount: PublicKey;
  let payerTokenAccount: PublicKey;
  let factorTokenAccount: PublicKey;
  let protocolFeeTokenAccount: PublicKey;

  let protocolConfigPda: PublicKey;
  let protocolConfigBump: number;

  const invoiceId = "INV-KRAKOW-2025-001";
  const invoiceAmount = new anchor.BN(10_000_000_000); // 10,000 USDC (6 decimals)
  const advanceRateBps = 9000; // 90.00% advance rate
  const feeBps = 100; // 1.00% protocol fee

  let invoicePda: PublicKey;
  let invoiceBump: number;
  let vaultAccountPda: PublicKey;
  let vaultBump: number;

  before(async () => {
    // Airdrop SOL to test actors
    for (const actor of [admin, feeRecipient, vendor, payer, factorLp]) {
      const sig = await provider.connection.requestAirdrop(
        actor.publicKey,
        50 * anchor.web3.LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig, "confirmed");
    }

    // Create Mock USDC Token
    mint = await createMint(
      provider.connection,
      admin,
      admin.publicKey,
      null,
      6
    );

    // Setup Token Accounts
    adminTokenAccount = await createAccount(provider.connection, admin, mint, admin.publicKey);
    vendorTokenAccount = await createAccount(provider.connection, vendor, mint, vendor.publicKey);
    payerTokenAccount = await createAccount(provider.connection, payer, mint, payer.publicKey);
    factorTokenAccount = await createAccount(provider.connection, factorLp, mint, factorLp.publicKey);
    protocolFeeTokenAccount = await createAccount(provider.connection, feeRecipient, mint, feeRecipient.publicKey);

    // Mint test supplies
    await mintTo(provider.connection, admin, mint, factorTokenAccount, admin, 50_000_000_000); // 50,000 to LP
    await mintTo(provider.connection, admin, mint, payerTokenAccount, admin, 50_000_000_000);  // 50,000 to Payer

    // Derive PDAs
    [protocolConfigPda, protocolConfigBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("protocol_config")],
      program.programId
    );

    [invoicePda, invoiceBump] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("invoice"),
        vendor.publicKey.toBuffer(),
        payer.publicKey.toBuffer(),
        Buffer.from(invoiceId),
      ],
      program.programId
    );

    [vaultAccountPda, vaultBump] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), invoicePda.toBuffer()],
      program.programId
    );
  });

  it("1. Initializes protocol governance parameters strictly", async () => {
    await program.methods
      .initializeProtocol(feeBps)
      .accounts({
        protocolConfig: protocolConfigPda,
        authority: admin.publicKey,
        feeRecipient: feeRecipient.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([admin])
      .rpc();

    const config = await program.account.protocolConfig.fetch(protocolConfigPda);
    expect(config.authority.toBase58()).to.equal(admin.publicKey.toBase58());
    expect(config.feeBasisPoints).to.equal(feeBps);
    expect(config.isPaused).to.be.false;
  });

  it("2. Registers a verified commercial invoice PDA", async () => {
    const dueTimestamp = new anchor.BN(Math.floor(Date.now() / 1000) + 86400 * 30); // 30 days
    const invoiceHash = Array.from(crypto.createHash("sha256").update("OFFCHAIN_PAYLOAD_PDF").digest());

    await program.methods
      .createInvoice(
        invoiceId,
        invoiceAmount,
        advanceRateBps,
        dueTimestamp,
        invoiceHash
      )
      .accounts({
        protocolConfig: protocolConfigPda,
        invoice: invoicePda,
        vendor: vendor.publicKey,
        payer: payer.publicKey,
        mint: mint,
        vaultAccount: vaultAccountPda,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .signers([vendor])
      .rpc();

    const inv = await program.account.invoice.fetch(invoicePda);
    expect(inv.amount.toString()).to.equal(invoiceAmount.toString());
    expect(inv.isFactored).to.be.false;
    expect(inv.isSettled).to.be.false;
    expect(inv.advanceRateBps).to.equal(9000);
  });

  it("3. Factors invoice with LP liquidity advance and fee deductions", async () => {
    const vendorPreBalance = (await getAccount(provider.connection, vendorTokenAccount)).amount;
    const feePreBalance = (await getAccount(provider.connection, protocolFeeTokenAccount)).amount;

    await program.methods
      .factorInvoice()
      .accounts({
        protocolConfig: protocolConfigPda,
        invoice: invoicePda,
        mint: mint,
        factorLp: factorLp.publicKey,
        factorTokenAccount: factorTokenAccount,
        vendor: vendor.publicKey,
        vendorTokenAccount: vendorTokenAccount,
        protocolFeeTokenAccount: protocolFeeTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([factorLp])
      .rpc();

    const inv = await program.account.invoice.fetch(invoicePda);
    expect(inv.isFactored).to.be.true;
    expect(inv.factorLp.toBase58()).to.equal(factorLp.publicKey.toBase58());

    // 9,000 total advance, 1% fee = 90 USDC fee, 8,910 USDC net to vendor
    const vendorPostBalance = (await getAccount(provider.connection, vendorTokenAccount)).amount;
    const feePostBalance = (await getAccount(provider.connection, protocolFeeTokenAccount)).amount;

    expect(Number(vendorPostBalance - vendorPreBalance)).to.equal(8_910_000_000);
    expect(Number(feePostBalance - feePreBalance)).to.equal(90_000_000);
  });

  it("4. Prevents duplicate factoring attempts (re-entrancy / double-pledging defense)", async () => {
    try {
      await program.methods
        .factorInvoice()
        .accounts({
          protocolConfig: protocolConfigPda,
          invoice: invoicePda,
          mint: mint,
          factorLp: factorLp.publicKey,
          factorTokenAccount: factorTokenAccount,
          vendor: vendor.publicKey,
          vendorTokenAccount: vendorTokenAccount,
          protocolFeeTokenAccount: protocolFeeTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([factorLp])
        .rpc();
      expect.fail("Should have thrown AlreadyFactored error");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("AlreadyFactored");
    }
  });

  it("5. Settles invoice: payer satisfies face value directly to factor LP", async () => {
    const lpPreBalance = (await getAccount(provider.connection, factorTokenAccount)).amount;

    await program.methods
      .settleInvoice()
      .accounts({
        protocolConfig: protocolConfigPda,
        invoice: invoicePda,
        mint: mint,
        payer: payer.publicKey,
        payerTokenAccount: payerTokenAccount,
        vendorTokenAccount: vendorTokenAccount,
        factorTokenAccount: factorTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([payer])
      .rpc();

    const inv = await program.account.invoice.fetch(invoicePda);
    expect(inv.isSettled).to.be.true;

    const lpPostBalance = (await getAccount(provider.connection, factorTokenAccount)).amount;
    // LP received full 10,000 USDC face value
    expect(Number(lpPostBalance - lpPreBalance)).to.equal(10_000_000_000);
  });

  it("6. Rejects settlement on an already settled invoice", async () => {
    try {
      await program.methods
        .settleInvoice()
        .accounts({
          protocolConfig: protocolConfigPda,
          invoice: invoicePda,
          mint: mint,
          payer: payer.publicKey,
          payerTokenAccount: payerTokenAccount,
          vendorTokenAccount: vendorTokenAccount,
          factorTokenAccount: factorTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([payer])
        .rpc();
      expect.fail("Should have failed with AlreadySettled");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("AlreadySettled");
    }
  });
});