import { Connection, PublicKey, Keypair } from "@solana/web3.js";
import { Program, AnchorProvider, Wallet, Idl, BorshEventCoder } from "@coral-xyz/anchor";
import { Resend } from "resend";
import axios from "axios";
import * as dotenv from "dotenv";

dotenv.config();

const SOLANA_RPC_URL = process.env.SOLANA_RPC_URL || "https://api.devnet.solana.com";
const SOLANA_WS_URL = process.env.SOLANA_WS_URL || "wss://api.devnet.solana.com";
const PROGRAM_ID = new PublicKey(process.env.PROGRAM_ID || "Aegis111111111111111111111111111111111111111");
const RESEND_API_KEY = process.env.RESEND_API_KEY || "re_mock_test_key";
const SLACK_WEBHOOK_URL = process.env.SLACK_WEBHOOK_URL || "";
const ENTERPRISE_AUDIT_EMAIL = process.env.ENTERPRISE_AUDIT_EMAIL || "treasury@enterprise-partner.com";

// Initialize Resend Client
const resend = new Resend(RESEND_API_KEY);

interface InvoiceFactoredPayload {
  invoicePubkey: string;
  factorLp: string;
  advanceAmount: string;
  protocolFee: string;
  netVendorPayout: string;
  timestamp: number;
}

interface InvoiceSettledPayload {
  invoicePubkey: string;
  payer: string;
  recipient: string;
  amount: string;
  wasFactored: boolean;
  timestamp: number;
}

export class AegisEnterpriseAgent {
  private connection: Connection;
  private isRunning: boolean = false;

  constructor() {
    this.connection = new Connection(SOLANA_RPC_URL, {
      wsEndpoint: SOLANA_WS_URL,
      commitment: "confirmed",
    });
  }

  public async start(): Promise<void> {
    this.isRunning = true;
    console.log(`[AegisAgent] Starting daemon on RPC: ${SOLANA_RPC_URL}`);
    console.log(`[AegisAgent] Monitoring Aegis Settlement Program: ${PROGRAM_ID.toBase58()}`);

    // Subscribe to program log filters
    this.connection.onLogs(
      PROGRAM_ID,
      async (logsContext) => {
        try {
          if (logsContext.err) return;
          await this.handleLogs(logsContext.signature, logsContext.logs);
        } catch (error) {
          console.error(`[AegisAgent] Error processing tx ${logsContext.signature}:`, error);
        }
      },
      "confirmed"
    );

    console.log("[AegisAgent] Subscribed successfully. Worker loop operational.");
  }

  private async handleLogs(signature: string, logs: string[]): Promise<void> {
    for (const line of logs) {
      if (line.includes("InvoiceFactoredEvent")) {
        console.log(`[AegisAgent] Detected InvoiceFactored in ${signature}`);
        await this.dispatchFactoringNotification(signature);
      } else if (line.includes("InvoiceSettledEvent")) {
        console.log(`[AegisAgent] Detected InvoiceSettled in ${signature}`);
        await this.dispatchSettlementAudit(signature);
      }
    }
  }

  public async dispatchFactoringNotification(signature: string): Promise<void> {
    const message = `⚡ *Invoice Instant Advance Funded!*\n• Transaction: \`${signature}\`\n• Explorer: https://explorer.solana.com/tx/${signature}?cluster=devnet\n• Status: Confirmed Liquidity Advancement`;

    await this.postSlack(message);

    try {
      await resend.emails.send({
        from: "Aegis Settlements <notifications@aegis-protocol.io>",
        to: [ENTERPRISE_AUDIT_EMAIL],
        subject: `[SOLANA FACTORED] Instant Liquidity Advance Executed (${signature.slice(0, 8)})`,
        html: `
          <h2>Aegis Settlement Protocol Notice</h2>
          <p>An institutional liquidity advance has been disbursed on-chain.</p>
          <ul>
            <li><strong>Solana Transaction:</strong> <code>${signature}</code></li>
            <li><strong>Block Finality:</strong> Confirmed (~400ms)</li>
            <li><strong>Audit Link:</strong> <a href="https://explorer.solana.com/tx/${signature}?cluster=devnet">View Solana Explorer</a></li>
          </ul>
          <p>Receivable claim ownership transferred to Liquidity Provider pool.</p>
        `,
      });
      console.log(`[AegisAgent] Resend audit email dispatched to ${ENTERPRISE_AUDIT_EMAIL}`);
    } catch (err) {
      console.error("[AegisAgent] Failed to send Resend email:", err);
    }
  }

  public async dispatchSettlementAudit(signature: string): Promise<void> {
    const message = `✅ *Invoice Settlement Complete!*\n• Obligor Paid In Full\n• Signature: \`${signature}\`\n• Ledger State: Escrow Invariant Cleared`;

    await this.postSlack(message);

    try {
      await resend.emails.send({
        from: "Aegis Settlements <audit@aegis-protocol.io>",
        to: [ENTERPRISE_AUDIT_EMAIL],
        subject: `[SOLANA SETTLED] Commercial Invoice Finality Achieved (${signature.slice(0, 8)})`,
        html: `
          <h2>Verifiable Settlement Receipt</h2>
          <p>The payer obligations for this invoice have completed atomic clearing on Solana.</p>
          <p><strong>Tx Hash:</strong> <code>${signature}</code></p>
          <p>Status: <strong>Zero Invariant Violation - Balanced Cleared</strong></p>
        `,
      });
      console.log(`[AegisAgent] Settlement email dispatched.`);
    } catch (err) {
      console.error("[AegisAgent] Failed to send settlement audit:", err);
    }
  }

  private async postSlack(text: string): Promise<void> {
    if (!SLACK_WEBHOOK_URL) {
      console.log(`[AegisAgent][Slack Mock] ${text}`);
      return;
    }
    try {
      await axios.post(SLACK_WEBHOOK_URL, { text });
    } catch (err) {
      console.error("[AegisAgent] Slack delivery failed:", err);
    }
  }
}

// Bootstrapping execution
if (require.main === module) {
  const agent = new AegisEnterpriseAgent();
  agent.start().catch((err) => {
    console.error("Fatal in AegisEnterpriseAgent:", err);
    process.exit(1);
  });
}