import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { KrakowFlashstream } from "../target/types/krakow_flashstream";
import {
  createMint,
  createAccount,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { expect } from "chai";

describe("krakow_flashstream e2e test suite", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.KrakowFlashstream as Program<KrakowFlashstream>;

  let mint: PublicKey;
  let payerTokenAccount: PublicKey;
  let recipientTokenAccount: PublicKey;
  const payer = (provider.wallet as anchor.Wallet).payer;
  const recipient = Keypair.generate();

  const channelId = new anchor.BN(4242);
  const depositAmount = new anchor.BN(10_000_000); // 10 USDC (6 decimals)
  const durationSlots = new anchor.BN(1000);

  let channelPda: PublicKey;
  let vaultAuthPda: PublicKey;
  let vaultKeypair: Keypair;

  before(async () => {
    // 1. Airdrop SOL to recipient
    const sig = await provider.connection.requestAirdrop(recipient.publicKey, 2 * anchor.web3.LAMPORTS_PER_SOL);
    await provider.connection.confirmTransaction(sig);

    // 2. Setup SPL Token Mint & Token Accounts
    mint = await createMint(provider.connection, payer, payer.publicKey, null, 6);
    payerTokenAccount = await createAccount(provider.connection, payer, mint, payer.publicKey);
    recipientTokenAccount = await createAccount(provider.connection, payer, mint, recipient.publicKey);

    // 3. Mint 100 tokens to payer
    await mintTo(provider.connection, payer, mint, payerTokenAccount, payer, 100_000_000);

    // 4. Derive PDAs
    [channelPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("channel"),
        payer.publicKey.toBuffer(),
        recipient.publicKey.toBuffer(),
        channelId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    [vaultAuthPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_auth"), channelPda.toBuffer()],
      program.programId
    );

    vaultKeypair = Keypair.generate();
  });

  it("1. Successfully opens streaming channel and locks collateral in PDA vault", async () => {
    await program.methods
      .openChannel(channelId, depositAmount, durationSlots)
      .accounts({
        channel: channelPda,
        vaultAuthority: vaultAuthPda,
        vault: vaultKeypair.publicKey,
        payer: payer.publicKey,
        recipient: recipient.publicKey,
        payerTokenAccount: payerTokenAccount,
        mint: mint,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([vaultKeypair])
      .rpc();

    const channelAccount = await program.account.channelState.fetch(channelPda);
    expect(channelAccount.payer.toBase58()).to.equal(payer.publicKey.toBase58());
    expect(channelAccount.recipient.toBase58()).to.equal(recipient.publicKey.toBase58());
    expect(channelAccount.depositedAmount.toNumber()).to.equal(10_000_000);
    expect(channelAccount.status).to.equal(1); // Active

    const vaultBalance = await getAccount(provider.connection, vaultKeypair.publicKey);
    expect(Number(vaultBalance.amount)).to.equal(10_000_000);
  });

  it("2. Executes cooperative close with dual signatures and atomic payout/refund", async () => {
    const finalPayout = new anchor.BN(4_000_000); // 4 USDC to Recipient
    const voucherNonce = new anchor.BN(150);

    const payerBefore = await getAccount(provider.connection, payerTokenAccount);
    const recipientBefore = await getAccount(provider.connection, recipientTokenAccount);

    await program.methods
      .cooperativeClose(channelId, finalPayout, voucherNonce)
      .accounts({
        channel: channelPda,
        vaultAuthority: vaultAuthPda,
        vault: vaultKeypair.publicKey,
        payer: payer.publicKey,
        recipient: recipient.publicKey,
        payerTokenAccount: payerTokenAccount,
        recipientTokenAccount: recipientTokenAccount,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .signers([recipient])
      .rpc();

    const channelAccount = await program.account.channelState.fetch(channelPda);
    expect(channelAccount.status).to.equal(3); // Closed
    expect(channelAccount.transferredAmount.toNumber()).to.equal(4_000_000);

    const payerAfter = await getAccount(provider.connection, payerTokenAccount);
    const recipientAfter = await getAccount(provider.connection, recipientTokenAccount);

    // Recipient received 4 USDC
    expect(Number(recipientAfter.amount) - Number(recipientBefore.amount)).to.equal(4_000_000);
    // Payer refunded remainder (6 USDC)
    expect(Number(payerAfter.amount) - Number(payerBefore.amount)).to.equal(6_000_000);
  });

  it("3. Rejects invalid withdrawal exceeding initial channel deposit", async () => {
    const maliciousChannelId = new anchor.BN(9999);
    const [maliciousPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("channel"),
        payer.publicKey.toBuffer(),
        recipient.publicKey.toBuffer(),
        maliciousChannelId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );
    const [maliciousVaultAuth] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_auth"), maliciousPda.toBuffer()],
      program.programId
    );
    const maliciousVault = Keypair.generate();

    await program.methods
      .openChannel(maliciousChannelId, new anchor.BN(1_000_000), durationSlots)
      .accounts({
        channel: maliciousPda,
        vaultAuthority: maliciousVaultAuth,
        vault: maliciousVault.publicKey,
        payer: payer.publicKey,
        recipient: recipient.publicKey,
        payerTokenAccount: payerTokenAccount,
        mint: mint,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([maliciousVault])
      .rpc();

    try {
      await program.methods
        .initiateUnilateralClose(maliciousChannelId, new anchor.BN(5_000_000), new anchor.BN(1))
        .accounts({
          channel: maliciousPda,
          recipient: recipient.publicKey,
        })
        .signers([recipient])
        .rpc();
      expect.fail("Expected AmountExceedsDeposit error");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("AmountExceedsDeposit");
    }
  });

  it("4. Slashes fraudulent actor upon valid challenge nonce detection", async () => {
    const fraudChannelId = new anchor.BN(777);
    const [fraudChannelPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("channel"),
        payer.publicKey.toBuffer(),
        recipient.publicKey.toBuffer(),
        fraudChannelId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );
    const [fraudVaultAuth] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault_auth"), fraudChannelPda.toBuffer()],
      program.programId
    );
    const fraudVault = Keypair.generate();

    // Open channel with 2 USDC
    await program.methods
      .openChannel(fraudChannelId, new anchor.BN(2_000_000), durationSlots)
      .accounts({
        channel: fraudChannelPda,
        vaultAuthority: fraudVaultAuth,
        vault: fraudVault.publicKey,
        payer: payer.publicKey,
        recipient: recipient.publicKey,
        payerTokenAccount: payerTokenAccount,
        mint: mint,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([fraudVault])
      .rpc();

    // Recipient attempts unilateral close with old Nonce 5
    await program.methods
      .initiateUnilateralClose(fraudChannelId, new anchor.BN(1_000_000), new anchor.BN(5))
      .accounts({
        channel: fraudChannelPda,
        recipient: recipient.publicKey,
      })
      .signers([recipient])
      .rpc();

    // Payer demonstrates Counter-Nonce 12 > Nonce 5
    await program.methods
      .challengeFraud(fraudChannelId, new anchor.BN(12), new anchor.BN(1_200_000))
      .accounts({
        channel: fraudChannelPda,
        vaultAuthority: fraudVaultAuth,
        vault: fraudVault.publicKey,
        payer: payer.publicKey,
        payerTokenAccount: payerTokenAccount,
        tokenProgram: anchor.utils.token.TOKEN_PROGRAM_ID,
      })
      .rpc();

    const channelState = await program.account.channelState.fetch(fraudChannelPda);
    expect(channelState.status).to.equal(4); // Slashed
  });
});