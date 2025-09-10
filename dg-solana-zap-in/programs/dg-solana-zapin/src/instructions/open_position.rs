use anchor_lang::{Accounts, require, require_keys_eq};
use anchor_lang::prelude::*;
use anchor_spl::metadata::Metadata;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{Token, TokenAccount};
use anchor_spl::token_2022::Token2022;
use crate::helpers::{do_open_position_v2, tick_array_start_index};
use crate::state::{ExecStage, OperationType};
use anchor_spl::token_interface::{
    Mint as InterfaceMint,
    TokenAccount as InterfaceTokenAccount,
};
use crate::errors::ErrorCode;
use raydium_amm_v3::{
    cpi,
    program::AmmV3,
    states::{PoolState, AmmConfig, POSITION_SEED, TICK_ARRAY_SEED, ObservationState, TickArrayState, ProtocolPositionState, PersonalPositionState},
};
use crate::OperationData;
use anchor_spl::token::Mint;

pub fn handler(ctx: Context<OpenPosition>, transfer_id: [u8; 32]) -> Result<()> {
    let od_ro = &ctx.accounts.operation_data;

    // 阶段&权限
    require!(od_ro.initialized, ErrorCode::NotInitialized);
    require!(!od_ro.executed, ErrorCode::AlreadyExecuted);
    require!(od_ro.transfer_id == transfer_id, ErrorCode::InvalidTransferId);
    require!(matches!(od_ro.operation_type, OperationType::ZapIn), ErrorCode::InvalidParams);
    require!(ctx.accounts.user.key() == od_ro.executor, ErrorCode::Unauthorized);
    require!(od_ro.stage == ExecStage::Swapped, ErrorCode::InvalidParams);

    // 计算 tick array 起点
    let pool = ctx.accounts.pool_state.load()?;
    let tick_spacing: i32 = pool.tick_spacing.into();
    let lower_start = tick_array_start_index(od_ro.tick_lower, tick_spacing);
    let upper_start = tick_array_start_index(od_ro.tick_upper, tick_spacing);

    // 如 position mint 账户未创建则创建（与原逻辑一致）
    if ctx.accounts.position_nft_mint.data_is_empty() {
        let mint_space = Mint::LEN;
        let rent_lamports = Rent::get()?.minimum_balance(mint_space);
        let (m, bump2) = Pubkey::find_program_address(
            &[b"pos_nft_mint", ctx.accounts.user.key.as_ref(), od_ro.pool_state.as_ref()],
            ctx.program_id,
        );
        require_keys_eq!(m, od_ro.position_nft_mint, ErrorCode::InvalidParams);

        let create_ix = system_instruction::create_account(
            &ctx.accounts.user.key(),
            &m,
            rent_lamports,
            mint_space as u64,
            &anchor_spl::token::ID,
        );
        let seed_arr_mint: [&[u8]; 4] = [b"pos_nft_mint", ctx.accounts.user.key.as_ref(), od_ro.pool_state.as_ref(), &[bump2]];
        invoke_signed(
            &create_ix,
            &[
                ctx.accounts.user.to_account_info(),
                ctx.accounts.position_nft_mint.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            &[&seed_arr_mint],
        )?;
        // 如有需要，可在此调用 initialize_mint2(decimals=0, mint_authority=PDA) —— 但与原逻辑保持一致，交由 Raydium 处理。
    }

    // PDA signer
    let bump = ctx.bumps.operation_data;
    let seeds = &[b"operation_data".as_ref(), transfer_id.as_ref(), &[bump]];
    let signer_seeds = &[&seeds[..]];

    // 计算 Metaplex metadata PDA（已由前端传入 accounts）
    let metadata_pid = anchor_spl::metadata::Metadata::id();
    let (meta_pda, _) = Pubkey::find_program_address(
        &[b"metadata", metadata_pid.as_ref(), od_ro.position_nft_mint.as_ref()],
        &metadata_pid,
    );
    require_keys_eq!(meta_pda, ctx.accounts.metadata_account.key(), ErrorCode::InvalidParams);

    // 调 Raydium 开仓
    do_open_position_v2(
        ctx.accounts.clmm_program.to_account_info(),
        ctx.accounts.operation_data.to_account_info(),
        ctx.accounts.pool_state.to_account_info(),
        ctx.accounts.user.to_account_info(),
        ctx.accounts.position_nft_mint.to_account_info(),
        ctx.accounts.position_nft_account.to_account_info(),
        ctx.accounts.personal_position.to_account_info(),
        ctx.accounts.protocol_position.to_account_info(),
        ctx.accounts.tick_array_lower.to_account_info(),
        ctx.accounts.tick_array_upper.to_account_info(),
        ctx.accounts.token_program.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
        ctx.accounts.rent.to_account_info(),
        ctx.accounts.associated_token_program.to_account_info(),
        ctx.accounts.pda_token0.to_account_info(),
        ctx.accounts.pda_token1.to_account_info(),
        ctx.accounts.token_vault_0.to_account_info(),
        ctx.accounts.token_vault_1.to_account_info(),
        ctx.accounts.token_mint_0.to_account_info(),
        ctx.accounts.token_mint_1.to_account_info(),
        ctx.accounts.metadata_program.to_account_info(),
        ctx.accounts.metadata_account.to_account_info(),
        ctx.accounts.token_program_2022.to_account_info(),
        od_ro.tick_lower,
        od_ro.tick_upper,
        lower_start,
        upper_start,
        signer_seeds,
    )?;

    let od = &mut ctx.accounts.operation_data;
    if od.personal_position == Pubkey::default() {
        od.personal_position = ctx.accounts.personal_position.key();
    }
    od.stage = ExecStage::Opened;
    Ok(())
}


#[derive(Accounts)]
#[instruction(transfer_id: [u8; 32])]
pub struct OpenPosition<'info> {
    #[account(
        mut,
        seeds = [b"operation_data", &transfer_id],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    #[account(mut)]
    pub user: Signer<'info>,

    pub clmm_program: Program<'info, AmmV3>,

    #[account(mut, address = operation_data.pool_state)]
    pub pool_state: AccountLoader<'info, PoolState>,
    #[account(mut, address = operation_data.tick_array_lower)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,
    #[account(mut, address = operation_data.tick_array_upper)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,
    /// CHECK:  operation_data.protocol_position is not a valid address
    #[account(mut, address = operation_data.protocol_position)]
    pub protocol_position: UncheckedAccount<'info>,
    /// CHECK:  operation_data.personal_position is not a valid address
    #[account(mut)]
    pub personal_position: UncheckedAccount<'info>,

    #[account(mut)]
    /// CHECK:  operation_data.position_nft_mint is not a valid address
    pub position_nft_mint: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK:  operation_data.position_nft_account is not a valid address
    pub position_nft_account: UncheckedAccount<'info>, // user 的 ATA，可由 CPI 创建

    #[account(address = operation_data.token_mint_0)]
    pub token_mint_0: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = operation_data.token_mint_1)]
    pub token_mint_1: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(mut, address = operation_data.token_vault_0)]
    pub token_vault_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = operation_data.token_vault_1)]
    pub token_vault_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(mut)]
    pub pda_token0: Account<'info, TokenAccount>,
    #[account(mut)]
    pub pda_token1: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
    pub associated_token_program: Program<'info, AssociatedToken>,

    pub metadata_program: Program<'info, Metadata>,
    /// CHECK: PDA by (metadata, mint)
    #[account(mut)]
    pub metadata_account: UncheckedAccount<'info>,
}