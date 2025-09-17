use anchor_lang::context::CpiContext;
use anchor_lang::{AccountDeserialize, AnchorDeserialize, err, error, require, require_keys_eq, Key, ToAccountInfo};
use anchor_lang::prelude::{AccountInfo, msg, Pubkey, Result, Context, UncheckedAccount};
use crate::{OperationData, OperationType};
use anchor_spl::token::{self, Transfer};
use anchor_spl::token::spl_token;
use raydium_amm_v3::libraries::{U256, MulDiv, tick_math};
use raydium_amm_v3::states::{PoolState, FEE_RATE_DENOMINATOR_VALUE, AmmConfig};
use crate::errors::ErrorCode;
use crate::state::{ZapInParams, ActionData};
use crate::Execute;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::solana_program::keccak::hash;
const Q64_U128: u128 = 1u128 << 64;


#[inline(never)]
pub fn do_decrease_liquidity_v2<'a>(
    clmm_prog_ai: AccountInfo<'a>,
    nft_owner: AccountInfo<'a>,
    nft_account: AccountInfo<'a>,
    pool_state: AccountInfo<'a>,
    protocol_position: AccountInfo<'a>,
    personal_position: AccountInfo<'a>,
    tick_array_lower: AccountInfo<'a>,
    tick_array_upper: AccountInfo<'a>,
    recipient_token_account_0: AccountInfo<'a>,
    recipient_token_account_1: AccountInfo<'a>,
    token_vault_0: AccountInfo<'a>,
    token_vault_1: AccountInfo<'a>,
    token_program: AccountInfo<'a>,
    token_program_2022: AccountInfo<'a>,
    vault_0_mint: AccountInfo<'a>,
    vault_1_mint: AccountInfo<'a>,
    memo_program: AccountInfo<'a>,
    liquidity: u128,
    amount_0_min: u64,
    amount_1_min: u64,
) -> Result<()> {
    let accts = raydium_amm_v3::cpi::accounts::DecreaseLiquidityV2 {
        nft_owner,
        nft_account,
        pool_state,
        protocol_position,
        personal_position,
        tick_array_lower,
        tick_array_upper,
        recipient_token_account_0,
        recipient_token_account_1,
        token_vault_0,
        token_vault_1,
        token_program,
        token_program_2022,
        vault_0_mint,
        vault_1_mint,
        memo_program,
    };
    let ctx = CpiContext::new(clmm_prog_ai, accts);
    raydium_amm_v3::cpi::decrease_liquidity_v2(ctx, liquidity, amount_0_min, amount_1_min)
}

pub fn load_token_amount(ai: &AccountInfo) -> Result<u64> {
    let Some(acc) = unpack_token_account(ai) else {
        msg!("not a valid SPL token account: {}", ai.key);
        return err!(ErrorCode::InvalidParams);
    };
    Ok(acc.amount)
}

const TICK_ARRAY_SIZE: i32 = 88; // Raydium/UniV3 each TickArray spans 88 tick intervals

#[inline]
pub fn tick_array_start_index(tick_index: i32, tick_spacing: i32) -> i32 {
    let span = tick_spacing * TICK_ARRAY_SIZE;
    // Floor division, handling negative ticks
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

/// Convert transfer_id string into 32-byte hash
pub fn transfer_id_hash_bytes(transfer_id: &str) -> [u8; 32] {
    let hash = hash(transfer_id.as_bytes());
    hash.to_bytes()
}

/// Calculate liquidity amounts
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

/// Execute increase_liquidity operation
#[inline(never)]
pub fn execute_increase_liquidity(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    p: &ZapInParams,
    is_base_input: bool,
) -> Result<()> {
    msg!("DEBUG: About to start increase_liquidity logic");
    
    // Compute liquidity amounts
    let (pre0, pre1) = calculate_liquidity_amounts(p, is_base_input)?;
    msg!("DEBUG: pre0 = {}, pre1 = {}", pre0, pre1);
    
    // Use helper function to call Raydium increase_liquidity_v2
    let stored_id = ctx.accounts.operation_data.transfer_id;
    require!(stored_id == transfer_id, ErrorCode::InvalidTransferId);
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"operation_data",
        stored_id.as_ref(),
        &[ctx.bumps.operation_data]
    ]];
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
    msg!("DEBUG: About to call raydium_amm_v3::cpi::increase_liquidity_v2");
    msg!("DEBUG: amount_0_max = {}, amount_1_max = {}", amount_0_max, amount_1_max);
    msg!("DEBUG: base_flag = {}", base_flag);
    raydium_amm_v3::cpi::increase_liquidity_v2(ctx, 0, amount_0_max, amount_1_max, Some(base_flag))
}

/// Finalize execute operation
#[inline]
pub fn finalize_execute(
    ctx: &mut Context<Execute>,
    transfer_id: [u8; 32],
) -> Result<()> {
    msg!("DEBUG: About to finalize execution");
    
    // Mark as executed
    ctx.accounts.operation_data.executed = true;
    
    msg!("DEBUG: Execution finalized successfully");
    Ok(())
}

/// Validate operation state
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

/// Validate account addresses
#[inline(never)]
pub fn validate_accounts_only(
    ctx: &Context<Execute>,
) -> Result<bool> {
    msg!("DEBUG: About to validate account addresses and determine base input");
    // Use lightweight key parser to avoid full PoolState deserialization on stack
    let (amm_cfg_key, obs_key, mint0_key, mint1_key, vault0_key, vault1_key, tick_spacing) =
        parse_pool_state_keys(&ctx.accounts.pool_state)?;

    // Validate individual account addresses
    validate_single_account(&ctx.accounts.amm_config.to_account_info().key, &amm_cfg_key, "amm_config")?;
    validate_single_account(&ctx.accounts.observation_state.to_account_info().key, &obs_key, "observation_state")?;
    validate_single_account(&ctx.accounts.token_mint_0.to_account_info().key, &mint0_key, "token_mint_0")?;
    validate_single_account(&ctx.accounts.token_mint_1.to_account_info().key, &mint1_key, "token_mint_1")?;
    validate_single_account(&ctx.accounts.token_vault_0.to_account_info().key, &vault0_key, "token_vault_0")?;
    validate_single_account(&ctx.accounts.token_vault_1.to_account_info().key, &vault1_key, "token_vault_1")?;

    // Determine which side is the input token
    let is_base_input = mint0_key == *ctx.accounts.token_mint_0.to_account_info().key;
    msg!("DEBUG: determine base_input = {} (tick_spacing={})", is_base_input, tick_spacing);
    Ok(is_base_input)
}

/// Get is_base_input flag
#[inline(never)]
pub fn get_is_base_input(
    ctx: &Context<Execute>,
) -> Result<bool> {
    // Avoid deserializing PoolState to reduce stack use. Infer input side from deposited SPL TA.
    let Some(program_ta) = unpack_token_account(&ctx.accounts.program_token_account.to_account_info()) else {
        return err!(ErrorCode::InvalidParams);
    };
    let is_base_input = program_ta.mint == ctx.accounts.token_mint_0.key();
    msg!(
        "DEBUG: program_token_account.mint: {}, token_mint_0: {}, base_input: {}",
        program_ta.mint,
        ctx.accounts.token_mint_0.key(),
        is_base_input
    );
    Ok(is_base_input)
}

/// Execute swap operation
#[inline(never)]
pub fn execute_swap_operation_wrapper(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    p: &ZapInParams,
    is_base_input: bool,
    transfer_amount: u64,
) -> Result<()> {
    // Avoid deserializing PoolState here to reduce stack usage. Swap logic will
    // derive needed values from token vault balances and provided params.
    execute_swap_operation(
        ctx,
        transfer_id,
        p,
        is_base_input,
        transfer_amount,
    )?;
    Ok(())
}

/// Execute swap operation
pub fn execute_swap_operation(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    params: &ZapInParams,
    base_input_flag: bool,
    transfer_amount: u64,
) -> Result<()> {
    msg!("DEBUG: About to start swap operation (no pool_state/amm_config deserialization)");
    
    // Transfer funds to PDA account
    let program_balance = load_token_amount(&ctx.accounts.program_token_account.to_account_info())?;
    msg!("DEBUG: program_token_account balance: {}", program_balance);
    msg!("DEBUG: transfer_amount requested: {}", transfer_amount);
    require!(program_balance >= transfer_amount, ErrorCode::InvalidAmount);

    // Build signer seeds using the stored transfer_id on the PDA; ensure it matches param
    let stored_id = ctx.accounts.operation_data.transfer_id;
    require!(stored_id == transfer_id, ErrorCode::InvalidTransferId);
    let bump_seeds = ctx.bumps.operation_data;
    msg!("DEBUG: bump_seeds: {}", bump_seeds);
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"operation_data",
        stored_id.as_ref(),
        &[bump_seeds]
    ]];

    // Transfer from program_token_account -> PDA token account for the input side
    let dest_pda_ata = if base_input_flag {
        ctx.accounts.pda_token0.to_account_info()
    } else {
        ctx.accounts.pda_token1.to_account_info()
    };
    let cpi_accounts = Transfer {
        from: ctx.accounts.program_token_account.to_account_info(),
        to: dest_pda_ata,
        authority: ctx.accounts.operation_data.to_account_info(),
    };
    let cpi_program = ctx.accounts.token_program.to_account_info();
    let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
    msg!("DEBUG: About to invoke token::transfer CPI");
    token::transfer(cpi_ctx, transfer_amount)?;
    msg!("DEBUG: token::transfer CPI succeeded");

    // Execute swap logic without deserializing pool_state/amm_config. We'll
    // approximate price from vault balances and use slippage to compute min_out.
    execute_swap_logic_no_deser(
        ctx,
        stored_id,
        params,
        base_input_flag,
    )?;

    Ok(())
}

// New: swap logic that avoids deserializing PoolState/AmmConfig
fn execute_swap_logic_no_deser(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    params: &ZapInParams,
    base_input_flag: bool,
) -> Result<()> {
    msg!("DEBUG: execute_swap_logic_no_deser start");

    // Read vault balances to infer price: price ~ vault_out / vault_in
    let vault0_bal = load_token_amount(&ctx.accounts.token_vault_0.to_account_info())? as u128;
    let vault1_bal = load_token_amount(&ctx.accounts.token_vault_1.to_account_info())? as u128;
    msg!("DEBUG: token_vault_0 balance: {}", vault0_bal);
    msg!("DEBUG: token_vault_1 balance: {}", vault1_bal);

    // Prevent division by zero
    require!(vault0_bal > 0 && vault1_bal > 0, ErrorCode::InvalidParams);

    // Compute a Q64.64-ish price approximation as u128 (price_q64 ~ (vault1/vault0) * 2^64)
    // price_q64 = (vault1_bal << 64) / vault0_bal
    let price_q64_u128 = ((vault1_bal as u128) << 64) / (vault0_bal as u128);
    if price_q64_u128 == 0 {
        msg!("DEBUG: computed price_q64_u128 == 0");
        return err!(ErrorCode::InvalidParams);
    }
    msg!("DEBUG: price_q64_u128: {}", price_q64_u128);

    // Use u128 arithmetic where possible to reduce U256 allocations
    // Theoretical out using approximated price
    let theoretical_out_u128: u128 = if base_input_flag {
        // amount_in * price_q64 / Q64
        (params.amount_in as u128).saturating_mul(price_q64_u128) / Q64_U128
    } else {
        // amount_in * Q64 / price_q64
        (params.amount_in as u128).saturating_mul(Q64_U128) / price_q64_u128
    };

    // Apply slippage only (we don't have fees without parsing AmmConfig). This is
    // conservative: require user to provide enough slippage to accommodate fees.
    let slip_bps = params.slippage_bps.min(10_000) as u128;
    let one_slip = 10_000u128;
    let min_out_u128 = theoretical_out_u128.saturating_mul(one_slip.saturating_sub(slip_bps)) / one_slip;
    let min_amount_out = min_out_u128 as u64;
    msg!("DEBUG: approximated min_amount_out: {}", min_amount_out);
    msg!("DEBUG: tick_lower: {}, tick_upper: {}", params.tick_lower, params.tick_upper);

    // Compute swap_amount portion using ticks from params only (same as before)
    let sa = tick_math::get_sqrt_price_at_tick(params.tick_lower).map_err(|_| error!(ErrorCode::InvalidParams))?;
    let sb = tick_math::get_sqrt_price_at_tick(params.tick_upper).map_err(|_| error!(ErrorCode::InvalidParams))?;
    msg!("DEBUG: sa (Q64.64): {}", sa);
    msg!("DEBUG: sb (Q64.64): {}", sb);
    msg!("DEBUG: sp_q64_approx (Q64.64 approx): {}", price_q64_u128);

    // If provided ticks don't contain current price, fail (automatic derivation disabled to avoid stack/AV)
    if !(sa < price_q64_u128 && price_q64_u128 < sb) {
        msg!("DEBUG: provided tick bounds do not contain current price; automatic derivation disabled to avoid stack/write issues");
        return err!(ErrorCode::InvalidParams);
    }

    // For the fractional arithmetic that may overflow u128, use U256 but keep values on heap
    // to avoid large stack usage by allocating temporaries in the heap (Box).
    let sa_u = U256::from(sa);
    let sb_u = U256::from(sb);
    let sp_u = U256::from(price_q64_u128);

    let sp_minus_sa = if sp_u >= sa_u { sp_u - sa_u } else { return err!(ErrorCode::InvalidParams); };
    let sb_minus_sp = if sb_u >= sp_u { sb_u - sp_u } else { return err!(ErrorCode::InvalidParams); };

    // Compute r_num and r_den and move results to heap
    let r_num = Box::new(sb_u
        .mul_div_floor(sp_minus_sa, U256::from(1u8))
        .ok_or(error!(ErrorCode::InvalidParams))?);
    let r_den = Box::new(sp_u
        .mul_div_floor(sb_minus_sp, U256::from(1u8))
        .ok_or(error!(ErrorCode::InvalidParams))?);

    let frac_den = r_den.checked_add(*r_num).ok_or(error!(ErrorCode::InvalidParams))?;
    require!(frac_den > U256::from(0u8), ErrorCode::InvalidParams);

    let swap_amount_u = if base_input_flag {
        U256::from(params.amount_in)
            .mul_div_floor(*r_num, frac_den)
            .ok_or(error!(ErrorCode::InvalidParams))?
    } else {
        U256::from(params.amount_in)
            .mul_div_floor(*r_den, frac_den)
            .ok_or(error!(ErrorCode::InvalidParams))?
    };

    let swap_amount = swap_amount_u.to_underflow_u64();
    msg!("DEBUG: computed swap_amount: {}", swap_amount);

    // Execute the actual swap CPI
    execute_actual_swap(
        ctx,
        transfer_id,
        base_input_flag,
        swap_amount,
        min_amount_out,
    )?;

    msg!("DEBUG: execute_swap_logic_no_deser completed successfully");
    Ok(())
}

/// Execute the actual swap CPI
fn execute_actual_swap(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    base_input_flag: bool,
    swap_amount: u64,
    min_amount_out: u64,
) -> Result<()> {
    // Assemble input/output sides
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
    let bump_seeds = ctx.bumps.operation_data;
    msg!("DEBUG: bump_seeds: {}", bump_seeds);
    // Prepare signer_seeds for PDA authority over program-owned token accounts
    let signer_seeds: &[&[&[u8]]] = &[&[
        b"operation_data",
        transfer_id.as_ref(),
        &[bump_seeds]
    ]];
    msg!("DEBUG: signer_seeds = {:?}", signer_seeds);
    
    // Execute Raydium swap
    do_swap_single_v2(
        ctx.accounts.clmm_program.to_account_info(),
        ctx.accounts.token_program.to_account_info(),
        ctx.accounts.token_program_2022.to_account_info(),
        ctx.accounts.memo_program.to_account_info(),
        ctx.accounts.amm_config.to_account_info(),
        ctx.accounts.pool_state.to_account_info(),
        ctx.accounts.observation_state.to_account_info(),
        ctx.accounts.tick_array_lower.to_account_info(),
        ctx.accounts.tick_array_upper.to_account_info(),
        in_acc,
        out_acc,
        in_vault,
        out_vault,
        in_mint,
        out_mint,
        // ctx.accounts.operation_data.to_account_info(), // payer
        ctx.accounts.caller.to_account_info(), // payer changed to caller
        &signer_seeds,
        swap_amount,
        min_amount_out,
        base_input_flag,
    )?;
    
    msg!("DEBUG: do_swap_single_v2 completed successfully");
    Ok(())
}

#[inline(never)]
pub fn do_swap_single_v2<'a>(
    clmm_prog_ai: AccountInfo<'a>,
    token_prog_ai: AccountInfo<'a>,
    token22_prog_ai: AccountInfo<'a>,
    memo_prog_ai: AccountInfo<'a>,
    amm_config: AccountInfo<'a>,
    pool_state: AccountInfo<'a>,
    observation: AccountInfo<'a>,
    tick_array_lower_ai: AccountInfo<'a>,
    tick_array_upper_ai: AccountInfo<'a>,
    input_acc: AccountInfo<'a>,
    output_acc: AccountInfo<'a>,
    input_vault: AccountInfo<'a>,
    output_vault: AccountInfo<'a>,
    input_mint: AccountInfo<'a>,
    output_mint: AccountInfo<'a>,
    payer: AccountInfo<'a>,
    signer_seeds: &[&[&[u8]]],
    amount_in: u64,
    min_out: u64,
    is_base_input: bool,
) -> Result<()> {
    let accts = raydium_amm_v3::cpi::accounts::SwapSingleV2 {
        payer: payer,
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
    let remaining_accounts = vec![tick_array_lower_ai, tick_array_upper_ai];
    let ctx = CpiContext::new(clmm_prog_ai, accts)
        .with_signer(signer_seeds);
        // .with_remaining_accounts(remaining_accounts);
    // let ctx = CpiContext::new_with_signer(clmm_prog_ai, accts, signer_seeds);
    msg!("DEBUG: About to call raydium_amm_v3::cpi::swap_v2");
    msg!("DEBUG: amount_in: {}", amount_in);
    msg!("DEBUG: min_out: {}", min_out);
    msg!("DEBUG: is_base_input: {}", is_base_input);
    raydium_amm_v3::cpi::swap_v2(ctx, amount_in, min_out, 0, is_base_input)
}




/// Execute open_position operation (with pool_state loading)
#[inline(never)]
pub fn execute_open_position_with_loading(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    p: &ZapInParams,
) -> Result<()> {
    let pool_state = parse_pool_state(&ctx.accounts.pool_state)?;
    execute_open_position(ctx, transfer_id, p, &*pool_state)
}

/// Execute open_position operation
#[inline(never)]
pub fn execute_open_position(
    ctx: &Context<Execute>,
    transfer_id: [u8; 32],
    p: &ZapInParams,
    pool_state: &PoolState,
) -> Result<()> {
    msg!("DEBUG: About to start open_position logic");
    
    // Compute tick array start indices
    let lower_start = tick_array_start_index(p.tick_lower, pool_state.tick_spacing as i32);
    let upper_start = tick_array_start_index(p.tick_upper, pool_state.tick_spacing as i32);
    msg!("DEBUG: lower_start: {}, upper_start: {}", lower_start, upper_start);
    
    // Call Raydium open_position_v2 via helper
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
        false,          // with_metadata (Raydium will use provided metadata PDA)
        Some(true),     // base_flag
    )
}


/// Parse PoolState data
#[inline]
pub fn parse_pool_state(pool_state: &UncheckedAccount) -> Result<Box<PoolState>> {
    let pool_state_data = pool_state.try_borrow_data()?;
    let ps: PoolState = PoolState::try_deserialize(&mut &pool_state_data[..])?;
    Ok(Box::new(ps))
}

/// Lightweight parser: extract only keys and tick_spacing from PoolState raw bytes
/// This avoids allocating the full PoolState struct on the stack (which can trigger
/// stack-overflows when the struct is large). If the data is too small or parsing
/// fails, fall back to full `parse_pool_state()`.
#[inline]
pub fn parse_pool_state_keys(pool_state: &UncheckedAccount) -> Result<(Pubkey, Pubkey, Pubkey, Pubkey, Pubkey, Pubkey, i32)> {
    let data = match pool_state.try_borrow_data() {
        Ok(d) => d,
        Err(e) => return Err(e.into()),
    };
    // Account discriminator (8) + 6 * 32 (pubkeys) + 4 (tick_spacing) = 8 + 192 + 4 = 204
    if data.len() >= 204 {
        // safe slicing and conversion to arrays
        let amm_cfg_arr: [u8; 32] = data[8..40].try_into().map_err(|_| error!(ErrorCode::InvalidParams))?;
        let obs_arr: [u8; 32] = data[40..72].try_into().map_err(|_| error!(ErrorCode::InvalidParams))?;
        let mint0_arr: [u8; 32] = data[72..104].try_into().map_err(|_| error!(ErrorCode::InvalidParams))?;
        let mint1_arr: [u8; 32] = data[104..136].try_into().map_err(|_| error!(ErrorCode::InvalidParams))?;
        let vault0_arr: [u8; 32] = data[136..168].try_into().map_err(|_| error!(ErrorCode::InvalidParams))?;
        let vault1_arr: [u8; 32] = data[168..200].try_into().map_err(|_| error!(ErrorCode::InvalidParams))?;
        let tick_spacing_bytes: [u8; 4] = data[200..204].try_into().map_err(|_| error!(ErrorCode::InvalidParams))?;
        let amm_cfg = Pubkey::new_from_array(amm_cfg_arr);
        let obs = Pubkey::new_from_array(obs_arr);
        let mint0 = Pubkey::new_from_array(mint0_arr);
        let mint1 = Pubkey::new_from_array(mint1_arr);
        let vault0 = Pubkey::new_from_array(vault0_arr);
        let vault1 = Pubkey::new_from_array(vault1_arr);
        let tick_spacing = i32::from_le_bytes(tick_spacing_bytes);
        msg!("DEBUG: parse_pool_state_keys success: tick_spacing={}", tick_spacing);
        return Ok((amm_cfg, obs, mint0, mint1, vault0, vault1, tick_spacing));
    }

    // Fallback: full deserialization (may allocate on stack)
    msg!("DEBUG: parse_pool_state_keys falling back to full parse (data.len={})", data.len());
    let boxed = parse_pool_state(pool_state)?;
    Ok((
        boxed.amm_config,
        boxed.observation_key,
        boxed.token_mint_0,
        boxed.token_mint_1,
        boxed.token_vault_0,
        boxed.token_vault_1,
        boxed.tick_spacing as i32,
    ))
}

/// Validate a single account address
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

/// Validate whether account addresses match
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
    // Use lightweight parser to avoid full deserialization on stack
    let (amm_cfg_key, obs_key, mint0_key, mint1_key, vault0_key, vault1_key, _tick_spacing) =
        parse_pool_state_keys(pool_state)?;
    // Validate individual account addresses
    validate_single_account(&amm_config.key, &amm_cfg_key, "amm_config")?;
    validate_single_account(&observation_state.key, &obs_key, "observation_state")?;
    validate_single_account(&token_mint_0.key, &mint0_key, "token_mint_0")?;
    validate_single_account(&token_mint_1.key, &mint1_key, "token_mint_1")?;
    validate_single_account(&token_vault_0.key, &vault0_key, "token_vault_0")?;
    validate_single_account(&token_vault_1.key, &vault1_key, "token_vault_1")?;
    
    Ok(())
}
