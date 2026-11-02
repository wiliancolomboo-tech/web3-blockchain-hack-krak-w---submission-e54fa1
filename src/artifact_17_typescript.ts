import { Resend } from 'resend';
import fetch from 'node-fetch';

export interface MilestoneApprovedNotification {
  escrowPubkey: string;
  milestoneIndex: number;
  amountSol: number;
  contractor: string;
  txSignature: string;
}

export class EnterpriseNotifier {
  private resend: Resend | null = null;
  private slackWebhookUrl: string;
  private senderEmail: string;
  private billingAlertEmail: string;

  constructor() {
    const resendApiKey = process.env.RESEND_API_KEY;
    if (resendApiKey) {
      this.resend = new Resend(resendApiKey);
    }
    this.slackWebhookUrl = process.env.SLACK_WEBHOOK_URL || '';
    this.senderEmail = process.env.RESEND_FROM_EMAIL || 'billing@krakescrew.io';
    this.billingAlertEmail = process.env.ENTERPRISE_ALERT_EMAIL || 'finance@enterprise.com';
  }

  public async notifyMilestoneApproved(data: MilestoneApprovedNotification): Promise<void> {
    const explorerUrl = `https://explorer.solana.com/tx/${data.txSignature}?cluster=devnet`;
    
    console.log(`[Notifier] Broadcasting milestone ${data.milestoneIndex} approved for ${data.amountSol} SOL.`);

    // 1. Dispatch via Resend API
    if (this.resend) {
      try {
        const response = await this.resend.emails.send({
          from: this.senderEmail,
          to: this.billingAlertEmail,
          subject: `[KrakEscrow Settled] Milestone ${data.milestoneIndex + 1} Released: ${data.amountSol} SOL`,
          html: `
            <div style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif; max-width: 600px; margin: 0 auto; padding: 24px; border: 1px solid #eaeaea; border-radius: 8px;">
              <h2 style="color: #0c1024; margin-bottom: 8px;">KrakEscrow Enterprise Settlement Notice</h2>
              <p style="color: #666; font-size: 14px;">Instant settlement confirmed on Solana Devnet.</p>
              <hr style="border: 0; border-top: 1px solid #eaeaea; margin: 20px 0;" />
              <table style="width: 100%; border-collapse: collapse; font-size: 14px;">
                <tr>
                  <td style="padding: 8px 0; color: #555;"><strong>Escrow Account:</strong></td>
                  <td style="padding: 8px 0; font-family: monospace;">${data.escrowPubkey}</td>
                </tr>
                <tr>
                  <td style="padding: 8px 0; color: #555;"><strong>Milestone Index:</strong></td>
                  <td style="padding: 8px 0;">#${data.milestoneIndex + 1}</td>
                </tr>
                <tr>
                  <td style="padding: 8px 0; color: #555;"><strong>Amount Released:</strong></td>
                  <td style="padding: 8px 0; font-weight: bold; color: #00875A;">${data.amountSol} SOL</td>
                </tr>
                <tr>
                  <td style="padding: 8px 0; color: #555;"><strong>Contractor Recipient:</strong></td>
                  <td style="padding: 8px 0; font-family: monospace;">${data.contractor}</td>
                </tr>
              </table>
              <div style="margin-top: 24px;">
                <a href="${explorerUrl}" style="background-color: #14F195; color: #000; text-decoration: none; padding: 12px 20px; font-weight: bold; border-radius: 4px; display: inline-block;">
                  View on Solana Explorer
                </a>
              </div>
            </div>
          `,
        });
        console.log(`[Notifier] Resend email dispatched successfully. ID: ${response.data?.id}`);
      } catch (err) {
        console.error('[Notifier] Failed to send email through Resend:', err);
      }
    }

    // 2. Dispatch to Slack Webhook
    if (this.slackWebhookUrl) {
      try {
        const payload = {
          text: `💰 *Milestone Approved & Released via KrakEscrow!*`,
          blocks: [
            {
              type: 'section',
              text: {
                type: 'mrkdwn',
                text: `*KrakEscrow Payout Notification*\n*Milestone Index:* #${data.milestoneIndex + 1}\n*Amount:* \`${data.amountSol} SOL\`\n*Contractor:* \`${data.contractor}\``,
              },
            },
            {
              type: 'actions',
              elements: [
                {
                  type: 'button',
                  text: {
                    type: 'plain_text',
                    text: 'Verify Solana Explorer',
                  },
                  url: explorerUrl,
                  style: 'primary',
                },
              ],
            },
          ],
        };

        const res = await fetch(this.slackWebhookUrl, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload),
        });

        if (!res.ok) {
          console.error(`[Notifier] Slack response failure status: ${res.status}`);
        } else {
          console.log('[Notifier] Slack webhook dispatched successfully.');
        }
      } catch (err) {
        console.error('[Notifier] Failed to push to Slack Webhook:', err);
      }
    }
  }
}