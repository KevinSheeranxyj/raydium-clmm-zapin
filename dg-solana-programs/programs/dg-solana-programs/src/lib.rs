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
        ca: Pubkey,
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
        operation_data.ca = ca;

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

        let amount = ctx.accounts.operation_data.amount; //
        let op_type = ctx.accounts.operation_data.operation_type.clone();
        let action  = ctx.accounts.operation_data.action.clone();
        let ca = ctx.accounts.operation_data.ca;

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

                // check pool token_a_mint == ca
                require!(p.token_a_mint == ca, OperationError::InvalidMint);

                // --- if amount < p.amount_in then refund to user DA ---
                if amount < p.amount_in {
                    // sanity check: return fund account must be aligned with program token account
                    require!(
                         ctx.accounts.recipient_token_account.mint == ctx.accounts.program_token_account.mint,
                         OperationError::InvalidMint
                    );

                    let refund_accounts = Transfer {
                        from: ctx.accounts.program_token_account.to_account_info(),
                        to: ctx.accounts.recipient_token_account.to_account_info(),
                        authority: ctx.accounts.operation_data.to_account_info()
                    };
                    token::transfer(
                        CpiContext::new_with_signer(
                            token_program,
                            refund_accounts,
                            signer_seeds, // PDA as authority
                        ),
                        amount, // actual refund account
                    )?;

                    // make executed as true
                    ctx.accounts.operation_data.executed = true;
                    msg!(
                        "ZapIn refund: expected amount_in = {}, received_amount = {}, refund all to recipient",
                        p.amount_in,
                        amount
                    );
                    return Ok(());
                }

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
                let amount_in = p.amount_in;

                let amount_in_u256 = U256::from(amount_in); // User Input Amount
                let swap_amount_u256 = if is_base_input {
                    amount_in_u256.mul_div_floor(r_num, frac_den).ok_or(error!(OperationError::InvalidParams))?
                } else {
                    amount_in_u256.mul_div_floor(r_den, frac_den).ok_or(error!(OperationError::InvalidParams))?
                };
                let swap_amount = swap_amount_u256.to_underflow_u64();
                require!(swap_amount as u128 <= u64::MAX as u128, OperationError::InvalidParams);

                let to_acc_info = if is_base_input {
                    ctx.accounts.input_token_account.to_account_info()
                } else {
                    ctx.accounts.output_token_account.to_account_info()
                };
                // transfer from program_tokne_account -> input_token_account
                let fund_move = anchor_spl::token::Transfer {
                    from:      ctx.accounts.program_token_account.to_account_info(),
                    to:        to_acc_info,
                    authority: ctx.accounts.operation_data.to_account_info(), // PDA 作为 authority
                };
                let fund_move_ctx = CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    fund_move,
                ).with_signer(signer_seeds);
                // 把用户deposit进来的amount挪到ZapIn 那一侧
                anchor_spl::token::transfer(fund_move_ctx, amount)?;

                // 记录前后余额
                let pre_out = get_token_balance(&mut ctx.accounts.output_token_account)?;
                let pre_in  = get_token_balance(&mut ctx.accounts.input_token_account)?;

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
                let post_out = get_token_balance(&mut ctx.accounts.output_token_account)?;
                let post_in  = get_token_balance(&mut ctx.accounts.input_token_account)?;
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

                    let pool = ctx.accounts.pool_state.load()?;
                    let tick_spacing: i32 = pool.tick_spacing.into();

                    let lower_start = tick_array_start_index(p.tick_lower, tick_spacing);
                    let upper_start = tick_array_start_index(p.tick_upper, tick_spacing);

                    {
                        let ta_lower = ctx.accounts.tick_array_lower.load()?;
                        let ta_upper = ctx.accounts.tick_array_upper.load()?;

                        require!(ta_lower.start_tick_index == lower_start, OperationError::InvalidParams);
                        require!(ta_upper.start_tick_index == upper_start, OperationError::InvalidParams);
                    }


                    let with_metadata = false;
                    let base_flag = Some(true);

                    cpi::open_position_v2(
                        open_ctx,
                        p.tick_lower,
                        p.tick_upper,
                        lower_start,
                        upper_start,
                        0u128,
                        0u64,
                        0u64,
                        with_metadata,
                        base_flag,
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


            OperationType::Claim => {
                let p: ClaimParams = deserialize_params(&action)?;
                // 要求收款 ATA 的 mint 就是 USDC，且它必须与池子的 token0 或 token1 之一相等
                let usdc_mint = ctx.accounts.recipient_token_account.mint;
                require!(
                    usdc_mint == ctx.accounts.input_vault_mint.key() || usdc_mint == ctx.accounts.output_vault_mint.key(),
                    OperationError::InvalidMint
                    );

                // 记录 claim 前余额（PDA 名下的两边 Token 账户）
                let pre0 = get_token_balance(&mut ctx.accounts.input_token_account)?;   // token0
                let pre1 = get_token_balance(&mut ctx.accounts.output_token_account)?;  // token1
                let pre_usdc = if usdc_mint == ctx.accounts.input_vault_mint.key() { pre0 } else { pre1 };

                // ---- 第一步：只结算手续费（liquidity=0）----
                // 注意：这里的收款账户指向 PDA 名下的两边 Token 账户（input_token_account/output_token_account），
                // 这样便于后面直接做单边 swap。
                {
                    let dec_accounts = cpi::accounts::DecreaseLiquidityV2 {
                        nft_owner:         ctx.accounts.user.to_account_info(),
                        nft_account:       ctx.accounts.position_nft_account.to_account_info(),
                        pool_state:        ctx.accounts.pool_state.to_account_info(),
                        protocol_position: ctx.accounts.protocol_position.to_account_info(),
                        personal_position: ctx.accounts.personal_position.to_account_info(),
                        tick_array_lower:  ctx.accounts.tick_array_lower.to_account_info(),
                        tick_array_upper:  ctx.accounts.tick_array_upper.to_account_info(),
                        recipient_token_account_0: ctx.accounts.input_token_account.to_account_info(),   // token0 -> PDA
                        recipient_token_account_1: ctx.accounts.output_token_account.to_account_info(),  // token1 -> PDA
                        token_vault_0:     ctx.accounts.token_vault_0.to_account_info(),
                        token_vault_1:     ctx.accounts.token_vault_1.to_account_info(),
                        token_program:     ctx.accounts.token_program.to_account_info(),
                        token_program_2022:ctx.accounts.token_program_2022.to_account_info(),
                        vault_0_mint:      ctx.accounts.input_vault_mint.to_account_info(),
                        vault_1_mint:      ctx.accounts.output_vault_mint.to_account_info(),
                        memo_program:      ctx.accounts.memo_program.to_account_info(),
                    };
                    let dec_ctx = CpiContext::new(
                        ctx.accounts.clmm_program.to_account_info(),
                        dec_accounts,
                    ).with_signer(signer_seeds);
                    // Only claim：liquidity=0，minimum value is 0
                    cpi::decrease_liquidity_v2(dec_ctx, 0u128, 0u64, 0u64)?;
                }

                // 计算刚刚 claim 到手的手续费数量
                let post0 = get_token_balance(&mut ctx.accounts.input_token_account)?;
                let post1 = get_token_balance(&mut ctx.accounts.output_token_account)?;
                let got0 = post0.checked_sub(pre0).ok_or(error!(OperationError::InvalidParams))?;
                let got1 = post1.checked_sub(pre1).ok_or(error!(OperationError::InvalidParams))?;

                // ---- 第二步：把非 USDC 的一边全部换成 USDC ----
                let mut total_usdc_after_swap: u64;
                if usdc_mint == ctx.accounts.input_vault_mint.key() {
                    // USDC 是 token0，需把 got1(=token1) 全部换成 token0
                    total_usdc_after_swap = pre_usdc + got0; // 先加上 token0 的手续费
                    if got1 > 0 {
                        let swap_accounts = cpi::accounts::SwapSingleV2 {
                            payer:               ctx.accounts.operation_data.to_account_info(),
                            amm_config:          ctx.accounts.amm_config.to_account_info(),
                            pool_state:          ctx.accounts.pool_state.to_account_info(),
                            input_token_account: ctx.accounts.output_token_account.to_account_info(), // in: token1 (PDA)
                            output_token_account:ctx.accounts.input_token_account.to_account_info(),  // out: token0 (PDA)
                            input_vault:         ctx.accounts.output_vault.to_account_info(),         // vault1
                            output_vault:        ctx.accounts.input_vault.to_account_info(),          // vault0
                            observation_state:   ctx.accounts.observation_state.to_account_info(),
                            token_program:       ctx.accounts.token_program.to_account_info(),
                            token_program_2022:  ctx.accounts.token_program_2022.to_account_info(),
                            memo_program:        ctx.accounts.memo_program.to_account_info(),
                            input_vault_mint:    ctx.accounts.output_vault_mint.to_account_info(),
                            output_vault_mint:   ctx.accounts.input_vault_mint.to_account_info(),
                        };
                        let swap_ctx = CpiContext::new(
                            ctx.accounts.clmm_program.to_account_info(),
                            swap_accounts,
                        ).with_signer(signer_seeds);

                        // is_base_input=false: 从 token1 -> token0
                        cpi::swap_v2(
                            swap_ctx,
                            got1,                       // 全部换
                            p.other_amount_threshold,   // e.g. 0
                            p.sqrt_price_limit_x64,     // 0 表示默认不限制
                            false,
                        )?;
                        // 刷新 USDC(token0) 余额增量
                        let new_token0 = get_token_balance(&mut ctx.accounts.input_token_account)?;
                        total_usdc_after_swap = new_token0; // 由于 pre0 + got0 + swap_out 都在同一账户里，直接取最新余额更稳
                    }
                } else {
                    // USDC 是 token1，需把 got0(=token0) 全部换成 token1
                    total_usdc_after_swap = pre_usdc + got1;
                    if got0 > 0 {
                        let swap_accounts = cpi::accounts::SwapSingleV2 {
                            payer:               ctx.accounts.operation_data.to_account_info(),
                            amm_config:          ctx.accounts.amm_config.to_account_info(),
                            pool_state:          ctx.accounts.pool_state.to_account_info(),
                            input_token_account: ctx.accounts.input_token_account.to_account_info(),  // in: token0 (PDA)
                            output_token_account:ctx.accounts.output_token_account.to_account_info(), // out: token1 (PDA/USDC)
                            input_vault:         ctx.accounts.input_vault.to_account_info(),          // vault0
                            output_vault:        ctx.accounts.output_vault.to_account_info(),         // vault1
                            observation_state:   ctx.accounts.observation_state.to_account_info(),
                            token_program:       ctx.accounts.token_program.to_account_info(),
                            token_program_2022:  ctx.accounts.token_program_2022.to_account_info(),
                            memo_program:        ctx.accounts.memo_program.to_account_info(),
                            input_vault_mint:    ctx.accounts.input_vault_mint.to_account_info(),
                            output_vault_mint:   ctx.accounts.output_vault_mint.to_account_info(),
                        };
                        let swap_ctx = CpiContext::new(
                            ctx.accounts.clmm_program.to_account_info(),
                            swap_accounts,
                        ).with_signer(signer_seeds);

                        // is_base_input=true: 从 token0 -> token1
                        cpi::swap_v2(
                            swap_ctx,
                            got0,
                            p.other_amount_threshold,
                            p.sqrt_price_limit_x64,
                            true,
                        )?;
                        let new_token1 = get_token_balance(&mut ctx.accounts.output_token_account)?;
                        total_usdc_after_swap = new_token1;
                    }
                }

                // ---- 第三步：最低到手保护 + 转给收款人 USDC ATA ----
                require!(total_usdc_after_swap >= p.min_usdc_out, OperationError::InvalidParams);

                let (from_acc, _non_usdc_acc) = if usdc_mint == ctx.accounts.input_vault_mint.key() {
                    (ctx.accounts.input_token_account.to_account_info(), ctx.accounts.output_token_account.to_account_info())
                } else {
                    (ctx.accounts.output_token_account.to_account_info(), ctx.accounts.input_token_account.to_account_info())
                };

                // 从 PDA 名下 USDC 账户 -> 用户的 USDC ATA（recipient_token_account）
                let cpi_accounts = anchor_spl::token::Transfer {
                    from:      from_acc,
                    to:        ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.operation_data.to_account_info(),
                };
                let token_ctx = CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    cpi_accounts
                ).with_signer(signer_seeds);
                anchor_spl::token::transfer(token_ctx, total_usdc_after_swap)?;


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
                let pre0 = get_token_balance(&mut ctx.accounts.input_token_account)?;
                let pre1 = get_token_balance(&mut ctx.accounts.output_token_account)?;

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
                let post0 = get_token_balance(&mut ctx.accounts.input_token_account)?;
                let post1 = get_token_balance(&mut ctx.accounts.output_token_account)?;
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
                        let new_base = get_token_balance(&mut ctx.accounts.input_token_account)?;
                        total_out = new_base.checked_sub(pre0).ok_or(error!(OperationError::InvalidParams))?;
                    } else {
                        let new_quote = get_token_balance(&mut ctx.accounts.output_token_account)?;
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
const TICK_ARRAY_SIZE: i32 = 88; //Raydium/UniV3 每个 TickArray 覆盖 88 个 tick 间隔
#[inline]
fn tick_array_start_index(tick_index: i32, tick_spacing: i32) -> i32 {
    let span = tick_spacing * TICK_ARRAY_SIZE;
    // floor 除法，处理负 tick
    let q = if tick_index >= 0 {
        tick_index / span
    } else {
        (tick_index - (span - 1)) / span
    };
    q * span
}

#[inline]
fn apply_slippage_min(estimate: u64, bps: u32) -> u64 {
    let num = (estimate as u128) * (10_000u128 - bps as u128);
    (num / 10_000u128) as u64
}

fn get_token_balance(acc: &mut InterfaceAccount<InterfaceTokenAccount>) -> Result<u64> {
    acc.reload()?; // Fetch the latest on-chain data
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

    #[account(mut,
    constraint = input_token_account.mint == input_vault_mint.key(),
    constraint = input_token_account.owner == operation_data.key()
    )]
    pub input_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut,
    constraint = output_token_account.mint == output_vault_mint.key(),
    constraint = output_token_account.owner == operation_data.key()
    )]
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
    pub ca: Pubkey, // contract address
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum OperationType {
    Transfer,
    ZapIn,
    ZapOut,
    Claim
}
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ClaimParams {
    // 期望把所有手续费都换成 USDC 并打到 `recipient_token_account`。
    pub min_usdc_out: u64, // 汇总到手的USDC 最低保护
    pub other_amount_threshold: u64, // 给 swap_v2 的另一侧最小值（滑点保护），一般给 0
    pub sqrt_price_limit_x64: u128,  // swap 价格限制（不想限制就给 0）
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
            1 +    // executed
            32;  // CA

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