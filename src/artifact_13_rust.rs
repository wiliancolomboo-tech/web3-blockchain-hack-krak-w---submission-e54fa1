use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::KrakError;
use crate::events::DisputeResolved;

#[derive(Accounts)]
pub struct ResolveDispute<'info> {
    pub arbiter: Signer<'info>,

    /// CHECK: Verified against escrow.payer
    #[account(mut, address = escrow.payer @ KrakError::Unauthorized)]
    pub payer: AccountInfo<'info>,

    /// CHECK: Verified against escrow.contractor
    #[account(mut, address = escrow.contractor @ KrakError::Unauthorized)]
    pub contractor: AccountInfo<'info>,

    #[account(
        mut,
        has_one = arbiter @ KrakError::Unauthorized,
        seeds = [b"escrow", escrow.payer.as_ref(), escrow.contractor.as_ref(), &escrow.escrow_id.to_le_bytes()],
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

pub fn handler(
    ctx: Context<ResolveDispute>,
    milestone_index: u8,
    contractor_basis_points: u16,
) -> Result<()> {
    require!(contractor_basis_points <= 10_000, KrakError::InvalidBasisPoints);

    let escrow = &mut ctx.accounts.escrow;
    let idx = milestone_index as usize;
    require!(idx < escrow.milestones.len(), KrakError::InvalidMilestoneIndex);

    let milestone_amount: u64;
    {
        let milestone = &mut escrow.milestones[idx];
        require!(milestone.status == MilestoneStatus::Disputed, KrakError::InvalidMilestoneStatus);
        milestone.status = MilestoneStatus::Resolved;
        milestone_amount = milestone.amount;
    }

    let contractor_amount = ((milestone_amount as u128)
        .checked_mul(contractor_basis_points as u128)
        .ok_or(KrakError::MathOverflow)? / 10_000) as u64;

    let refund_amount = milestone_amount
        .checked_sub(contractor_amount)
        .ok_or(KrakError::MathOverflow)?;

    escrow.released_amount = escrow
        .released_amount
        .checked_add(milestone_amount)
        .ok_or(KrakError::MathOverflow)?;

    // Check if other milestones are disputed; if not, return escrow to active or completed
    let any_disputed = escrow.milestones.iter().any(|m| m.status == MilestoneStatus::Disputed);
    let all_settled = escrow.milestones.iter().all(|m| {
        m.status == MilestoneStatus::Approved || m.status == MilestoneStatus::Resolved
    });

    if all_settled {
        escrow.status = EscrowStatus::Completed;
    } else if !any_disputed {
        escrow.status = EscrowStatus::Active;
    }

    let escrow_key = escrow.key();
    let vault_seeds: &[&[u8]] = &[
        b"vault",
        escrow_key.as_ref(),
        &[escrow.vault_bump],
    ];
    let signer_seeds = &[vault_seeds];

    // Transfer Contractor allocation
    if contractor_amount > 0 {
        let ix_contractor = anchor_lang::solana_program::system_instruction::transfer(
            ctx.accounts.vault.key,
            ctx.accounts.contractor.key,
            contractor_amount,
        );
        anchor_lang::solana_program::program::invoke_signed(
            &ix_contractor,
            &[
                ctx.accounts.vault.to_account_info(),
                ctx.accounts.contractor.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            signer_seeds,
        )?;
    }

    // Transfer Payer Refund allocation
    if refund_amount > 0 {
        let ix_refund = anchor_lang::solana_program::system_instruction::transfer(
            ctx.accounts.vault.key,
            ctx.accounts.payer.key,
            refund_amount,
        );
        anchor_lang::solana_program::program::invoke_signed(
            &ix_refund,
            &[
                ctx.accounts.vault.to_account_info(),
                ctx.accounts.payer.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            signer_seeds,
        )?;
    }

    emit!(DisputeResolved {
        escrow_pubkey: escrow.key(),
        milestone_index,
        contractor_amount,
        refund_amount,
    });

    Ok(())
}