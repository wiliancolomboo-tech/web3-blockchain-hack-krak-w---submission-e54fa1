use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::KrakError;
use crate::events::MilestoneSubmitted;

#[derive(Accounts)]
pub struct SubmitMilestone<'info> {
    pub contractor: Signer<'info>,

    #[account(
        mut,
        has_one = contractor @ KrakError::Unauthorized,
        seeds = [b"escrow", escrow.payer.as_ref(), contractor.key().as_ref(), &escrow.escrow_id.to_le_bytes()],
        bump = escrow.bump,
    )]
    pub escrow: Account<'info, EscrowAccount>,
}

pub fn handler(
    ctx: Context<SubmitMilestone>,
    milestone_index: u8,
    proof_hash: [u8; 32],
) -> Result<()> {
    let escrow = &mut ctx.accounts.escrow;
    require!(escrow.status == EscrowStatus::Active, KrakError::EscrowNotActive);
    
    let idx = milestone_index as usize;
    require!(idx < escrow.milestones.len(), KrakError::InvalidMilestoneIndex);

    let milestone = &mut escrow.milestones[idx];
    require!(
        milestone.status == MilestoneStatus::Pending || milestone.status == MilestoneStatus::Submitted,
        KrakError::InvalidMilestoneStatus
    );

    milestone.proof_hash = proof_hash;
    milestone.status = MilestoneStatus::Submitted;

    emit!(MilestoneSubmitted {
        escrow_pubkey: escrow.key(),
        milestone_index,
        proof_hash,
    });

    Ok(())
}