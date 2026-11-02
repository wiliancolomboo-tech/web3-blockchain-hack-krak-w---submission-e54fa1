import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { KrakowSentry } from "../target/types/krakow_sentry";
import {
  createMint,
  createAssociatedTokenAccount,
  mintTo,
  getAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { Keypair, PublicKey } from "@solana/web3.js";
import { expect } from "chai";

describe("krakow-sentry protocol tests", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.KrakowSentry as Program<KrakowSentry>;

  let mint: PublicKey;
  let clientTokenAccount: PublicKey;
  let providerTokenAccount: PublicKey;

  const client = Keypair.generate();
  const serviceProvider = Keypair.generate();
  const oracle = Keypair.generate();
  const unauthorizedOracle = Keypair.generate();

  const slaId = new anchor.BN(1001);
  const slaIdBytes = slaId.toArrayLike(Buffer, "le", 8);

  const [slaPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("sla-contract"), slaIdBytes],
    program.programId
  );

  const [vaultPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("sla-vault"), slaPda.toBuffer()],
    program.programId
  );

  const payoutRate = new anchor.BN(1_000_000); // 1.0 Token
  const epochDurationSeconds = new anchor.BN(2);
  const minUptimeBps = 9900; // 99.00%
  const maxLatencyMs = 120;
  const slashPenaltyBps = 3000; // 30%

  before(async () => {
    // Airdrop SOL to test participants
    for (const kp of [client, serviceProvider, oracle, unauthorizedOracle]) {
      const airdropSig = await provider.connection.requestAirdrop(kp.publicKey, 10 * anchor.web3.LAMPORTS_PER_SOL);
      const latestBlockhash = await provider.connection.getLatestBlockhash();
      await provider.connection.confirmTransaction({
        signature: airdropSig,
        ...latestBlockhash,
      });
    }

    // Create SPL Mint with client as authority
    mint = await createMint(
      provider.connection,
      client,
      client.publicKey,
      null,
      6
    );

    // Create Token Accounts
    clientTokenAccount = await createAssociatedTokenAccount(
      provider.connection,
      client,
      mint,
      client.publicKey
    );

    providerTokenAccount = await createAssociatedTokenAccount(
      provider.connection,
      client,
      mint,
      serviceProvider.publicKey
    );

    // Mint initial 100 tokens to client
    await mintTo(
      provider.connection,
      client,
      mint,
      clientTokenAccount,
      client,
      100_000_000
    );
  });

  it("1. Initializes SLA Contract and Escrow PDA successfully", async () => {
    await program.methods
      .initializeSla(
        slaId,
        payoutRate,
        epochDurationSeconds,
        minUptimeBps,
        maxLatencyMs,
        slashPenaltyBps
      )
      .accounts({
        slaContract: slaPda,
        escrowVault: vaultPda,
        tokenMint: mint,
        client: client.publicKey,
        provider: serviceProvider.publicKey,
        telemetryOracle: oracle.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .signers([client])
      .rpc();

    const slaAccount = await program.account.slaContract.fetch(slaPda);
    expect(slaAccount.slaId.toNumber()).to.equal(1001);
    expect(slaAccount.client.toBase58()).to.equal(client.publicKey.toBase58());
    expect(slaAccount.provider.toBase58()).to.equal(serviceProvider.publicKey.toBase58());
    expect(slaAccount.minUptimeBps).to.equal(9900);
    expect(slaAccount.maxLatencyMs).to.equal(120);
    expect(slaAccount.isActive).to.be.true;
  });

  it("2. Client deposits funds into the SLA Escrow Vault", async () => {
    const depositAmount = new anchor.BN(10_000_000);

    await program.methods
      .depositEscrow(depositAmount)
      .accounts({
        slaContract: slaPda,
        escrowVault: vaultPda,
        clientTokenAccount: clientTokenAccount,
        client: client.publicKey,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([client])
      .rpc();

    const vaultBalance = await getAccount(provider.connection, vaultPda);
    expect(Number(vaultBalance.amount)).to.equal(10_000_000);
  });

  it("3. Reject telemetry submission if epoch duration has not elapsed", async () => {
    try {
      await program.methods
        .submitTelemetryAndSettle(new anchor.BN(1), 9990, 80)
        .accounts({
          slaContract: slaPda,
          telemetryOracle: oracle.publicKey,
          escrowVault: vaultPda,
          clientTokenAccount: clientTokenAccount,
          providerTokenAccount: providerTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([oracle])
        .rpc();
      expect.fail("Should have thrown EpochDurationNotMet error");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("EpochDurationNotMet");
    }
  });

  it("4. Settles clean SLA telemetry with full provider payout", async () => {
    // Wait for epoch duration
    await new Promise((resolve) => setTimeout(resolve, 2500));

    const providerBefore = await getAccount(provider.connection, providerTokenAccount);

    await program.methods
      .submitTelemetryAndSettle(new anchor.BN(1), 9950, 90) // 99.50% uptime, 90ms latency
      .accounts({
        slaContract: slaPda,
        telemetryOracle: oracle.publicKey,
        escrowVault: vaultPda,
        clientTokenAccount: clientTokenAccount,
        providerTokenAccount: providerTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([oracle])
      .rpc();

    const providerAfter = await getAccount(provider.connection, providerTokenAccount);
    expect(Number(providerAfter.amount) - Number(providerBefore.amount)).to.equal(1_000_000);

    const slaAccount = await program.account.slaContract.fetch(slaPda);
    expect(slaAccount.currentEpoch.toNumber()).to.equal(1);
  });

  it("5. Triggers automated slashing and compensatory client refund on SLA breach", async () => {
    await new Promise((resolve) => setTimeout(resolve, 2500));

    const clientBefore = await getAccount(provider.connection, clientTokenAccount);
    const providerBefore = await getAccount(provider.connection, providerTokenAccount);

    // Latency breach: 250ms > 120ms limit
    await program.methods
      .submitTelemetryAndSettle(new anchor.BN(2), 9980, 250)
      .accounts({
        slaContract: slaPda,
        telemetryOracle: oracle.publicKey,
        escrowVault: vaultPda,
        clientTokenAccount: clientTokenAccount,
        providerTokenAccount: providerTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([oracle])
      .rpc();

    const clientAfter = await getAccount(provider.connection, clientTokenAccount);
    const providerAfter = await getAccount(provider.connection, providerTokenAccount);

    // 30% slashed refund: 300,000 units to client, 700,000 to provider
    expect(Number(clientAfter.amount) - Number(clientBefore.amount)).to.equal(300_000);
    expect(Number(providerAfter.amount) - Number(providerBefore.amount)).to.equal(700_000);
  });

  it("6. Rejects telemetry signed by unauthorized oracle authority", async () => {
    await new Promise((resolve) => setTimeout(resolve, 2500));

    try {
      await program.methods
        .submitTelemetryAndSettle(new anchor.BN(3), 9999, 45)
        .accounts({
          slaContract: slaPda,
          telemetryOracle: unauthorizedOracle.publicKey,
          escrowVault: vaultPda,
          clientTokenAccount: clientTokenAccount,
          providerTokenAccount: providerTokenAccount,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([unauthorizedOracle])
        .rpc();
      expect.fail("Should have rejected unauthorized oracle");
    } catch (err: any) {
      expect(err.error.errorCode.code).to.equal("UnauthorizedOracle");
    }
  });

  it("7. Gracefully terminates SLA contract and refunds remaining escrow to client", async () => {
    const vaultBefore = await getAccount(provider.connection, vaultPda);
    const clientBefore = await getAccount(provider.connection, clientTokenAccount);

    await program.methods
      .terminateSla()
      .accounts({
        slaContract: slaPda,
        client: client.publicKey,
        escrowVault: vaultPda,
        clientTokenAccount: clientTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([client])
      .rpc();

    const vaultAfter = await getAccount(provider.connection, vaultPda);
    const clientAfter = await getAccount(provider.connection, clientTokenAccount);
    const slaAccount = await program.account.slaContract.fetch(slaPda);

    expect(Number(vaultAfter.amount)).to.equal(0);
    expect(Number(clientAfter.amount) - Number(clientBefore.amount)).to.equal(Number(vaultBefore.amount));
    expect(slaAccount.isActive).to.be.false;
  });
});