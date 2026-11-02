use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

declare_id!("Aegis111111111111111111111111111111111111111");

pub const MAX_INVOICE_ID_LEN: usize = 32;
pub const BASIS_POINTS_DIVISOR: u64 = 10_000;

#[program]
pub mod aegis_settlement {
    use super::*;

    /// Initializes global protocol configuration and platform fee recipient.
    pub fn initialize_protocol(
        ctx: Context<InitializeProtocol>,
        fee_basis_points: u16,
    ) -> Result<()> {
        require!(
            fee_basis_points <= 500,
            AegisError::InvalidFeeBasisPoints
        ); // Max 5.00%

        let protocol_config = &mut ctx.accounts.protocol_config;
        protocol_config.authority = ctx.accounts.authority.key();
        protocol_config.fee_recipient = ctx.accounts.fee_recipient.key();
        protocol_config.fee_basis_points = fee_basis_points;
        protocol_config.is_paused = false;
        protocol_config.bump = ctx.bumps.protocol_config;

        emit!(ProtocolInitializedEvent {
            authority: protocol_config.authority,
            fee_recipient: protocol_config.fee_recipient,
            fee_basis_points,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Registers a new commercial invoice under a deterministic PDA.
    pub fn create_invoice(
        ctx: Context<CreateInvoice>,
        invoice_id: String,
        amount: u64,
        advance_rate_bps: u16,
        due_timestamp: i64,
        invoice_hash: [u8; 32],
    ) -> Result<()> {
        let protocol_config = &ctx.accounts.protocol_config;
        require!(!protocol_config.is_paused, AegisError::ProtocolPaused);
        require!(
            invoice_id.len() > 0 && invoice_id.len() <= MAX_INVOICE_ID_LEN,
            AegisError::InvalidInvoiceIdLength
        );
        require!(amount > 0, AegisError::ZeroAmount);
        require!(
            advance_rate_bps <= BASIS_POINTS_DIVISOR as u16,
            AegisError::InvalidAdvanceRate
        );

        let clock = Clock::get()?;
        require!(
            due_timestamp > clock.unix_timestamp,
            AegisError::InvalidDueTimestamp
        );

        let invoice = &mut ctx.accounts.invoice;
        invoice.vendor = ctx.accounts.vendor.key();
        invoice.payer = ctx.accounts.payer.key();
        invoice.mint = ctx.accounts.mint.key();
        invoice.invoice_id = invoice_id.clone();
        invoice.amount = amount;
        invoice.advance_rate_bps = advance_rate_bps;
        invoice.funded_amount = 0;
        invoice.due_timestamp = due_timestamp;
        invoice.invoice_hash = invoice_hash;
        invoice.factor_lp = Pubkey::default();
        invoice.is_factored = false;
        invoice.is_settled = false;
        invoice.is_disputed = false;
        invoice.bump = ctx.bumps.invoice;
        invoice.vault_bump = ctx.bumps.vault_account;

        emit!(InvoiceCreatedEvent {
            invoice_pubkey: invoice.key(),
            vendor: invoice.vendor,
            payer: invoice.payer,
            mint: invoice.mint,
            invoice_id,
            amount,
            advance_rate_bps,
            due_timestamp,
            invoice_hash,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Liquidity Provider purchases the invoice receivable at a pre-agreed advance rate.
    pub fn factor_invoice(ctx: Context<FactorInvoice>) -> Result<()> {
        let protocol_config = &ctx.accounts.protocol_config;
        require!(!protocol_config.is_paused, AegisError::ProtocolPaused);

        let invoice = &mut ctx.accounts.invoice;
        require!(!invoice.is_factored, AegisError::AlreadyFactored);
        require!(!invoice.is_settled, AegisError::AlreadySettled);
        require!(!invoice.is_disputed, AegisError::InvoiceInDispute);

        let clock = Clock::get()?;
        require!(
            clock.unix_timestamp < invoice.due_timestamp,
            AegisError::InvoiceExpired
        );

        // Calculate upfront advance amount based on basis points
        let advance_amount = (invoice.amount as u128)
            .checked_mul(invoice.advance_rate_bps as u128)
            .ok_or(AegisError::NumericalOverflow)?
            .checked_div(BASIS_POINTS_DIVISOR as u128)
            .ok_or(AegisError::NumericalOverflow)? as u64;

        require!(advance_amount > 0, AegisError::ZeroAmount);

        // Protocol fee on factoring advance
        let protocol_fee = (advance_amount as u128)
            .checked_mul(protocol_config.fee_basis_points as u128)
            .ok_or(AegisError::NumericalOverflow)?
            .checked_div(BASIS_POINTS_DIVISOR as u128)
            .ok_or(AegisError::NumericalOverflow)? as u64;

        let net_vendor_payout = advance_amount
            .checked_sub(protocol_fee)
            .ok_or(AegisError::NumericalOverflow)?;

        let decimals = ctx.accounts.mint.decimals;

        // 1. Transfer net advance from LP directly to Vendor token account
        let cpi_accounts_vendor = TransferChecked {
            from: ctx.accounts.factor_token_account.to_account_info(),
            mint: ctx.accounts.mint.to_account_info(),
            to: ctx.accounts.vendor_token_account.to_account_info(),
            authority: ctx.accounts.factor_lp.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        token::transfer_checked(
            CpiContext::new(cpi_program.clone(), cpi_accounts_vendor),
            net_vendor_payout,
            decimals,
        )?;

        // 2. Transfer protocol fee from LP to protocol fee recipient if fee > 0
        if protocol_fee > 0 {
            let cpi_accounts_fee = TransferChecked {
                from: ctx.accounts.factor_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.protocol_fee_token_account.to_account_info(),
                authority: ctx.accounts.factor_lp.to_account_info(),
            };
            token::transfer_checked(
                CpiContext::new(cpi_program, cpi_accounts_fee),
                protocol_fee,
                decimals,
            )?;
        }

        invoice.is_factored = true;
        invoice.factor_lp = ctx.accounts.factor_lp.key();
        invoice.funded_amount = advance_amount;

        emit!(InvoiceFactoredEvent {
            invoice_pubkey: invoice.key(),
            factor_lp: invoice.factor_lp,
            advance_amount,
            protocol_fee,
            net_vendor_payout,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    /// Payer or Vendor settles invoice principal into the escrow vault or directly to the LP.
    pub fn settle_invoice(ctx: Context<SettleInvoice>) -> Result<()> {
        let protocol_config = &ctx.accounts.protocol_config;
        require!(!protocol_config.is_paused, AegisError::ProtocolPaused);

        let invoice = &mut ctx.accounts.invoice;
        require!(!invoice.is_settled, AegisError::AlreadySettled);
        require!(!invoice.is_disputed, AegisError::InvoiceInDispute);

        let total_amount = invoice.amount;
        let decimals = ctx.accounts.mint.decimals;
        let cpi_program = ctx.accounts.token_program.to_account_info();

        if invoice.is_factored {
            // Factor LP funded the invoice: payer pays full amount to Factor LP
            let cpi_accounts_lp = TransferChecked {
                from: ctx.accounts.payer_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.factor_token_account.to_account_info(),
                authority: ctx.accounts.payer.to_account_info(),
            };
            token::transfer_checked(
                CpiContext::new(cpi_program, cpi_accounts_lp),
                total_amount,
                decimals,
            )?;
        } else {
            // Invoice was unfactored: payer pays vendor full amount directly
            let cpi_accounts_vendor = TransferChecked {
                from: ctx.accounts.payer_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.vendor_token_account.to_account_info(),
                authority: ctx.accounts.payer.to_account_info(),
            };
            token::transfer_checked(
                CpiContext::new(cpi_program, cpi_accounts_vendor),
                total_amount,
                decimals,
            )?;
        }

        invoice.is_settled = true;

        emit!(InvoiceSettledEvent {
            invoice_pubkey: invoice.key(),
            payer: ctx.accounts.payer.key(),
            recipient: if invoice.is_factored {
                invoice.factor_lp
            } else {
                invoice.vendor
            },
            amount: total_amount,
            was_factored: invoice.is_factored,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Payer or Vendor flags dispute, freezing token flows until arbitrated.
    pub fn raise_dispute(ctx: Context<RaiseDispute>, reason: String) -> Result<()> {
        let invoice = &mut ctx.accounts.invoice;
        require!(!invoice.is_settled, AegisError::AlreadySettled);
        require!(!invoice.is_disputed, AegisError::AlreadyDisputed);

        invoice.is_disputed = true;

        emit!(InvoiceDisputedEvent {
            invoice_pubkey: invoice.key(),
            disputed_by: ctx.accounts.authority.key(),
            reason,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Protocol Governance resolves dispute, releasing hold.
    pub fn resolve_dispute(ctx: Context<ResolveDispute>, resume_settlement: bool) -> Result<()> {
        let invoice = &mut ctx.accounts.invoice;
        require!(invoice.is_disputed, AegisError::NotInDispute);

        invoice.is_disputed = false;
        if !resume_settlement {
            invoice.is_settled = true; // Forcibly canceled/liquidated
        }

        emit!(InvoiceDisputeResolvedEvent {
            invoice_pubkey: invoice.key(),
            resume_settlement,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION STRUCTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeProtocol<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + ProtocolConfig::LEN,
        seeds = [b"protocol_config"],
        bump
    )]
    pub protocol_config: Account<'info, ProtocolConfig>,

    #[account(mut)]
    pub authority: Signer<'info>,

    /// CHECK: Safe receiving address for protocol governance fees
    pub fee_recipient: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(invoice_id: String)]
pub struct CreateInvoice<'info> {
    #[account(
        seeds = [b"protocol_config"],
        bump = protocol_config.bump
    )]
    pub protocol_config: Account<'info, ProtocolConfig>,

    #[account(
        init,
        payer = vendor,
        space = 8 + Invoice::LEN,
        seeds = [
            b"invoice",
            vendor.key().as_ref(),
            payer.key().as_ref(),
            invoice_id.as_bytes()
        ],
        bump
    )]
    pub invoice: Account<'info, Invoice>,

    #[account(mut)]
    pub vendor: Signer<'info>,

    /// CHECK: Payer public key identifying invoice obligor
    pub payer: UncheckedAccount<'info>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = vendor,
        seeds = [b"vault", invoice.key().as_ref()],
        bump,
        token::mint = mint,
        token::authority = invoice
    )]
    pub vault_account: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct FactorInvoice<'info> {
    #[account(
        seeds = [b"protocol_config"],
        bump = protocol_config.bump
    )]
    pub protocol_config: Account<'info, ProtocolConfig>,

    #[account(
        mut,
        seeds = [
            b"invoice",
            invoice.vendor.as_ref(),
            invoice.payer.as_ref(),
            invoice.invoice_id.as_bytes()
        ],
        bump = invoice.bump,
        has_one = mint @ AegisError::InvalidMintMatch,
        has_one = vendor @ AegisError::InvalidVendorAccount
    )]
    pub invoice: Account<'info, Invoice>,

    pub mint: Account<'info, Mint>,

    #[account(mut)]
    pub factor_lp: Signer<'info>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = factor_lp
    )]
    pub factor_token_account: Account<'info, TokenAccount>,

    /// CHECK: Verified by has_one on invoice
    pub vendor: UncheckedAccount<'info>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = vendor
    )]
    pub vendor_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = protocol_config.fee_recipient
    )]
    pub protocol_fee_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct SettleInvoice<'info> {
    #[account(
        seeds = [b"protocol_config"],
        bump = protocol_config.bump
    )]
    pub protocol_config: Account<'info, ProtocolConfig>,

    #[account(
        mut,
        seeds = [
            b"invoice",
            invoice.vendor.as_ref(),
            invoice.payer.as_ref(),
            invoice.invoice_id.as_bytes()
        ],
        bump = invoice.bump,
        has_one = mint @ AegisError::InvalidMintMatch,
        has_one = payer @ AegisError::InvalidPayerAccount
    )]
    pub invoice: Account<'info, Invoice>,

    pub mint: Account<'info, Mint>,

    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        mut,
        token::mint = mint,
        token::authority = payer
    )]
    pub payer_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = mint
    )]
    pub vendor_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = mint
    )]
    pub factor_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RaiseDispute<'info> {
    #[account(
        mut,
        seeds = [
            b"invoice",
            invoice.vendor.as_ref(),
            invoice.payer.as_ref(),
            invoice.invoice_id.as_bytes()
        ],
        bump = invoice.bump
    )]
    pub invoice: Account<'info, Invoice>,

    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct ResolveDispute<'info> {
    #[account(
        seeds = [b"protocol_config"],
        bump = protocol_config.bump,
        has_one = authority @ AegisError::UnauthorizedProtocolAdmin
    )]
    pub protocol_config: Account<'info, ProtocolConfig>,

    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [
            b"invoice",
            invoice.vendor.as_ref(),
            invoice.payer.as_ref(),
            invoice.invoice_id.as_bytes()
        ],
        bump = invoice.bump
    )]
    pub invoice: Account<'info, Invoice>,
}

// -----------------------------------------------------------------------------
// DATA LAYOUTS
// -----------------------------------------------------------------------------

#[account]
pub struct ProtocolConfig {
    pub authority: Pubkey,         // 32
    pub fee_recipient: Pubkey,     // 32
    pub fee_basis_points: u16,     // 2
    pub is_paused: bool,           // 1
    pub bump: u8,                  // 1
}

impl ProtocolConfig {
    pub const LEN: usize = 32 + 32 + 2 + 1 + 1;
}

#[account]
pub struct Invoice {
    pub vendor: Pubkey,            // 32
    pub payer: Pubkey,             // 32
    pub mint: Pubkey,              // 32
    pub invoice_id: String,        // 4 + 32
    pub amount: u64,               // 8
    pub advance_rate_bps: u16,     // 2
    pub funded_amount: u64,        // 8
    pub due_timestamp: i64,        // 8
    pub invoice_hash: [u8; 32],    // 32
    pub factor_lp: Pubkey,         // 32
    pub is_factored: bool,         // 1
    pub is_settled: bool,          // 1
    pub is_disputed: bool,         // 1
    pub bump: u8,                  // 1
    pub vault_bump: u8,            // 1
}

impl Invoice {
    pub const LEN: usize = 32 + 32 + 32 + (4 + MAX_INVOICE_ID_LEN) + 8 + 2 + 8 + 8 + 32 + 32 + 1 + 1 + 1 + 1 + 1;
}

// -----------------------------------------------------------------------------
// EVENTS
// -----------------------------------------------------------------------------

#[event]
pub struct ProtocolInitializedEvent {
    pub authority: Pubkey,
    pub fee_recipient: Pubkey,
    pub fee_basis_points: u16,
    pub timestamp: i64,
}

#[event]
pub struct InvoiceCreatedEvent {
    pub invoice_pubkey: Pubkey,
    pub vendor: Pubkey,
    pub payer: Pubkey,
    pub mint: Pubkey,
    pub invoice_id: String,
    pub amount: u64,
    pub advance_rate_bps: u16,
    pub due_timestamp: i64,
    pub invoice_hash: [u8; 32],
    pub timestamp: i64,
}

#[event]
pub struct InvoiceFactoredEvent {
    pub invoice_pubkey: Pubkey,
    pub factor_lp: Pubkey,
    pub advance_amount: u64,
    pub protocol_fee: u64,
    pub net_vendor_payout: u64,
    pub timestamp: i64,
}

#[event]
pub struct InvoiceSettledEvent {
    pub invoice_pubkey: Pubkey,
    pub payer: Pubkey,
    pub recipient: Pubkey,
    pub amount: u64,
    pub was_factored: bool,
    pub timestamp: i64,
}

#[event]
pub struct InvoiceDisputedEvent {
    pub invoice_pubkey: Pubkey,
    pub disputed_by: Pubkey,
    pub reason: String,
    pub timestamp: i64,
}

#[event]
pub struct InvoiceDisputeResolvedEvent {
    pub invoice_pubkey: Pubkey,
    pub resume_settlement: bool,
    pub timestamp: i64,
}

// -----------------------------------------------------------------------------
// ERROR CODES
// -----------------------------------------------------------------------------

#[error_code]
pub enum AegisError {
    #[msg("Protocol operations are currently paused by governance")]
    ProtocolPaused,
    #[msg("Protocol fee basis points must be between 0 and 500 (5%)")]
    InvalidFeeBasisPoints,
    #[msg("Invoice identifier must be between 1 and 32 characters")]
    InvalidInvoiceIdLength,
    #[msg("Invoice amount must be strictly greater than zero")]
    ZeroAmount,
    #[msg("Advance rate must not exceed 10000 basis points")]
    InvalidAdvanceRate,
    #[msg("Invoice maturity due date must be in the future")]
    InvalidDueTimestamp,
    #[msg("Invoice is already factored by an institutional liquidity provider")]
    AlreadyFactored,
    #[msg("Invoice has already reached final settlement")]
    AlreadySettled,
    #[msg("Action prohibited: Invoice is flagged under active commercial dispute")]
    InvoiceInDispute,
    #[msg("Invoice has expired past its maturity timestamp")]
    InvoiceExpired,
    #[msg("Integer overflow detected during financial math calculation")]
    NumericalOverflow,
    #[msg("Supplied SPL token mint does not match invoice contract")]
    InvalidMintMatch,
    #[msg("Supplied vendor token account does not match invoice vendor")]
    InvalidVendorAccount,
    #[msg("Supplied payer account does not match invoice obligor")]
    InvalidPayerAccount,
    #[msg("Commercial invoice is already in a disputed state")]
    AlreadyDisputed,
    #[msg("Invoice is not in an active dispute")]
    NotInDispute,
    #[msg("Signer is not authorized as protocol administrator")]
    UnauthorizedProtocolAdmin,
}