import axios from "axios";

export interface AlertPayload {
  slaId: string;
  epoch: number;
  measuredUptimeBps: number;
  measuredLatencyMs: number;
  minUptimeBps: number;
  maxLatencyMs: number;
  slashedRefundAmount: number;
  providerReceivedAmount: number;
  txSignature: string;
}

export class EnterpriseNotifier {
  private resendApiKey?: string;
  private enterpriseEmail?: string;
  private webhookUrl?: string;

  constructor() {
    this.resendApiKey = process.env.RESEND_API_KEY;
    this.enterpriseEmail = process.env.ENTERPRISE_ALERT_EMAIL;
    this.webhookUrl = process.env.ALERT_WEBHOOK_URL;
  }

  public async notifySLABreach(payload: AlertPayload): Promise<void> {
    const formattedUptime = (payload.measuredUptimeBps / 100).toFixed(2);
    const targetUptime = (payload.minUptimeBps / 100).toFixed(2);

    console.log(`[ALERT] SLA Breach detected on contract #${payload.slaId} (Epoch: ${payload.epoch})`);

    // 1. Send Enterprise Webhook (Discord / Slack compatible)
    if (this.webhookUrl) {
      try {
        await axios.post(this.webhookUrl, {
          username: "Kraków Sentry SLA Bot",
          content: `🚨 **ENTERPRISE SLA BREACH DETECTED & SETTLED ON SOLANA** 🚨`,
          embeds: [
            {
              title: `Contract SLA Breach: #${payload.slaId}`,
              color: 15158332, // Red
              fields: [
                { name: "Epoch", value: `${payload.epoch}`, inline: true },
                { name: "Observed Latency", value: `${payload.measuredLatencyMs} ms (Limit: ${payload.maxLatencyMs} ms)`, inline: true },
                { name: "Observed Uptime", value: `${formattedUptime}% (Target: ${targetUptime}%)`, inline: true },
                { name: "Slashed to Client", value: `${payload.slashedRefundAmount} units`, inline: true },
                { name: "Provider Retained", value: `${payload.providerReceivedAmount} units`, inline: true },
                { name: "Solana Transaction", value: `[Explorer Link](https://explorer.solana.com/tx/${payload.txSignature}?cluster=custom)` }
              ],
              footer: { text: "Kraków Sentry Automated Oracle Engine" },
              timestamp: new Date().toISOString(),
            },
          ],
        });
        console.log("[NOTIFIER] Slack/Discord alert dispatched.");
      } catch (err: any) {
        console.error("[NOTIFIER] Failed to send webhook alert:", err.message);
      }
    }

    // 2. Dispatch Enterprise Email via Resend
    if (this.resendApiKey && this.enterpriseEmail) {
      try {
        await axios.post(
          "https://api.resend.com/emails",
          {
            from: "Krakow Sentry <alerts@resend.dev>",
            to: [this.enterpriseEmail],
            subject: `[CRITICAL SLA BREACH] Contract #${payload.slaId} Epoch ${payload.epoch} Penalized`,
            html: `
              <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto; border: 1px solid #e11d48; border-radius: 8px; padding: 24px;">
                <h2 style="color: #e11d48; margin-top: 0;">Autonomous SLA Slash Executed</h2>
                <p>Kraków Sentry detected performance telemetry outside contractual parameters.</p>
                <table style="width: 100%; border-collapse: collapse; margin-bottom: 20px;">
                  <tr><td style="padding: 8px; border-bottom: 1px solid #ddd;"><strong>SLA ID:</strong></td><td style="padding: 8px; border-bottom: 1px solid #ddd;">${payload.slaId}</td></tr>
                  <tr><td style="padding: 8px; border-bottom: 1px solid #ddd;"><strong>Epoch:</strong></td><td style="padding: 8px; border-bottom: 1px solid #ddd;">${payload.epoch}</td></tr>
                  <tr><td style="padding: 8px; border-bottom: 1px solid #ddd;"><strong>Latency:</strong></td><td style="padding: 8px; border-bottom: 1px solid #ddd;">${payload.measuredLatencyMs} ms (Threshold: ${payload.maxLatencyMs} ms)</td></tr>
                  <tr><td style="padding: 8px; border-bottom: 1px solid #ddd;"><strong>Uptime:</strong></td><td style="padding: 8px; border-bottom: 1px solid #ddd;">${formattedUptime}% (Floor: ${targetUptime}%)</td></tr>
                  <tr><td style="padding: 8px; border-bottom: 1px solid #ddd;"><strong>Refund Slashed:</strong></td><td style="padding: 8px; border-bottom: 1px solid #ddd;">${payload.slashedRefundAmount}</td></tr>
                </table>
                <p><strong>Transaction Signature:</strong></p>
                <code style="background-color: #f1f5f9; padding: 4px 8px; border-radius: 4px; word-break: break-all;">${payload.txSignature}</code>
              </div>
            `,
          },
          {
            headers: {
              Authorization: `Bearer ${this.resendApiKey}`,
              "Content-Type": "application/json",
            },
          }
        );
        console.log(`[NOTIFIER] Resend incident report sent to ${this.enterpriseEmail}`);
      } catch (err: any) {
        console.error("[NOTIFIER] Failed to send Resend email:", err.response?.data || err.message);
      }
    }
  }
}