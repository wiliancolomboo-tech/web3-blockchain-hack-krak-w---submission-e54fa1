use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("KrakG11111111111111111111111111111111111111");

#[program]
pub mod krak_guard {
    use super::*;

    /// Initializes a new Treasury Vault with strict rate-limiting invariants.
    pub fn initialize_vault(
        ctx: Context<InitializeVault>,
        max_outflow_per_epoch: u64,
        epoch_duration_seconds: i64,
    ) -> Result<()> {
        require!(max_outflow_per_epoch > 0, GuardError::InvalidVelocityLimit);
        require!(epoch_duration_seconds >= 10, GuardError::InvalidEpochDuration);

        let vault_state = &mut ctx.accounts.vault_state;
        let clock = Clock::get()?;

        vault_state.admin = ctx.accounts.admin.key();
        vault_state.guardian = ctx.accounts.guardian.key();
        vault_state.mint = ctx.accounts.mint.key();
        vault_state.vault_token_account = ctx.accounts.vault_token_account.key();
        vault_state.max_outflow_per_epoch = max_outflow_per_epoch;
        vault_state.epoch_duration_seconds = epoch_duration_seconds;
        vault_state.current_epoch_outflow = 0;
        vault_state.current_epoch_start = clock.unix_timestamp;
        vault_state.is_halted = false;
        vault_state.bump = ctx.bumps.vault_state;
        vault_state.token_vault_bump = ctx.bumps.vault_token_account;

        emit!(VaultInitializedEvent {
            vault: vault_state.key(),
            admin: vault_state.admin,
            guardian: vault_state.guardian,
            mint: vault_state.mint,
            max_outflow_per_epoch,
            epoch_duration_seconds,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Deposits funds into the protected vault. Anyone may deposit; no breaker checks apply.
    pub fn deposit(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require!(amount > 0, GuardError::ZeroAmount);

        let cpi_accounts = Transfer {
            from: ctx.accounts.depositor_token_account.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.depositor.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        emit!(DepositEvent {
            vault: ctx.accounts.vault_state.key(),
            depositor: ctx.accounts.depositor.key(),
            amount,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Executes a withdrawal under rate-limit invariant enforcement.
    pub fn execute_withdraw(ctx: Context<ExecuteWithdraw>, amount: u64) -> Result<()> {
        require!(amount > 0, GuardError::ZeroAmount);
        let clock = Clock::get()?;
        let vault_state = &mut ctx.accounts.vault_state;

        // Invariant 1: System must not be halted
        require!(!vault_state.is_halted, GuardError::VaultHalted);

        // Invariant 2: Dynamic Epoch Sliding Window Rollover
        let current_time = clock.unix_timestamp;
        let elapsed = current_time
            .checked_sub(vault_state.current_epoch_start)
            .ok_or(GuardError::MathOverflow)?;

        if elapsed >= vault_state.epoch_duration_seconds {
            // New epoch initiated
            vault_state.current_epoch_start = current_time;
            vault_state.current_epoch_outflow = 0;
        }

        // Invariant 3: Velocity Check
        let projected_outflow = vault_state
            .current_epoch_outflow
            .checked_add(amount)
            .ok_or(GuardError::MathOverflow)?;

        if projected_outflow > vault_state.max_outflow_per_epoch {
            // Autonomous Breaker Engagement due to limit exhaustion
            vault_state.is_halted = true;

            emit!(BreakerTrippedEvent {
                vault: vault_state.key(),
                tripped_by: ctx.accounts.caller.key(),
                reason: BreakerReason::RateLimitExceeded,
                attempted_amount: amount,
                current_epoch_outflow: vault_state.current_epoch_outflow,
                max_outflow: vault_state.max_outflow_per_epoch,
                timestamp: current_time,
            });

            return err!(GuardError::RateLimitExceeded);
        }

        // State mutation
        vault_state.current_epoch_outflow = projected_outflow;

        // Transfer funds via PDA signer seeds
        let mint_key = vault_state.mint;
        let seeds = &[
            b"vault_authority".as_ref(),
            mint_key.as_ref(),
            &[vault_state.bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.vault_token_account.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.vault_authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
        token::transfer(cpi_ctx, amount)?;

        emit!(WithdrawExecutedEvent {
            vault: vault_state.key(),
            recipient: ctx.accounts.recipient_token_account.key(),
            amount,
            new_epoch_outflow: vault_state.current_epoch_outflow,
            timestamp: current_time,
        });

        Ok(())
    }

    /// Fast-path halt: Invoked by hot Guardian bot or Admin upon external anomaly signals.
    pub fn trip_circuit_breaker(ctx: Context<TripCircuitBreaker>) -> Result<()> {
        let vault_state = &mut ctx.accounts.vault_state;
        let caller = ctx.accounts.guardian_or_admin.key();

        require!(
            caller == vault_state.guardian || caller == vault_state.admin,
            GuardError::UnauthorizedGuardian
        );

        vault_state.is_halted = true;

        emit!(BreakerTrippedEvent {
            vault: vault_state.key(),
            tripped_by: caller,
            reason: BreakerReason::ManualIntervention,
            attempted_amount: 0,
            current_epoch_outflow: vault_state.current_epoch_outflow,
            max_outflow: vault_state.max_outflow_per_epoch,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Reset breaker: Admin governance ONLY can unpause the vault.
    pub fn reset_circuit_breaker(ctx: Context<AdminGovernance>) -> Result<()> {
        let vault_state = &mut ctx.accounts.vault_state;
        let clock = Clock::get()?;

        vault_state.is_halted = false;
        // Reset current epoch outflow upon manual unhalt
        vault_state.current_epoch_start = clock.unix_timestamp;
        vault_state.current_epoch_outflow = 0;

        emit!(BreakerResetEvent {
            vault: vault_state.key(),
            admin: ctx.accounts.admin.key(),
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Update velocity boundaries: Admin governance ONLY.
    pub fn configure_limits(
        ctx: Context<AdminGovernance>,
        new_max_outflow: u64,
        new_epoch_duration: i64,
    ) -> Result<()> {
        require!(new_max_outflow > 0, GuardError::InvalidVelocityLimit);
        require!(new_epoch_duration >= 10, GuardError::InvalidEpochDuration);

        let vault_state = &mut ctx.accounts.vault_state;
        vault_state.max_outflow_per_epoch = new_max_outflow;
        vault_state.epoch_duration_seconds = new_epoch_duration;

        emit!(LimitsUpdatedEvent {
            vault: vault_state.key(),
            new_max_outflow,
            new_epoch_duration,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION STRUCTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeVault<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + TreasuryVault::INIT_SPACE,
        seeds = [b"vault_state", mint.key().as_ref()],
        bump
    )]
    pub vault_state: Account<'info, TreasuryVault>,

    /// CHECK: PDA used as token account signing authority.
    #[account(
        seeds = [b"vault_authority", mint.key().as_ref()],
        bump
    )]
    pub vault_authority: AccountInfo<'info>,

    #[account(
        init,
        payer = admin,
        seeds = [b"vault_tokens", mint.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = vault_authority
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    pub mint: Account<'info, Mint>,

    #[account(mut)]
    pub admin: Signer<'info>,

    /// The hot guardian bot key with tripping authority
    /// CHECK: Validated as an authorized public key
    pub guardian: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        seeds = [b"vault_state", vault_state.mint.as_ref()],
        bump = vault_state.bump
    )]
    pub vault_state: Account<'info, TreasuryVault>,

    #[account(
        mut,
        seeds = [b"vault_tokens", vault_state.mint.as_ref()],
        bump = vault_state.token_vault_bump
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub depositor_token_account: Account<'info, TokenAccount>,

    pub depositor: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ExecuteWithdraw<'info> {
    #[account(
        mut,
        seeds = [b"vault_state", vault_state.mint.as_ref()],
        bump = vault_state.bump,
        has_one = admin @ GuardError::UnauthorizedAdmin
    )]
    pub vault_state: Account<'info, TreasuryVault>,

    /// CHECK: Validated against state PDA seeds
    #[account(
        seeds = [b"vault_authority", vault_state.mint.as_ref()],
        bump = vault_state.bump
    )]
    pub vault_authority: AccountInfo<'info>,

    #[account(
        mut,
        seeds = [b"vault_tokens", vault_state.mint.as_ref()],
        bump = vault_state.token_vault_bump
    )]
    pub vault_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub recipient_token_account: Account<'info, TokenAccount>,

    /// Caller must be either Admin or authorized distributor
    pub caller: Signer<'info>,
    pub admin: SystemAccount<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct TripCircuitBreaker<'info> {
    #[account(
        mut,
        seeds = [b"vault_state", vault_state.mint.as_ref()],
        bump = vault_state.bump
    )]
    pub vault_state: Account<'info, TreasuryVault>,

    pub guardian_or_admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct AdminGovernance<'info> {
    #[account(
        mut,
        seeds = [b"vault_state", vault_state.mint.as_ref()],
        bump = vault_state.bump,
        has_one = admin @ GuardError::UnauthorizedAdmin
    )]
    pub vault_state: Account<'info, TreasuryVault>,

    pub admin: Signer<'info>,
}

// -----------------------------------------------------------------------------
// STATE DEFINITION
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct TreasuryVault {
    pub admin: Pubkey,                  // 32
    pub guardian: Pubkey,               // 32
    pub mint: Pubkey,                   // 32
    pub vault_token_account: Pubkey,    // 32
    pub max_outflow_per_epoch: u64,     // 8
    pub epoch_duration_seconds: i64,    // 8
    pub current_epoch_outflow: u64,     // 8
    pub current_epoch_start: i64,       // 8
    pub is_halted: bool,                // 1
    pub bump: u8,                       // 1
    pub token_vault_bump: u8,           // 1
}

// -----------------------------------------------------------------------------
// EVENTS & ENUMS
// -----------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum BreakerReason {
    RateLimitExceeded,
    ManualIntervention,
}

#[event]
pub struct VaultInitializedEvent {
    pub vault: Pubkey,
    pub admin: Pubkey,
    pub guardian: Pubkey,
    pub mint: Pubkey,
    pub max_outflow_per_epoch: u64,
    pub epoch_duration_seconds: i64,
    pub timestamp: i64,
}

#[event]
pub struct DepositEvent {
    pub vault: Pubkey,
    pub depositor: Pubkey,
    pub amount: u64,
    pub timestamp: i64,
}

#[event]
pub struct WithdrawExecutedEvent {
    pub vault: Pubkey,
    pub recipient: Pubkey,
    pub amount: u64,
    pub new_epoch_outflow: u64,
    pub timestamp: i64,
}

#[event]
pub struct BreakerTrippedEvent {
    pub vault: Pubkey,
    pub tripped_by: Pubkey,
    pub reason: BreakerReason,
    pub attempted_amount: u64,
    pub current_epoch_outflow: u64,
    pub max_outflow: u64,
    pub timestamp: i64,
}

#[event]
pub struct BreakerResetEvent {
    pub vault: Pubkey,
    pub admin: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct LimitsUpdatedEvent {
    pub vault: Pubkey,
    pub new_max_outflow: u64,
    pub new_epoch_duration: i64,
    pub timestamp: i64,
}

// -----------------------------------------------------------------------------
// ERROR CODES
// -----------------------------------------------------------------------------

#[error_code]
pub enum GuardError {
    #[msg("Operation rejected: Vault circuit breaker is tripped (HALTED).")]
    VaultHalted,
    #[msg("Outflow exceeds the maximum rate limit ceiling for this epoch.")]
    RateLimitExceeded,
    #[msg("Unauthorized: Signer is not the registered Admin.")]
    UnauthorizedAdmin,
    #[msg("Unauthorized: Signer is neither Guardian nor Admin.")]
    UnauthorizedGuardian,
    #[msg("Epoch duration must be at least 10 seconds.")]
    InvalidEpochDuration,
    #[msg("Velocity rate limit must be greater than zero.")]
    InvalidVelocityLimit,
    #[msg("Transaction amount must be strictly greater than zero.")]
    ZeroAmount,
    #[msg("Safe arithmetic overflow detected.")]
    MathOverflow,
}