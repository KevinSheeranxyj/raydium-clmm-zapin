use anchor_lang::{Accounts, err, error, require};
use anchor_lang::idl::types::IdlType::U256;
use anchor_lang::prelude::Context;
use raydium_amm_v3::libraries::tick_math;
use crate::helpers::do_swap_single_v2;
use crate::state::{ExecStage, OperationType};

pub fn handler(ctx: Context<SwapForBalance>, transfer_id: [u8;32]) -> Result<()> {
    let od = &mut ctx.accounts.operation_data;

    // 阶段&权限
    require!(od.initialized, OperationError::NotInitialized);
    require!(!od.executed, OperationError::AlreadyExecuted);
    require!(od.transfer_id == transfer_id, OperationError::InvalidTransferId);
    require!(matches!(od.operation_type, OperationType::ZapIn), OperationError::InvalidParams);
    require!(ctx.accounts.user.key() == od.executor, OperationError::Unauthorized);
    require!(od.stage == ExecStage::Prepared, OperationError::InvalidParams);

    // 解析参数
    let p: ZapInParams = deserialize_params(&od.action)?;
    require!(p.tick_lower < p.tick_upper, OperationError::InvalidTickRange);

    // 读取价格、费率
    let ps = ctx.accounts.pool_state.load()?;
    let cfg = ctx.accounts.amm_config.as_ref();
    let sp = ps.sqrt_price_x64;
    let trade_fee_bps: u32 = cfg.trade_fee_rate.into();
    let protocol_fee_bps: u32 = cfg.protocol_fee_rate.into();

    // 计算 min_out
    let sp_u   = U256::from(sp);
    let q64_u  = U256::from(Q64_U128);
    let price_q64 = sp_u.mul_div_floor(sp_u, q64_u).ok_or(error!(OperationError::InvalidParams))?;

    let total_fee_bps = trade_fee_bps + protocol_fee_bps;
    let slip_bps = p.slippage_bps.min(10_000);
    let one = U256::from(10_000u32);
    let fee_factor = one - U256::from(total_fee_bps);
    let slip_factor = one - U256::from(slip_bps);
    let discount = fee_factor.mul_div_floor(slip_factor, one).ok_or(error!(OperationError::InvalidParams))?;

    let amount_in_u = U256::from(p.amount_in);
    let min_out_u = if od.base_input_flag {
        amount_in_u.mul_div_floor(price_q64, q64_u).ok_or(error!(OperationError::InvalidParams))?
    } else {
        amount_in_u.mul_div_floor(q64_u, price_q64.max(U256::from(1u8))).ok_or(error!(OperationError::InvalidParams))?
    }.mul_div_floor(discount, one).ok_or(error!(OperationError::InvalidParams))?;
    let min_amount_out = min_out_u.to_underflow_u64();

    // 计算一次 swap 的分摊（与你现有逻辑一致）
    let sa = tick_math::get_sqrt_price_at_tick(p.tick_lower).map_err(|_| error!(OperationError::InvalidParams))?;
    let sb = tick_math::get_sqrt_price_at_tick(p.tick_upper).map_err(|_| error!(OperationError::InvalidParams))?;
    let sa_u = U256::from(sa);
    let sb_u = U256::from(sb);
    let sp_u2 = U256::from(sp);
    require!(sa < sb, OperationError::InvalidTickRange);
    let sp_minus_sa = if sp_u2 >= sa_u { sp_u2 - sa_u } else { return err!(OperationError::InvalidParams); };
    let sb_minus_sp = if sb_u >= sp_u2 { sb_u - sp_u2 } else { return err!(OperationError::InvalidParams); };
    let r_num = sb_u * sp_minus_sa;
    let r_den = sp_u2 * sb_minus_sp;
    let frac_den = r_den + r_num;
    require!(frac_den > U256::from(0u8), OperationError::InvalidParams);

    let swap_amount_u = if od.base_input_flag {
        U256::from(p.amount_in).mul_div_floor(r_num, frac_den).ok_or(error!(OperationError::InvalidParams))?
    } else {
        U256::from(p.amount_in).mul_div_floor(r_den, frac_den).ok_or(error!(OperationError::InvalidParams))?
    };
    let swap_amount = swap_amount_u.to_underflow_u64();

    // PDA signer
    let bump = ctx.bumps.operation_data;
    let seeds = &[b"operation_data".as_ref(), transfer_id.as_ref(), &[bump]];
    let signer_seeds = &[&seeds[..]];

    // 组装输入/输出侧
    let (in_acc, out_acc, in_vault, out_vault, in_mint, out_mint) = if od.base_input_flag {
        (
            ctx.accounts.pda_token0.to_account_info(),
            ctx.accounts.pda_token1.to_account_info(),
            ctx.accounts.token_vault_0.to_account_info(),
            ctx.accounts.token_vault_1.to_account_info(),
            ctx.accounts.token_mint_0.to_account_info(),
            ctx.accounts.token_mint_1.to_account_info(),
        )
    } else {
        (
            ctx.accounts.pda_token1.to_account_info(),
            ctx.accounts.pda_token0.to_account_info(),
            ctx.accounts.token_vault_1.to_account_info(),
            ctx.accounts.token_vault_0.to_account_info(),
            ctx.accounts.token_mint_1.to_account_info(),
            ctx.accounts.token_mint_0.to_account_info(),
        )
    };

    // 执行 Raydium swap
    do_swap_single_v2(
        ctx.accounts.clmm_program.to_account_info(),
        ctx.accounts.token_program.to_account_info(),
        ctx.accounts.token_program_2022.to_account_info(),
        ctx.accounts.memo_program.to_account_info(),
        ctx.accounts.amm_config.to_account_info(),
        ctx.accounts.pool_state.to_account_info(),
        ctx.accounts.observation_state.to_account_info(),
        in_acc,
        out_acc,
        in_vault,
        out_vault,
        in_mint,
        out_mint,
        ctx.accounts.operation_data.to_account_info(),
        signer_seeds,
        swap_amount,
        min_amount_out,
        od.base_input_flag,
    )?;

    od.stage = ExecStage::Swapped;
    Ok(())
}

#[derive(Accounts)]
#[instruction(transfer_id: [u8; 32])]
pub struct SwapForBalance<'info> {
    #[account(
        mut,
        seeds = [b"operation_data", &transfer_id],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    #[account(mut)]
    pub user: Signer<'info>,

    // Raydium 程序
    pub clmm_program: Program<'info, AmmV3>,

    // 池相关（可 mut）
    #[account(mut, address = operation_data.pool_state)]
    pub pool_state: AccountLoader<'info, PoolState>,
    #[account(address = operation_data.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,
    #[account(mut, address = operation_data.observation_state)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    // PDA 自有 token 账户（作为 input/output）
    #[account(mut, address = operation_data.token_mint_0 @ OperationError::InvalidMint)]
    pub token_mint_0: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(mut, address = operation_data.token_mint_1 @ OperationError::InvalidMint)]
    pub token_mint_1: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(mut)]
    pub pda_token0: Account<'info, TokenAccount>,
    #[account(mut)]
    pub pda_token1: Account<'info, TokenAccount>,
    #[account(mut, address = operation_data.token_vault_0)]
    pub token_vault_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = operation_data.token_vault_1)]
    pub token_vault_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub memo_program: Program<'info, spl_memo::id>, // 也可用 UncheckedAccount
}