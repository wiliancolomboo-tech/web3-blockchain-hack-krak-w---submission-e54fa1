use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("Krak111111111111111111111111111111111111111");

pub const MAX_MILESTONES: u8 = 10;
pub const ESCROW_SEED: &[u8] = b"krak_escrow";
pub const VAULT_SEED: &[u8] = b"krak_vault";

#[program]
pub mod kraksettle {
    use super::*;

    /// Initializes an enterprise escrow agreement with deterministic milestones.
    pub fn initialize_escrow(
        ctx: Context<InitializeEscrow>,
        escrow_id: u64,
        total_amount: u64,
        milestone_counts: u8,
        milestone_amounts: Vec<u64>,
        timeout_seconds: i64,
    ) -> Result<()> {
        require!(milestone_counts > 0, KrakError::ZeroMilestones);
        require!(
            milestone_counts <= MAX_MILESTONES,
            KrakError::ExceedsMaxMilestones
        );
        require!(
            milestone_amounts.len() == milestone_counts as usize,
            KrakError::MilestoneMismatch
        );

        let sum: u64 = milestone_amounts.iter().sum();
        require!(sum == total_amount, KrakError::InvalidAmountSum);

        let escrow = &mut ctx.accounts.escrow;
        let clock = Clock::get()?;

        escrow.client = ctx.accounts.client.key();
        escrow.recipient = ctx.accounts.recipient.key();
        escrow.mint = ctx.accounts.mint.key();
        escrow.escrow_id = escrow_id;
        escrow.total_amount = total_amount;
        escrow.remaining_amount = total_amount;
        escrow.milestone_counts = milestone_counts;
        escrow.current_milestone_index = 0;
        escrow.state = EscrowState::Active;
        escrow.bump = ctx.bumps.escrow;
        escrow.vault_bump = ctx.bumps.vault;
        escrow.created_at = clock.unix_timestamp;
        escrow.timeout_deadline = clock.unix_timestamp.checked_add(timeout_seconds)
            .ok_or(KrakError::NumericalOverflow)?;

        // Populate milestone structures
        for i in 0..milestone_counts as usize {
            escrow.milestones[i] = Milestone {
                amount: milestone_amounts[i],
                proof_hash: [0u8; 32],
                status: MilestoneStatus::Pending,
                submitted_at: 0,
                approved_at: 0,
            };
        }

        // Deposit initial funding from Client Token Account into Vault PDA
        let cpi_accounts = Transfer {
            from: ctx.accounts.client_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.client.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, total_amount)?;

        emit!(EscrowInitializedEvent {
            escrow_id,
            client: escrow.client,
            recipient: escrow.recipient,
            total_amount,
            milestones: milestone_counts,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Submits deliverable proof for the current pending milestone by the vendor.
    pub fn submit_milestone(
        ctx: Context<SubmitMilestone>,
        proof_hash: [u8; 32],
    ) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        let clock = Clock::get()?;

        require!(escrow.state == EscrowState::Active, KrakError::EscrowNotActive);
        let idx = escrow.current_milestone_index as usize;
        require!(idx < escrow.milestone_counts as usize, KrakError::AllMilestonesCompleted);

        let milestone = &mut escrow.milestones[idx];
        require!(
            milestone.status == MilestoneStatus::Pending,
            KrakError::InvalidMilestoneState
        );

        milestone.proof_hash = proof_hash;
        milestone.status = MilestoneStatus::Submitted;
        milestone.submitted_at = clock.unix_timestamp;

        emit!(MilestoneSubmittedEvent {
            escrow_id: escrow.escrow_id,
            milestone_index: idx as u8,
            proof_hash,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Approves the deliverable, transferring milestone funds to recipient via CPI.
    pub fn approve_and_release(ctx: Context<ApproveAndRelease>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        let clock = Clock::get()?;

        require!(escrow.state == EscrowState::Active, KrakError::EscrowNotActive);
        let idx = escrow.current_milestone_index as usize;
        require!(idx < escrow.milestone_counts as usize, KrakError::AllMilestonesCompleted);

        let milestone = &mut escrow.milestones[idx];
        require!(
            milestone.status == MilestoneStatus::Submitted,
            KrakError::MilestoneNotSubmitted
        );

        let payout_amount = milestone.amount;
        require!(
            escrow.remaining_amount >= payout_amount,
            KrakError::InsufficientVaultBalance
        );

        milestone.status = MilestoneStatus::Approved;
        milestone.approved_at = clock.unix_timestamp;
        escrow.remaining_amount = escrow
            .remaining_amount
            .checked_sub(payout_amount)
            .ok_or(KrakError::NumericalOverflow)?;
        escrow.current_milestone_index += 1;

        if escrow.current_milestone_index == escrow.milestone_counts {
            escrow.state = EscrowState::Completed;
        }

        // Seeds for signing PDA CPI transfer
        let escrow_id_bytes = escrow.escrow_id.to_le_bytes();
        let seeds = &[
            ESCROW_SEED,
            escrow.client.as_ref(),
            escrow_id_bytes.as_ref(),
            &[escrow.bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            authority: escrow.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, payout_amount)?;

        emit!(MilestoneApprovedEvent {
            escrow_id: escrow.escrow_id,
            milestone_index: idx as u8,
            amount: payout_amount,
            recipient: ctx.accounts.recipient_token_account.owner,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Freezes escrow into Disputed state in case of breach or unacceptable deliverable.
    pub fn raise_dispute(ctx: Context<RaiseDispute>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        let clock = Clock::get()?;

        require!(escrow.state == EscrowState::Active, KrakError::EscrowNotActive);
        escrow.state = EscrowState::Disputed;

        emit!(EscrowDisputedEvent {
            escrow_id: escrow.escrow_id,
            disputed_by: ctx.accounts.authority.key(),
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Enables full or partial refund to client if contract timeout has elapsed or during dispute.
    pub fn refund_timeout(ctx: Context<RefundTimeout>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        let clock = Clock::get()?;

        let is_timed_out = clock.unix_timestamp >= escrow.timeout_deadline;
        let is_disputed = escrow.state == EscrowState::Disputed;

        require!(is_timed_out || is_disputed, KrakError::RefundConditionsNotMet);
        require!(escrow.remaining_amount > 0, KrakError::NoRemainingFunds);

        let refund_amount = escrow.remaining_amount;
        escrow.remaining_amount = 0;
        escrow.state = EscrowState::Refunded;

        let escrow_id_bytes = escrow.escrow_id.to_le_bytes();
        let seeds = &[
            ESCROW_SEED,
            escrow.client.as_ref(),
            escrow_id_bytes.as_ref(),
            &[escrow.bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.client_token_account.to_account_info(),
            authority: escrow.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, refund_amount)?;

        emit!(EscrowRefundedEvent {
            escrow_id: escrow.escrow_id,
            refund_amount,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(escrow_id: u64)]
pub struct InitializeEscrow<'info> {
    #[account(mut)]
    pub client: Signer<'info>,

    /// CHECK: Recipient arbitrary address verified on milestone approval
    pub recipient: AccountInfo<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = client,
        space = 8 + EscrowAccount::INIT_SPACE,
        seeds = [ESCROW_SEED, client.key().as_ref(), escrow_id.to_le_bytes().as_ref()],
        bump
    )]
    pub escrow: Account<'info, EscrowAccount>,

    #[account(
        init,
        payer = client,
        seeds = [VAULT_SEED, escrow.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = escrow
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = client_token_account.mint == mint.key() @ KrakError::InvalidMint,
        constraint = client_token_account.owner == client.key() @ KrakError::InvalidTokenOwner
    )]
    pub client_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct SubmitMilestone<'info> {
    pub recipient: Signer<'info>,

    #[account(
        mut,
        seeds = [ESCROW_SEED, escrow.client.as_ref(), escrow.escrow_id.to_le_bytes().as_ref()],
        bump = escrow.bump,
        has_one = recipient @ KrakError::UnauthorizedRecipient
    )]
    pub escrow: Account<'info, EscrowAccount>,
}

#[derive(Accounts)]
pub struct ApproveAndRelease<'info> {
    #[account(mut)]
    pub client: Signer<'info>,

    #[account(
        mut,
        seeds = [ESCROW_SEED, client.key().as_ref(), escrow.escrow_id.to_le_bytes().as_ref()],
        bump = escrow.bump,
        has_one = client @ KrakError::UnauthorizedClient
    )]
    pub escrow: Account<'info, EscrowAccount>,

    #[account(
        mut,
        seeds = [VAULT_SEED, escrow.key().as_ref()],
        bump = escrow.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = recipient_token_account.owner == escrow.recipient @ KrakError::InvalidRecipientTokenOwner,
        constraint = recipient_token_account.mint == escrow.mint @ KrakError::InvalidMint
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RaiseDispute<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [ESCROW_SEED, escrow.client.as_ref(), escrow.escrow_id.to_le_bytes().as_ref()],
        bump = escrow.bump,
        constraint = authority.key() == escrow.client || authority.key() == escrow.recipient @ KrakError::UnauthorizedDisputeParty
    )]
    pub escrow: Account<'info, EscrowAccount>,
}

#[derive(Accounts)]
pub struct RefundTimeout<'info> {
    #[account(mut)]
    pub client: Signer<'info>,

    #[account(
        mut,
        seeds = [ESCROW_SEED, client.key().as_ref(), escrow.escrow_id.to_le_bytes().as_ref()],
        bump = escrow.bump,
        has_one = client @ KrakError::UnauthorizedClient
    )]
    pub escrow: Account<'info, EscrowAccount>,

    #[account(
        mut,
        seeds = [VAULT_SEED, escrow.key().as_ref()],
        bump = escrow.vault_bump
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = client_token_account.owner == client.key() @ KrakError::InvalidTokenOwner,
        constraint = client_token_account.mint == escrow.mint @ KrakError::InvalidMint
    )]
    pub client_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// STATE STRUCTURES
// -----------------------------------------------------------------------------

#[account]
pub struct EscrowAccount {
    pub client: Pubkey,
    pub recipient: Pubkey,
    pub mint: Pubkey,
    pub escrow_id: u64,
    pub total_amount: u64,
    pub remaining_amount: u64,
    pub milestone_counts: u8,
    pub current_milestone_index: u8,
    pub state: EscrowState,
    pub bump: u8,
    pub vault_bump: u8,
    pub created_at: i64,
    pub timeout_deadline: i64,
    pub milestones: [Milestone; 10],
}

impl EscrowAccount {
    pub const INIT_SPACE: usize = 32 // client
        + 32 // recipient
        + 32 // mint
        + 8  // escrow_id
        + 8  // total_amount
        + 8  // remaining_amount
        + 1  // milestone_counts
        + 1  // current_milestone_index
        + 1  // state
        + 1  // bump
        + 1  // vault_bump
        + 8  // created_at
        + 8  // timeout_deadline
        + (Milestone::SPACE * 10); // milestones fixed array
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub struct Milestone {
    pub amount: u64,
    pub proof_hash: [u8; 32],
    pub status: MilestoneStatus,
    pub submitted_at: i64,
    pub approved_at: i64,
}

impl Milestone {
    pub const SPACE: usize = 8 + 32 + 1 + 8 + 8;
}

impl Default for Milestone {
    fn default() -> Self {
        Self {
            amount: 0,
            proof_hash: [0u8; 32],
            status: MilestoneStatus::Pending,
            submitted_at: 0,
            approved_at: 0,
        }
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum EscrowState {
    Active,
    Completed,
    Disputed,
    Refunded,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum MilestoneStatus {
    Pending,
    Submitted,
    Approved,
}

// -----------------------------------------------------------------------------
// EVENTS
// -----------------------------------------------------------------------------

#[event]
pub struct EscrowInitializedEvent {
    pub escrow_id: u64,
    pub client: Pubkey,
    pub recipient: Pubkey,
    pub total_amount: u64,
    pub milestones: u8,
    pub timestamp: i64,
}

#[event]
pub struct MilestoneSubmittedEvent {
    pub escrow_id: u64,
    pub milestone_index: u8,
    pub proof_hash: [u8; 32],
    pub timestamp: i64,
}

#[event]
pub struct MilestoneApprovedEvent {
    pub escrow_id: u64,
    pub milestone_index: u8,
    pub amount: u64,
    pub recipient: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct EscrowDisputedEvent {
    pub escrow_id: u64,
    pub disputed_by: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct EscrowRefundedEvent {
    pub escrow_id: u64,
    pub refund_amount: u64,
    pub timestamp: i64,
}

// -----------------------------------------------------------------------------
// CUSTOM ERRORS
// -----------------------------------------------------------------------------

#[error_code]
pub enum KrakError {
    #[msg("Milestone count must be greater than zero.")]
    ZeroMilestones,
    #[msg("Milestone count exceeds maximum configured capacity of 10.")]
    ExceedsMaxMilestones,
    #[msg("Provided milestone amounts do not match milestone count.")]
    MilestoneMismatch,
    #[msg("Sum of milestone amounts does not equal total escrow amount.")]
    InvalidAmountSum,
    #[msg("Escrow is not in active state.")]
    EscrowNotActive,
    #[msg("All milestones have already been completed.")]
    AllMilestonesCompleted,
    #[msg("Milestone is not in pending state.")]
    InvalidMilestoneState,
    #[msg("Milestone deliverable has not been submitted yet.")]
    MilestoneNotSubmitted,
    #[msg("Vault does not hold sufficient balance for release.")]
    InsufficientVaultBalance,
    #[msg("Caller is not the designated recipient.")]
    UnauthorizedRecipient,
    #[msg("Caller is not the designated client authority.")]
    UnauthorizedClient,
    #[msg("Caller is not authorized to raise a dispute on this escrow.")]
    UnauthorizedDisputeParty,
    #[msg("Timeout conditions or dispute requirements not met for refund.")]
    RefundConditionsNotMet,
    #[msg("No remaining funds in escrow vault.")]
    NoRemainingFunds,
    #[msg("Invalid token mint account.")]
    InvalidMint,
    #[msg("Invalid token owner specified.")]
    InvalidTokenOwner,
    #[msg("Invalid recipient token owner specified.")]
    InvalidRecipientTokenOwner,
    #[msg("Mathematical overflow occurred.")]
    NumericalOverflow,
}