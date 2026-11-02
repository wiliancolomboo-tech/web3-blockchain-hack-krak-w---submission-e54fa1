import { Connection, PublicKey, Keypair, LAMPORTS_PER_SOL } from '@solana/web3.js';
import * as anchor from '@coral-xyz/anchor';
import express, { Request, Response } from 'express';
import dotenv from 'dotenv';
import { TelemetryDispatcher, TelemetryEvent } from './telemetry';

dotenv.config();

const RPC_ENDPOINT = process.env.SOLANA_RPC_URL || 'https://api.devnet.solana.com';
const WS_ENDPOINT = process.env.SOLANA_WS_URL || 'wss://api.devnet.solana.com';
const PROGRAM_ID = new PublicKey(process.env.PROGRAM_ID || 'WaweL6zW8d9ZqV5vM5hP6jF8wZ2qL9Yk5bNx7c4E1aP');
const PORT = process.env.PORT || 8080;

export class WawelKeeperDaemon {
  private connection: Connection;
  private telemetry: TelemetryDispatcher;
  private isHealthy: boolean = false;
  private processedSignatures: Set<string> = new Set();

  constructor() {
    this.connection = new Connection(RPC_ENDPOINT, {
      wsEndpoint: WS_ENDPOINT,
      commitment: 'confirmed'
    });
    this.telemetry = new TelemetryDispatcher();
  }

  public async start(): Promise<void> {
    console.log(`[Worker] Initializing Wawel Keeper Daemon on Solana Cluster...`);
    console.log(`[Worker] Target Program ID: ${PROGRAM_ID.toBase58()}`);

    try {
      const slot = await this.connection.getSlot();
      console.log(`[Worker] Connected to Solana RPC. Current Slot: ${slot}`);
      this.isHealthy = true;
    } catch (err) {
      console.error('[Worker] Fatal RPC Connection Error:', err);
      process.exit(1);
    }

    // Subscribe to Program account changes and emitted transaction logs
    this.connection.onLogs(
      PROGRAM_ID,
      async (logsContext) => {
        const { signature, err, logs } = logsContext;
        if (err || this.processedSignatures.has(signature)) return;
        this.processedSignatures.add(signature);

        await this.handleLogEvent(signature, logs);
      },
      'confirmed'
    );

    this.startHttpServer();
  }

  private async handleLogEvent(signature: string, logs: string[]): Promise<void> {
    console.log(`[Worker] Processing Tx Log: ${signature}`);
    
    // Scan logs for program events
    const isDisbursed = logs.some(l => l.includes('EscrowDisbursedEvent'));
    const isCreated = logs.some(l => l.includes('VaultInitializedEvent'));
    const isProof = logs.some(l => l.includes('MilestoneSubmittedEvent'));

    let eventType: TelemetryEvent['eventType'] | null = null;
    if (isDisbursed) eventType = 'DISBURSED';
    else if (isCreated) eventType = 'VAULT_CREATED';
    else if (isProof) eventType = 'PROOF_SUBMITTED';

    if (!eventType) return;

    // Build event object
    const eventPayload: TelemetryEvent = {
      eventType,
      vaultId: '1001',
      signature,
      depositor: '9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin',
      beneficiary: '5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1',
      amountSol: 5.5,
      timestamp: new Date().toISOString()
    };

    console.log(`[Worker] Detected ${eventType} event. Forwarding to enterprise sinks...`);
    await Promise.all([
      this.telemetry.notifySlack(eventPayload),
      this.telemetry.sendResendReceipt(eventPayload)
    ]);
  }

  private startHttpServer(): void {
    const app = express();

    app.get('/healthz', async (_req: Request, res: Response) => {
      try {
        const currentSlot = await this.connection.getSlot();
        res.status(200).json({
          status: 'UP',
          healthy: this.isHealthy,
          clusterSlot: currentSlot,
          programId: PROGRAM_ID.toBase58(),
          cachedSignaturesCount: this.processedSignatures.size,
          uptime: process.uptime()
        });
      } catch (err: any) {
        res.status(503).json({ status: 'DOWN', error: err.message });
      }
    });

    app.listen(PORT, () => {
      console.log(`[Worker] Telemetry healthcheck operational at http://0.0.0.0:${PORT}/healthz`);
    });
  }
}

if (require.main === module) {
  const daemon = new WawelKeeperDaemon();
  daemon.start();
}