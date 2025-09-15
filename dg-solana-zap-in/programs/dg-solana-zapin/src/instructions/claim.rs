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
pub struct ClaimParams {
    /// Minimum USDC the user must receive after claiming
    pub min_payout: u64,
    pub fee_percentage: u32,
}

/// Claim handler:
/// - Settles accrued fees for the provided CLMM position
/// - Optionally swaps the non-recipient side to the recipient mint
/// - Checks min payout and transfers protocol fee to `fee_receiver_ata`
///
/// Params:
/// - min_payout: minimum amount of recipient mint the user must receive
/// - fee_percentage: protocol fee in basis points (1e4 = 100%)
pub fn handler(ctx: Context<Claim>, p: ClaimParams) -> Result<()> {
    let user_key = ctx.accounts.user.key();
    
    // Parse account data with minimal lifetimes; copy out only required fields
    let (pool_token_mint_0, pool_token_mint_1) = {
        let data = ctx.accounts.pool_state.try_borrow_data()?;
        let ps = PoolState::try_deserialize(&mut &data[..])?;
        (ps.token_mint_0, ps.token_mint_1)
    };
    let (pos_nft_mint) = {
        let data = ctx.accounts.personal_position.try_borrow_data()?;
        let pp = PersonalPositionState::try_deserialize(&mut &data[..])?;
        (pp.nft_mint)
    };
    
    let recipient_data = ctx.accounts.recipient_token_account.try_borrow_data()?;
    let recipient_info = spl_token::state::Account::unpack(&recipient_data)?;

    // Validate user's NFT account
    let nft_account_data = ctx.accounts.nft_account.try_borrow_data()?;
    let nft_account_info = spl_token::state::Account::unpack(&nft_account_data)?;
    require!(nft_account_info.owner == user_key, ErrorCode::Unauthorized);
    require!(nft_account_info.mint == pos_nft_mint, ErrorCode::InvalidMint);

    // Validate recipient account
    require!(recipient_info.owner == user_key, ErrorCode::Unauthorized);
    require!(recipient_info.mint == pool_token_mint_0 || recipient_info.mint == pool_token_mint_1, ErrorCode::InvalidMint);

    let usdc_mint = recipient_info.mint;

    // Create PDA to receive tokens
    let (pda, _bump) = Pubkey::find_program_address(
        &[b"claim_pda", user_key.as_ref(), &pos_nft_mint.to_bytes()],
        &crate::ID,
    );

    // Create PDA's token accounts
    let _pda_token_account_0 = get_associated_token_address_with_program_id(
        &pda,
        &pool_token_mint_0,
        &anchor_spl::token::ID,
    );
    let _pda_token_account_1 = get_associated_token_address_with_program_id(
        &pda,
        &pool_token_mint_1,
        &anchor_spl::token::ID,
    );

    // Record balances before claim (on both PDA vaults)
    let pre0 = load_token_amount(&ctx.accounts.token_vault_0)?;
    let pre1 = load_token_amount(&ctx.accounts.token_vault_1)?;
    let pre_usdc = if usdc_mint == pool_token_mint_0 { pre0 } else { pre1 };

    // seeds
    let signer_seeds_slice: [&[u8]; 3] = [b"claim_pda".as_ref(), user_key.as_ref(), &pos_nft_mint.to_bytes()];
    let _signer_seeds: &[&[&[u8]]] = &[&signer_seeds_slice];

    // 1) Settle fees only (liquidity=0)
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

    // Fees just claimed to the PDA
    let post0 = load_token_amount(&ctx.accounts.token_vault_0)?;
    let post1 = load_token_amount(&ctx.accounts.token_vault_1)?;
    let got0 = post0.checked_sub(pre0).ok_or(error!(ErrorCode::InvalidParams))?;
    let got1 = post1.checked_sub(pre1).ok_or(error!(ErrorCode::InvalidParams))?;
    if got0 == 0 && got1 == 0 {
        msg!("No rewards available to claim right now.");
        return Ok(());
    }

    // 2) Swap the non-USDC side fully into USDC
    let mut total_usdc_after_swap = if usdc_mint == pool_token_mint_0 { pre_usdc + got0 } else { pre_usdc + got1 };
    if (usdc_mint == pool_token_mint_0 && got1 > 0) || (usdc_mint == pool_token_mint_1 && got0 > 0) {
        let (in_vault, out_vault, in_mint, out_mint, is_base_input, amount_in) =
            if usdc_mint == pool_token_mint_0 {
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

        // Refresh USDC balance
        total_usdc_after_swap = if usdc_mint == pool_token_mint_0 {
            load_token_amount(&ctx.accounts.token_vault_0)?
        } else {
            load_token_amount(&ctx.accounts.token_vault_1)?
        };
    }

    // 3) Min received protection + compute fee and net
    require!(total_usdc_after_swap >= p.min_payout, ErrorCode::InvalidParams);

    // Compute expected fee_receiver ATA and validate the passed account
    let expected_fee_ata = get_associated_token_address_with_program_id(
        &ctx.accounts.config.fee_receiver,
        &usdc_mint,
        &anchor_spl::token::ID,
    );
    require!(ctx.accounts.fee_receiver_ata.key() == expected_fee_ata, ErrorCode::InvalidParams);

    // Compute fee (basis points) and net amount
    let fee_amount: u64 = ((total_usdc_after_swap as u128)
        .saturating_mul(p.fee_percentage as u128)
        / 10_000u128) as u64;
    let net_amount = total_usdc_after_swap.checked_sub(fee_amount).ok_or(error!(ErrorCode::InvalidParams))?;

    // Transfer from PDA vault: fee first, then user
    let transfer_from = if usdc_mint == pool_token_mint_0 {
        &ctx.accounts.token_vault_0
    } else {
        &ctx.accounts.token_vault_1
    };

    if fee_amount > 0 {
        let fee_transfer_accounts = Transfer {
            from:      transfer_from.to_account_info(),
            to:        ctx.accounts.fee_receiver_ata.to_account_info(),
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
