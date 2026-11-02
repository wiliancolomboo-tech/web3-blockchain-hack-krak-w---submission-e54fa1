use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("KrkwSentry1111111111111111111111111111111111");

pub const BASIS_POINTS_DIVISOR: u64 = 10_000;
pub const SLA_SEED: &[u8] = b"sla-contract";
pub const VAULT_SEED: &[u8] = b"sla-vault";

#[program]
pub mod krakow_sentry {
    use super::*;

    pub fn initialize_sla(
        ctx: Context<InitializeSLA>,
        sla_id: u64,
        payout_rate_per_epoch: u64,
        epoch_duration_seconds: i64,
        min_uptime_bps: u16,        // e.g. 9900 = 99.00%
        max_latency_ms: u32,        // e.g. 150 ms
        slash_penalty_bps: u16,     // e.g. 2000 = 20% penalty applied on breach
    ) -> Result<()> {
        require!(min_uptime_bps <= 10_000, SentryError::InvalidBasisPoints);
        require!(slash_penalty_bps <= 10_000, SentryError::InvalidBasisPoints);
        require!(epoch_duration_seconds > 0, SentryError::InvalidEpochDuration);
        require!(payout_rate_per_epoch > 0, SentryError::InvalidPayoutRate);

        let sla = &mut ctx.accounts.sla_contract;
        let clock = Clock::get()?;

        sla.sla_id = sla_id;
        sla.client = ctx.accounts.client.key();
        sla.provider = ctx.accounts.provider.key();
        sla.telemetry_oracle = ctx.accounts.telemetry_oracle.key();
        sla.token_mint = ctx.accounts.token_mint.key();
        sla.escrow_vault = ctx.accounts.escrow_vault.key();
        sla.payout_rate_per_epoch = payout_rate_per_epoch;
        sla.epoch_duration_seconds = epoch_duration_seconds;
        sla.min_uptime_bps = min_uptime_bps;
        sla.max_latency_ms = max_latency_ms;
        sla.slash_penalty_bps = slash_penalty_bps;
        sla.last_settled_timestamp = clock.unix_timestamp;
        sla.current_epoch = 0;
        sla.is_active = true;
        sla.bump = ctx.bumps.sla_contract;
        sla.vault_bump = ctx.bumps.escrow_vault;

        emit!(SLAInitializedEvent {
            sla_id,
            client: sla.client,
            provider: sla.provider,
            min_uptime_bps,
            max_latency_ms,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    pub fn deposit_escrow(ctx: Context<DepositEscrow>, amount: u64) -> Result<()> {
        require!(amount > 0, SentryError::ZeroDepositAmount);
        require!(ctx.accounts.sla_contract.is_active, SentryError::SLANotActive);

        let transfer_cpi = Transfer {
            from: ctx.accounts.client_token_account.to_account_info(),
            to: ctx.accounts.escrow_vault.to_account_info(),
            authority: ctx.accounts.client.to_account_info(),
        };

        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer(CpiContext::new(cpi_program, transfer_cpi), amount)?;

        emit!(EscrowFundedEvent {
            sla_id: ctx.accounts.sla_contract.sla_id,
            client: ctx.accounts.client.key(),
            amount,
            total_vault_balance: ctx.accounts.escrow_vault.amount.saturating_add(amount),
        });

        Ok(())
    }

    pub fn submit_telemetry_and_settle(
        ctx: Context<SubmitTelemetryAndSettle>,
        epoch_index: u64,
        measured_uptime_bps: u16,
        measured_latency_ms: u32,
    ) -> Result<()> {
        let sla = &mut ctx.accounts.sla_contract;
        let clock = Clock::get()?;

        require!(sla.is_active, SentryError::SLANotActive);
        require!(
            epoch_index == sla.current_epoch.checked_add(1).ok_or(SentryError::MathOverflow)?,
            SentryError::InvalidEpochSequence
        );

        let time_elapsed = clock.unix_timestamp.checked_sub(sla.last_settled_timestamp)
            .ok_or(SentryError::MathOverflow)?;
        require!(
            time_elapsed >= sla.epoch_duration_seconds,
            SentryError::EpochDurationNotMet
        );

        let is_uptime_breached = measured_uptime_bps < sla.min_uptime_bps;
        let is_latency_breached = measured_latency_ms > sla.max_latency_ms;
        let is_breached = is_uptime_breached || is_latency_breached;

        let available_balance = ctx.accounts.escrow_vault.amount;
        let payout_amount = std::cmp::min(sla.payout_rate_per_epoch, available_balance);

        let sla_id_bytes = sla.sla_id.to_le_bytes();
        let signer_seeds: &[&[&[u8]]] = &[&[
            SLA_SEED,
            sla_id_bytes.as_ref(),
            &[sla.bump],
        ]];

        if is_breached {
            // Calculate refund penalty from payout amount
            let penalty_amount = (payout_amount as u128)
                .checked_mul(sla.slash_penalty_bps as u128)
                .ok_or(SentryError::MathOverflow)?
                .checked_div(BASIS_POINTS_DIVISOR as u128)
                .ok_or(SentryError::MathOverflow)? as u64;

            let earned_provider_amount = payout_amount.saturating_sub(penalty_amount);

            // Refund penalty component back to enterprise client
            if penalty_amount > 0 {
                let transfer_to_client = Transfer {
                    from: ctx.accounts.escrow_vault.to_account_info(),
                    to: ctx.accounts.client_token_account.to_account_info(),
                    authority: ctx.accounts.sla_contract.to_account_info(),
                };
                token::transfer(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        transfer_to_client,
                        signer_seeds,
                    ),
                    penalty_amount,
                )?;
            }

            // Transfer remainder to provider
            if earned_provider_amount > 0 {
                let transfer_to_provider = Transfer {
                    from: ctx.accounts.escrow_vault.to_account_info(),
                    to: ctx.accounts.provider_token_account.to_account_info(),
                    authority: ctx.accounts.sla_contract.to_account_info(),
                };
                token::transfer(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        transfer_to_provider,
                        signer_seeds,
                    ),
                    earned_provider_amount,
                )?;
            }

            emit!(SLABreachedEvent {
                sla_id: sla.sla_id,
                epoch_index,
                measured_uptime_bps,
                measured_latency_ms,
                penalty_slashed_to_client: penalty_amount,
                provider_received: earned_provider_amount,
                timestamp: clock.unix_timestamp,
            });
        } else {
            // SLA Honored in full
            if payout_amount > 0 {
                let transfer_to_provider = Transfer {
                    from: ctx.accounts.escrow_vault.to_account_info(),
                    to: ctx.accounts.provider_token_account.to_account_info(),
                    authority: ctx.accounts.sla_contract.to_account_info(),
                };
                token::transfer(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        transfer_to_provider,
                        signer_seeds,
                    ),
                    payout_amount,
                )?;
            }

            emit!(SLASettledCleanEvent {
                sla_id: sla.sla_id,
                epoch_index,
                measured_uptime_bps,
                measured_latency_ms,
                payout_amount,
                timestamp: clock.unix_timestamp,
            });
        }

        sla.current_epoch = epoch_index;
        sla.last_settled_timestamp = clock.unix_timestamp;

        Ok(())
    }

    pub fn terminate_sla(ctx: Context<TerminateSLA>) -> Result<()> {
        let sla = &mut ctx.accounts.sla_contract;
        require!(sla.is_active, SentryError::SLANotActive);

        let remaining_vault_balance = ctx.accounts.escrow_vault.amount;
        let sla_id_bytes = sla.sla_id.to_le_bytes();
        let signer_seeds: &[&[&[u8]]] = &[&[
            SLA_SEED,
            sla_id_bytes.as_ref(),
            &[sla.bump],
        ]];

        if remaining_vault_balance > 0 {
            let refund_transfer = Transfer {
                from: ctx.accounts.escrow_vault.to_account_info(),
                to: ctx.accounts.client_token_account.to_account_info(),
                authority: ctx.accounts.sla_contract.to_account_info(),
            };
            token::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    refund_transfer,
                    signer_seeds,
                ),
                remaining_vault_balance,
            )?;
        }

        sla.is_active = false;

        emit!(SLATerminatedEvent {
            sla_id: sla.sla_id,
            remaining_refunded: remaining_vault_balance,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
#[instruction(sla_id: u64)]
pub struct InitializeSLA<'info> {
    #[account(
        init,
        payer = client,
        space = 8 + SLAContract::INIT_SPACE,
        seeds = [SLA_SEED, sla_id.to_le_bytes().as_ref()],
        bump
    )]
    pub sla_contract: Account<'info, SLAContract>,

    #[account(
        init,
        payer = client,
        seeds = [VAULT_SEED, sla_contract.key().as_ref()],
        bump,
        token::mint = token_mint,
        token::authority = sla_contract
    )]
    pub escrow_vault: Account<'info, TokenAccount>,

    pub token_mint: Account<'info, Mint>,

    #[account(mut)]
    pub client: Signer<'info>,

    /// CHECK: The provider public key to receive operational rewards
    pub provider: AccountInfo<'info>,

    /// CHECK: Public key of the verified telemetry oracle agent
    pub telemetry_oracle: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct DepositEscrow<'info> {
    #[account(
        mut,
        has_one = client @ SentryError::UnauthorizedClient,
        has_one = escrow_vault @ SentryError::InvalidVault
    )]
    pub sla_contract: Account<'info, SLAContract>,

    #[account(mut)]
    pub escrow_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = client_token_account.owner == client.key() @ SentryError::InvalidTokenOwner,
        constraint = client_token_account.mint == sla_contract.token_mint @ SentryError::InvalidMint
    )]
    pub client_token_account: Account<'info, TokenAccount>,

    pub client: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct SubmitTelemetryAndSettle<'info> {
    #[account(
        mut,
        has_one = telemetry_oracle @ SentryError::UnauthorizedOracle,
        has_one = escrow_vault @ SentryError::InvalidVault
    )]
    pub sla_contract: Account<'info, SLAContract>,

    pub telemetry_oracle: Signer<'info>,

    #[account(mut)]
    pub escrow_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = client_token_account.owner == sla_contract.client @ SentryError::InvalidTokenOwner,
        constraint = client_token_account.mint == sla_contract.token_mint @ SentryError::InvalidMint
    )]
    pub client_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = provider_token_account.owner == sla_contract.provider @ SentryError::InvalidTokenOwner,
        constraint = provider_token_account.mint == sla_contract.token_mint @ SentryError::InvalidMint
    )]
    pub provider_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct TerminateSLA<'info> {
    #[account(
        mut,
        has_one = client @ SentryError::UnauthorizedClient,
        has_one = escrow_vault @ SentryError::InvalidVault
    )]
    pub sla_contract: Account<'info, SLAContract>,

    pub client: Signer<'info>,

    #[account(mut)]
    pub escrow_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = client_token_account.owner == client.key() @ SentryError::InvalidTokenOwner,
        constraint = client_token_account.mint == sla_contract.token_mint @ SentryError::InvalidMint
    )]
    pub client_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// STATE STRUCTURES
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct SLAContract {
    pub sla_id: u64,
    pub client: Pubkey,
    pub provider: Pubkey,
    pub telemetry_oracle: Pubkey,
    pub token_mint: Pubkey,
    pub escrow_vault: Pubkey,
    pub payout_rate_per_epoch: u64,
    pub epoch_duration_seconds: i64,
    pub last_settled_timestamp: i64,
    pub current_epoch: u64,
    pub min_uptime_bps: u16,
    pub max_latency_ms: u32,
    pub slash_penalty_bps: u16,
    pub is_active: bool,
    pub bump: u8,
    pub vault_bump: u8,
}

// -----------------------------------------------------------------------------
// EVENTS
// -----------------------------------------------------------------------------

#[event]
pub struct SLAInitializedEvent {
    pub sla_id: u64,
    pub client: Pubkey,
    pub provider: Pubkey,
    pub min_uptime_bps: u16,
    pub max_latency_ms: u32,
    pub timestamp: i64,
}

#[event]
pub struct EscrowFundedEvent {
    pub sla_id: u64,
    pub client: Pubkey,
    pub amount: u64,
    pub total_vault_balance: u64,
}

#[event]
pub struct SLASettledCleanEvent {
    pub sla_id: u64,
    pub epoch_index: u64,
    pub measured_uptime_bps: u16,
    pub measured_latency_ms: u32,
    pub payout_amount: u64,
    pub timestamp: i64,
}

#[event]
pub struct SLABreachedEvent {
    pub sla_id: u64,
    pub epoch_index: u64,
    pub measured_uptime_bps: u16,
    pub measured_latency_ms: u32,
    pub penalty_slashed_to_client: u64,
    pub provider_received: u64,
    pub timestamp: i64,
}

#[event]
pub struct SLATerminatedEvent {
    pub sla_id: u64,
    pub remaining_refunded: u64,
    pub timestamp: i64,
}

// -----------------------------------------------------------------------------
// ERROR CODES
// -----------------------------------------------------------------------------

#[error_code]
pub enum SentryError {
    #[msg("Value must not exceed 10000 basis points.")]
    InvalidBasisPoints,
    #[msg("Epoch duration must be greater than zero.")]
    InvalidEpochDuration,
    #[msg("Payout rate per epoch must be greater than zero.")]
    InvalidPayoutRate,
    #[msg("Deposit amount must be greater than zero.")]
    ZeroDepositAmount,
    #[msg("SLA contract is not active.")]
    SLANotActive,
    #[msg("Signer is not authorized client.")]
    UnauthorizedClient,
    #[msg("Signer is not authorized telemetry oracle.")]
    UnauthorizedOracle,
    #[msg("Specified vault account does not match SLA registration.")]
    InvalidVault,
    #[msg("Token account owner mismatch.")]
    InvalidTokenOwner,
    #[msg("Token account mint mismatch.")]
    InvalidMint,
    #[msg("Epoch sequence must be strictly sequential (n+1).")]
    InvalidEpochSequence,
    #[msg("Required epoch duration has not elapsed yet.")]
    EpochDurationNotMet,
    #[msg("Arithmetic overflow occurred.")]
    MathOverflow,
}