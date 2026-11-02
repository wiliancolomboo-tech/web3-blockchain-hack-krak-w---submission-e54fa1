use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::KrakError;
use crate::events::DisputeRaised;

#[derive(Accounts)]
pub struct RaiseDispute<'info> {
    pub signer: Signer<'info>,

    #[account(
        mut,
        seeds = [b"escrow", escrow.payer.as_ref(), escrow.contractor.as_ref(), &escrow.escrow_id.to_le_bytes()],
        bump = escrow.bump,
    )]
    pub escrow: Account<'info, EscrowAccount>,
}

pub fn handler(ctx: Context<RaiseDispute>, milestone_index: u8) -> Result<()> {
    let escrow = &mut ctx.accounts.escrow;
    let caller = ctx.accounts.signer.key();
    require!(
        caller == escrow.payer || caller == escrow.contractor,
        KrakError::Unauthorized
    );

    let idx = milestone_index as usize;
    require!(idx < escrow.milestones.len(), KrakError::InvalidMilestoneIndex);

    let milestone = &mut escrow.milestones[idx];
    require!(
        milestone.status == MilestoneStatus::Submitted || milestone.status == MilestoneStatus::Pending,
        KrakError::InvalidMilestoneStatus
    );

    milestone.status = MilestoneStatus::Disputed;
    escrow.status = EscrowStatus::Disputed;

    emit!(DisputeRaised {
        escrow_pubkey: escrow.key(),
        milestone_index,
        raised_by: caller,
    });

    Ok(())
}