use anchor_lang::prelude::*;
use anchor_spl::token::{self, CloseAccount, Token, TokenAccount, Transfer};

declare_id!("Kssp111111111111111111111111111111111111111");

#[program]
pub mod kssp_protocol {
    use super::*;

    /// Initialize a new milestone-gated vesting stream with escrow custody.
    pub fn create_stream(
        ctx: Context<CreateStream>,
        stream_id: u64,
        total_amount: u64,
        milestone_count: u8,
        start_time: i64,
        end_time: i64,
        milestone_allocations: Vec<u64>,
        milestone_hashes: Vec<[u8; 32]>,
    ) -> Result<()> {
        require!(total_amount > 0, KsspError::InvalidAmount);
        require!(end_time > start_time, KsspError::InvalidTimeRange);
        require!(
            milestone_count as usize == milestone_allocations.len(),
            KsspError::MilestoneMismatch
        );
        require!(
            milestone_count as usize == milestone_hashes.len(),
            KsspError::MilestoneMismatch
        );

        let total_milestone_funds: u64 = milestone_allocations.iter().sum();
        require!(
            total_milestone_funds <= total_amount,
            KsspError::InvalidMilestoneAllocation
        );

        let stream = &mut ctx.accounts.stream;
        stream.sender = ctx.accounts.sender.key();
        stream.recipient = ctx.accounts.recipient.key();
        stream.arbitrator = ctx.accounts.arbitrator.key();
        stream.token_mint = ctx.accounts.token_mint.key();
        stream.escrow_vault = ctx.accounts.escrow_vault.key();
        stream.stream_id = stream_id;
        stream.total_amount = total_amount;
        stream.claimed_amount = 0;
        stream.start_time = start_time;
        stream.end_time = end_time;
        stream.is_active = true;
        stream.is_disputed = false;
        stream.milestone_count = milestone_count;
        stream.completed_milestones = 0;
        stream.bump = ctx.bumps.stream;
        stream.vault_bump = ctx.bumps.escrow_vault;

        // Populate milestone arrays up to max 10
        require!(milestone_count <= 10, KsspError::TooManyMilestones);
        let mut allocations = [0u64; 10];
        let mut hashes = [[0u8; 32]; 10];
        let mut statuses = [false; 10];

        for i in 0..milestone_count as usize {
            allocations[i] = milestone_allocations[i];
            hashes[i] = milestone_hashes[i];
            statuses[i] = false;
        }

        stream.milestone_allocations = allocations;
        stream.milestone_hashes = hashes;
        stream.milestone_completed = statuses;

        // Transfer funds from sender to escrow vault via CPI
        let cpi_accounts = Transfer {
            from: ctx.accounts.sender_token_account.to_account_info(),
            to: ctx.accounts.escrow_vault.to_account_info(),
            authority: ctx.accounts.sender.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, total_amount)?;

        emit!(StreamCreatedEvent {
            stream_id,
            sender: stream.sender,
            recipient: stream.recipient,
            total_amount,
            start_time,
            end_time,
            milestones: milestone_count,
        });

        Ok(())
    }

    /// Complete and verify a milestone deliverable hash, unlocking its designated funds.
    pub fn unlock_milestone(
        ctx: Context<UnlockMilestone>,
        milestone_index: u8,
        proof_hash: [u8; 32],
    ) -> Result<()> {
        let stream = &mut ctx.accounts.stream;
        require!(stream.is_active, KsspError::StreamInactive);
        require!(!stream.is_disputed, KsspError::StreamUnderDispute);
        require!(
            milestone_index < stream.milestone_count,
            KsspError::MilestoneOutOfBounds
        );
        require!(
            !stream.milestone_completed[milestone_index as usize],
            KsspError::MilestoneAlreadyCompleted
        );
        require!(
            stream.milestone_hashes[milestone_index as usize] == proof_hash,
            KsspError::InvalidProofHash
        );

        stream.milestone_completed[milestone_index as usize] = true;
        stream.completed_milestones = stream.completed_milestones.checked_add(1).unwrap();

        emit!(MilestoneUnlockedEvent {
            stream_id: stream.stream_id,
            milestone_index,
            proof_hash,
            amount_unlocked: stream.milestone_allocations[milestone_index as usize],
        });

        Ok(())
    }

    /// Withdraw unlocked linear vesting and verified milestone allocations.
    pub fn withdraw(ctx: Context<Withdraw>) -> Result<()> {
        let clock = Clock::get()?;
        let stream = &mut ctx.accounts.stream;

        require!(stream.is_active, KsspError::StreamInactive);
        require!(!stream.is_disputed, KsspError::StreamUnderDispute);

        let current_time = clock.unix_timestamp;
        let mut total_vested_milestones: u64 = 0;

        for i in 0..stream.milestone_count as usize {
            if stream.milestone_completed[i] {
                total_vested_milestones = total_vested_milestones
                    .checked_add(stream.milestone_allocations[i])
                    .ok_or(KsspError::MathOverflow)?;
            }
        }

        let total_milestone_pool: u64 = stream
            .milestone_allocations
            .iter()
            .take(stream.milestone_count as usize)
            .sum();

        let linear_pool = stream
            .total_amount
            .checked_sub(total_milestone_pool)
            .ok_or(KsspError::MathOverflow)?;

        let linear_vested = if current_time <= stream.start_time {
            0u64
        } else if current_time >= stream.end_time {
            linear_pool
        } else {
            let elapsed = (current_time - stream.start_time) as u64;
            let duration = (stream.end_time - stream.start_time) as u64;
            linear_pool
                .checked_mul(elapsed)
                .ok_or(KsspError::MathOverflow)?
                .checked_div(duration)
                .ok_or(KsspError::MathOverflow)?
        };

        let total_available = linear_vested
            .checked_add(total_vested_milestones)
            .ok_or(KsspError::MathOverflow)?;

        let withdrawable = total_available
            .checked_sub(stream.claimed_amount)
            .ok_or(KsspError::MathOverflow)?;

        require!(withdrawable > 0, KsspError::NothingToWithdraw);

        stream.claimed_amount = stream
            .claimed_amount
            .checked_add(withdrawable)
            .ok_or(KsspError::MathOverflow)?;

        // Seed signers for PDA vault transfer
        let sender_key = stream.sender;
        let stream_id_bytes = stream.stream_id.to_le_bytes();
        let signer_seeds: &[&[&[u8]]] = &[&[
            b"stream_vault",
            sender_key.as_ref(),
            stream_id_bytes.as_ref(),
            &[stream.vault_bump],
        ]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.escrow_vault.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.escrow_vault.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
        token::transfer(cpi_ctx, withdrawable)?;

        emit!(FundsWithdrawnEvent {
            stream_id: stream.stream_id,
            recipient: stream.recipient,
            amount: withdrawable,
            total_claimed: stream.claimed_amount,
        });

        // Close stream if fully finalized
        if stream.claimed_amount == stream.total_amount {
            stream.is_active = false;
        }

        Ok(())
    }

    /// Open dispute freezing claims pending arbitrator determination.
    pub fn raise_dispute(ctx: Context<RaiseDispute>) -> Result<()> {
        let stream = &mut ctx.accounts.stream;
        require!(stream.is_active, KsspError::StreamInactive);
        require!(!stream.is_disputed, KsspError::StreamUnderDispute);

        stream.is_disputed = true;

        emit!(DisputeOpenedEvent {
            stream_id: stream.stream_id,
            initiator: ctx.accounts.authority.key(),
        });

        Ok(())
    }

    /// Arbitrator settles dispute, dividing remaining escrow between sender and recipient.
    pub fn settle_dispute(
        ctx: Context<SettleDispute>,
        recipient_payout: u64,
        sender_refund: u64,
    ) -> Result<()> {
        let stream = &mut ctx.accounts.stream;
        require!(stream.is_active, KsspError::StreamInactive);
        require!(stream.is_disputed, KsspError::StreamNotUnderDispute);

        let remaining_in_vault = ctx.accounts.escrow_vault.amount;
        let total_settlement = recipient_payout
            .checked_add(sender_refund)
            .ok_or(KsspError::MathOverflow)?;

        require!(
            total_settlement == remaining_in_vault,
            KsspError::SettlementExceedsEscrow
        );

        let sender_key = stream.sender;
        let stream_id_bytes = stream.stream_id.to_le_bytes();
        let signer_seeds: &[&[&[u8]]] = &[&[
            b"stream_vault",
            sender_key.as_ref(),
            stream_id_bytes.as_ref(),
            &[stream.vault_bump],
        ]];

        let token_program = ctx.accounts.token_program.to_account_info();

        if recipient_payout > 0 {
            let cpi_recipient = Transfer {
                from: ctx.accounts.escrow_vault.to_account_info(),
                to: ctx.accounts.recipient_token_account.to_account_info(),
                authority: ctx.accounts.escrow_vault.to_account_info(),
            };
            let cpi_ctx =
                CpiContext::new_with_signer(token_program.clone(), cpi_recipient, signer_seeds);
            token::transfer(cpi_ctx, recipient_payout)?;
        }

        if sender_refund > 0 {
            let cpi_sender = Transfer {
                from: ctx.accounts.escrow_vault.to_account_info(),
                to: ctx.accounts.sender_token_account.to_account_info(),
                authority: ctx.accounts.escrow_vault.to_account_info(),
            };
            let cpi_ctx =
                CpiContext::new_with_signer(token_program.clone(), cpi_sender, signer_seeds);
            token::transfer(cpi_ctx, sender_refund)?;
        }

        // Close the escrow vault account returning lamports to sender
        let close_accounts = CloseAccount {
            account: ctx.accounts.escrow_vault.to_account_info(),
            destination: ctx.accounts.sender.to_account_info(),
            authority: ctx.accounts.escrow_vault.to_account_info(),
        };
        let close_ctx = CpiContext::new_with_signer(token_program, close_accounts, signer_seeds);
        token::close_account(close_ctx)?;

        stream.is_active = false;
        stream.is_disputed = false;

        emit!(DisputeSettledEvent {
            stream_id: stream.stream_id,
            recipient_amount: recipient_payout,
            sender_amount: sender_refund,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION STRUCTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(stream_id: u64)]
pub struct CreateStream<'info> {
    #[account(mut)]
    pub sender: Signer<'info>,

    /// CHECK: Recipient public key to receive funds
    pub recipient: AccountInfo<'info>,

    /// CHECK: Arbitrator trusted entity for milestone / dispute settlement
    pub arbitrator: AccountInfo<'info>,

    pub token_mint: Account<'info, anchor_spl::token::Mint>,

    #[account(
        mut,
        constraint = sender_token_account.mint == token_mint.key(),
        constraint = sender_token_account.owner == sender.key()
    )]
    pub sender_token_account: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = sender,
        space = StreamAccount::LEN,
        seeds = [b"stream", sender.key().as_ref(), stream_id.to_le_bytes().as_ref()],
        bump
    )]
    pub stream: Account<'info, StreamAccount>,

    #[account(
        init,
        payer = sender,
        seeds = [b"stream_vault", sender.key().as_ref(), stream_id.to_le_bytes().as_ref()],
        bump,
        token::mint = token_mint,
        token::authority = escrow_vault
    )]
    pub escrow_vault: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct UnlockMilestone<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        has_one = arbitrator @ KsspError::UnauthorizedArbitrator
    )]
    pub stream: Account<'info, StreamAccount>,
    pub arbitrator: Signer<'info>,
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub recipient: Signer<'info>,

    #[account(
        mut,
        has_one = recipient @ KsspError::UnauthorizedRecipient,
        has_one = escrow_vault @ KsspError::InvalidEscrowVault
    )]
    pub stream: Account<'info, StreamAccount>,

    #[account(
        mut,
        seeds = [b"stream_vault", stream.sender.as_ref(), stream.stream_id.to_le_bytes().as_ref()],
        bump = stream.vault_bump
    )]
    pub escrow_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = recipient_token_account.mint == stream.token_mint,
        constraint = recipient_token_account.owner == recipient.key()
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RaiseDispute<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        constraint = authority.key() == stream.sender || authority.key() == stream.recipient @ KsspError::UnauthorizedParty
    )]
    pub stream: Account<'info, StreamAccount>,
}

#[derive(Accounts)]
pub struct SettleDispute<'info> {
    pub arbitrator: Signer<'info>,

    #[account(
        mut,
        has_one = arbitrator @ KsspError::UnauthorizedArbitrator,
        has_one = escrow_vault @ KsspError::InvalidEscrowVault
    )]
    pub stream: Account<'info, StreamAccount>,

    /// CHECK: Sender address to receive lamports upon vault closure
    #[account(mut, address = stream.sender)]
    pub sender: AccountInfo<'info>,

    #[account(
        mut,
        seeds = [b"stream_vault", stream.sender.as_ref(), stream.stream_id.to_le_bytes().as_ref()],
        bump = stream.vault_bump
    )]
    pub escrow_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = recipient_token_account.mint == stream.token_mint,
        constraint = recipient_token_account.owner == stream.recipient
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = sender_token_account.mint == stream.token_mint,
        constraint = sender_token_account.owner == stream.sender
    )]
    pub sender_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// PROTOCOL STATE DEFINITIONS
// -----------------------------------------------------------------------------

#[account]
pub struct StreamAccount {
    pub sender: Pubkey,                   // 32
    pub recipient: Pubkey,                // 32
    pub arbitrator: Pubkey,               // 32
    pub token_mint: Pubkey,               // 32
    pub escrow_vault: Pubkey,             // 32
    pub stream_id: u64,                   // 8
    pub total_amount: u64,                // 8
    pub claimed_amount: u64,              // 8
    pub start_time: i64,                  // 8
    pub end_time: i64,                    // 8
    pub is_active: bool,                  // 1
    pub is_disputed: bool,                // 1
    pub milestone_count: u8,              // 1
    pub completed_milestones: u8,          // 1
    pub bump: u8,                         // 1
    pub vault_bump: u8,                   // 1
    pub milestone_allocations: [u64; 10], // 80 (8 * 10)
    pub milestone_hashes: [[u8; 32]; 10], // 320 (32 * 10)
    pub milestone_completed: [bool; 10],  // 10
}

impl StreamAccount {
    // 8 (discriminator) + 32*5 + 8*5 + 1*4 + 80 + 320 + 10 = 662 bytes
    pub const LEN: usize = 8 + 160 + 40 + 4 + 80 + 320 + 10 + 64; // with safety padding
}

// -----------------------------------------------------------------------------
// PROTOCOL EVENTS
// -----------------------------------------------------------------------------

#[event]
pub struct StreamCreatedEvent {
    pub stream_id: u64,
    pub sender: Pubkey,
    pub recipient: Pubkey,
    pub total_amount: u64,
    pub start_time: i64,
    pub end_time: i64,
    pub milestones: u8,
}

#[event]
pub struct MilestoneUnlockedEvent {
    pub stream_id: u64,
    pub milestone_index: u8,
    pub proof_hash: [u8; 32],
    pub amount_unlocked: u64,
}

#[event]
pub struct FundsWithdrawnEvent {
    pub stream_id: u64,
    pub recipient: Pubkey,
    pub amount: u64,
    pub total_claimed: u64,
}

#[event]
pub struct DisputeOpenedEvent {
    pub stream_id: u64,
    pub initiator: Pubkey,
}

#[event]
pub struct DisputeSettledEvent {
    pub stream_id: u64,
    pub recipient_amount: u64,
    pub sender_amount: u64,
}

// -----------------------------------------------------------------------------
// ERROR CODES
// -----------------------------------------------------------------------------

#[error_code]
pub mod KsspError {
    #[msg("Provided deposit amount must be greater than zero.")]
    InvalidAmount,
    #[msg("Stream end time must strictly be after start time.")]
    InvalidTimeRange,
    #[msg("Milestone configurations and hash arrays do not match length.")]
    MilestoneMismatch,
    #[msg("Sum of milestone allocations exceeds total stream amount.")]
    InvalidMilestoneAllocation,
    #[msg("Maximum supported milestones is 10.")]
    TooManyMilestones,
    #[msg("Stream is no longer active.")]
    StreamInactive,
    #[msg("Stream is currently under active dispute.")]
    StreamUnderDispute,
    #[msg("Stream is not currently flagged as disputed.")]
    StreamNotUnderDispute,
    #[msg("Specified milestone index is out of bounds.")]
    MilestoneOutOfBounds,
    #[msg("Milestone deliverable has already been validated and completed.")]
    MilestoneAlreadyCompleted,
    #[msg("Cryptographic milestone proof hash does not match contract registry.")]
    InvalidProofHash,
    #[msg("Signer is not the authorized arbitrator for this stream.")]
    UnauthorizedArbitrator,
    #[msg("Signer is not the designated recipient.")]
    UnauthorizedRecipient,
    #[msg("Only the stream sender or recipient can initiate a dispute.")]
    UnauthorizedParty,
    #[msg("Escrow vault account does not match expected PDA.")]
    InvalidEscrowVault,
    #[msg("Zero funds currently eligible for withdrawal.")]
    NothingToWithdraw,
    #[msg("Settlement distribution total must exactly match remaining vault balance.")]
    SettlementExceedsEscrow,
    #[msg("Arithmetic math overflow or underflow.")]
    MathOverflow,
}