use anchor_lang::prelude::*;
use anchor_lang::{Accounts, AnchorDeserialize, AnchorSerialize, error, require};
use anchor_spl::token::{self, Token, Transfer};
use anchor_spl::token_2022::Token2022;
use anchor_spl::memo::spl_memo;
use raydium_amm_v3::libraries::tick_math;
use raydium_amm_v3::libraries::U256;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use raydium_amm_v3::{cpi, program::AmmV3, states::{PoolState, AmmConfig, ObservationState, PersonalPositionState, ProtocolPositionState, TickArrayState}};
use anchor_spl::token::spl_token;

use crate::errors::ErrorCode;
use crate::helpers::{load_token_amount, amounts_from_liquidity_burn_q64, apply_slippage_min};
use crate::state::GlobalConfig;
use anchor_lang::solana_program::program_pack::Pack;

const Q64_U128: u128 = 1u128 << 64;

#[derive(Accounts)]
pub struct Withdraw<'info> {
    /// CHECK: Forwarded to Raydium as nft_owner (no signature required)
    pub user: UncheckedAccount<'info>,

    /// CHECK: spl-memo
    #[account(address = spl_memo::id())]
    pub memo_program: UncheckedAccount<'info>,

    // Raydium CLMM program
    #[account(constraint = clmm_program.key() == crate::RAYDIUM_CLMM_PROGRAM_ID)]
    pub clmm_program: Program<'info, AmmV3>,
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,

    // Raydium CLMM accounts - UncheckedAccount passthrough
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub pool_state: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub amm_config: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub observation_state: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub protocol_position: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub personal_position: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub tick_array_lower: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub tick_array_upper: UncheckedAccount<'info>,

    // Token accounts
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub token_vault_0: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub token_vault_1: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub token_mint_0: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub token_mint_1: UncheckedAccount<'info>,

    // User NFT account
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub nft_account: UncheckedAccount<'info>,

    // User recipient account
    #[account(mut)]
    /// CHECK: Forwarded-only account for Raydium
    pub recipient_token_account: UncheckedAccount<'info>,

    /// Global config (to read fee_receiver)
    #[account(seeds = [b"global_config"], bump)]
    pub config: Account<'info, GlobalConfig>,

    // Explicit fee receiver ATA passed in to avoid remaining_accounts lifetimes
    #[account(mut)]
    /// CHECK: validated against expected ATA in handler
    pub fee_receiver_ata: UncheckedAccount<'info>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct WithdrawParams {
    pub want_base: bool,
    pub slippage_bps: u32,
    pub liquidity_to_burn_u64: u64,
    pub min_payout: u64,
    pub fee_percentage: u32,
}

/// Withdraw handler (ZapOut-like):
/// - Burns specified or full liquidity from the user's CLMM position
/// - Optionally swaps single-sided to the desired token (base/quote)
/// - Enforces min payout and transfers protocol fee to `fee_receiver_ata`
///
/// Params:
/// - want_base: when true, receive token0; when false, receive token1
/// - slippage_bps: slippage tolerance in basis points (1e4 = 100%)
/// - liquidity_to_burn_u64: liquidity to burn (0 means burn all)
/// - min_payout: minimum amount of desired token the user must receive
/// - fee_percentage: protocol fee in basis points (1e4 = 100%)
pub fn handler(ctx: Context<Withdraw>, p: WithdrawParams) -> Result<()> {
    let user_key = ctx.accounts.user.key();
    
    // Parse account data
    let pool_state_data = ctx.accounts.pool_state.try_borrow_data()?;
    let pool_state = PoolState::try_deserialize(&mut &pool_state_data[..])?;
    
    let personal_position_data = ctx.accounts.personal_position.try_borrow_data()?;
    let personal_position = PersonalPositionState::try_deserialize(&mut &personal_position_data[..])?;
    
    let recipient_data = ctx.accounts.recipient_token_account.try_borrow_data()?;
    let recipient_info = spl_token::state::Account::unpack(&recipient_data)?;

    // Validate user's NFT account
    let nft_account_data = ctx.accounts.nft_account.try_borrow_data()?;
    let nft_account_info = spl_token::state::Account::unpack(&nft_account_data)?;
    require!(nft_account_info.owner == user_key, ErrorCode::Unauthorized);
    require!(nft_account_info.mint == personal_position.nft_mint, ErrorCode::InvalidMint);

    // Validate recipient account
    let want_mint = if p.want_base { 
        pool_state.token_mint_0 
    } else { 
        pool_state.token_mint_1 
    };
    require!(recipient_info.owner == user_key, ErrorCode::Unauthorized);
    require!(recipient_info.mint == want_mint, ErrorCode::InvalidMint);

    // Read position (to estimate minimum outputs)
    let full_liquidity: u128 = personal_position.liquidity;
    require!(full_liquidity > 0, ErrorCode::InvalidParams);
    let burn_liq: u128 = if p.liquidity_to_burn_u64 > 0 { 
        p.liquidity_to_burn_u64 as u128 
    } else { 
        full_liquidity 
    };
    require!(burn_liq <= full_liquidity, ErrorCode::InvalidParams);

    let tick_lower = personal_position.tick_lower_index;
    let tick_upper = personal_position.tick_upper_index;

    let sa = tick_math::get_sqrt_price_at_tick(tick_lower).map_err(|_| error!(ErrorCode::InvalidParams))?;
    let sb = tick_math::get_sqrt_price_at_tick(tick_upper).map_err(|_| error!(ErrorCode::InvalidParams))?;
    require!(sa < sb, ErrorCode::InvalidTickRange);

    // Current price
    let sp = pool_state.sqrt_price_x64;

    // Estimate minimum outputs
    let (est0, est1) = amounts_from_liquidity_burn_q64(sa, sb, sp, burn_liq);
    let min0 = apply_slippage_min(est0, p.slippage_bps);
    let min1 = apply_slippage_min(est1, p.slippage_bps);

    // Create user's NFT ATA
    let nft_ata = get_associated_token_address_with_program_id(
        &user_key, 
        &personal_position.nft_mint, 
        &anchor_spl::token::ID
    );
    require!(nft_ata == ctx.accounts.nft_account.key(), ErrorCode::InvalidParams);

    // Balances before redeem
    let pre0 = load_token_amount(&ctx.accounts.token_vault_0)?;
    let pre1 = load_token_amount(&ctx.accounts.token_vault_1)?;

    // ---------- A: Redeem liquidity ----------
    {
        let dec_accounts = cpi::accounts::DecreaseLiquidityV2 {
            nft_owner:                 ctx.accounts.user.to_account_info(),
            nft_account:               ctx.accounts.nft_account.to_account_info(),
            pool_state:                ctx.accounts.pool_state.to_account_info(),
            protocol_position:         ctx.accounts.protocol_position.to_account_info(),
            personal_position:         ctx.accounts.personal_position.to_account_info(),
            tick_array_lower:          ctx.accounts.tick_array_lower.to_account_info(),
            tick_array_upper:          ctx.accounts.tick_array_upper.to_account_info(),
            recipient_token_account_0: ctx.accounts.token_vault_0.to_account_info(),
            recipient_token_account_1: ctx.accounts.token_vault_1.to_account_info(),
            token_vault_0:             ctx.accounts.token_vault_0.to_account_info(),
            token_vault_1:             ctx.accounts.token_vault_1.to_account_info(),
            token_program:             ctx.accounts.token_program.to_account_info(),
            token_program_2022:        ctx.accounts.token_program_2022.to_account_info(),
            vault_0_mint:              ctx.accounts.token_mint_0.to_account_info(),
            vault_1_mint:              ctx.accounts.token_mint_1.to_account_info(),
            memo_program:              ctx.accounts.memo_program.to_account_info(),
        };
        let dec_ctx = CpiContext::new(
            ctx.accounts.clmm_program.to_account_info(), 
            dec_accounts
        );
        cpi::decrease_liquidity_v2(dec_ctx, burn_liq, min0, min1)?;
    }

    // Increments after redeem
    let post0 = load_token_amount(&ctx.accounts.token_vault_0)?;
    let post1 = load_token_amount(&ctx.accounts.token_vault_1)?;
    let got0  = post0.checked_sub(pre0).ok_or(error!(ErrorCode::InvalidParams))?;
    let got1  = post1.checked_sub(pre1).ok_or(error!(ErrorCode::InvalidParams))?;

    // ---------- B: One-sided swap (optional) ----------
    let (mut total_out, swap_amount, is_base_input) = if p.want_base {
        (got0, got1, false)
    } else {
        (got1, got0, true)
    };

    if swap_amount > 0 {
        let (in_vault, out_vault, in_mint, out_mint) =
            if p.want_base {
                (ctx.accounts.token_vault_1.to_account_info(), 
                 ctx.accounts.token_vault_0.to_account_info(),
                 ctx.accounts.token_mint_1.to_account_info(), 
                 ctx.accounts.token_mint_0.to_account_info())
            } else {
                (ctx.accounts.token_vault_0.to_account_info(), 
                 ctx.accounts.token_vault_1.to_account_info(),
                 ctx.accounts.token_mint_0.to_account_info(), 
                 ctx.accounts.token_mint_1.to_account_info())
            };

        {
            let swap_accounts = cpi::accounts::SwapSingleV2 {
                payer:                 ctx.accounts.user.to_account_info(),
                amm_config:            ctx.accounts.amm_config.to_account_info(),
                pool_state:            ctx.accounts.pool_state.to_account_info(),
                input_token_account:   in_vault.clone(),
                output_token_account:  out_vault.clone(),
                input_vault:           in_vault,
                output_vault:          out_vault,
                observation_state:     ctx.accounts.observation_state.to_account_info(),
                token_program:         ctx.accounts.token_program.to_account_info(),
                token_program_2022:    ctx.accounts.token_program_2022.to_account_info(),
                memo_program:          ctx.accounts.memo_program.to_account_info(),
                input_vault_mint:      in_mint,
                output_vault_mint:     out_mint,
            };
            let swap_ctx = CpiContext::new(
                ctx.accounts.clmm_program.to_account_info(), 
                swap_accounts
            );
            cpi::swap_v2(swap_ctx, swap_amount, 0, 0, is_base_input)?;
        }
        // Refresh total after one-sided swap
        total_out = if p.want_base {
            load_token_amount(&ctx.accounts.token_vault_0)?
                .checked_sub(pre0).ok_or(error!(ErrorCode::InvalidParams))?
        } else {
            load_token_amount(&ctx.accounts.token_vault_1)?
                .checked_sub(pre1).ok_or(error!(ErrorCode::InvalidParams))?
        };
    }

    // ---------- C: Minimum received check ----------
    require!(total_out >= p.min_payout, ErrorCode::InvalidParams);

    // Compute expected fee_receiver ATA and validate the passed account
    let want_mint = if p.want_base { 
        pool_state.token_mint_0 
    } else { 
        pool_state.token_mint_1 
    };
    let expected_fee_ata = get_associated_token_address_with_program_id(
        &ctx.accounts.config.fee_receiver,
        &want_mint,
        &anchor_spl::token::ID,
    );
    require!(ctx.accounts.fee_receiver_ata.key() == expected_fee_ata, ErrorCode::InvalidParams);

    // Compute fee (basis points) and net amount
    let fee_amount: u64 = ((total_out as u128)
        .saturating_mul(p.fee_percentage as u128)
        / 10_000u128) as u64;
    let net_amount = total_out.checked_sub(fee_amount).ok_or(error!(ErrorCode::InvalidParams))?;

    // ---------- D: Transfers (fee first, then user) ----------
    let from_vault = if p.want_base { 
        &ctx.accounts.token_vault_0 
    } else { 
        &ctx.accounts.token_vault_1 
    };

    if fee_amount > 0 {
        let fee_transfer = Transfer {
            from:      from_vault.to_account_info(),
            to:        ctx.accounts.fee_receiver_ata.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                fee_transfer
            ),
            fee_amount,
        )?;
    }

    if net_amount > 0 {
        let user_transfer = Transfer {
            from:      from_vault.to_account_info(),
            to:        ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(), 
                user_transfer
            ),
            net_amount,
        )?;
    }

    Ok(())
}