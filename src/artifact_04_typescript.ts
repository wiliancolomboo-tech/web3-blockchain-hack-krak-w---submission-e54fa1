import { Connection, PublicKey, Keypair } from "@solana/web3.js";
import { Program, AnchorProvider, Wallet, Idl, BN } from "@coral-xyz/anchor";
import { Resend } from "resend";
import axios from "axios";
import * as crypto from "crypto";

// Environment Configuration
const SOLANA_RPC_URL = process.env.SOLANA_RPC_URL || "http://127.0.0.1:8899";
const SOLANA_WS_URL = process.env.SOLANA_WS_URL || "ws://127.0.0.1:8900";
const RESEND_API_KEY = process.env.RESEND_API_KEY || "re_test_000000000000000000000000";
const DISCORD_WEBHOOK_URL = process.env.DISCORD_WEBHOOK_URL || "";
const INVOICE_SENDER_EMAIL = process.env.INVOICE_SENDER_EMAIL || "settlement@kraksettle.io";

const resend = new Resend(RESEND_API_KEY);

export interface MilestoneReceipt {
  escrowId: string;
  milestoneIndex: number;
  amountUsdc: number;
  recipient: string;
  txHash: string;
  timestamp: string;
}

export class KrakSettleEnterpriseAgent {
  private connection: Connection;
  private provider: AnchorProvider;

  constructor(signerKeypair: Keypair) {
    this.connection = new Connection(SOLANA_RPC_URL, {
      wsEndpoint: SOLANA_WS_URL,
      commitment: "confirmed",
    });

    const wallet = new Wallet(signerKeypair);
    this.provider = new AnchorProvider(this.connection, wallet, {
      preflightCommitment: "confirmed",
    });
  }

  /**
   * Helper: Calculates deterministic Sha256 hash for deliverable artifact
   */
  public static computeArtifactHash(deliverableBuffer: Buffer): number[] {
    const hash = crypto.createHash("sha256").update(deliverableBuffer).digest();
    return Array.from(hash);
  }

  /**
   * Dispatches branded enterprise invoice & audit log through Resend API
   */
  public async dispatchEnterpriseReceipt(
    receipt: MilestoneReceipt,
    clientEmail: string,
    vendorEmail: string
  ): Promise<boolean> {
    const htmlBody = `
      <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto; border: 1px solid #e2e8f0; border-radius: 8px; overflow: hidden;">
        <div style="background-color: #0f172a; padding: 24px; color: #ffffff;">
          <h1 style="margin: 0; font-size: 20px;">KrakSettle Settlement Confirmation</h1>
          <p style="margin: 4px 0 0; color: #94a3b8; font-size: 13px;">Proof of Payment & Cryptographic Milestone Audit</p>
        </div>
        <div style="padding: 24px; background-color: #ffffff;">
          <table style="width: 100%; border-collapse: collapse; margin-bottom: 20px;">
            <tr>
              <td style="padding: 8px 0; color: #64748b;">Escrow Agreement ID:</td>
              <td style="padding: 8px 0; font-weight: bold; text-align: right;">#${receipt.escrowId}</td>
            </tr>
            <tr>
              <td style="padding: 8px 0; color: #64748b;">Milestone Index:</td>
              <td style="padding: 8px 0; font-weight: bold; text-align: right;">Milestone #${receipt.milestoneIndex + 1}</td>
            </tr>
            <tr>
              <td style="padding: 8px 0; color: #64748b;">Amount Settled:</td>
              <td style="padding: 8px 0; font-weight: bold; color: #059669; text-align: right;">$${receipt.amountUsdc.toFixed(2)} USDC</td>
            </tr>
            <tr>
              <td style="padding: 8px 0; color: #64748b;">Recipient Pubkey:</td>
              <td style="padding: 8px 0; font-family: monospace; font-size: 11px; text-align: right;">${receipt.recipient}</td>
            </tr>
            <tr>
              <td style="padding: 8px 0; color: #64748b;">Settlement Timestamp:</td>
              <td style="padding: 8px 0; text-align: right;">${receipt.timestamp}</td>
            </tr>
          </table>
          <div style="background-color: #f8fafc; padding: 12px; border-radius: 4px; border-left: 4px solid #3b82f6;">
            <p style="margin: 0; font-size: 12px; color: #475569;">
              <strong>On-Chain Confirmation:</strong> Verified on Solana. Transaction Signature: <br/>
              <span style="font-family: monospace; word-break: break-all;">${receipt.txHash}</span>
            </p>
          </div>
        </div>
      </div>
    `;

    try {
      if (RESEND_API_KEY && RESEND_API_KEY !== "re_test_000000000000000000000000") {
        await resend.emails.send({
          from: INVOICE_SENDER_EMAIL,
          to: [clientEmail, vendorEmail],
          subject: `[Settled] KrakSettle Milestone Release - Escrow #${receipt.escrowId}`,
          html: htmlBody,
        });
        console.log(`[Resend] Successfully dispatched audit receipt to ${clientEmail} & ${vendorEmail}`);
      } else {
        console.log(`[Resend Mock Mode] Invoice dispatched for Escrow #${receipt.escrowId} ($${receipt.amountUsdc} USDC)`);
      }
      return true;
    } catch (err: any) {
      console.error(`[Resend Error] Failed to transmit email receipt: ${err.message}`);
      return false;
    }
  }

  /**
   * Posts Real-Time Notification to Discord / Slack Webhook
   */
  public async postWebhookNotification(receipt: MilestoneReceipt): Promise<void> {
    if (!DISCORD_WEBHOOK_URL) {
      console.log(`[Webhook Mock Mode] Webhook event emitted for milestone #${receipt.milestoneIndex}`);
      return;
    }

    try {
      await axios.post(DISCORD_WEBHOOK_URL, {
        embeds: [
          {
            title: `⚡ KrakSettle Milestone Settlement Executed`,
            color: 3066993, // Green
            fields: [
              { name: "Escrow ID", value: receipt.escrowId, inline: true },
              { name: "Milestone", value: `#${receipt.milestoneIndex + 1}`, inline: true },
              { name: "Disbursed Amount", value: `$${receipt.amountUsdc} USDC`, inline: true },
              { name: "Recipient", value: `\`${receipt.recipient}\``, inline: false },
              { name: "Tx Signature", value: `\`${receipt.txHash}\``, inline: false },
            ],
            timestamp: new Date().toISOString(),
          },
        ],
      });
    } catch (err: any) {
      console.error(`[Webhook Alert Error] Failed to publish webhook: ${err.message}`);
    }
  }

  /**
   * Starts active WebSocket listener for automated invoice dispatching
   */
  public async startTelemetryDaemon(programId: PublicKey, idl: Idl): Promise<void> {
    const program = new Program(idl, this.provider);
    console.log(`[Daemon] KrakSettle Autonomous Daemon active on Program ID: ${programId.toBase58()}`);

    program.addEventListener("MilestoneApprovedEvent", async (event: any, slot: number, signature: string) => {
      console.log(`[Event Received] Milestone Approved for Escrow #${event.escrowId.toString()} at slot ${slot}`);
      
      const receipt: MilestoneReceipt = {
        escrowId: event.escrowId.toString(),
        milestoneIndex: event.milestoneIndex,
        amountUsdc: new BN(event.amount).toNumber() / 1_000_000,
        recipient: event.recipient.toBase58(),
        txHash: signature,
        timestamp: new Date(event.timestamp.toNumber() * 1000).toUTCString(),
      };

      await this.dispatchEnterpriseReceipt(
        receipt,
        "procurement-dept@enterprise.com",
        "billing@vendorpartner.eu"
      );
      await this.postWebhookNotification(receipt);
    });
  }
}

// Direct CLI Execution Entrypoint
if (require.main === module) {
  const dummyKeypair = Keypair.generate();
  const agent = new KrakSettleEnterpriseAgent(dummyKeypair);
  console.log("[Init] KrakSettle Enterprise Integration Service initialized successfully.");
  console.log(`[Config] Connection established to Solana RPC: ${SOLANA_RPC_URL}`);
}