use anchor_lang::context::CpiContext;
use anchor_lang::{AccountDeserialize, AnchorDeserialize, err, error, require, require_keys_eq};
use anchor_lang::prelude::{AccountInfo, msg, Pubkey};
use anchor_spl::token;
use anchor_spl::token::spl_token;
use raydium_amm_v3::libraries::U256;
use raydium_amm_v3::libraries::MulDiv;
use crate::errors::ErrorCode;
use anchor_lang::solana_program::program_pack::Pack;
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
) -> anchor_lang::Result<()> {
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
) -> anchor_lang::Result<()> {
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
) -> anchor_lang::Result<()> {
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

pub fn load_token_amount(ai: &AccountInfo) -> anchor_lang::Result<u64> {
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

#[inline]
fn apply_slippage_min(amount: u64, slippage_bps: u32) -> u64 {
    // min_out = amount * (1 - bps/1e4)
    let one = 10_000u128;
    let bps = (slippage_bps as u128).min(one);
    let num = (amount as u128).saturating_mul(one.saturating_sub(bps));
    (num / one) as u64
}

#[inline]
fn amounts_from_liquidity_burn_q64(
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

/// Helper function to deserialize params
pub fn deserialize_params<T: AnchorDeserialize>(data: &[u8]) -> anchor_lang::Result<T> {
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

fn unpack_token_account(ai: &AccountInfo) -> Option<spl_token::state::Account> {
    // Only SPL Token or Token-2022 accounts can be unpacked
    if *ai.owner != token::ID && *ai.owner != anchor_spl::token_2022::ID {
        return None;
    }
    let Ok(data) = ai.try_borrow_data() else { return None; };
    if data.len() < spl_token::state::Account::LEN { return None; }
    spl_token::state::Account::unpack_from_slice(&data).ok()
}

