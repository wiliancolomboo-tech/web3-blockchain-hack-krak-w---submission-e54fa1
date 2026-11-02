use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};
use crate::state::*;
use crate::errors::KrakError;
use crate::events::EscrowInitialized;

#[derive(Accounts)]
#[instruction(escrow_id: u64, milestone_amounts: Vec<u64>, total_amount: u64)]
pub struct InitializeEscrow<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: Validated as recipient of contract funds upon milestone release.
    pub contractor: AccountInfo<'info>,

    /// CHECK: Validated as authorized arbitrator for multi-sig dispute resolution.
    pub arbiter: AccountInfo<'info>,

    #[account(
        init,
        payer = payer,
        space = 8 + EscrowAccount::INIT_SPACE,
        seeds = [b"escrow", payer.key().as_ref(), contractor.key().as_ref(), &escrow_id.to_le_bytes()],
        bump
    )]
    pub escrow: Account<'info, EscrowAccount>,

    /// CHECK: PDA holding the locked lamports safely.
    #[account(
        mut,
        seeds = [b"vault", escrow.key().as_ref()],
        bump
    )]
    pub vault: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(
    ctx: Context<InitializeEscrow>,
    escrow_id: u64,
    milestone_amounts: Vec<u64>,
    total_amount: u64,
) -> Result<()> {
    let count = milestone_amounts.len();
    require!(count > 0 && count <= MAX_MILESTONES, KrakError::InvalidMilestoneCount);

    let mut sum: u64 = 0;
    let mut milestones: Vec<Milestone> = Vec::with_capacity(count);

    for amt in milestone_amounts.iter() {
        sum = sum.checked_add(*amt).ok_or(KrakError::MathOverflow)?;
        milestones.push(Milestone {
            amount: *amt,
            proof_hash: [0u8; 32],
            status: MilestoneStatus::Pending,
        });
    }

    require!(sum == total_amount, KrakError::MilestoneSumMismatch);

    // Transfer SOL from payer to Vault PDA
    let cpi_context = CpiContext::new(
        ctx.accounts.system_program.to_account_info(),
        Transfer {
            from: ctx.accounts.payer.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
        },
    );
    transfer(cpi_context, total_amount)?;

    let escrow = &mut ctx.accounts.escrow;
    escrow.payer = ctx.accounts.payer.key();
    escrow.contractor = ctx.accounts.contractor.key();
    escrow.arbiter = ctx.accounts.arbiter.key();
    escrow.escrow_id = escrow_id;
    escrow.total_amount = total_amount;
    escrow.released_amount = 0;
    escrow.status = EscrowStatus::Active;
    escrow.bump = ctx.bumps.escrow;
    escrow.vault_bump = ctx.bumps.vault;
    escrow.milestone_count = count as u8;
    escrow.milestones = milestones;

    emit!(EscrowInitialized {
        escrow_pubkey: escrow.key(),
        payer: escrow.payer,
        contractor: escrow.contractor,
        arbiter: escrow.arbiter,
        escrow_id,
        total_amount,
        milestone_count: count as u8,
    });

    Ok(())
}