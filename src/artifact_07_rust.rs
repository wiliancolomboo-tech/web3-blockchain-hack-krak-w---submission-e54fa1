use anchor_lang::prelude::*;

#[error_code]
pub enum KrakError {
    #[msg("Milestone count cannot be zero or exceed maximum limit.")]
    InvalidMilestoneCount,

    #[msg("Sum of milestone allocations must match total escrow amount.")]
    MilestoneSumMismatch,

    #[msg("Escrow is not in active state.")]
    EscrowNotActive,

    #[msg("Milestone index out of bounds.")]
    InvalidMilestoneIndex,

    #[msg("Milestone is not in an executable state.")]
    InvalidMilestoneStatus,

    #[msg("Signer is not authorized to execute this escrow action.")]
    Unauthorized,

    #[msg("Contractor percentage basis points must not exceed 10000 (100%).")]
    InvalidBasisPoints,

    #[msg("Cannot cancel escrow after milestones have been approved.")]
    MilestonesAlreadySettled,

    #[msg("Arithmetic overflow occurred.")]
    MathOverflow,
}