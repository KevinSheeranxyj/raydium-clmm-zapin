use anchor_lang::prelude::*;
use anchor_lang::{Accounts, instruction, require};
use anchor_lang::context::Context;
use crate::state::ExecStage;
use crate::errors::ErrorCode;
use crate::OperationData;

pub fn handler(ctx: Context<FinalizeExecute>, transfer_id: [u8;32]) -> Result<()> {
    let od = &mut ctx.accounts.operation_data;

    require!(od.initialized, ErrorCode::NotInitialized);
    require!(!od.executed, ErrorCode::AlreadyExecuted);
    require!(od.transfer_id == transfer_id, ErrorCode::InvalidTransferId);
    require!(ctx.accounts.user.key() == od.executor, ErrorCode::Unauthorized);
    require!(od.stage == ExecStage::LiquidityAdded, ErrorCode::InvalidParams);

    od.executed = true;
    od.stage = ExecStage::Finalized;
    Ok(())
}


#[derive(Accounts)]
#[instruction(transfer_id: [u8; 32])]
pub struct FinalizeExecute<'info> {
    #[account(
        mut,
        seeds = [b"operation_data", &transfer_id],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    pub user: Signer<'info>,

}