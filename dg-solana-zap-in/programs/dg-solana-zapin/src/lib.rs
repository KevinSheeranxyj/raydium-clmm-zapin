use anchor_lang::prelude::*;

declare_id!("DisiSrRg8fWzsy8UXAGwh8VobnCTTg1uiC6iKSNaBrYL");

pub mod instructions;
pub mod errors;
pub mod state;
pub mod helpers;
pub mod events;


use anchor_lang::solana_program::pubkey;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use anchor_spl::token_interface::{Token2022, Mint as InterfaceMint, TokenAccount as InterfaceTokenAccount};
use anchor_spl::metadata::Metadata;
use anchor_lang::prelude::Rent;
use anchor_spl::memo::spl_memo;
use anchor_lang::system_program;
use anchor_lang::prelude::Sysvar;
use anchor_lang::error::Error;
use raydium_amm_v3::libraries::{big_num::*, full_math::MulDiv, tick_math};
use anchor_spl::associated_token::AssociatedToken;
use std::str::FromStr;
use anchor_lang::solana_program::sysvar;
use raydium_amm_v3::{
    cpi,
    program::AmmV3,
    states::{PoolState, AmmConfig, POSITION_SEED, TICK_ARRAY_SEED, ObservationState, TickArrayState, ProtocolPositionState, PersonalPositionState},
};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_lang::solana_program::hash::hash as solana_hash;
use anchor_lang::solana_program::{
    program::invoke_signed,
    system_instruction,
};
use anchor_spl::token::spl_token;
use instructions::*;
use crate::errors::ErrorCode;


use crate::helpers::{to_hex32, deserialize_params, tick_array_start_index};
use crate::state::{ExecStage, OperationType, TransferParams, ZapInParams, Registry};
use crate::events::{DepositEvent, ExecutorAssigned};

// Devnet: DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH
pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
    pubkey!("DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH"); // devnet program ID

/// NOTE: For ZapIn & ZapOut, we're leveraging the Raydium-Amm-v3 Protocol SDK to robost our requirement
#[program]
pub mod dg_solana_zapin {
    use super::*;

    // Initialize the PDA and set the authority
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let od = &mut ctx.accounts.operation_data;
        if !od.initialized {
            od.authority = ctx.accounts.authority.key();
            od.initialized = true;
            msg!("Initialized PDA with authority: {}", od.authority);
        } else {
            msg!("PDA already initialized; authority: {}", od.authority);
        }
        Ok(())
    }

    // Deposit transfer details into PDA
    pub fn deposit(
        ctx: Context<Deposit>,
        transfer_id: [u8; 32],
        operation_type: OperationType,
        action: Vec<u8>,
        amount: u64,
        ca: Pubkey,
        authorized_executor: Pubkey,
    ) -> Result<()> {
        let od = &mut ctx.accounts.operation_data;
        // 初始化（首次该 transfer_id）
        if !od.initialized {
            od.authority = ctx.accounts.authority.key();
            od.initialized = true;
            msg!("Initialized operation_data for transfer_id {} with authority {}", to_hex32(&transfer_id), od.authority);
        }
        let id_hash = transfer_id;
        let reg = &mut ctx.accounts.registry;
        require!(!reg.used_ids.contains(&id_hash), ErrorCode::DuplicateTransferId);
        reg.used_ids.push(id_hash);

        require!(authorized_executor != Pubkey::default(), ErrorCode::InvalidParams);
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(!transfer_id.is_empty(), ErrorCode::InvalidTransferId);

        // 资金转入（保持原逻辑）
        let cpi_accounts = Transfer {
            from: ctx.accounts.authority_ata.to_account_info(),
            to: ctx.accounts.program_token_account.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        // 存基础参数
        od.transfer_id = transfer_id.clone();
        od.amount = amount;
        od.executed = false;
        od.ca = ca;
        od.operation_type = operation_type.clone();
        od.action = action.clone(); // 保留原始参数
        od.executor = authorized_executor; // 授权执行人
        // ====== 存 Raydium 固定账户（直接从 ctx 读 pubkey）======
        msg!("clmm_program_id: {}", ctx.accounts.clmm_program.key());
        od.clmm_program_id   = ctx.accounts.clmm_program.key();
        od.pool_state        = ctx.accounts.pool_state.key();
        od.amm_config        = ctx.accounts.amm_config.key();
        od.observation_state = ctx.accounts.observation_state.key();
        od.token_vault_0     = ctx.accounts.token_vault_0.key();
        od.token_vault_1     = ctx.accounts.token_vault_1.key();
        od.token_mint_0      = ctx.accounts.token_mint_0.key();
        od.token_mint_1      = ctx.accounts.token_mint_1.key();

        // 如果是 ZapIn，解析参数并派生 tick array / protocol_position 等，存起来
        if let OperationType::ZapIn = operation_type {
            let p: ZapInParams = deserialize_params(&od.action)?;
            od.tick_lower = p.tick_lower;
            od.tick_upper = p.tick_upper;

            // 根据 pool 的 tick_spacing 计算 tick array 起始
            let pool = ctx.accounts.pool_state.load()?;
            let tick_spacing: i32 = pool.tick_spacing.into();
            let lower_start = tick_array_start_index(p.tick_lower, tick_spacing);
            let upper_start = tick_array_start_index(p.tick_upper, tick_spacing);

            // Raydium tick array PDA（由外部提供，但我们把“应有地址”存起来用作后续校验）
            let (ta_lower, _) = Pubkey::find_program_address(
                &[
                    TICK_ARRAY_SEED.as_bytes(),
                    ctx.accounts.pool_state.key().as_ref(),
                    &lower_start.to_be_bytes(),
                ],
                &ctx.accounts.clmm_program.key(),
            );
            let (ta_upper, _) = Pubkey::find_program_address(
                &[
                    TICK_ARRAY_SEED.as_bytes(),
                    ctx.accounts.pool_state.key().as_ref(),
                    &upper_start.to_be_bytes(),
                ],
                &ctx.accounts.clmm_program.key(),
            );
            od.tick_array_lower = ta_lower;
            od.tick_array_upper = ta_upper;

            // 协议仓位 PDA（Raydium POSITION_SEED, pool, lower_start, upper_start）
            let (pp, _) = Pubkey::find_program_address(
                &[
                    POSITION_SEED.as_bytes(),
                    ctx.accounts.pool_state.key().as_ref(),
                    &lower_start.to_be_bytes(),
                    &upper_start.to_be_bytes(),
                ],
                &ctx.accounts.clmm_program.key(),
            );
            od.protocol_position = pp;

            // Position NFT mint（deposit 阶段未持有 user；先置空，execute 再写）
            od.position_nft_mint = Pubkey::default();
        }

        // 如果是 Transfer，存 recipient
        if let OperationType::Transfer = operation_type {
            let p: TransferParams = deserialize_params(&od.action)?;
            od.recipient = p.recipient;
        }

        emit!(DepositEvent { transfer_id_hex: to_hex32(&transfer_id), amount, recipient: od.recipient });
        emit!(ExecutorAssigned { transfer_id_hex: to_hex32(&transfer_id), executor: od.executor });
        Ok(())
    }

    pub fn prepare_execute(ctx: Context<PrepareExecute>, transfer_id: [u8;32]) -> Result<()> {
        instructions::prepare_execute::handler(ctx, transfer_id)
    }

    pub fn swap_for_balance(ctx: Context<SwapForBalance>, transfer_id: [u8;32]) -> Result<()> {
        instructions::swap_for_balance::handler(ctx, transfer_id)
    }

    pub fn open_position_step(ctx: Context<OpenPosition>, transfer_id: [u8;32]) -> Result<()> {
        instructions::open_position::handler(ctx, transfer_id)
    }

    pub fn increase_liquidity_step(ctx: Context<IncreaseLiquidity>, transfer_id: [u8;32]) -> Result<()> {
        instructions::increase_liquidity::handler(ctx, transfer_id)
    }

    pub fn finalize_execute(ctx: Context<FinalizeExecute>, transfer_id: [u8;32]) -> Result<()> {
        instructions::finalize_execute::handler(ctx, transfer_id)
    }

    pub fn cancel(ctx: Context<Cancel>, transfer_id: [u8;32]) -> Result<()> {
        instructions::cancel::handler(ctx, transfer_id)
    }

    // Withdraw instruction
    pub fn withdraw(ctx: Context<Withdraw>, p: WithdrawParams) -> Result<()> {
        instructions::withdraw::handler(ctx, p)
    }

    // Claim instruction
    pub fn claim(ctx: Context<Claim>, p: ClaimParams) -> Result<()> {
        instructions::claim::handler(ctx, p)
    }

    // Modify PDA Authority
    pub fn modify_pda_authority(
        ctx: Context<ModifyPdaAuthority>,
        new_authority: Pubkey,
    ) -> Result<()> {
        let operation_data = &mut ctx.accounts.operation_data;

        // Verify current authority
        require!(operation_data.initialized, ErrorCode::NotInitialized);
        require!(
            operation_data.authority == ctx.accounts.current_authority.key(),
            ErrorCode::Unauthorized
        );

        // Update authority
        operation_data.authority = new_authority;
        msg!("Update PDA Authority to: {}", new_authority);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + OperationData::LEN,
        seeds = [b"operation_data"],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}
#[account]
#[derive(Default)]
pub struct OperationData {
    pub authority: Pubkey,

    pub initialized: bool,
    pub transfer_id: [u8; 32],
    pub recipient: Pubkey,
    pub operation_type: OperationType,
    pub action: Vec<u8>,      // Serialize operation-specific parameters (cap below)
    pub amount: u64,
    pub executed: bool,
    pub ca: Pubkey,           // contract address

    pub executor: Pubkey,     // 授权执行人

    // ===== Raydium CLMM & 池静态信息（deposit 时落库） =====
    pub clmm_program_id: Pubkey, // << 新增：存 Raydium 程序ID
    pub pool_state: Pubkey,
    pub amm_config: Pubkey,
    pub observation_state: Pubkey,
    pub token_vault_0: Pubkey,
    pub token_vault_1: Pubkey,
    pub token_mint_0: Pubkey,
    pub token_mint_1: Pubkey,

    // ===== ZapIn/Position 相关 =====
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub tick_array_lower: Pubkey,
    pub tick_array_upper: Pubkey,
    pub protocol_position: Pubkey,
    pub personal_position: Pubkey,
    pub position_nft_mint: Pubkey,

    // ===== 执行阶段控制 =====
    pub stage: ExecStage,      // << 新增
    pub base_input_flag: bool, // << 新增：是否 token0 为输入
}

impl OperationData {
    // 这里的 LEN 选择一个保守上界即可，注意要包含新增字段
    pub const LEN: usize =
        32  // authority
            + 1 // initialized
            + 32 // transfer_id
            + 32 // recipient
            + 1  // operation_type (enum tag)
            + (4 + 256) // action: 4字节长度 + 预留 256
            + 8  // amount
            + 1  // executed
            + 32 // ca
            + 32 // executor
            + 32 // clmm_program_id
            + 32*7 // pool/vault/mint 共7个
            + 4 + 4 // tick_lower/tick_upper
            + 32*5 // tick arrays + positions + nft mint
            + 1  // stage (enum tag)
            + 1; // base_input_flag
}

#[derive(Accounts)]
#[instruction(transfer_id: [u8; 32])]
pub struct Deposit<'info> {
    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + Registry::LEN,
        seeds = [b"registry"],
        bump
    )]
    pub registry: Account<'info, Registry>,

    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + OperationData::LEN,
        seeds = [b"operation_data".as_ref(), transfer_id.as_ref()],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,

    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        mut,
        constraint = authority_ata.owner == authority.key() @ ErrorCode::Unauthorized
    )]
    pub authority_ata: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = program_token_account.owner == operation_data.key() @ ErrorCode::InvalidProgramAccount
    )]
    pub program_token_account: Account<'info, TokenAccount>,

    // ---- Raydium Program（你在 deposit 里用到了 clmm_program.key()） ----
    pub clmm_program: Program<'info, AmmV3>,

    // 池 & 配置
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    // Vault & Mint
    #[account(mut, address = pool_state.load()?.token_vault_0)]
    pub token_vault_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.token_vault_1)]
    pub token_vault_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(address = pool_state.load()?.token_mint_0)]
    pub token_mint_0: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = pool_state.load()?.token_mint_1)]
    pub token_mint_1: Box<InterfaceAccount<'info, InterfaceMint>>,

    // ---- 必须加：init / init_if_needed 需要 system_program；CPI 转账需要 token_program ----
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(transfer_id: String)]
pub struct ModifyPdaAuthority<'info> {
    #[account(
        mut,
        seeds = [b"operation_data".as_ref(), transfer_id.as_bytes()],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    #[account(
        constraint = current_authority.key() == operation_data.authority @ ErrorCode::Unauthorized
    )]
    pub current_authority: Signer<'info>,
}