use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use anchor_spl::token_interface::{Mint as InterfaceMint, TokenAccount as InterfaceTokenAccount};
use anchor_spl::associated_token::AssociatedToken;
use raydium_amm_v3::states::{PoolState, AmmConfig, ObservationState};
use crate::{errors::*, state::*, helpers::*, OperationType};

pub fn handler(ctx: Context<PrepareExecute>, transfer_id: [u8; 32]) -> Result<()> {
    let od = &mut ctx.accounts.operation_data;

    // 基本校验
    require!(od.initialized, OperationError::NotInitialized);
    require!(!od.executed, OperationError::AlreadyExecuted);
    require!(od.transfer_id == transfer_id, OperationError::InvalidTransferId);
    require!(matches!(od.operation_type, OperationType::ZapIn), OperationError::InvalidParams);
    require!(ctx.accounts.user.key() == od.executor, OperationError::Unauthorized);
    require!(od.stage == ExecStage::None, OperationError::InvalidParams);

    // 解析参数 + tick 校验 + 派生 tick arrays / protocol position
    let p: ZapInParams = deserialize_params(&od.action)?;
    require!(p.tick_lower < p.tick_upper, OperationError::InvalidTickRange);
    require_keys_eq!(od.pool_state, ctx.accounts.pool_state.key(), OperationError::InvalidParams);

    let pool = ctx.accounts.pool_state.load()?;
    let tick_spacing: i32 = pool.tick_spacing.into();

    let lower_start = tick_array_start_index(p.tick_lower, tick_spacing);
    let upper_start = tick_array_start_index(p.tick_upper, tick_spacing);

    let (ta_lower, _) = Pubkey::find_program_address(
        &[TICK_ARRAY_SEED.as_bytes(), od.pool_state.as_ref(), &lower_start.to_be_bytes()],
        &od.clmm_program_id,
    );
    let (ta_upper, _) = Pubkey::find_program_address(
        &[TICK_ARRAY_SEED.as_bytes(), od.pool_state.as_ref(), &upper_start.to_be_bytes()],
        &od.clmm_program_id,
    );
    let (pp, _) = Pubkey::find_program_address(
        &[POSITION_SEED.as_bytes(), od.pool_state.as_ref(), &lower_start.to_be_bytes(), &upper_start.to_be_bytes()],
        &od.clmm_program_id,
    );

    od.tick_lower = p.tick_lower;
    od.tick_upper = p.tick_upper;
    od.tick_array_lower = ta_lower;
    od.tick_array_upper = ta_upper;
    od.protocol_position = pp;

    // 确定输入侧：program_token_account 的 mint 是否等于 token_mint_0
    let is_base_input = ctx.accounts.program_token_account.mint == od.token_mint_0;
    require!(
            ctx.accounts.program_token_account.mint == od.token_mint_0
            || ctx.accounts.program_token_account.mint == od.token_mint_1,
            OperationError::InvalidMint
        );

    // 如果金额不足，直接退款并结束（与原逻辑一致）
    if od.amount < p.amount_in {
        let bump = ctx.bumps.operation_data;
        let seeds = &[b"operation_data".as_ref(), transfer_id.as_ref(), &[bump]];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = anchor_spl::token::Transfer {
            from: ctx.accounts.program_token_account.to_account_info(),
            to: ctx.accounts.refund_ata.to_account_info(),
            authority: ctx.accounts.operation_data.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts)
            .with_signer(signer_seeds);
        token::transfer(cpi_ctx, od.amount)?;

        od.executed = true;
        od.stage = ExecStage::Finalized;
        msg!("prepare_execute: deposit insufficient; refunded {} and finalized", od.amount);
        return Ok(());
    }

    // 将 deposit 金额搬运到 PDA 自有 token 账户（按输入侧）
    let bump = ctx.bumps.operation_data;
    let seeds = &[b"operation_data".as_ref(), transfer_id.as_ref(), &[bump]];
    let signer_seeds = &[&seeds[..]];

    let (dst, expect_mint) = if is_base_input {
        (&ctx.accounts.pda_token0, od.token_mint_0)
    } else {
        (&ctx.accounts.pda_token1, od.token_mint_1)
    };
    // 账户一致性约束（运行时再校验一次）
    require!(dst.owner == od.key(), OperationError::InvalidProgramAccount);
    require!(dst.mint == expect_mint, OperationError::InvalidMint);

    let cpi_accounts = anchor_spl::token::Transfer {
        from: ctx.accounts.program_token_account.to_account_info(),
        to: dst.to_account_info(),
        authority: ctx.accounts.operation_data.to_account_info(),
    };
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts)
        .with_signer(signer_seeds);
    token::transfer(cpi_ctx, od.amount)?;

    // Position NFT mint 的派生（存起来，后续 open_position 使用）
    if od.position_nft_mint == Pubkey::default() {
        let (m, _) = Pubkey::find_program_address(
            &[b"pos_nft_mint", ctx.accounts.user.key.as_ref(), od.pool_state.as_ref()],
            ctx.program_id,
        );
        od.position_nft_mint = m;
    }

    od.base_input_flag = is_base_input;
    od.stage = ExecStage::Prepared;
    Ok(())
}

#[derive(Accounts)]
#[instruction(transfer_id: [u8; 32])]
pub struct PrepareExecute<'info> {
    #[account(
        mut,
        seeds = [b"operation_data", &transfer_id],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,

    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        constraint = program_token_account.owner == operation_data.key() @ OperationError::InvalidProgramAccount
    )]
    pub program_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub refund_ata: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = pda_token0.owner == operation_data.key() @ OperationError::InvalidProgramAccount,
        constraint = pda_token0.mint  == operation_data.token_mint_0 @ OperationError::InvalidMint,
    )]
    pub pda_token0: Account<'info, TokenAccount>,
    #[account(
        mut,
        constraint = pda_token1.owner == operation_data.key() @ OperationError::InvalidProgramAccount,
        constraint = pda_token1.mint  == operation_data.token_mint_1 @ OperationError::InvalidMint,
    )]
    pub pda_token1: Account<'info, TokenAccount>,

    #[account(mut, address = operation_data.pool_state)]
    pub pool_state: AccountLoader<'info, PoolState>,
    #[account(address = operation_data.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,
    #[account(address = operation_data.observation_state)]
    pub observation_state: AccountLoader<'info, ObservationState>,
    #[account(address = operation_data.token_vault_0)]
    pub token_vault_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(address = operation_data.token_vault_1)]
    pub token_vault_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(address = operation_data.token_mint_0)]
    pub token_mint_0: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = operation_data.token_mint_1)]
    pub token_mint_1: Box<InterfaceAccount<'info, InterfaceMint>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}