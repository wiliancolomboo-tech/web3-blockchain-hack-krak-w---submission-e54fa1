use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::KrakError;
use crate::events::MilestoneApproved;

#[derive(Accounts)]
pub struct ApproveMilestone<'info> {
    pub payer: Signer<'info>,

    /// CHECK: Recipient of the milestone payout. Validated against escrow.contractor.
    #[account(mut, address = escrow.contractor @ KrakError::Unauthorized)]
    pub contractor: AccountInfo<'info>,

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

pub fn handler(ctx: Context<ApproveMilestone>, milestone_index: u8) -> Result<()> {
    let escrow = &mut ctx.accounts.escrow;
    require!(escrow.status == EscrowStatus::Active, KrakError::EscrowNotActive);

    let idx = milestone_index as usize;
    require!(idx < escrow.milestones.len(), KrakError::InvalidMilestoneIndex);

    let milestone_amount: u64;
    {
        let milestone = &mut escrow.milestones[idx];
        require!(
            milestone.status == MilestoneStatus::Submitted || milestone.status == MilestoneStatus::Pending,
            KrakError::InvalidMilestoneStatus
        );
        milestone.status = MilestoneStatus::Approved;
        milestone_amount = milestone.amount;
    }

    escrow.released_amount = escrow
        .released_amount
        .checked_add(milestone_amount)
        .ok_or(KrakError::MathOverflow)?;

    // If all milestones settled, mark complete
    let all_settled = escrow.milestones.iter().all(|m| {
        m.status == MilestoneStatus::Approved || m.status == MilestoneStatus::Resolved
    });
    if all_settled {
        escrow.status = EscrowStatus::Completed;
    }

    // Direct PDA CPI Lamport transfer from Vault to Contractor
    let escrow_key = escrow.key();
    let vault_seeds: &[&[u8]] = &[
        b"vault",
        escrow_key.as_ref(),
        &[escrow.vault_bump],
    ];
    let signer_seeds = &[vault_seeds];

    let ix = anchor_lang::solana_program::system_instruction::transfer(
        ctx.accounts.vault.key,
        ctx.accounts.contractor.key,
        milestone_amount,
    );

    anchor_lang::solana_program::program::invoke_signed(
        &ix,
        &[
            ctx.accounts.vault.to_account_info(),
            ctx.accounts.contractor.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        ],
        signer_seeds,
    )?;

    emit!(MilestoneApproved {
        escrow_pubkey: escrow.key(),
        milestone_index,
        amount_released: milestone_amount,
        contractor: ctx.accounts.contractor.key(),
    });

    Ok(())
}