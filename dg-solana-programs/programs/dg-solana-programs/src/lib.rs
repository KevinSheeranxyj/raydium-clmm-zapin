use anchor_lang::prelude::*;
use anchor_lang::solana_program::pubkey;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use anchor_spl::token_interface::{Token2022, Mint as InterfaceMint, TokenAccount as InterfaceTokenAccount};
use anchor_spl::metadata::Metadata;
use anchor_lang::prelude::Rent;
use anchor_spl::memo::spl_memo;
use anchor_lang::prelude::Sysvar;
use anchor_lang::error::Error;
use raydium_amm_v3::libraries::{big_num::*, full_math::MulDiv, tick_math};
use anchor_spl::associated_token::AssociatedToken;
use std::str::FromStr;
use raydium_amm_v3::{
    cpi,
    program::AmmV3,
    states::{PoolState, AmmConfig, POSITION_SEED, TICK_ARRAY_SEED, ObservationState, TickArrayState, ProtocolPositionState, PersonalPositionState},
};

declare_id!("2f7mzs8Hqra1L6aLCEdoR4inNtNBFmNgsiuJMr8q2x7A");

/// NOTE: For ZapIn & ZapOut, we're leveraging the Raydium-Amm-v3 Protocol SDK to robost our requirement
#[program]
pub mod dg_solana_programs {
    use super::*;

    pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
        pubkey!("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"); // mainnet program ID

    // Initialize the PDA and set the authority
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let operation_data = &mut ctx.accounts.operation_data;
        operation_data.authority = ctx.accounts.authority.key();
        operation_data.initialized = true;
        msg!("Initialized PDA with authority: {}", operation_data.authority);
        Ok(())
    }

    #[event]
    pub struct DepositEvent {
        pub transfer_id: String,
        pub amount: u64,
        pub recipient: Pubkey,
    }

    // Deposit transfer details into PDA
    pub fn deposit(
        ctx: Context<Deposit>,
        transfer_id: String,
        operation_type: OperationType,
        action: Vec<u8>,
        amount: u64,
    ) -> Result<()> {
        let operation_data = &mut ctx.accounts.operation_data;

        // Verify transfer params
        require!(operation_data.initialized, OperationError::NotInitialized);
        require!(amount > 0, OperationError::InvalidAmount);
        require!(!transfer_id.is_empty(), OperationError::InvalidTransferId);

        // Perform SPL token transfer to program token account
        let cpi_accounts = Transfer {
            from: ctx.accounts.authority_ata.to_account_info(),
            to: ctx.accounts.program_token_account.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        // Store transfer details
        operation_data.transfer_id = transfer_id.clone();
        operation_data.amount = amount;
        operation_data.executed = false;

        operation_data.operation_type = operation_type.clone();
        operation_data.action = action;
        if let OperationType::Transfer = operation_type {
            let p: TransferParams = deserialize_params(&operation_data.action)?;
            operation_data.recipient = p.recipient;
        }
        if let OperationType::ZapIn = operation_type {
            let _p: ZapInParams = deserialize_params(&operation_data.action)?;
        }
        if let OperationType::ZapOut = operation_type {
            let _p: ZapOutParams = deserialize_params(&operation_data.action)?;
        }

        msg!(
            "Deposited transfer details: ID={}, Amount={}, Recipient={}",
            operation_data.transfer_id,
            operation_data.amount,
            operation_data.recipient,
        );
        emit!(DepositEvent {
            transfer_id: transfer_id.clone(),
            amount,
            recipient: operation_data.recipient,
        });
        Ok(())
    }

    // Execute the token transfer
    pub fn execute(ctx: Context<Execute>) -> Result<()> {
        require!(ctx.accounts.operation_data.initialized, OperationError::NotInitialized);
        require!(!ctx.accounts.operation_data.executed, OperationError::AlreadyExecuted);
        require!(ctx.accounts.operation_data.amount > 0, OperationError::InvalidAmount);

        let amount = ctx.accounts.operation_data.amount;
        let op_type = ctx.accounts.operation_data.operation_type.clone();
        let action  = ctx.accounts.operation_data.action.clone();

        let bump = ctx.bumps.operation_data;
        let seeds: &[&[u8]] = &[b"operation_data", &[bump]];
        let signer_seeds: &[&[&[u8]]] = &[seeds];

        let token_program = ctx.accounts.token_program.to_account_info();

        match op_type {
            OperationType::Transfer => {
                let p: TransferParams = deserialize_params(&action)?;
                require!(p.amount == amount, OperationError::InvalidParams);
                require!(ctx.accounts.recipient_token_account.owner == p.recipient, OperationError::Unauthorized);

                let cpi_accounts = Transfer {
                    from: ctx.accounts.program_token_account.to_account_info(),
                    to: ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.operation_data.to_account_info(),
                };
                token::transfer(
                    CpiContext::new_with_signer(token_program, cpi_accounts, signer_seeds),
                    amount,
                )?;
            }

            OperationType::ZapIn => {
                let p: ZapInParams = deserialize_params(&action)?;
                require!(p.tick_lower < p.tick_upper, OperationError::InvalidTickRange);

                let sp = {
                    let pool = ctx.accounts.pool_state.load()?; // Ref<PoolState>
                    pool.sqrt_price_x64
                };

                // 1) get sqrt prices for ticks
                let sa = tick_math::get_sqrt_price_at_tick(p.tick_lower)
                    .map_err(|_| error!(OperationError::InvalidParams))?;
                let sb = tick_math::get_sqrt_price_at_tick(p.tick_upper)
                    .map_err(|_| error!(OperationError::InvalidParams))?;

                require!(sa < sb, OperationError::InvalidTickRange);
                require!(sp >= sa && sp <= sb, OperationError::InvalidParams);

                let sa_u = U256::from(sa);
                let sb_u = U256::from(sb);
                let sp_u = U256::from(sp);

                let sp_minus_sa = if sp_u >= sa_u { sp_u - sa_u } else { return err!(OperationError::InvalidParams); };
                let sb_minus_sp = if sb_u >= sp_u { sb_u - sp_u } else { return err!(OperationError::InvalidParams); };

                let r_num = sb_u * sp_minus_sa;
                let r_den = sp_u * sb_minus_sp;

                let frac_den = r_den + r_num;
                require!(frac_den > U256::from(0u8), OperationError::InvalidParams);

                let is_base_input = ctx.accounts.program_token_account.mint == ctx.accounts.input_vault_mint.key();

                let amount_in_u256 = U256::from(amount);
                let swap_amount_u256 = if is_base_input {
                    amount_in_u256.mul_div_floor(r_num, frac_den).ok_or(error!(OperationError::InvalidParams))?
                } else {
                    amount_in_u256.mul_div_floor(r_den, frac_den).ok_or(error!(OperationError::InvalidParams))?
                };
                let swap_amount = swap_amount_u256.to_underflow_u64();
                require!(swap_amount as u128 <= u64::MAX as u128, OperationError::InvalidParams);

                // 记录前后余额
                let pre_out = get_token_balance(&ctx.accounts.output_token_account)?;
                let pre_in  = get_token_balance(&ctx.accounts.input_token_account)?;

                {
                    let clmm          = ctx.accounts.clmm_program.to_account_info();
                    let payer         = ctx.accounts.operation_data.to_account_info();
                    let amm_cfg       = ctx.accounts.amm_config.to_account_info();
                    let pool_state    = ctx.accounts.pool_state.to_account_info();
                    let in_acc        = ctx.accounts.input_token_account.to_account_info();
                    let out_acc       = ctx.accounts.output_token_account.to_account_info();
                    let in_vault      = ctx.accounts.input_vault.to_account_info();
                    let out_vault     = ctx.accounts.output_vault.to_account_info();
                    let obs           = ctx.accounts.observation_state.to_account_info();
                    let token_prog    = ctx.accounts.token_program.to_account_info();
                    let token2022     = ctx.accounts.token_program_2022.to_account_info();
                    let memo          = ctx.accounts.memo_program.to_account_info();
                    let in_mint       = ctx.accounts.input_vault_mint.to_account_info();
                    let out_mint      = ctx.accounts.output_vault_mint.to_account_info();

                    let swap_accounts = cpi::accounts::SwapSingleV2 {
                        payer: payer,
                        amm_config: amm_cfg,
                        pool_state: pool_state,
                        input_token_account: in_acc,
                        output_token_account: out_acc,
                        input_vault: in_vault,
                        output_vault: out_vault,
                        observation_state: obs,
                        token_program: token_prog,
                        token_program_2022: token2022,
                        memo_program: memo,
                        input_vault_mint: in_mint,
                        output_vault_mint: out_mint,
                    };

                    let other_amount_threshold = if is_base_input {
                        p.min_amount_out
                    } else {
                        p.other_amount_threshold
                    };

                    let swap_ctx = CpiContext::new(clmm, swap_accounts)
                        .with_signer(signer_seeds);

                    cpi::swap_v2(
                        swap_ctx,
                        swap_amount,
                        other_amount_threshold,
                        p.sqrt_price_limit_x64,
                        is_base_input,
                    )?;
                }

                // 成交后余额差
                let post_out = get_token_balance(&ctx.accounts.output_token_account)?;
                let post_in  = get_token_balance(&ctx.accounts.input_token_account)?;
                let received = post_out.checked_sub(pre_out).ok_or(error!(OperationError::InvalidParams))?;
                let spent    = pre_in.checked_sub(post_in).ok_or(error!(OperationError::InvalidParams))?;
                let remaining = amount.checked_sub(spent).ok_or(error!(OperationError::InvalidParams))?;

                {
                    let clmm            = ctx.accounts.clmm_program.to_account_info();
                    let payer           = ctx.accounts.operation_data.to_account_info();
                    let pool_state      = ctx.accounts.pool_state.to_account_info();
                    let nft_owner       = ctx.accounts.user.to_account_info();
                    let nft_mint        = ctx.accounts.position_nft_mint.to_account_info();
                    let nft_account     = ctx.accounts.position_nft_account.to_account_info();
                    let personal_pos    = ctx.accounts.personal_position.to_account_info();
                    let protocol_pos    = ctx.accounts.protocol_position.to_account_info();
                    let ta_lower        = ctx.accounts.tick_array_lower.to_account_info();
                    let ta_upper        = ctx.accounts.tick_array_upper.to_account_info();
                    let token_prog      = ctx.accounts.token_program.to_account_info();
                    let sys_prog        = ctx.accounts.system_program.to_account_info();
                    let rent            = ctx.accounts.rent.to_account_info();
                    let ata_prog        = ctx.accounts.associated_token_program.to_account_info();
                    let token_acc_0     = ctx.accounts.token_account_0.to_account_info();
                    let token_acc_1     = ctx.accounts.token_account_1.to_account_info();
                    let token_vault_0   = ctx.accounts.input_vault.to_account_info();
                    let token_vault_1   = ctx.accounts.output_vault.to_account_info();
                    let vault_0_mint    = ctx.accounts.input_vault_mint.to_account_info();
                    let vault_1_mint    = ctx.accounts.output_vault_mint.to_account_info();
                    let metadata_prog   = ctx.accounts.metadata_program.to_account_info();
                    let metadata        = ctx.accounts.metadata_account.to_account_info();
                    let token2022       = ctx.accounts.token_program_2022.to_account_info();

                    let open_accounts = cpi::accounts::OpenPositionV2 {
                        payer: payer,
                        pool_state: pool_state,
                        position_nft_owner: nft_owner,
                        position_nft_mint: nft_mint,
                        position_nft_account: nft_account,
                        personal_position: personal_pos,
                        protocol_position: protocol_pos,
                        tick_array_lower: ta_lower,
                        tick_array_upper: ta_upper,
                        token_program: token_prog,
                        system_program: sys_prog,
                        rent: rent,
                        associated_token_program: ata_prog,
                        token_account_0: token_acc_0,
                        token_account_1: token_acc_1,
                        token_vault_0: token_vault_0,
                        token_vault_1: token_vault_1,
                        vault_0_mint: vault_0_mint,
                        vault_1_mint: vault_1_mint,
                        metadata_program: metadata_prog,
                        metadata_account: metadata,
                        token_program_2022: token2022,
                    };

                    let open_ctx = CpiContext::new(clmm, open_accounts)
                        .with_signer(signer_seeds);

                    // TODO: pass real value
                    let tick_array_lower_start_index = 1;
                    let tick_array_upper_start_index = 1;
                    let liquidity = 1u128;
                    let amount_0_max_dummy = 2u64;
                    let amount_1_max_dummy = 12u64;
                    let with_matedata = false;
                    let base_flag = true;

                    cpi::open_position_v2(
                        open_ctx,
                        p.tick_lower,
                        p.tick_upper,
                        tick_array_lower_start_index,
                        tick_array_upper_start_index,
                        liquidity,
                        amount_0_max_dummy,
                        amount_1_max_dummy,
                        with_matedata,
                        Some(base_flag),
                    )?;
                }

                {
                    let (amount_0_max, amount_1_max) = if is_base_input {
                        (remaining, received)
                    } else {
                        (received, remaining)
                    };

                    let clmm          = ctx.accounts.clmm_program.to_account_info();
                    let nft_owner       = ctx.accounts.user.to_account_info();
                    let nft_account     = ctx.accounts.position_nft_account.to_account_info();
                    let pool_state      = ctx.accounts.pool_state.to_account_info();
                    let protocol_pos    = ctx.accounts.protocol_position.to_account_info();
                    let personal_pos    = ctx.accounts.personal_position.to_account_info();
                    let ta_lower        = ctx.accounts.tick_array_lower.to_account_info();
                    let ta_upper        = ctx.accounts.tick_array_upper.to_account_info();
                    let token_acc_0     = ctx.accounts.input_token_account.to_account_info();
                    let token_acc_1     = ctx.accounts.output_token_account.to_account_info();
                    let token_vault_0   = ctx.accounts.input_vault.to_account_info();
                    let token_vault_1   = ctx.accounts.output_vault.to_account_info();
                    let token_prog      = ctx.accounts.token_program.to_account_info();
                    let token2022       = ctx.accounts.token_program_2022.to_account_info();
                    let v0_mint         = ctx.accounts.input_vault_mint.to_account_info();
                    let v1_mint         = ctx.accounts.output_vault_mint.to_account_info();

                    let inc_accounts = cpi::accounts::IncreaseLiquidityV2 {
                        nft_owner: nft_owner  ,
                        nft_account: nft_account  ,
                        pool_state: pool_state  ,
                        protocol_position: protocol_pos  ,
                        personal_position: personal_pos  ,
                        tick_array_lower: ta_lower  ,
                        tick_array_upper: ta_upper  ,
                        token_account_0: token_acc_0  ,
                        token_account_1: token_acc_1  ,
                        token_vault_0: token_vault_0  ,
                        token_vault_1: token_vault_1  ,
                        token_program: token_prog  ,
                        token_program_2022: token2022  ,
                        vault_0_mint: v0_mint  ,
                        vault_1_mint: v1_mint  ,
                    };

                    let inc_ctx = CpiContext::new(clmm  , inc_accounts)
                        .with_signer(signer_seeds);

                    cpi::increase_liquidity_v2(
                        inc_ctx,
                        0, // Raydium calculate liquidity
                        amount_0_max,
                        amount_1_max,
                        Some(is_base_input),
                    )?;
                }
            }

            OperationType::ZapOut => {
                let p: ZapOutParams = deserialize_params(&action)?;

                // Basic auth and routing checks
                require!(ctx.accounts.recipient_token_account.owner == p.recipient, OperationError::Unauthorized);
                require!(ctx.accounts.input_token_account.owner == ctx.accounts.operation_data.key(), OperationError::Unauthorized);
                require!(ctx.accounts.output_token_account.owner == ctx.accounts.operation_data.key(), OperationError::Unauthorized);

                // 目标侧 mint 与收款 ATA 一致
                let want_mint = if p.want_base {
                    ctx.accounts.input_vault_mint.key()
                } else {
                    ctx.accounts.output_vault_mint.key()
                };
                require!(ctx.accounts.recipient_token_account.mint == want_mint, OperationError::InvalidMint);

                // 赎回前余额
                let pre0 = get_token_balance(&ctx.accounts.input_token_account)?;
                let pre1 = get_token_balance(&ctx.accounts.output_token_account)?;

                // 流动性与 tick 信息读取（不带出引用）
                let full_liquidity: u128 = ctx.accounts.personal_position.liquidity;
                require!(full_liquidity > 0, OperationError::InvalidParams);
                let burn_liquidity_u128: u128 = if p.liquidity_to_burn_u64 > 0 {
                    p.liquidity_to_burn_u64 as u128
                } else {
                    full_liquidity
                };
                require!(burn_liquidity_u128 <= full_liquidity, OperationError::InvalidParams);

                let tick_lower = ctx.accounts.personal_position.tick_lower_index;
                let tick_upper = ctx.accounts.personal_position.tick_upper_index;

                let sa = tick_math::get_sqrt_price_at_tick(tick_lower)
                    .map_err(|_| error!(OperationError::InvalidParams))?;
                let sb = tick_math::get_sqrt_price_at_tick(tick_upper)
                    .map_err(|_| error!(OperationError::InvalidParams))?;
                require!(sa < sb, OperationError::InvalidTickRange);

                let sp = {
                    let pool = ctx.accounts.pool_state.load()?;
                    pool.sqrt_price_x64
                };

                let (est0, est1) = amounts_from_liquidity_burn_q64(sa, sb, sp, burn_liquidity_u128);
                let min0 = apply_slippage_min(est0, p.slippage_bps);
                let min1 = apply_slippage_min(est1, p.slippage_bps);

                {
                    let clmm            = ctx.accounts.clmm_program.to_account_info();
                    let nft_owner       = ctx.accounts.user.to_account_info();
                    let nft_account     = ctx.accounts.position_nft_account.to_account_info();
                    let pool_state      = ctx.accounts.pool_state.to_account_info();
                    let protocol_pos    = ctx.accounts.protocol_position.to_account_info();
                    let personal_pos    = ctx.accounts.personal_position.to_account_info();
                    let ta_lower        = ctx.accounts.tick_array_lower.to_account_info();
                    let ta_upper        = ctx.accounts.tick_array_upper.to_account_info();
                    let rec0            = ctx.accounts.recipient_token_account_0.to_account_info();
                    let rec1            = ctx.accounts.recipient_token_account_1.to_account_info();
                    let token_vault_0   = ctx.accounts.input_vault.to_account_info();
                    let token_vault_1   = ctx.accounts.output_vault.to_account_info();
                    let token_prog      = ctx.accounts.token_program.to_account_info();
                    let token2022       = ctx.accounts.token_program_2022.to_account_info();
                    let v0_mint         = ctx.accounts.input_vault_mint.to_account_info();
                    let v1_mint         = ctx.accounts.output_vault_mint.to_account_info();
                    let memo            = ctx.accounts.memo_program.to_account_info();

                    let dec_accounts = cpi::accounts::DecreaseLiquidityV2 {
                        nft_owner: nft_owner ,
                        nft_account: nft_account ,
                        pool_state: pool_state ,
                        protocol_position: protocol_pos ,
                        personal_position: personal_pos,
                        tick_array_lower: ta_lower,
                        tick_array_upper: ta_upper,
                        recipient_token_account_0: rec0,
                        recipient_token_account_1: rec1,
                        token_vault_0: token_vault_0,
                        token_vault_1: token_vault_1,
                        token_program: token_prog,
                        token_program_2022: token2022,
                        vault_0_mint: v0_mint,
                        vault_1_mint: v1_mint,
                        memo_program: memo,
                    };

                    let dec_ctx = CpiContext::new(clmm, dec_accounts)
                        .with_signer(signer_seeds);

                    cpi::decrease_liquidity_v2(
                        dec_ctx,
                        burn_liquidity_u128,
                        min0,
                        min1,
                    )?;
                }

                // 实际到账
                let post0 = get_token_balance(&ctx.accounts.input_token_account)?;
                let post1 = get_token_balance(&ctx.accounts.output_token_account)?;
                let got0 = post0.checked_sub(pre0).ok_or(error!(OperationError::InvalidParams))?;
                let got1 = post1.checked_sub(pre1).ok_or(error!(OperationError::InvalidParams))?;

                // 需要单边时的 swap
                let (mut total_out, mut swap_amount, is_base_input) = if p.want_base {
                    (got0, got1, false)
                } else {
                    (got1, got0, true)
                };

                if swap_amount > 0 {
                    // ---------- Block F: swap_v2 for one-sided exit ----------
                    {
                        let clmm       = ctx.accounts.clmm_program.to_account_info();
                        let payer      = ctx.accounts.operation_data.to_account_info();
                        let amm_cfg    = ctx.accounts.amm_config.to_account_info();
                        let pool_state = ctx.accounts.pool_state.to_account_info();
                        let (in_acc, out_acc, in_vault, out_vault, in_mint, out_mint) =
                            if p.want_base {
                                (
                                    ctx.accounts.output_token_account.to_account_info(),
                                    ctx.accounts.input_token_account.to_account_info(),
                                    ctx.accounts.output_vault.to_account_info(),
                                    ctx.accounts.input_vault.to_account_info(),
                                    ctx.accounts.output_vault_mint.to_account_info(),
                                    ctx.accounts.input_vault_mint.to_account_info(),
                                )
                            } else {
                                (
                                    ctx.accounts.input_token_account.to_account_info(),
                                    ctx.accounts.output_token_account.to_account_info(),
                                    ctx.accounts.input_vault.to_account_info(),
                                    ctx.accounts.output_vault.to_account_info(),
                                    ctx.accounts.input_vault_mint.to_account_info(),
                                    ctx.accounts.output_vault_mint.to_account_info(),
                                )
                            };
                        let obs        = ctx.accounts.observation_state.to_account_info();
                        let token_prog = ctx.accounts.token_program.to_account_info();
                        let token2022  = ctx.accounts.token_program_2022.to_account_info();
                        let memo       = ctx.accounts.memo_program.to_account_info();

                        let swap_accounts = cpi::accounts::SwapSingleV2 {
                            payer: payer,
                            amm_config: amm_cfg,
                            pool_state: pool_state,
                            input_token_account: in_acc,
                            output_token_account: out_acc,
                            input_vault: in_vault,
                            output_vault: out_vault,
                            observation_state: obs,
                            token_program: token_prog,
                            token_program_2022: token2022,
                            memo_program: memo,
                            input_vault_mint: in_mint,
                            output_vault_mint: out_mint,
                        };

                        let swap_ctx = CpiContext::new(clmm, swap_accounts)
                            .with_signer(signer_seeds);

                        cpi::swap_v2(
                            swap_ctx,
                            swap_amount,
                            p.other_amount_threshold,
                            p.sqrt_price_limit_x64,
                            is_base_input,
                        )?;
                    }

                    // 刷新单边后的总量
                    if p.want_base {
                        let new_base = get_token_balance(&ctx.accounts.input_token_account)?;
                        total_out = new_base.checked_sub(pre0).ok_or(error!(OperationError::InvalidParams))?;
                    } else {
                        let new_quote = get_token_balance(&ctx.accounts.output_token_account)?;
                        total_out = new_quote.checked_sub(pre1).ok_or(error!(OperationError::InvalidParams))?;
                    }
                }

                // 最低支付保护
                require!(total_out >= p.min_payout, OperationError::InvalidParams);
                require!(total_out >= amount, OperationError::InvalidParams);

                // 输出到 recipient
                let from = if p.want_base {
                    ctx.accounts.input_token_account.to_account_info()
                } else {
                    ctx.accounts.output_token_account.to_account_info()
                };

                let cpi_accounts = Transfer {
                    from: from,
                    to: ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.operation_data.to_account_info(),
                };
                token::transfer(
                    CpiContext::new_with_signer(token_program, cpi_accounts, signer_seeds),
                    total_out,
                )?;

                // 若仓位已空，关闭仓位
                if ctx.accounts.personal_position.liquidity == 0 {
                    let clmm         = ctx.accounts.clmm_program.to_account_info();
                    let payer        = ctx.accounts.operation_data.to_account_info();
                    let pool_state   = ctx.accounts.pool_state.to_account_info();
                    let protocol_pos = ctx.accounts.protocol_position.to_account_info();
                    let personal_pos = ctx.accounts.personal_position.to_account_info();
                    let nft_owner    = ctx.accounts.user.to_account_info();
                    let nft_mint     = ctx.accounts.position_nft_mint.to_account_info();
                    let nft_account  = ctx.accounts.position_nft_account.to_account_info();
                    let token_prog   = ctx.accounts.token_program.to_account_info();
                    let token2022    = ctx.accounts.token_program_2022.to_account_info();
                    let sys_prog     = ctx.accounts.system_program.to_account_info();
                    let metadata_prog= ctx.accounts.metadata_program.to_account_info();
                    let metadata     = ctx.accounts.metadata_account.to_account_info();

                    let close_accounts = cpi::accounts::ClosePosition {
                        nft_owner: protocol_pos,
                        personal_position: personal_pos,
                        position_nft_mint: nft_mint,
                        position_nft_account: nft_account,
                        token_program: token_prog,
                        system_program: sys_prog,
                    };

                    let close_ctx = CpiContext::new(clmm, close_accounts)
                        .with_signer(signer_seeds);

                    cpi::close_position(close_ctx)?;
                    msg!("Position closed and NFT burned.");
                }
            }
        }

        ctx.accounts.operation_data.executed = true;
        Ok(())
    }

    // Modify PDA Authority
    pub fn modify_pda_authority(
        ctx: Context<ModifyPdaAuthority>,
        new_authority: Pubkey,
    ) -> Result<()> {
        let operation_data = &mut ctx.accounts.operation_data;

        // Verify current authority
        require!(operation_data.initialized, OperationError::NotInitialized);
        require!(
            operation_data.authority == ctx.accounts.current_authority.key(),
            OperationError::Unauthorized
        );

        // Update authority
        operation_data.authority = new_authority;
        msg!("Update PDA Authority to: {}", new_authority);
        Ok(())
    }
}

const Q64_U128: u128 = 1u128 << 64;

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

#[inline]
fn apply_slippage_min(estimate: u64, bps: u32) -> u64 {
    let num = (estimate as u128) * (10_000u128 - bps as u128);
    (num / 10_000u128) as u64
}

fn get_token_balance(acc: &InterfaceAccount<InterfaceTokenAccount>) -> Result<u64> {
    Ok(acc.amount)
}

/// Helper function to deserialize params
fn deserialize_params<T: AnchorDeserialize>(data: &[u8]) -> Result<T> {
    T::try_from_slice(data).map_err(|_| error!(OperationError::InvalidParams))
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
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

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        seeds = [b"operation_data"],
        bump
    )]
    pub operation_data: Box<Account<'info, OperationData>>,
    #[account(
        mut,
        constraint = authority.key() == operation_data.authority @ OperationError::Unauthorized
    )]
    pub authority: Signer<'info>,
    #[account(
        mut,
        constraint = authority_ata.owner == authority.key() @ OperationError::Unauthorized
    )]
    pub authority_ata: Account<'info, TokenAccount>,
    #[account(
        mut,
        constraint = program_token_account.owner == operation_data.key() @ OperationError::InvalidProgramAccount
    )]
    pub program_token_account: Account<'info, TokenAccount>,
    #[account(
        constraint = token_program.key() == token::ID @ OperationError::InvalidTokenProgram
    )]
    pub token_program: Program<'info, Token>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct PositionBounds {
    pub tick_lower: i32,
    pub tick_upper: i32,
}

#[derive(Accounts)]
#[instruction(bounds: PositionBounds)]
pub struct Execute<'info> {
    #[account(
        mut,
        seeds = [b"operation_data"],
        bump
    )]
    pub operation_data: Box<Account<'info, OperationData>>,

    #[account(mut)]
    pub program_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut)]
    pub recipient_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut, constraint = input_token_account.mint == input_vault_mint.key())]
    pub input_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, constraint = output_token_account.mint == output_vault_mint.key())]
    pub output_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(mut)]
    pub position_nft_mint: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(mut)]
    pub position_nft_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    #[account(mut, address = pool_state.load()?.token_vault_0)]
    pub input_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.token_vault_1)]
    pub output_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    #[account(address = pool_state.load()?.token_mint_0)]
    pub input_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = pool_state.load()?.token_mint_1)]
    pub output_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,
    /// CHECK: memo_program pub key
    #[account(address = spl_memo::id())]
    pub memo_program: UncheckedAccount<'info>,

    #[account(constraint = clmm_program.key() == RAYDIUM_CLMM_PROGRAM_ID)]
    pub clmm_program: Program<'info, AmmV3>,

    /// The destination token account for receive amount_0
    #[account(
        mut,
        constraint = recipient_token_account_0.mint == token_vault_0.mint
    )]
    pub recipient_token_account_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    /// The destination token account for receive amount_1
    #[account(
        mut,
        constraint = recipient_token_account_1.mint == token_vault_1.mint
    )]
    pub recipient_token_account_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(
        mut,
        seeds = [
        POSITION_SEED.as_bytes(),
        pool_state.key().as_ref(),
        &bounds.tick_lower.to_be_bytes(),
        &bounds.tick_upper.to_be_bytes(),
        ],
        seeds::program = clmm_program,
        bump,
        constraint = protocol_position.pool_id == pool_state.key(),
    )]
    pub protocol_position: Box<Account<'info, ProtocolPositionState>>,

    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    pub rent: Sysvar<'info, Rent>,

    #[account(mut)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,
    #[account(mut)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    #[account(
        mut,
        constraint = token_account_0.mint == token_vault_0.mint
    )]
    pub token_account_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(
        mut,
        constraint = token_account_1.mint == token_vault_1.mint
    )]
    pub token_account_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// CHECK: metadata_account
    #[account(mut)]
    pub metadata_account: UncheckedAccount<'info>,
    pub metadata_program: Program<'info, Metadata>,

    #[account(
        mut,
        constraint = token_vault_0.key() == pool_state.load()?.token_vault_0
    )]
    pub token_vault_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(
        mut,
        constraint = token_vault_1.key() == pool_state.load()?.token_vault_1
    )]
    pub token_vault_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    pub associated_token_program: Program<'info, AssociatedToken>,

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ModifyPdaAuthority<'info> {
    #[account(
        mut,
        seeds = [b"operation_data"],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    #[account(
        constraint = current_authority.key() == operation_data.authority @ OperationError::Unauthorized
    )]
    pub current_authority: Signer<'info>,
}

// Helper function to get swapped amount (placeholder; implement based on your needs)
fn get_swapped_amount(_output_token_account: &InterfaceAccount<InterfaceTokenAccount>) -> Result<u64> {
    Ok(0)
}

#[account]
#[derive(Default)]
pub struct OperationData {
    pub authority: Pubkey,
    pub initialized: bool,
    pub transfer_id: String,
    pub recipient: Pubkey,
    pub operation_type: OperationType,
    pub action: Vec<u8>, // Serialize operation-specific parameters
    pub amount: u64,
    pub executed: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum OperationType {
    Transfer,
    ZapIn,
    ZapOut,
}

impl Default for OperationType {
    fn default() -> Self {
        OperationType::Transfer
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct TransferParams {
    pub amount: u64,
    pub recipient: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ZapInParams {
    pub amount_in: u64,
    pub min_amount_out: u64,
    pub pool: Pubkey,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub sqrt_price_limit_x64: u128,
    pub other_amount_threshold: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ZapOutParams {
    pub want_base: bool,
    pub min_payout: u64,
    pub sqrt_price_limit_x64: u128,
    pub other_amount_threshold: u64,
    pub recipient: Pubkey,
    pub liquidity_to_burn_u64: u64,
    pub slippage_bps: u32,
}

impl OperationData {
    pub const LEN: usize =
        32 + // authority
            1 +  // initialized
            4 + 64 + // transfer_id (prefix + max size)
            32 + // recipient pubkey
            1 +  // operation_type (enum discriminator)
            4 + 128 + // action vec<u8> (prefix + max size)
            8 +  // amount
            1;   // executed
}

#[error_code]
pub enum OperationError {
    #[msg("PDA not initialized")]
    NotInitialized,
    #[msg("Invalid transfer amount")]
    InvalidAmount,
    #[msg("Invalid transfer ID")]
    InvalidTransferId,
    #[msg("Transfer already executed")]
    AlreadyExecuted,
    #[msg("Unauthorized access")]
    Unauthorized,
    #[msg("Invalid mint")]
    InvalidMint,
    #[msg("Invalid token program")]
    InvalidTokenProgram,
    #[msg("Invalid parameters")]
    InvalidParams,
    #[msg("Invalid tick range")]
    InvalidTickRange,
    #[msg("Invalid program account")]
    InvalidProgramAccount,
}