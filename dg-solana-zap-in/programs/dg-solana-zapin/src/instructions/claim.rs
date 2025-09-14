use anchor_lang::prelude::*;
use anchor_lang::{Accounts, AnchorDeserialize, AnchorSerialize, error, require};
use anchor_spl::token::{self, Token, Transfer};
use anchor_spl::token_2022::Token2022;
use anchor_spl::memo::spl_memo;
use raydium_amm_v3::libraries::tick_math;
use raydium_amm_v3::{cpi, program::AmmV3, states::{PoolState, AmmConfig, ObservationState, PersonalPositionState, ProtocolPositionState, TickArrayState}};
use anchor_spl::token::spl_token;

use crate::errors::ErrorCode;
use crate::helpers::{load_token_amount, transfer_id_hash_bytes};
use crate::state::GlobalConfig;
use crate::events::ClaimEvent;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;

#[derive(Accounts)]
pub struct Claim<'info> {
    /// CHECK: 仅作转发给 Raydium 的 nft_owner（不要求签名）
    pub user: UncheckedAccount<'info>,

    /// CHECK: spl-memo
    #[account(address = spl_memo::id())]
    pub memo_program: UncheckedAccount<'info>,

    // Raydium CLMM 程序
    #[account(constraint = clmm_program.key() == crate::RAYDIUM_CLMM_PROGRAM_ID)]
    pub clmm_program: Program<'info, AmmV3>,
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,

    // Raydium CLMM 账户 - 使用 UncheckedAccount
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub pool_state: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub amm_config: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub observation_state: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub protocol_position: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub personal_position: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub tick_array_lower: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub tick_array_upper: UncheckedAccount<'info>,

    // Token 账户
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub token_vault_0: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub token_vault_1: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub token_mint_0: UncheckedAccount<'info>,
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub token_mint_1: UncheckedAccount<'info>,

    // 用户 NFT 账户
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub nft_account: UncheckedAccount<'info>,

    // 用户接收账户
    #[account(mut)]
    /// CHECK: 仅作转发给 Raydium 的账户
    pub recipient_token_account: UncheckedAccount<'info>,

    /// 全局配置（用于读取 fee_receiver）
    #[account(seeds = [b"global_config"], bump)]
    pub config: Account<'info, GlobalConfig>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ClaimParams {
    /// 领取后，最终到手的 USDC 不得低于该值
    pub min_payout: u64,
    pub fee_percentage: u32,
}

pub fn handler(ctx: Context<Claim>, p: ClaimParams) -> Result<()> {
    let user_key = ctx.accounts.user.key();
    
    // 解析账户数据
    let pool_state_data = ctx.accounts.pool_state.try_borrow_data()?;
    let pool_state = PoolState::try_deserialize(&mut &pool_state_data[..])?;
    
    let personal_position_data = ctx.accounts.personal_position.try_borrow_data()?;
    let personal_position = PersonalPositionState::try_deserialize(&mut &personal_position_data[..])?;
    
    let recipient_data = ctx.accounts.recipient_token_account.try_borrow_data()?;
    let recipient_info = spl_token::state::Account::unpack(&recipient_data)?;

    // 验证用户NFT账户
    let nft_account_data = ctx.accounts.nft_account.try_borrow_data()?;
    let nft_account_info = spl_token::state::Account::unpack(&nft_account_data)?;
    require!(nft_account_info.owner == user_key, ErrorCode::Unauthorized);
    require!(nft_account_info.mint == personal_position.nft_mint, ErrorCode::InvalidMint);

    // 验证接收账户
    require!(recipient_info.owner == user_key, ErrorCode::Unauthorized);
    require!(recipient_info.mint == pool_state.token_mint_0 || recipient_info.mint == pool_state.token_mint_1, ErrorCode::InvalidMint);

    let usdc_mint = recipient_info.mint;

    // 创建PDA用于接收代币
    let (pda, bump) = Pubkey::find_program_address(
        &[b"claim_pda", user_key.as_ref(), &personal_position.nft_mint.to_bytes()],
        &crate::ID,
    );

    // 创建PDA的token账户
    let pda_token_account_0 = get_associated_token_address_with_program_id(
        &pda,
        &pool_state.token_mint_0,
        &anchor_spl::token::ID,
    );
    let pda_token_account_1 = get_associated_token_address_with_program_id(
        &pda,
        &pool_state.token_mint_1,
        &anchor_spl::token::ID,
    );

    // 记录 claim 前余额（PDA 名下两边）
    let pre0 = load_token_amount(&ctx.accounts.token_vault_0)?;
    let pre1 = load_token_amount(&ctx.accounts.token_vault_1)?;
    let pre_usdc = if usdc_mint == pool_state.token_mint_0 { pre0 } else { pre1 };

    // seeds
    let signer_seeds_slice: [&[u8]; 3] = [b"claim_pda".as_ref(), user_key.as_ref(), &personal_position.nft_mint.to_bytes()];
    let signer_seeds: &[&[&[u8]]] = &[&signer_seeds_slice];

    // 1) 只结算手续费（liquidity=0）
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
        cpi::decrease_liquidity_v2(dec_ctx, 0u128, 0u64, 0u64)?;
    }

    // 刚领取到 PDA 的手续费数量
    let post0 = load_token_amount(&ctx.accounts.token_vault_0)?;
    let post1 = load_token_amount(&ctx.accounts.token_vault_1)?;
    let got0 = post0.checked_sub(pre0).ok_or(error!(ErrorCode::InvalidParams))?;
    let got1 = post1.checked_sub(pre1).ok_or(error!(ErrorCode::InvalidParams))?;
    if got0 == 0 && got1 == 0 {
        msg!("No rewards available to claim right now.");
        return Ok(());
    }

    // 2) 将非 USDC 一侧全量 swap 成 USDC
    let mut total_usdc_after_swap = if usdc_mint == pool_state.token_mint_0 { pre_usdc + got0 } else { pre_usdc + got1 };
    if (usdc_mint == pool_state.token_mint_0 && got1 > 0) || (usdc_mint == pool_state.token_mint_1 && got0 > 0) {
        let (in_vault, out_vault, in_mint, out_mint, is_base_input, amount_in) =
            if usdc_mint == pool_state.token_mint_0 {
                (ctx.accounts.token_vault_1.to_account_info(), 
                 ctx.accounts.token_vault_0.to_account_info(),
                 ctx.accounts.token_mint_1.to_account_info(), 
                 ctx.accounts.token_mint_0.to_account_info(),
                 false, got1)
            } else {
                (ctx.accounts.token_vault_0.to_account_info(), 
                 ctx.accounts.token_vault_1.to_account_info(),
                 ctx.accounts.token_mint_0.to_account_info(), 
                 ctx.accounts.token_mint_1.to_account_info(),
                 true, got0)
            };

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
        cpi::swap_v2(swap_ctx, amount_in, 0, 0, is_base_input)?;

        // 刷新 USDC 余额
        total_usdc_after_swap = if usdc_mint == pool_state.token_mint_0 {
            load_token_amount(&ctx.accounts.token_vault_0)?
        } else {
            load_token_amount(&ctx.accounts.token_vault_1)?
        };
    }

    // 3) 最小到手保护 + 计算手续费与净额
    require!(total_usdc_after_swap >= p.min_payout, ErrorCode::InvalidParams);

    // 计算 fee_receiver 的 ATA，并从 remaining_accounts 中取出对应账户
    let expected_fee_ata = get_associated_token_address_with_program_id(
        &ctx.accounts.config.fee_receiver,
        &usdc_mint,
        &anchor_spl::token::ID,
    );
    let idx = ctx
        .remaining_accounts
        .iter()
        .position(|ai| ai.key == &expected_fee_ata)
        .ok_or(error!(ErrorCode::InvalidParams))?;
    let fee_receiver_ata_ai = ctx.remaining_accounts[idx].to_account_info();

    // 计算手续费（按万分比）与净额
    let fee_amount: u64 = ((total_usdc_after_swap as u128)
        .saturating_mul(p.fee_percentage as u128)
        / 10_000u128) as u64;
    let net_amount = total_usdc_after_swap.checked_sub(fee_amount).ok_or(error!(ErrorCode::InvalidParams))?;

    // 从 PDA 金库转账：先给 fee，再给用户
    let transfer_from = if usdc_mint == pool_state.token_mint_0 {
        &ctx.accounts.token_vault_0
    } else {
        &ctx.accounts.token_vault_1
    };

    if fee_amount > 0 {
        let fee_transfer_accounts = Transfer {
            from:      transfer_from.to_account_info(),
            to:        fee_receiver_ata_ai,
            authority: ctx.accounts.user.to_account_info(),
        };
        let fee_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            fee_transfer_accounts,
        );
        token::transfer(fee_ctx, fee_amount)?;
    }

    if net_amount > 0 {
        let user_transfer_accounts = Transfer {
            from:      transfer_from.to_account_info(),
            to:        ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let user_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            user_transfer_accounts,
        );
        token::transfer(user_ctx, net_amount)?;
    }

    emit!(ClaimEvent {
        pool: ctx.accounts.pool_state.key(),
        beneficiary: user_key,
        mint: usdc_mint,
        amount: total_usdc_after_swap,
    });

    Ok(())
}
