use anchor_lang::prelude::*;

#[event]
pub struct EscrowInitialized {
    pub escrow_pubkey: Pubkey,
    pub payer: Pubkey,
    pub contractor: Pubkey,
    pub arbiter: Pubkey,
    pub escrow_id: u64,
    pub total_amount: u64,
    pub milestone_count: u8,
}

#[event]
pub struct MilestoneSubmitted {
    pub escrow_pubkey: Pubkey,
    pub milestone_index: u8,
    pub proof_hash: [u8; 32],
}

#[event]
pub struct MilestoneApproved {
    pub escrow_pubkey: Pubkey,
    pub milestone_index: u8,
    pub amount_released: u64,
    pub contractor: Pubkey,
}

#[event]
pub struct DisputeRaised {
    pub escrow_pubkey: Pubkey,
    pub milestone_index: u8,
    pub raised_by: Pubkey,
}

#[event]
pub struct DisputeResolved {
    pub escrow_pubkey: Pubkey,
    pub milestone_index: u8,
    pub contractor_amount: u64,
    pub refund_amount: u64,
}

#[event]
pub struct EscrowCancelled {
    pub escrow_pubkey: Pubkey,
    pub refunded_amount: u64,
}