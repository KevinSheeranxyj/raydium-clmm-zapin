use anchor_lang::context::CpiContext;
use anchor_lang::{AccountDeserialize, AnchorDeserialize, err, error, require, require_keys_eq, Key, ToAccountInfo};
use anchor_lang::prelude::{AccountInfo, msg, Pubkey, Result, Context, UncheckedAccount};
use crate::{OperationData, OperationType};
use anchor_spl::token::{self, Transfer};
use anchor_spl::token::spl_token;
use raydium_amm_v3::libraries::{U256, MulDiv, tick_math};
use raydium_amm_v3::states::{PoolState, FEE_RATE_DENOMINATOR_VALUE, AmmConfig};
use crate::errors::ErrorCode;
use crate::state::ZapInParams;
use crate::Execute;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::solana_program::keccak::hash;
const Q64_U128: u128 = 1u128 << 64;

#[inline(never)]
pub fn do_swap_single_v2<'a>(
    clmm_prog_ai: AccountInfo<'a>,
    token_prog_ai: AccountInfo<'a>,
    token22_prog_ai: AccountInfo<'a>,
    memo_prog_ai: AccountInfo<'a>,
    amm_config: AccountInfo<'a>,
    pool_state: AccountInfo<'a>,
    observation: AccountInfo<'a>,
    input_acc: AccountInfo<'a>,
    output_acc: AccountInfo<'a>,
    input_vault: AccountInfo<'a>,
    output_vault: AccountInfo<'a>,
    input_mint: AccountInfo<'a>,
    output_mint: AccountInfo<'a>,
    operation_ai: AccountInfo<'a>,
    signer_seeds: &[&[&[u8]]],
    amount_in: u64,
    min_out: u64,
    is_base_input: bool,
) -> Result<()> {
    let accts = raydium_amm_v3::cpi::accounts::SwapSingleV2 {
        payer: operation_ai,
        amm_config,
        pool_state,
        input_token_account: input_acc,
        output_token_account: output_acc,
        input_vault,
        output_vault,
        observation_state: observation,
        token_program: token_prog_ai,
        token_program_2022: token22_prog_ai,
        memo_program: memo_prog_ai,
        input_vault_mint: input_mint,
        output_vault_mint: output_mint,
    };
    let ctx = CpiContext::new(clmm_prog_ai, accts).with_signer(signer_seeds);
    raydium_amm_v3::cpi::swap_v2(ctx, amount_in, min_out, 0, is_base_input)
}


#[inline(never)]
pub fn do_open_position_v2<'a>(
    clmm_prog_ai: AccountInfo<'a>,
    operation_ai: AccountInfo<'a>,
    pool_state: AccountInfo<'a>,
    user_ai: AccountInfo<'a>,
    position_nft_mint_ai: AccountInfo<'a>,
    position_nft_account: AccountInfo<'a>,
    personal_position: AccountInfo<'a>,
    protocol_pos: AccountInfo<'a>,
    ta_lower: AccountInfo<'a>,
    ta_upper: AccountInfo<'a>,
    token_prog_ai: AccountInfo<'a>,
    system_prog_ai: AccountInfo<'a>,
    rent_sysvar_ai: AccountInfo<'a>,
    ata_prog_ai: AccountInfo<'a>,
    pda_token0: AccountInfo<'a>,
    pda_token1: AccountInfo<'a>,
    vault0: AccountInfo<'a>,
    vault1: AccountInfo<'a>,
    mint0: AccountInfo<'a>,
    mint1: AccountInfo<'a>,
    metadata_prog_ai: AccountInfo<'a>,
    metadata_account_ai: AccountInfo<'a>,
    token22_prog_ai: AccountInfo<'a>,
    tick_lower: i32,
    tick_upper: i32,
    lower_start: i32,
    upper_start: i32,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let accts = raydium_amm_v3::cpi::accounts::OpenPositionV2 {
        payer: operation_ai,
        pool_state,
        position_nft_owner: user_ai,
        position_nft_mint: position_nft_mint_ai,
        position_nft_account: position_nft_account,
        personal_position,
        protocol_position: protocol_pos,
        tick_array_lower: ta_lower,
        tick_array_upper: ta_upper,
        token_program: token_prog_ai,
        system_program: system_prog_ai,
        rent: rent_sysvar_ai,
        associated_token_program: ata_prog_ai,
        token_account_0: pda_token0,
        token_account_1: pda_token1,
        token_vault_0: vault0,
        token_vault_1: vault1,
        vault_0_mint: mint0,
        vault_1_mint: mint1,
        metadata_program: metadata_prog_ai,
        metadata_account: metadata_account_ai,
        token_program_2022: token22_prog_ai,
    };
    let ctx = CpiContext::new(clmm_prog_ai, accts).with_signer(signer_seeds);
    raydium_amm_v3::cpi::open_position_v2(
        ctx,
        tick_lower,
        tick_upper,
        lower_start,
        upper_start,
        0u128,
        0u64,
        0u64,
        false,          // with_metadata（Raydium 会基于传入 metadata PDA 处理）
        Some(true),     // base_flag
    )
}

#[inline(never)]
pub fn do_increase_liquidity_v2<'a>(
    clmm_prog_ai: AccountInfo<'a>,
    user_ai: AccountInfo<'a>,
    pos_nft_account: AccountInfo<'a>,
    pool_state: AccountInfo<'a>,
    protocol_pos: AccountInfo<'a>,
    personal_position: AccountInfo<'a>,
    ta_lower: AccountInfo<'a>,
    ta_upper: AccountInfo<'a>,
    pda_token0: AccountInfo<'a>,
    pda_token1: AccountInfo<'a>,
    vault0: AccountInfo<'a>,
    vault1: AccountInfo<'a>,
    token_prog_ai: AccountInfo<'a>,
    token22_prog_ai: AccountInfo<'a>,
    mint0: AccountInfo<'a>,
    mint1: AccountInfo<'a>,
    signer_seeds: &[&[&[u8]]],
    amount_0_max: u64,
    amount_1_max: u64,
    base_flag: bool,
) -> Result<()> {
    let accts = raydium_amm_v3::cpi::accounts::IncreaseLiquidityV2 {
        nft_owner: user_ai,
        nft_account: pos_nft_account,
        pool_state,
        protocol_position: protocol_pos,
        personal_position,
        tick_array_lower: ta_lower,
        tick_array_upper: ta_upper,
        token_account_0: pda_token0,
        token_account_1: pda_token1,
        token_vault_0: vault0,
        token_vault_1: vault1,
        token_program: token_prog_ai,
        token_program_2022: token22_prog_ai,
        vault_0_mint: mint0,
        vault_1_mint: mint1,
    };
    let ctx = CpiContext::new(clmm_prog_ai, accts).with_signer(signer_seeds);
    raydium_amm_v3::cpi::increase_liquidity_v2(ctx, 0, amount_0_max, amount_1_max, Some(base_flag))
}

pub fn load_token_amount(ai: &AccountInfo) -> Result<u64> {
    let Some(acc) = unpack_token_account(ai) else {
        msg!("not a valid SPL token account: {}", ai.key);
        return err!(ErrorCode::InvalidParams);
    };
    Ok(acc.amount)
}

const TICK_ARRAY_SIZE: i32 = 88; //Raydium/UniV3 每个 TickArray 覆盖 88 个 tick 间隔

#[inline]
pub fn tick_array_start_index(tick_index: i32, tick_spacing: i32) -> i32 {
    let span = tick_spacing * TICK_ARRAY_SIZE;
    // floor 除法，处理负 tick
    let q = if tick_index >= 0 {
        tick_index / span
    } else {
        (tick_index - (span - 1)) / span
    };
    q * span
}

pub fn to_hex32(bytes: &[u8;32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 64];
    for (i, b) in bytes.iter().enumerate() {
        out[2*i]   = HEX[(b >> 4) as usize];
        out[2*i+1] = HEX[(b & 0x0f) as usize];
    }
    String::from_utf8(out.to_vec()).unwrap()
}


/// Helper function to deserialize params
pub fn deserialize_params<T: AnchorDeserialize>(data: &[u8]) -> Result<T> {
    T::try_from_slice(data).map_err(|_| error!(ErrorCode::InvalidParams))
}


fn try_deser_anchor_account<T: AccountDeserialize>(
    ai: &AccountInfo,
    expected_owner: &Pubkey,
    label: &str,
) -> anchor_lang::Result<T> {
    require_keys_eq!(*ai.owner, *expected_owner, ErrorCode::InvalidParams);
    let Ok(data) = ai.try_borrow_data() else {
        msg!("{}: borrow_data failed", label);
        return err!(ErrorCode::InvalidParams);
    };
    require!(data.len() >= 8, ErrorCode::InvalidParams);
    let mut bytes: &[u8] = &data;
    T::try_deserialize(&mut bytes).map_err(|_| {
        msg!("{}: anchor deserialize failed (wrong account type/len)", label);
        error!(ErrorCode::InvalidParams)
    })
}

/// True iff the account looks like `T` **and** is owned by `owner`.
fn is_anchor_account_owned<T: AccountDeserialize>(
    ai: &AccountInfo,
    owner: &Pubkey,
) -> bool {
    if *ai.owner != *owner { return false; }
    if let Ok(data) = ai.try_borrow_data() {
        if data.len() < 8 { return false; }
        let mut bytes: &[u8] = &data;
        T::try_deserialize(&mut bytes).is_ok()
    } else {
        false
    }
}


fn find_acc_idx(ras: &[AccountInfo], key: &Pubkey, label: &str) -> anchor_lang::Result<usize> {
    ras.iter()
        .position(|ai| *ai.key == *key)
        .ok_or_else(|| {
            msg!("missing account in remaining_accounts: {} = {}", label, key);
            error!(ErrorCode::InvalidParams)
        })
}

pub fn unpack_token_account(ai: &AccountInfo) -> Option<spl_token::state::Account> {
    // Only SPL Token or Token-2022 accounts can be unpacked
    if *ai.owner != token::ID && *ai.owner != anchor_spl::token_2022::ID {
        return None;
    }
    let Ok(data) = ai.try_borrow_data() else { return None; };
    if data.len() < spl_token::state::Account::LEN { return None; }
    spl_token::state::Account::unpack_from_slice(&data).ok()
}

#[inline]
pub fn apply_slippage_min(amount: u64, slippage_bps: u32) -> u64 {
    // min_out = amount * (1 - bps/1e4)
    let one = 10_000u128;
    let bps = (slippage_bps as u128).min(one);
    let num = (amount as u128).saturating_mul(one.saturating_sub(bps));
    (num / one) as u64
}

#[inline]
pub fn amounts_from_liquidity_burn_q64(
    sa: u128,    // sqrt(P_lower) in Q64.64
    sb: u128,    // sqrt(P_upper) in Q64.64
    sp: u128,    // sqrt(P_current) in Q64.64
    d_liq: u128, // ΔL (liquidity to burn)
) -> (u64, u64) {
    assert!(sa < sb, "invalid tick bounds");
    if d_liq == 0 {
        return (0, 0);
    }
    let sa_u = U256::from(sa);
    let sb_u = U256::from(sb);
    let sp_u = U256::from(sp);
    let dL_u = U256::from(d_liq);
    let q64  = U256::from(Q64_U128);
    let diff_sb_sa = sb_u - sa_u;

    let (amount0_u256, amount1_u256) = if sp_u <= sa_u {
        let num0 = dL_u * diff_sb_sa * q64;
        let den0 = sa_u * sb_u;
        let a0 = num0.mul_div_floor(U256::from(1u8), den0).unwrap_or(U256::from(0));
        (a0, U256::from(0))
    } else if sp_u >= sb_u {
        let a1 = (dL_u * diff_sb_sa).mul_div_floor(U256::from(1u8), q64).unwrap_or(U256::from(0));
        (U256::from(0), a1)
    } else {
        let num0 = dL_u * (sb_u - sp_u) * q64;
        let den0 = sp_u * sb_u;
        let a0 = num0.mul_div_floor(U256::from(1u8), den0).unwrap_or(U256::from(0));
        let a1 = (dL_u * (sp_u - sa_u)).mul_div_floor(U256::from(1u8), q64).unwrap_or(U256::from(0));
        (a0, a1)
    };

    let amount0 = amount0_u256.to_underflow_u64();
    let amount1 = amount1_u256.to_underflow_u64();
    (amount0, amount1)
}

/// 将 transfer_id 字符串转换为 32 字节哈希
pub fn transfer_id_hash_bytes(transfer_id: &str) -> [u8; 32] {
    let hash = hash(transfer_id.as_bytes());
    hash.to_bytes()
}

/// 执行 open_position 操作
#[inline(never)]
pub fn execute_open_position(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    p: &ZapInParams,
    pool_state: &PoolState,
) -> Result<()> {
    msg!("DEBUG: About to start open_position logic");
    
    // 计算 tick array 起始索引
    let lower_start = tick_array_start_index(p.tick_lower, pool_state.tick_spacing as i32);
    let upper_start = tick_array_start_index(p.tick_upper, pool_state.tick_spacing as i32);
    
    // 使用 helper 函数调用 Raydium open_position_v2
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"operation_data",
        transfer_id.as_ref(),
        &[ctx.bumps.operation_data]
    ]];
    msg!("do_open_position_v2");
    msg!("DEBUG: About to call do_open_position_v2");
    msg!("DEBUG: operation_data key before open_position: {}", ctx.accounts.operation_data.key());
    do_open_position_v2(
        ctx.accounts.clmm_program.to_account_info(),
        ctx.accounts.operation_data.to_account_info(),
        ctx.accounts.pool_state.to_account_info(),
        ctx.accounts.caller.to_account_info(),
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
        p.tick_lower,
        p.tick_upper,
        lower_start,
        upper_start,
        signer_seeds,
    )?;
    msg!("DEBUG: do_open_position_v2 completed successfully");
    Ok(())
}

/// 计算流动性数量
#[inline]
pub fn calculate_liquidity_amounts(
    p: &ZapInParams,
    is_base_input: bool,
) -> Result<(u64, u64)> {
    if is_base_input {
        Ok((p.amount_in, 0))
    } else {
        Ok((0, p.amount_in))
    }
}

/// 执行 increase_liquidity 操作
#[inline(never)]
pub fn execute_increase_liquidity(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    p: &ZapInParams,
    is_base_input: bool,
) -> Result<()> {
    msg!("DEBUG: About to start increase_liquidity logic");
    
    // 计算流动性数量
    let (pre0, pre1) = calculate_liquidity_amounts(p, is_base_input)?;
    
    // 使用 helper 函数调用 Raydium increase_liquidity_v2
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"operation_data",
        transfer_id.as_ref(),
        &[ctx.bumps.operation_data]
    ]];
    msg!("do_increase_liquidity_v2");
    msg!("DEBUG: About to call do_increase_liquidity_v2");
    do_increase_liquidity_v2(
        ctx.accounts.clmm_program.to_account_info(),
        ctx.accounts.caller.to_account_info(),
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
        is_base_input,
    )?;
    msg!("DEBUG: do_increase_liquidity_v2 completed successfully");
    Ok(())
}

/// 完成执行操作
#[inline]
pub fn finalize_execute(
    ctx: &mut Context<Execute>,
    transfer_id: [u8; 32],
) -> Result<()> {
    msg!("DEBUG: About to finalize execution");
    
    // 标记为已执行
    ctx.accounts.operation_data.executed = true;
    
    msg!("DEBUG: Execution finalized successfully");
    Ok(())
}

/// Helper function to deserialize TransferParams from Anchor format
pub fn deserialize_transfer_params_anchor(data: &[u8]) -> anchor_lang::Result<crate::state::TransferParams> {
    msg!("DEBUG: deserialize_transfer_params_anchor called with data len: {}", data.len());
    
    if data.is_empty() {
        msg!("DEBUG: TransferParams data is empty");
        return Err(error!(ErrorCode::InvalidParams));
    }
    
    msg!("DEBUG: TransferParams Anchor data - first 32 bytes: {:?}", &data[..data.len().min(32)]);
    
    // 找到实际数据的结束位置（第一个非零字节之后的所有零字节）
    let mut actual_len = data.len();
    for (i, &byte) in data.iter().enumerate().rev() {
        if byte != 0 {
            actual_len = i + 1;
            break;
        }
    }
    
    msg!("DEBUG: TransferParams Anchor actual data len: {}", actual_len);
    
    // 只反序列化实际使用的数据部分
    let actual_data = &data[..actual_len];
    
    // 使用 Anchor 反序列化
    match crate::state::TransferParams::try_from_slice(actual_data) {
        Ok(params) => {
            msg!("DEBUG: TransferParams Anchor deserialized successfully");
            Ok(params)
        }
        Err(e) => {
            msg!("DEBUG: TransferParams Anchor deserialization failed: {:?}", e);
            Err(error!(ErrorCode::InvalidParams))
        }
    }
}

/// 验证操作状态
#[inline(never)]
pub fn validate_operation_state(
    operation_data: &OperationData,
    caller_key: &Pubkey,
) -> Result<()> {
    msg!("DEBUG: About to check operation_data.initialized");
    require!(operation_data.initialized, ErrorCode::NotInitialized);
    msg!("DEBUG: operation_data.initialized check passed");
    
    msg!("DEBUG: About to check operation_data.executed");
    require!(!operation_data.executed, ErrorCode::AlreadyExecuted);
    msg!("DEBUG: operation_data.executed check passed");
    
    msg!("DEBUG: About to check operation_data.executor");
    require!(operation_data.executor == *caller_key, ErrorCode::Unauthorized);
    msg!("DEBUG: operation_data.executor check passed");
    
    msg!("DEBUG: About to check operation_data.operation_type");
    require!(operation_data.operation_type == OperationType::ZapIn, ErrorCode::InvalidParams);
    msg!("DEBUG: operation_data.operation_type check passed");
    
    Ok(())
}


/// 执行完整的 ZapIn 流程
#[inline(never)]
pub fn execute_zap_in_flow(
    ctx: &mut Context<Execute>,
    transfer_id: [u8; 32],
    p: &ZapInParams,
    transfer_amount: u64,
) -> Result<()> {
    // 验证账户地址和执行 swap
    let is_base_input = validate_and_execute_swap(ctx, p)?;
    
    // 执行 swap 操作
    execute_swap_operation_wrapper(ctx, transfer_id, p, is_base_input, transfer_amount)?;
    
    // 创建流动性头寸
    execute_open_position_with_loading(ctx, transfer_id, p)?;
    
    // 增加流动性
    execute_increase_liquidity(ctx, transfer_id, p, is_base_input)?;
    
    // 完成执行
    finalize_execute(ctx, transfer_id)?;
    
    Ok(())
}

/// 执行完整的 execute 流程
#[inline(never)]
pub fn execute_complete_flow(
    ctx: &mut Context<Execute>,
    transfer_id: [u8; 32],
) -> Result<()> {
    let caller_key = ctx.accounts.caller.key();
    msg!("DEBUG: caller_key: {}", caller_key);
    
    // 1. 验证状态
    validate_operation_state(&ctx.accounts.operation_data, &caller_key)?;
    
    // 2. 根据操作类型执行相应流程
    let amount = ctx.accounts.operation_data.amount;
    // 从存储的 action 字节中反序列化为 ZapInParams
    let params: ZapInParams = deserialize_params(&ctx.accounts.operation_data.action)?;
    // 目前仅支持 ZapIn
    require!(ctx.accounts.operation_data.operation_type == OperationType::ZapIn, ErrorCode::InvalidParams);
    execute_zap_in_flow(ctx, transfer_id, &params, amount)?;
    
    Ok(())
}

/// 验证账户地址
#[inline(never)]
pub fn validate_accounts_only(
    ctx: &Context<Execute>,
) -> Result<()> {
    validate_account_addresses_unchecked(
        &ctx.accounts.amm_config.to_account_info(),
        &ctx.accounts.observation_state.to_account_info(),
        &ctx.accounts.token_mint_0.to_account_info(),
        &ctx.accounts.token_mint_1.to_account_info(),
        &ctx.accounts.token_vault_0.to_account_info(),
        &ctx.accounts.token_vault_1.to_account_info(),
        &ctx.accounts.pool_state,
    )
}

/// 获取 is_base_input 标志
#[inline]
pub fn get_is_base_input(
    ctx: &Context<Execute>,
) -> Result<bool> {
    let pool_state = parse_pool_state(&ctx.accounts.pool_state)?;
    let is_base_input = pool_state.token_mint_0 == ctx.accounts.token_mint_0.key();
    msg!("pool_state.token_mint_0: {}, account.token_mint_0: {}", pool_state.token_mint_0, ctx.accounts.token_mint_0.key());
    Ok(is_base_input)
}

/// 紧凑的验证和执行函数
#[inline(never)]
pub fn validate_and_execute_swap(
    ctx: &Context<Execute>,
    _p: &ZapInParams,
) -> Result<bool> {
    // 验证账户地址
    validate_accounts_only(ctx)?;
    
    // 获取 is_base_input 标志
    get_is_base_input(ctx)
}

/// 执行 swap 操作
#[inline(never)]
pub fn execute_swap_operation_wrapper(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    p: &ZapInParams,
    is_base_input: bool,
    transfer_amount: u64,
) -> Result<()> {
    let pool_state = parse_pool_state(&ctx.accounts.pool_state)?;
    let pool_state_data = pool_state.clone();
    
    execute_swap_operation(
        ctx,
        transfer_id,
        p,
        &pool_state_data,
        is_base_input,
        transfer_amount,
    )?;
    Ok(())
}

/// 执行 open_position 操作（带 pool_state 加载）
#[inline(never)]
pub fn execute_open_position_with_loading(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    p: &ZapInParams,
) -> Result<()> {
    let pool_state = parse_pool_state(&ctx.accounts.pool_state)?;
    execute_open_position(ctx, transfer_id, p, &pool_state)
}

/// 解析 PoolState 数据
#[inline]
pub fn parse_pool_state(pool_state: &UncheckedAccount) -> Result<PoolState> {
    let pool_state_data = pool_state.try_borrow_data()?;
    PoolState::try_deserialize(&mut &pool_state_data[..])
}

#[inline]
pub fn parse_amm_config(amm_config: &UncheckedAccount) -> Result<AmmConfig> {
    let data = amm_config.try_borrow_data()?;
    AmmConfig::try_deserialize(&mut &data[..])
}

/// 验证单个账户地址
#[inline]
pub fn validate_single_account(
    account_key: &Pubkey,
    expected_key: &Pubkey,
    account_name: &str,
) -> Result<()> {
    msg!("DEBUG: {} key: {}, expected: {}", account_name, account_key, expected_key);
    require!(*account_key == *expected_key, ErrorCode::InvalidParams);
    msg!("DEBUG: {} validation passed", account_name);
    Ok(())
}

/// 验证账户地址是否匹配
#[inline(never)]
pub fn validate_account_addresses_unchecked(
    amm_config: &AccountInfo,
    observation_state: &AccountInfo,
    token_mint_0: &AccountInfo,
    token_mint_1: &AccountInfo,
    token_vault_0: &AccountInfo,
    token_vault_1: &AccountInfo,
    pool_state: &UncheckedAccount,
) -> Result<()> {
    msg!("DEBUG: About to validate account addresses with UncheckedAccount");
    
    // 手动解析 PoolState 数据
    let pool_state = parse_pool_state(pool_state)?;
    
    // 验证各个账户地址
    validate_single_account(&amm_config.key, &pool_state.amm_config, "amm_config")?;
    validate_single_account(&observation_state.key, &pool_state.observation_key, "observation_state")?;
    validate_single_account(&token_mint_0.key, &pool_state.token_mint_0, "token_mint_0")?;
    validate_single_account(&token_mint_1.key, &pool_state.token_mint_1, "token_mint_1")?;
    validate_single_account(&token_vault_0.key, &pool_state.token_vault_0, "token_vault_0")?;
    validate_single_account(&token_vault_1.key, &pool_state.token_vault_1, "token_vault_1")?;
    
    Ok(())
}

pub fn validate_account_addresses(
    amm_config: &AccountInfo,
    observation_state: &AccountInfo,
    token_mint_0: &AccountInfo,
    token_mint_1: &AccountInfo,
    token_vault_0: &AccountInfo,
    token_vault_1: &AccountInfo,
    pool_state: &PoolState,
) -> Result<()> {
    msg!("DEBUG: About to validate account addresses");
    msg!("DEBUG: amm_config key: {}, pool_state.amm_config: {}", amm_config.key, pool_state.amm_config);
    require!(*amm_config.key == pool_state.amm_config, ErrorCode::InvalidParams);
    msg!("DEBUG: amm_config validation passed");
    
    msg!("DEBUG: observation_state key: {}, pool_state.observation_key: {}", observation_state.key, pool_state.observation_key);
    require!(*observation_state.key == pool_state.observation_key, ErrorCode::InvalidParams);
    msg!("DEBUG: observation_state validation passed");
    
    msg!("DEBUG: token_mint_0 key: {}, pool_state.token_mint_0: {}", token_mint_0.key, pool_state.token_mint_0);
    require!(*token_mint_0.key == pool_state.token_mint_0, ErrorCode::InvalidParams);
    msg!("DEBUG: token_mint_0 validation passed");
    
    msg!("DEBUG: token_mint_1 key: {}, pool_state.token_mint_1: {}", token_mint_1.key, pool_state.token_mint_1);
    require!(*token_mint_1.key == pool_state.token_mint_1, ErrorCode::InvalidParams);
    msg!("DEBUG: token_mint_1 validation passed");
    
    msg!("DEBUG: token_vault_0 key: {}, pool_state.token_vault_0: {}", token_vault_0.key, pool_state.token_vault_0);
    require!(*token_vault_0.key == pool_state.token_vault_0, ErrorCode::InvalidParams);
    msg!("DEBUG: token_vault_0 validation passed");
    
    msg!("DEBUG: token_vault_1 key: {}, pool_state.token_vault_1: {}", token_vault_1.key, pool_state.token_vault_1);
    require!(*token_vault_1.key == pool_state.token_vault_1, ErrorCode::InvalidParams);
    msg!("DEBUG: token_vault_1 validation passed");
    
    Ok(())
}

/// 执行 swap 操作
pub fn execute_swap_operation(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    params: &ZapInParams,
    pool_state: &PoolState,
    base_input_flag: bool,
    transfer_amount: u64,
) -> Result<()> {
    msg!("DEBUG: About to start swap operation");
    
    // 计算 tick array 起始索引
    let lower_start = tick_array_start_index(params.tick_lower, pool_state.tick_spacing as i32);
    let upper_start = tick_array_start_index(params.tick_upper, pool_state.tick_spacing as i32);
    
    // 计算 tick array PDA 地址
    let pool_state_key = ctx.accounts.pool_state.key();
    let tick_array_lower_pda = Pubkey::find_program_address(
        &[
            b"tick_array",
            pool_state_key.as_ref(),
            &lower_start.to_le_bytes(),
        ],
        &ctx.accounts.clmm_program.key,
    ).0;
    
    let tick_array_upper_pda = Pubkey::find_program_address(
        &[
            b"tick_array",
            pool_state_key.as_ref(),
            &upper_start.to_le_bytes(),
        ],
        &ctx.accounts.clmm_program.key,
    ).0;
    
    // 转移资金到 PDA 账户
    let cpi_accounts = Transfer {
        from: ctx.accounts.program_token_account.to_account_info(),
        to: ctx.accounts.pda_token0.to_account_info(),
        authority: ctx.accounts.operation_data.to_account_info(),
    };
    let cpi_program = ctx.accounts.token_program.to_account_info();
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"operation_data",
        transfer_id.as_ref(),
        &[ctx.bumps.operation_data]
    ]];
    let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
    token::transfer(cpi_ctx, transfer_amount)?;
    
    // 执行 swap 逻辑
    execute_swap_logic(
        ctx,
        transfer_id,
        params,
        pool_state,
        base_input_flag,
        &tick_array_lower_pda,
        &tick_array_upper_pda,
    )?;
    
    Ok(())
}

/// 执行 swap 逻辑的核心部分
fn execute_swap_logic(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    params: &ZapInParams,
    pool_state: &PoolState,
    base_input_flag: bool,
    tick_array_lower_pda: &Pubkey,
    tick_array_upper_pda: &Pubkey,
) -> Result<()> {
    // 平衡代币比例 (swap_for_balance 逻辑)
    let sp = pool_state.sqrt_price_x64;
    // 解析 AmmConfig 数据
    let amm_cfg = parse_amm_config(&ctx.accounts.amm_config)?;
    
    // 费率是 1e6 制（ppm）
    let trade_fee_ppm: u32 = amm_cfg.trade_fee_rate.into();
    let protocol_fee_ppm: u32 = amm_cfg.protocol_fee_rate.into();
    
    // 计算 min_out
    let sp_u = U256::from(sp);
    let q64_u = U256::from(Q64_U128);
    let price_q64 = sp_u
        .mul_div_floor(sp_u, q64_u)
        .ok_or(error!(ErrorCode::InvalidParams))?;
    
    // 折扣 = (1 - fee_ppm/1e6) * (1 - slip_bps/1e4)
    let total_fee_ppm_u = U256::from(trade_fee_ppm) + U256::from(protocol_fee_ppm);
    let one_fee = U256::from(FEE_RATE_DENOMINATOR_VALUE); // 1_000_000
    let fee_factor_num = one_fee
        .checked_sub(total_fee_ppm_u)
        .ok_or(error!(ErrorCode::InvalidParams))?;
    
    let slip_bps = params.slippage_bps.min(10_000);
    let one_slip = U256::from(10_000u32); // 1e4
    let slip_factor_num = one_slip
        .checked_sub(U256::from(slip_bps))
        .ok_or(error!(ErrorCode::InvalidParams))?;
    
    let amount_in_u = U256::from(params.amount_in);
    
    // 先按价格得到理论输出
    let theoretical_out = if base_input_flag {
        amount_in_u
            .mul_div_floor(price_q64, q64_u)
            .ok_or(error!(ErrorCode::InvalidParams))?
    } else {
        amount_in_u
            .mul_div_floor(q64_u, price_q64.max(U256::from(1u8)))
            .ok_or(error!(ErrorCode::InvalidParams))?
    };
    
    // 先按费率（/1e6）折扣，再按滑点（/1e4）折扣，避免单位混用
    let after_fee = theoretical_out
        .mul_div_floor(fee_factor_num, one_fee)
        .ok_or(error!(ErrorCode::InvalidParams))?;
    let min_out_u = after_fee
        .mul_div_floor(slip_factor_num, one_slip)
        .ok_or(error!(ErrorCode::InvalidParams))?;
    let min_amount_out = min_out_u.to_underflow_u64();
    
    // 计算一次 swap 的分摊
    let sa = tick_math::get_sqrt_price_at_tick(params.tick_lower).map_err(|_| error!(ErrorCode::InvalidParams))?;
    let sb = tick_math::get_sqrt_price_at_tick(params.tick_upper).map_err(|_| error!(ErrorCode::InvalidParams))?;
    let sa_u = U256::from(sa);
    let sb_u = U256::from(sb);
    let sp_u2 = U256::from(sp);
    require!(sa < sb, ErrorCode::InvalidTickRange);
    msg!("sa: {}, sb: {}, sp: {}", sa_u, sb_u, sp_u2);
    let sp_minus_sa = if sp_u2 >= sa_u { sp_u2 - sa_u } else { return err!(ErrorCode::InvalidParams); };
    msg!("sp_minus_sa: {}", sp_minus_sa);
    let sb_minus_sp = if sb_u >= sp_u2 { sb_u - sp_u2 } else { return err!(ErrorCode::InvalidParams); };
    msg!("sb_minus_sp: {}", sb_minus_sp);
    
    // 直接用 mul_div_floor，确保不溢出
    let r_num = sb_u
        .mul_div_floor(sp_minus_sa, U256::from(1u8))
        .ok_or(error!(ErrorCode::InvalidParams))?;
    let r_den = sp_u2
        .mul_div_floor(sb_minus_sp, U256::from(1u8))
        .ok_or(error!(ErrorCode::InvalidParams))?;
    
    let frac_den = r_den.checked_add(r_num).ok_or(error!(ErrorCode::InvalidParams))?;
    require!(frac_den > U256::from(0u8), ErrorCode::InvalidParams);
    msg!("frac_den: {}, r_den: {}, r_num: {}", frac_den, r_den, r_num);
    let swap_amount_u = if base_input_flag {
        U256::from(params.amount_in)
            .mul_div_floor(r_num, frac_den)
            .ok_or(error!(ErrorCode::InvalidParams))?
    } else {
        U256::from(params.amount_in)
            .mul_div_floor(r_den, frac_den)
            .ok_or(error!(ErrorCode::InvalidParams))?
    };
    let swap_amount = swap_amount_u.to_underflow_u64();
    
    // 执行实际的 swap
    execute_actual_swap(
        ctx,
        transfer_id,
        base_input_flag,
        swap_amount,
        min_amount_out,
    )?;
    
    Ok(())
}

/// 执行实际的 swap 调用
fn execute_actual_swap(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    base_input_flag: bool,
    swap_amount: u64,
    min_amount_out: u64,
) -> Result<()> {
    // PDA signer
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"operation_data",
        transfer_id.as_ref(),
        &[ctx.bumps.operation_data]
    ]];

    // 组装输入/输出侧
    let (in_acc, out_acc, in_vault, out_vault, in_mint, out_mint) = if base_input_flag {
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
    
    msg!("raydium swap amount: {}", swap_amount);
    msg!("DEBUG: About to call do_swap_single_v2");
    
    // 准备 signer_seeds
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"operation_data",
        transfer_id.as_ref(),
        &[ctx.bumps.operation_data]
    ]];
    
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
        &signer_seeds,
        swap_amount,
        min_amount_out,
        base_input_flag,
    )?;
    
    msg!("DEBUG: do_swap_single_v2 completed successfully");
    Ok(())
}

