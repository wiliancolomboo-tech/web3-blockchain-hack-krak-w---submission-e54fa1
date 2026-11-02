use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::KrakError;
use crate::events::EscrowCancelled;

#[derive(Accounts)]
pub struct CancelEscrow<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        mut,
        has_one = payer @ KrakError::Unauthorized,
        seeds = [b"escrow", payer.key().as_ref(), escrow.contractor.as_ref(), &escrow.escrow_id.to_le_bytes()],
        bump = escrow.bump,
    )]
    pub escrow: Account<'info, EscrowAccount>,

    /// CHECK: Vault PDA verified with seeds.
    #[account(
        mut,
        seeds = [b"vault", escrow.key().as_ref()],
        bump = escrow.vault_bump,
    )]
    pub vault: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<CancelEscrow>) -> Result<()> {
    let escrow = &mut ctx.accounts.escrow;
    require!(escrow.status == EscrowStatus::Active, KrakError::EscrowNotActive);
    require!(escrow.released_amount == 0, KrakError::MilestonesAlreadySettled);

    // Ensure no milestones have already been submitted or approved
    let work_started = escrow.milestones.iter().any(|m| m.status != MilestoneStatus::Pending);
    require!(!work_started, KrakError::MilestonesAlreadySettled);

    escrow.status = EscrowStatus::Cancelled;

    let remaining_vault_lamports = ctx.accounts.vault.lamports();
    let escrow_key = escrow.key();
    let vault_seeds: &[&[u8]] = &[
        b"vault",
        escrow_key.as_ref(),
        &[escrow.vault_bump],
    ];
    let signer_seeds = &[vault_seeds];

    let ix = anchor_lang::solana_program::system_instruction::transfer(
        ctx.accounts.vault.key,
        ctx.accounts.payer.key,
        remaining_vault_lamports,
    );

    anchor_lang::solana_program::program::invoke_signed(
        &ix,
        &[
            ctx.accounts.vault.to_account_info(),
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        ],
        signer_seeds,
    )?;

    emit!(EscrowCancelled {
        escrow_pubkey: escrow.key(),
        refunded_amount: remaining_vault_lamports,
    });

    Ok(())
}