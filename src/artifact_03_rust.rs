use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("KFS1111111111111111111111111111111111111111");

pub const CHANNEL_SEED: &[u8] = b"channel";
pub const VAULT_AUTH_SEED: &[u8] = b"vault_auth";
pub const CHALLENGE_WINDOW_SLOTS: u64 = 500; // ~3.3 minutes at 400ms slots

#[program]
pub mod krakow_flashstream {
    use super::*;

    /// Initializes a streaming payment channel between Payer and Recipient.
    pub fn open_channel(
        ctx: Context<OpenChannel>,
        channel_id: u64,
        deposit_amount: u64,
        duration_slots: u64,
    ) -> Result<()> {
        require!(deposit_amount > 0, KfsError::ZeroDeposit);
        require!(duration_slots >= CHALLENGE_WINDOW_SLOTS, KfsError::InvalidDuration);

        let channel = &mut ctx.accounts.channel;
        channel.payer = ctx.accounts.payer.key();
        channel.recipient = ctx.accounts.recipient.key();
        channel.token_vault = ctx.accounts.vault.key();
        channel.channel_id = channel_id;
        channel.deposited_amount = deposit_amount;
        channel.transferred_amount = 0;
        channel.latest_nonce = 0;
        channel.status = ChannelStatus::Active as u8;
        channel.expiration_slot = Clock::get()?.slot.checked_add(duration_slots)
            .ok_or(KfsError::MathOverflow)?;
        channel.dispute_initiated_slot = 0;
        channel.bump = ctx.bumps.channel;
        channel.vault_bump = ctx.bumps.vault_authority;

        // Transfer funds from Payer to Escrow Vault PDA
        let cpi_accounts = Transfer {
            from: ctx.accounts.payer_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.payer.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer(CpiContext::new(cpi_program, cpi_accounts), deposit_amount)?;

        emit!(ChannelOpenedEvent {
            channel_id,
            payer: ctx.accounts.payer.key(),
            recipient: ctx.accounts.recipient.key(),
            deposited_amount: deposit_amount,
        });

        Ok(())
    }

    /// Cooperative instant settlement: both parties sign off on final state voucher.
    pub fn cooperative_close(
        ctx: Context<CooperativeClose>,
        channel_id: u64,
        final_transferred_amount: u64,
        signature_nonce: u64,
    ) -> Result<()> {
        let channel = &mut ctx.accounts.channel;
        require!(channel.status == ChannelStatus::Active as u8, KfsError::ChannelNotActive);
        require!(final_transferred_amount <= channel.deposited_amount, KfsError::AmountExceedsDeposit);
        require!(signature_nonce > channel.latest_nonce, KfsError::StaleNonce);

        let refund_amount = channel.deposited_amount
            .checked_sub(final_transferred_amount)
            .ok_or(KfsError::MathOverflow)?;

        // PDA signer seeds
        let channel_key = channel.key();
        let seeds = &[
            VAULT_AUTH_SEED,
            channel_key.as_ref(),
            &[channel.vault_bump],
        ];
        let signer = &[&seeds[..]];

        // Disburse settled amount to recipient
        if final_transferred_amount > 0 {
            let cpi_to_recipient = Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.recipient_token_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            };
            token::transfer(
                CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_to_recipient, signer),
                final_transferred_amount,
            )?;
        }

        // Return remaining escrow to payer
        if refund_amount > 0 {
            let cpi_to_payer = Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.payer_token_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            };
            token::transfer(
                CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_to_payer, signer),
                refund_amount,
            )?;
        }

        channel.transferred_amount = final_transferred_amount;
        channel.status = ChannelStatus::Closed as u8;

        emit!(ChannelSettledEvent {
            channel_id,
            final_payout: final_transferred_amount,
            refund_amount,
        });

        Ok(())
    }

    /// Optimistic unilateral close initiation by Recipient with off-chain voucher.
    pub fn initiate_unilateral_close(
        ctx: Context<UnilateralClose>,
        channel_id: u64,
        claimed_transferred_amount: u64,
        nonce: u64,
    ) -> Result<()> {
        let channel = &mut ctx.accounts.channel;
        require!(channel.status == ChannelStatus::Active as u8, KfsError::ChannelNotActive);
        require!(claimed_transferred_amount <= channel.deposited_amount, KfsError::AmountExceedsDeposit);
        require!(nonce > channel.latest_nonce, KfsError::StaleNonce);

        let current_slot = Clock::get()?.slot;
        channel.transferred_amount = claimed_transferred_amount;
        channel.latest_nonce = nonce;
        channel.status = ChannelStatus::Disputed as u8;
        channel.dispute_initiated_slot = current_slot;

        emit!(DisputeInitiatedEvent {
            channel_id,
            claimed_amount: claimed_transferred_amount,
            nonce,
            challenge_deadline: current_slot + CHALLENGE_WINDOW_SLOTS,
        });

        Ok(())
    }

    /// Fraud proof: Payer demonstrates that an equal or newer valid nonce exists.
    pub fn challenge_fraud(
        ctx: Context<ChallengeFraud>,
        channel_id: u64,
        counter_nonce: u64,
        counter_amount: u64,
    ) -> Result<()> {
        let channel = &mut ctx.accounts.channel;
        require!(channel.status == ChannelStatus::Disputed as u8, KfsError::NotInDispute);
        require!(counter_nonce > channel.latest_nonce, KfsError::InvalidChallengeNonce);
        require!(counter_amount <= channel.deposited_amount, KfsError::AmountExceedsDeposit);

        // Slash entire vault balance directly to the honest payer
        let remaining_vault_balance = ctx.accounts.vault.amount;
        let channel_key = channel.key();
        let seeds = &[
            VAULT_AUTH_SEED,
            channel_key.as_ref(),
            &[channel.vault_bump],
        ];
        let signer = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.payer_token_account.to_account_info(),
            authority: ctx.accounts.vault_authority.to_account_info(),
        };
        token::transfer(
            CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_accounts, signer),
            remaining_vault_balance,
        )?;

        channel.status = ChannelStatus::Slashed as u8;

        emit!(ChannelSlashedEvent {
            channel_id,
            fraudulent_recipient: channel.recipient,
            slashed_amount: remaining_vault_balance,
        });

        Ok(())
    }

    /// Finalize channel after dispute window elapsed without a counter-challenge.
    pub fn finalize_expired_dispute(
        ctx: Context<FinalizeExpiredDispute>,
        channel_id: u64,
    ) -> Result<()> {
        let channel = &mut ctx.accounts.channel;
        require!(channel.status == ChannelStatus::Disputed as u8, KfsError::NotInDispute);

        let current_slot = Clock::get()?.slot;
        require!(
            current_slot >= channel.dispute_initiated_slot.checked_add(CHALLENGE_WINDOW_SLOTS).ok_or(KfsError::MathOverflow)?,
            KfsError::ChallengeWindowActive
        );

        let final_amount = channel.transferred_amount;
        let refund_amount = channel.deposited_amount.checked_sub(final_amount).ok_or(KfsError::MathOverflow)?;

        let channel_key = channel.key();
        let seeds = &[
            VAULT_AUTH_SEED,
            channel_key.as_ref(),
            &[channel.vault_bump],
        ];
        let signer = &[&seeds[..]];

        if final_amount > 0 {
            let cpi_recipient = Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.recipient_token_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            };
            token::transfer(
                CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_recipient, signer),
                final_amount,
            )?;
        }

        if refund_amount > 0 {
            let cpi_payer = Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.payer_token_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            };
            token::transfer(
                CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_payer, signer),
                refund_amount,
            )?;
        }

        channel.status = ChannelStatus::Closed as u8;

        emit!(ChannelSettledEvent {
            channel_id,
            final_payout: final_amount,
            refund_amount,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION STRUCTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(channel_id: u64)]
pub struct OpenChannel<'info> {
    #[account(
        init,
        payer = payer,
        space = 8 + ChannelState::LEN,
        seeds = [CHANNEL_SEED, payer.key().as_ref(), recipient.key().as_ref(), &channel_id.to_le_bytes()],
        bump
    )]
    pub channel: Account<'info, ChannelState>,

    /// CHECK: Vault Authority PDA
    #[account(
        seeds = [VAULT_AUTH_SEED, channel.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        init,
        payer = payer,
        token::mint = mint,
        token::authority = vault_authority,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: Arbitrary recipient address
    pub recipient: AccountInfo<'info>,

    #[account(
        mut,
        constraint = payer_token_account.owner == payer.key() @ KfsError::InvalidAccountOwner,
        constraint = payer_token_account.mint == mint.key() @ KfsError::InvalidMint
    )]
    pub payer_token_account: Account<'info, TokenAccount>,

    pub mint: Account<'info, Mint>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
#[instruction(channel_id: u64)]
pub struct CooperativeClose<'info> {
    #[account(
        mut,
        seeds = [CHANNEL_SEED, channel.payer.as_ref(), channel.recipient.as_ref(), &channel_id.to_le_bytes()],
        bump = channel.bump,
        has_one = payer,
        has_one = recipient,
        has_one = token_vault @ KfsError::InvalidVault
    )]
    pub channel: Account<'info, ChannelState>,

    /// CHECK: Vault Authority PDA
    #[account(
        seeds = [VAULT_AUTH_SEED, channel.key().as_ref()],
        bump = channel.vault_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(mut)]
    pub recipient: Signer<'info>,

    #[account(mut)]
    pub payer_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub recipient_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(channel_id: u64)]
pub struct UnilateralClose<'info> {
    #[account(
        mut,
        seeds = [CHANNEL_SEED, channel.payer.as_ref(), channel.recipient.as_ref(), &channel_id.to_le_bytes()],
        bump = channel.bump,
        has_one = recipient
    )]
    pub channel: Account<'info, ChannelState>,

    #[account(mut)]
    pub recipient: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(channel_id: u64)]
pub struct ChallengeFraud<'info> {
    #[account(
        mut,
        seeds = [CHANNEL_SEED, channel.payer.as_ref(), channel.recipient.as_ref(), &channel_id.to_le_bytes()],
        bump = channel.bump,
        has_one = payer,
        has_one = token_vault @ KfsError::InvalidVault
    )]
    pub channel: Account<'info, ChannelState>,

    /// CHECK: Vault Authority PDA
    #[account(
        seeds = [VAULT_AUTH_SEED, channel.key().as_ref()],
        bump = channel.vault_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(mut)]
    pub payer_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
#[instruction(channel_id: u64)]
pub struct FinalizeExpiredDispute<'info> {
    #[account(
        mut,
        seeds = [CHANNEL_SEED, channel.payer.as_ref(), channel.recipient.as_ref(), &channel_id.to_le_bytes()],
        bump = channel.bump,
        has_one = payer,
        has_one = recipient,
        has_one = token_vault @ KfsError::InvalidVault
    )]
    pub channel: Account<'info, ChannelState>,

    /// CHECK: Vault Authority PDA
    #[account(
        seeds = [VAULT_AUTH_SEED, channel.key().as_ref()],
        bump = channel.vault_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    /// CHECK: Channel payer can be mut for refund
    #[account(mut)]
    pub payer: AccountInfo<'info>,

    /// CHECK: Channel recipient can be mut for payout
    #[account(mut)]
    pub recipient: AccountInfo<'info>,

    #[account(mut)]
    pub payer_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub recipient_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// STATE STRUCTS & ENUMS
// -----------------------------------------------------------------------------

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    Inactive = 0,
    Active = 1,
    Disputed = 2,
    Closed = 3,
    Slashed = 4,
}

#[account]
pub struct ChannelState {
    pub payer: Pubkey,                  // 32
    pub recipient: Pubkey,              // 32
    pub token_vault: Pubkey,            // 32
    pub channel_id: u64,                // 8
    pub deposited_amount: u64,          // 8
    pub transferred_amount: u64,        // 8
    pub latest_nonce: u64,              // 8
    pub expiration_slot: u64,           // 8
    pub dispute_initiated_slot: u64,    // 8
    pub status: u8,                     // 1
    pub bump: u8,                       // 1
    pub vault_bump: u8,                 // 1
}

impl ChannelState {
    pub const LEN: usize = 32 + 32 + 32 + 8 + 8 + 8 + 8 + 8 + 8 + 1 + 1 + 1; // 139 bytes
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct ChannelOpenedEvent {
    pub channel_id: u64,
    pub payer: Pubkey,
    pub recipient: Pubkey,
    pub deposited_amount: u64,
}

#[event]
pub struct ChannelSettledEvent {
    pub channel_id: u64,
    pub final_payout: u64,
    pub refund_amount: u64,
}

#[event]
pub struct DisputeInitiatedEvent {
    pub channel_id: u64,
    pub claimed_amount: u64,
    pub nonce: u64,
    pub challenge_deadline: u64,
}

#[event]
pub struct ChannelSlashedEvent {
    pub channel_id: u64,
    pub fraudulent_recipient: Pubkey,
    pub slashed_amount: u64,
}

#[error_code]
pub enum KfsError {
    #[msg("Initial deposit amount must be greater than zero.")]
    ZeroDeposit,
    #[msg("Duration slots must exceed minimum challenge dispute window.")]
    InvalidDuration,
    #[msg("Channel is not in active state.")]
    ChannelNotActive,
    #[msg("Settlement amount exceeds total deposited channel collateral.")]
    AmountExceedsDeposit,
    #[msg("Provided nonce is stale or invalid.")]
    StaleNonce,
    #[msg("Vault address does not match channel record.")]
    InvalidVault,
    #[msg("Channel is currently not under dispute.")]
    NotInDispute,
    #[msg("Challenge nonce must be strictly higher than disputed claim nonce.")]
    InvalidChallengeNonce,
    #[msg("Dispute challenge window is still active; cannot finalize yet.")]
    ChallengeWindowActive,
    #[msg("Arithmetic calculation resulted in overflow/underflow.")]
    MathOverflow,
    #[msg("Token account owner constraint violation.")]
    InvalidAccountOwner,
    #[msg("Token account mint constraint violation.")]
    InvalidMint,
}