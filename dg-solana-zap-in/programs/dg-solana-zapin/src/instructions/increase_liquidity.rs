use anchor_lang::prelude::*;
use anchor_lang::{Accounts, event, instruction, require};
use anchor_spl::token::{Token, TokenAccount};
use anchor_spl::token_2022::Token2022;
use crate::helpers::{do_increase_liquidity_v2, load_token_amount, to_hex32};
use crate::state::{ExecStage, OperationType};
use crate::events::*;
use anchor_lang::prelude::InterfaceAccount;
use anchor_spl::token_interface::{Mint as InterfaceMint, TokenAccount as InterfaceTokenAccount};
use crate::OperationData;
use crate::errors::ErrorCode;
use raydium_amm_v3::{
    cpi,
    program::AmmV3,
    states::{PoolState, AmmConfig, POSITION_SEED, TICK_ARRAY_SEED, ObservationState, TickArrayState, ProtocolPositionState, PersonalPositionState},
};


/*
    Add liquidity from an existing Raydium CLMM  Position.
 */
pub fn handler(ctx: Context<IncreaseLiquidity>, transfer_id: [u8;32]) -> Result<()> {
    let od = &mut ctx.accounts.operation_data;

    // 阶段&权限
    require!(od.initialized, ErrorCode::NotInitialized);
    require!(!od.executed, ErrorCode::AlreadyExecuted);
    require!(od.transfer_id == transfer_id, ErrorCode::InvalidTransferId);
    require!(matches!(od.operation_type, OperationType::ZapIn), ErrorCode::InvalidParams);
    require!(ctx.accounts.user.key() == od.executor, ErrorCode::Unauthorized);
    require!(od.stage == ExecStage::Opened, ErrorCode::InvalidParams);

    // 读取 PDA 两侧余额作为 max（由 CPI 内部按需要消耗）
    let pre0 = load_token_amount(&ctx.accounts.pda_token0.to_account_info())?;
    let pre1 = load_token_amount(&ctx.accounts.pda_token1.to_account_info())?;

    // PDA signer
    let bump = ctx.bumps.operation_data;
    let seeds = &[b"operation_data".as_ref(), transfer_id.as_ref(), &[bump]];
    let signer_seeds = &[&seeds[..]];

    do_increase_liquidity_v2(
        ctx.accounts.clmm_program.to_account_info(),
        ctx.accounts.user.to_account_info(),
        ctx.accounts.position_nft_account.to_account_info(),
        ctx.accounts.pool_state.to_account_info(),
        ctx.accounts.protocol_position.to_account_info(),
        ctx.accounts.personal_position.to_account_info(),
        ctx.accounts.tick_array_lower.to_account_info(),
        ctx.accounts.tick_array_upper.to_account_info(),
        ctx.accounts.pda_token0.to_account_info(),
        ctx.accounts.pda_token1.to_account_info(),
        ctx.accounts.token_vault_0.to_account_info(),
        ctx.accounts.token_vault_1.to_account_info(),
        ctx.accounts.token_program.to_account_info(),
        ctx.accounts.token_program_2022.to_account_info(),
        ctx.accounts.token_mint_0.to_account_info(),
        ctx.accounts.token_mint_1.to_account_info(),
        signer_seeds,
        pre0,
        pre1,
        od.base_input_flag, // 与原逻辑一致：base_flag = 是否 token0 为输入
    )?;

    let post0 = load_token_amount(&ctx.accounts.pda_token0.to_account_info())?;
    let post1 = load_token_amount(&ctx.accounts.pda_token1.to_account_info())?;

    emit!(LiquidityAdded {
            transfer_id: to_hex32(&transfer_id),
            token0_used: pre0.saturating_sub(post0),
            token1_used: pre1.saturating_sub(post1),
        });

    od.stage = ExecStage::LiquidityAdded;
    Ok(())

}

#[derive(Accounts)]
#[instruction(transfer_id: [u8; 32])]
pub struct IncreaseLiquidity<'info> {
    #[account(
        mut,
        seeds = [b"operation_data".as_ref(), transfer_id.as_ref()],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    #[account(mut)]
    pub user: Signer<'info>,

    pub clmm_program: Program<'info, AmmV3>,

    #[account(mut, address = operation_data.pool_state)]
    pub pool_state: AccountLoader<'info, PoolState>,
    /// CHECK:  operation_data.protocol_position is not a valid address
    #[account(mut, address = operation_data.protocol_position)]
    pub protocol_position: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK:  operation_data.personal_position is not a valid address
    pub personal_position: UncheckedAccount<'info>,
    #[account(mut, address = operation_data.tick_array_lower)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,
    #[account(mut, address = operation_data.tick_array_upper)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    #[account(mut)]
    pub pda_token0: Account<'info, TokenAccount>,
    #[account(mut)]
    pub pda_token1: Account<'info, TokenAccount>,
    #[account(mut, address = operation_data.token_vault_0)]
    pub token_vault_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = operation_data.token_vault_1)]
    pub token_vault_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(address = operation_data.token_mint_0)]
    pub token_mint_0: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = operation_data.token_mint_1)]
    pub token_mint_1: Box<InterfaceAccount<'info, InterfaceMint>>,

    /// CHECK:  operation_data.position_nft_account is not a valid address
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>, // user 的 position NFT ATA

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
}
