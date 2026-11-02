import * as anchor from '@coral-xyz/anchor';
import { Program, BN } from '@coral-xyz/anchor';
import { Keypair, SystemProgram, LAMPORTS_PER_SOL } from '@solana/web3.js';
import { assert } from 'chai';
import { KrakEscrow } from '../target/types/krak_escrow';

describe('krak_escrow tests', () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.KrakEscrow as Program<KrakEscrow>;

  const payer = Keypair.generate();
  const contractor = Keypair.generate();
  const arbiter = Keypair.generate();
  const attacker = Keypair.generate();

  const escrowId = new BN(42069);
  const milestone1 = new BN(1 * LAMPORTS_PER_SOL);
  const milestone2 = new BN(2 * LAMPORTS_PER_SOL);
  const totalAmount = milestone1.add(milestone2);

  let escrowPda: anchor.web3.PublicKey;
  let escrowBump: number;
  let vaultPda: anchor.web3.PublicKey;
  let vaultBump: number;

  before(async () => {
    // Airdrop SOL to test actors
    for (const actor of [payer, contractor, arbiter, attacker]) {
      const airdropSig = await provider.connection.requestAirdrop(
        actor.publicKey,
        10 * LAMPORTS_PER_SOL
      );
      const latestBlockhash = await provider.connection.getLatestBlockhash();
      await provider.connection.confirmTransaction({
        signature: airdropSig,
        ...latestBlockhash,
      });
    }

    [escrowPda, escrowBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from('escrow'),
        payer.publicKey.toBuffer(),
        contractor.publicKey.toBuffer(),
        escrowId.toArrayLike(Buffer, 'le', 8),
      ],
      program.programId
    );

    [vaultPda, vaultBump] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from('vault'), escrowPda.toBuffer()],
      program.programId
    );
  });

  it('1. Successfully initializes an Escrow with 2 milestones & funds vault', async () => {
    const tx = await program.methods
      .initializeEscrow(escrowId, [milestone1, milestone2], totalAmount)
      .accountsPartial({
        payer: payer.publicKey,
        contractor: contractor.publicKey,
        arbiter: arbiter.publicKey,
        escrow: escrowPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([payer])
      .rpc();

    assert.isString(tx);

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    assert.isTrue(escrowAccount.payer.equals(payer.publicKey));
    assert.isTrue(escrowAccount.contractor.equals(contractor.publicKey));
    assert.isTrue(escrowAccount.arbiter.equals(arbiter.publicKey));
    assert.equal(escrowAccount.totalAmount.toNumber(), totalAmount.toNumber());
    assert.equal(escrowAccount.milestoneCount, 2);
    assert.deepEqual(escrowAccount.status, { active: {} });

    // Validate Vault balance matches total deposit
    const vaultBalance = await provider.connection.getBalance(vaultPda);
    assert.equal(vaultBalance, totalAmount.toNumber());
  });

  it('2. Contractor submits Milestone 0 with SHA-256 proof hash', async () => {
    const dummyProof = Buffer.alloc(32, 7); // Hash representing IPFS delivery commit

    await program.methods
      .submitMilestone(0, Array.from(dummyProof))
      .accountsPartial({
        contractor: contractor.publicKey,
        escrow: escrowPda,
      })
      .signers([contractor])
      .rpc();

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    assert.deepEqual(escrowAccount.milestones[0].status, { submitted: {} });
    assert.deepEqual(Buffer.from(escrowAccount.milestones[0].proofHash), dummyProof);
  });

  it('3. Rejects milestone submission from an unauthorized attacker', async () => {
    const dummyProof = Buffer.alloc(32, 9);
    try {
      await program.methods
        .submitMilestone(1, Array.from(dummyProof))
        .accountsPartial({
          contractor: attacker.publicKey,
          escrow: escrowPda,
        })
        .signers([attacker])
        .rpc();
      assert.fail('Should have failed with unauthorized signer constraint');
    } catch (err: any) {
      assert.include(err.toString(), 'Constraint');
    }
  });

  it('4. Payer approves Milestone 0 -> releases 1 SOL to Contractor', async () => {
    const contractorBalBefore = await provider.connection.getBalance(contractor.publicKey);

    await program.methods
      .approveMilestone(0)
      .accountsPartial({
        payer: payer.publicKey,
        contractor: contractor.publicKey,
        escrow: escrowPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([payer])
      .rpc();

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    assert.deepEqual(escrowAccount.milestones[0].status, { approved: {} });
    assert.equal(escrowAccount.releasedAmount.toNumber(), milestone1.toNumber());

    const contractorBalAfter = await provider.connection.getBalance(contractor.publicKey);
    assert.equal(
      contractorBalAfter - contractorBalBefore,
      milestone1.toNumber(),
      'Contractor should receive exact milestone lamports'
    );
  });

  it('5. Raises dispute on Milestone 1', async () => {
    await program.methods
      .raiseDispute(1)
      .accountsPartial({
        signer: payer.publicKey,
        escrow: escrowPda,
      })
      .signers([payer])
      .rpc();

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    assert.deepEqual(escrowAccount.status, { disputed: {} });
    assert.deepEqual(escrowAccount.milestones[1].status, { disputed: {} });
  });

  it('6. Arbiter resolves dispute with a 70/30 split', async () => {
    // 70% to contractor (7000 basis points), 30% refund to payer
    const contractorBalBefore = await provider.connection.getBalance(contractor.publicKey);
    const payerBalBefore = await provider.connection.getBalance(payer.publicKey);

    await program.methods
      .resolveDispute(1, 7000)
      .accountsPartial({
        arbiter: arbiter.publicKey,
        payer: payer.publicKey,
        contractor: contractor.publicKey,
        escrow: escrowPda,
        vault: vaultPda,
        systemProgram: SystemProgram.programId,
      })
      .signers([arbiter])
      .rpc();

    const escrowAccount = await program.account.escrowAccount.fetch(escrowPda);
    assert.deepEqual(escrowAccount.status, { completed: {} });
    assert.deepEqual(escrowAccount.milestones[1].status, { resolved: {} });

    const expectedContractorAmount = milestone2.toNumber() * 0.7;
    const expectedPayerRefund = milestone2.toNumber() * 0.3;

    const contractorBalAfter = await provider.connection.getBalance(contractor.publicKey);
    const payerBalAfter = await provider.connection.getBalance(payer.publicKey);

    assert.equal(contractorBalAfter - contractorBalBefore, expectedContractorAmount);
    assert.equal(payerBalAfter - payerBalBefore, expectedPayerRefund);
  });
});