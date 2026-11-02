use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("Aegis11111111111111111111111111111111111111");

#[program]
pub mod aegis_escrow {
    use super::*;

    pub fn initialize_agreement(
        ctx: Context<InitializeAgreement>,
        agreement_id: [u8; 16],
        milestone_amounts: Vec<u64>,
        review_timeouts: Vec<i64>,
    ) -> Result<()> {
        instructions::initialize_agreement::handler(ctx, agreement_id, milestone_amounts, review_timeouts)
    }

    pub fn deposit_funds(ctx: Context<DepositFunds>, amount: u64) -> Result<()> {
        instructions::deposit_funds::handler(ctx, amount)
    }

    pub fn submit_milestone(
        ctx: Context<SubmitMilestone>,
        milestone_index: u8,
        submission_hash: [u8; 32],
    ) -> Result<()> {
        instructions::submit_milestone::handler(ctx, milestone_index, submission_hash)
    }

    pub fn approve_milestone(ctx: Context<ApproveMilestone>, milestone_index: u8) -> Result<()> {
        instructions::approve_milestone::handler(ctx, milestone_index)
    }

    pub fn raise_dispute(
        ctx: Context<RaiseDispute>,
        milestone_index: u8,
        dispute_evidence_hash: [u8; 32],
    ) -> Result<()> {
        instructions::raise_dispute::handler(ctx, milestone_index, dispute_evidence_hash)
    }

    pub fn resolve_dispute(
        ctx: Context<ResolveDispute>,
        milestone_index: u8,
        client_split_bps: u64,
        contractor_split_bps: u64,
    ) -> Result<()> {
        instructions::resolve_dispute::handler(ctx, milestone_index, client_split_bps, contractor_split_bps)
    }

    pub fn claim_timeout_release(ctx: Context<ClaimTimeoutRelease>, milestone_index: u8) -> Result<()> {
        instructions::claim_timeout_release::handler(ctx, milestone_index)
    }
}