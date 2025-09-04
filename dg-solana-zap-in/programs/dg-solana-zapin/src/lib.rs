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
use anchor_lang::solana_program::hash::hash as solana_hash;
use anchor_lang::solana_program::{
    program::invoke_signed,
    program_pack::Pack,
    system_instruction,
};
use anchor_spl::token::spl_token;

declare_id!("9T7YMp5SXZvP3nqUj9B7rQGFErfMmh8t59jvxtV3CnjB");

/// NOTE: For ZapIn & ZapOut, we're leveraging the Raydium-Amm-v3 Protocol SDK to robost our requirement
#[program]
pub mod dg_solana_zapin {
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

        msg!("op_type = {:?}", operation_type);
        msg!("action.len() = {}", action.len());
        if action.len() > 0 {
            let preview = &action[..core::cmp::min(16, action.len())];
            msg!("action[0..] = {:?}", preview);
        }

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
    // Execute the token transfer (ZapIn only)
    pub fn execute(ctx: Context<Execute>, transfer_id: String) -> Result<()> {
        let od = &mut ctx.accounts.operation_data;

        // 基础校验
        require!(od.initialized, OperationError::NotInitialized);
        require!(!od.executed, OperationError::AlreadyExecuted);
        require!(od.amount > 0, OperationError::InvalidAmount);
        require!(od.transfer_id == transfer_id, OperationError::InvalidTransferId);
        require!(matches!(od.operation_type, OperationType::ZapIn), OperationError::InvalidParams);

        let amount = od.amount;
        let ca = od.ca;

        // signer seeds：按 transfer_id
        let bump = ctx.bumps.operation_data;
        let h = transfer_id_hash_bytes(&transfer_id);
        let signer_seeds_slice: [&[u8]; 3] = [b"operation_data", &h, &[bump]];
        let signer_seeds: &[&[&[u8]]] = &[&signer_seeds_slice];

        // 从 OperationData.action 反序列化出 ZapInParams
        let p: ZapInParams = deserialize_params(&od.action)?;
        require!(p.tick_lower < p.tick_upper, OperationError::InvalidTickRange);

        let is_base_input = ctx.accounts.program_token_account.mint == ctx.accounts.input_vault_mint.key();
        // 下面基本沿用你原 execute 里的逻辑：
        // - 校验 ca 与 input_vault_mint
        // - 处理“实际 < 期望”时全额退款
        // - 计算价格、滑点、分配比例、swap、open_position_v2、increase_liquidity_v2 等
        // - 期间把原来依赖 bounds 的地方，改成使用 p.tick_lower / p.tick_upper
        // - 对 protocol_position / tick_array_* 做运行时派生与 require! 一致性校验

        require!(ca == ctx.accounts.input_vault_mint.key() || ca == ctx.accounts.output_vault_mint.key(), OperationError::InvalidMint);
        // 例：运行时派生 protocol_position PDA 并校验
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

        // 按 Raydium v3 的 POSITION_SEED 规则派生出应有的 protocol_position 地址并核对
        let pool_key: Pubkey = ctx.accounts.pool_state.key();
        let lower_bytes = lower_start.to_be_bytes();
        let upper_bytes = upper_start.to_be_bytes();
        let proto_seeds: [&[u8]; 4] = [
            POSITION_SEED.as_bytes(),
            pool_key.as_ref(),
            &lower_bytes,
            &upper_bytes,
        ];
        let clmm_pid: Pubkey = ctx.accounts.clmm_program.key();
        let (derived_pp, _) = Pubkey::find_program_address(&proto_seeds, &clmm_pid);
        require!(ctx.accounts.protocol_position.key() == derived_pp, OperationError::InvalidParams);

        // ----------------------------- ZapIn start -----------------------------

        // --- 如果用户实际存入 amount < 期望 p.amount_in，则全额原路退回到 recipient_token_account ---
        if amount < p.amount_in {
            // 退回账户必须与程序 token 账户 mint 一致
            require!(
            ctx.accounts.recipient_token_account.mint == ctx.accounts.program_token_account.mint,
            OperationError::InvalidMint
        );
            let token_program = ctx.accounts.token_program.to_account_info();

            let refund_accounts = Transfer {
                from: ctx.accounts.program_token_account.to_account_info(),
                to: ctx.accounts.recipient_token_account.to_account_info(),
                authority: ctx.accounts.operation_data.to_account_info()
            };

            token::transfer(
                CpiContext::new_with_signer(
                    token_program,
                    refund_accounts,
                    signer_seeds, // PDA 作为 authority
                ),
                amount,
            )?;

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

        // 将 sqrt 价转为价格 P = (sp^2) / Q64
        let sp_u = U256::from(sp);
        let q64_u = U256::from(Q64_U128);
        let price_q64 = sp_u.mul_div_floor(sp_u, q64_u).ok_or(error!(OperationError::InvalidParams))?;
        // 注意：price_q64 也是 Q64.64 定点

        let amount_in_u = U256::from(p.amount_in);

        // ---- 读取手续费（若有）并做折扣 ----
        let cfg = ctx.accounts.amm_config.as_ref(); // Box<Account<AmmConfig>>
        let trade_fee_bps: u32 = cfg.trade_fee_rate.into();           // 例：30 (0.3%)
        let protocol_fee_bps: u32 = cfg.protocol_fee_rate.into();     // 例：5  (0.05%)，具体以 Raydium 定义为准
        let total_fee_bps: u32 = trade_fee_bps + protocol_fee_bps;

        // user 滑点（正整数 bps）
        let slip_bps = p.slippage_bps.min(10_000);

        // 综合折扣系数 D = (1 - fee_bps/1e4) * (1 - slip_bps/1e4)
        let one = U256::from(10_000u32);
        let fee_factor = one - U256::from(total_fee_bps);
        let slip_factor = one - U256::from(slip_bps);
        let discount = fee_factor.mul_div_floor(slip_factor, one).ok_or(error!(OperationError::InvalidParams))?; // /1e4

        // 计算“理想输出”（忽略价格冲击），再乘以折扣
        let mut min_amount_out_u = if is_base_input {
            // token0 -> token1: out ≈ in * P
            // amount_out(Q0) = amount_in * price_q64 / Q64
            // 先算理想，再乘 discount，再 /1e4
            let ideal = amount_in_u
                .mul_div_floor(price_q64, q64_u)
                .ok_or(error!(OperationError::InvalidParams))?;
            ideal.mul_div_floor(discount, one).ok_or(error!(OperationError::InvalidParams))?
        } else {
            // token1 -> token0: out ≈ in / P
            // amount_out(Q1) = amount_in * Q64 / price_q64
            let ideal = amount_in_u
                .mul_div_floor(q64_u, price_q64.max(U256::from(1u8))) // 防 0
                .ok_or(error!(OperationError::InvalidParams))?;
            ideal.mul_div_floor(discount, one).ok_or(error!(OperationError::InvalidParams))?
        };

        // 转 u64，保护下界
        let min_amount_out = min_amount_out_u.to_underflow_u64();

        // 1) 计算区间端点的 sqrt 价格
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

        // 2) 计算一次 swap 的分配比例（Raydium/UniV3 常见公式）
        let r_num = sb_u * sp_minus_sa;
        let r_den = sp_u * sb_minus_sp;

        let frac_den = r_den + r_num;
        require!(frac_den > U256::from(0u8), OperationError::InvalidParams);

        let amount_in = p.amount_in;

        let amount_in_u256 = U256::from(amount_in); // 用户输入
        let swap_amount_u256 = if is_base_input {
            amount_in_u256.mul_div_floor(r_num, frac_den).ok_or(error!(OperationError::InvalidParams))?
        } else {
            amount_in_u256.mul_div_floor(r_den, frac_den).ok_or(error!(OperationError::InvalidParams))?
        };
        let swap_amount = swap_amount_u256.to_underflow_u64();
        require!(swap_amount as u128 <= u64::MAX as u128, OperationError::InvalidParams);

        // 3) 把用户在程序名下的存款挪到对应的 input 侧账户
        let to_acc_info = if is_base_input {
            ctx.accounts.input_token_account.to_account_info()
        } else {
            ctx.accounts.output_token_account.to_account_info()
        };
        let fund_move = anchor_spl::token::Transfer {
            from:      ctx.accounts.program_token_account.to_account_info(),
            to:        to_acc_info,
            authority: ctx.accounts.operation_data.to_account_info(),
        };
        let fund_move_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            fund_move,
        ).with_signer(signer_seeds);
        // 把用户 deposit 进来的 amount 全部挪过去（后面会用一部分做 swap，剩余的直接加仓）
        anchor_spl::token::transfer(fund_move_ctx, amount)?;

        // 记录 swap 前后余额
        let pre_out = get_token_balance(&mut ctx.accounts.output_token_account)?;
        let pre_in  = get_token_balance(&mut ctx.accounts.input_token_account)?;

        // 4) 在池内做单边 swap（base/quote 方向依据 is_base_input）
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
            let other_amount_threshold = min_amount_out;
            // 可选：如果你想限制价格滑动方向，给 sqrt_price_limit_x64 一个保守的上下限；
            // 否则传 0 表示不限制（以 Raydium 当前实现为准）
            let sqrt_price_limit_x64: u128 = 0;

            let swap_ctx = CpiContext::new(clmm, swap_accounts)
                .with_signer(signer_seeds);

            cpi::swap_v2(
                swap_ctx,
                swap_amount,
                other_amount_threshold,
                sqrt_price_limit_x64,
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
            let mint_info = &ctx.accounts.position_nft_mint.to_account_info();
            if mint_info.data_is_empty() {
                let mint_space = spl_token::state::Mint::LEN;
                let rent_lamports = Rent::get()?.minimum_balance(mint_space);

                let create_ix = system_instruction::create_account(
                    &ctx.accounts.user.key(),                 // 付款人
                    &ctx.accounts.position_nft_mint.key(),    // 要创建的 mint（PDA）
                    rent_lamports,
                    mint_space as u64,
                    &ctx.accounts.token_program.key(),        // owner = SPL Token Program
                );

                // 关键：把会临时生成的 key 先保存到局部变量，延长生命周期
                let user_key: Pubkey = ctx.accounts.user.key();
                let pool_key: Pubkey = ctx.accounts.pool_state.key();
                let bump: u8 = ctx.bumps.position_nft_mint;
                let bump_bytes: [u8; 1] = [bump];

                let pos_mint_seeds: &[&[u8]] = &[
                    b"pos_nft_mint",
                    user_key.as_ref(),        // <-- 引用局部变量，生命周期足够长
                    pool_key.as_ref(),        // <-- 同上
                    &bump_bytes,              // <-- 避免 &[bump] 的临时数组
                ];

                invoke_signed(
                    &create_ix,
                    &[
                        ctx.accounts.user.to_account_info(),
                        mint_info.clone(),
                        ctx.accounts.system_program.to_account_info(),
                    ],
                    &[pos_mint_seeds],
                )?;
            }
            // 此时：position_nft_mint 是“已创建但未初始化”的 SPL-Token Mint 账号
            // 后续交给 Raydium 的 open_position_v2 去 Initialize & Mint 到 position_nft_account
        }

        // 5) 开仓（铸造仓位 NFT）
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

        // 6) 追加流动性（把剩余与对侧收入都注入）
        {
            let (amount_0_max, amount_1_max) = if is_base_input {
                (remaining, received)
            } else {
                (received, remaining)
            };

            let clmm            = ctx.accounts.clmm_program.to_account_info();
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

        ctx.accounts.operation_data.executed = true;
        Ok(())
    }


    pub fn claim(ctx: Context<Claim>, transfer_id: String, p: ClaimParams) -> Result<()> {
        let od = &ctx.accounts.operation_data;
        // —— 基于 transfer_id 的 signer seeds ——
        let bump = ctx.bumps.operation_data;
        let h = transfer_id_hash_bytes(&transfer_id);
        let signer_seeds: &[&[&[u8]]] = &[&[b"operation_data", &h, &[bump]]];

        // 基础校验
        require!(od.initialized, OperationError::NotInitialized);
        require!(od.transfer_id == transfer_id, OperationError::InvalidTransferId);

        // ---------- NFT 归属强校验（原逻辑不变） ----------
        {
            let nft_acc = spl_token::state::Account::unpack_from_slice(
                &ctx.accounts.position_nft_account.try_borrow_data()?
            )?;
            require!(nft_acc.mint  == ctx.accounts.personal_position.nft_mint, OperationError::Unauthorized);
            require!(nft_acc.owner == ctx.accounts.user.key(),                 OperationError::Unauthorized);
            require!(nft_acc.amount == 1,                                       OperationError::Unauthorized);
        }

        // ---------- 新增：仓位归属权强校验 ----------
        {
            // 解包仓位 NFT 的 TokenAccount（position_nft_account 必须是该 NFT 的持仓账户）
            let nft_token_acc = spl_token::state::Account::unpack_from_slice(
                &ctx.accounts.position_nft_account.try_borrow_data()?
            )?;

            // 1) NFT mint 必须与个人仓位记录里的 nft_mint 一致
            require!(
                nft_token_acc.mint == ctx.accounts.personal_position.nft_mint,
                OperationError::Unauthorized
            );
            // 2) 该 NFT 的持有人必须是 user
            require!(
                nft_token_acc.owner == ctx.accounts.user.key(),
                OperationError::Unauthorized
            );
            // 3) 该账户里应当正好持有 1 枚 NFT
            require!(
                nft_token_acc.amount == 1,
                OperationError::Unauthorized
            );
        }

        // ---------- 领取币种必须是池子两侧之一（USDC 那侧） ----------
        let usdc_mint = ctx.accounts.recipient_token_account.mint;
        require!(
            usdc_mint == ctx.accounts.input_vault_mint.key()
                || usdc_mint == ctx.accounts.output_vault_mint.key(),
            OperationError::InvalidMint
        );

        // 记录 claim 前余额（PDA 名下两边）
        let pre0 = get_token_balance(&mut ctx.accounts.input_token_account)?;
        let pre1 = get_token_balance(&mut ctx.accounts.output_token_account)?;
        let pre_usdc = if usdc_mint == ctx.accounts.input_vault_mint.key() { pre0 } else { pre1 };

        // 1) 只结算手续费（liquidity=0）
        {
            let dec_accounts = cpi::accounts::DecreaseLiquidityV2 {
                // nft_owner 不需要签名；我们用 PDA 作为 payer/signer
                nft_owner:         ctx.accounts.user.to_account_info(),
                nft_account:       ctx.accounts.position_nft_account.to_account_info(),
                pool_state:        ctx.accounts.pool_state.to_account_info(),
                protocol_position: ctx.accounts.protocol_position.to_account_info(),
                personal_position: ctx.accounts.personal_position.to_account_info(),
                tick_array_lower:  ctx.accounts.tick_array_lower.to_account_info(),
                tick_array_upper:  ctx.accounts.tick_array_upper.to_account_info(),
                recipient_token_account_0: ctx.accounts.input_token_account.to_account_info(),
                recipient_token_account_1: ctx.accounts.output_token_account.to_account_info(),
                token_vault_0:     ctx.accounts.input_vault.to_account_info(),
                token_vault_1:     ctx.accounts.output_vault.to_account_info(),
                token_program:     ctx.accounts.token_program.to_account_info(),
                token_program_2022:ctx.accounts.token_program_2022.to_account_info(),
                vault_0_mint:      ctx.accounts.input_vault_mint.to_account_info(),
                vault_1_mint:      ctx.accounts.output_vault_mint.to_account_info(),
                memo_program:      ctx.accounts.memo_program.to_account_info(),
            };
            let dec_ctx = CpiContext::new(ctx.accounts.clmm_program.to_account_info(), dec_accounts)
                .with_signer(signer_seeds);
            cpi::decrease_liquidity_v2(dec_ctx, 0u128, 0u64, 0u64)?;
        }

        // 计算刚刚领取到 PDA 的手续费数量
        let post0 = get_token_balance(&mut ctx.accounts.input_token_account)?;
        let post1 = get_token_balance(&mut ctx.accounts.output_token_account)?;
        let got0 = post0.checked_sub(pre0).ok_or(error!(OperationError::InvalidParams))?;
        let got1 = post1.checked_sub(pre1).ok_or(error!(OperationError::InvalidParams))?;

        // ========== 新增：可用奖励判断，支持多次 claim ==========
        if got0 == 0 && got1 == 0 {
             msg!("No rewards available to claim right now.");
                return Ok(());
        }

        // 2) 将非 USDC 一侧全量 swap 成 USDC
        let mut total_usdc_after_swap: u64;
        let other_amount_threshold: u64 = 0;
        let sqrt_price_limit_x64: u128 = 0;
        if usdc_mint == ctx.accounts.input_vault_mint.key() {
            total_usdc_after_swap = pre_usdc + got0;
            if got1 > 0 {
                let swap_accounts = cpi::accounts::SwapSingleV2 {
                    payer:               ctx.accounts.operation_data.to_account_info(),
                    amm_config:          ctx.accounts.amm_config.to_account_info(),
                    pool_state:          ctx.accounts.pool_state.to_account_info(),
                    input_token_account: ctx.accounts.output_token_account.to_account_info(), // in: token1
                    output_token_account:ctx.accounts.input_token_account.to_account_info(),  // out: token0(USDC)
                    input_vault:         ctx.accounts.output_vault.to_account_info(),
                    output_vault:        ctx.accounts.input_vault.to_account_info(),
                    observation_state:   ctx.accounts.observation_state.to_account_info(),
                    token_program:       ctx.accounts.token_program.to_account_info(),
                    token_program_2022:  ctx.accounts.token_program_2022.to_account_info(),
                    memo_program:        ctx.accounts.memo_program.to_account_info(),
                    input_vault_mint:    ctx.accounts.output_vault_mint.to_account_info(),
                    output_vault_mint:   ctx.accounts.input_vault_mint.to_account_info(),
                };
                let swap_ctx = CpiContext::new(ctx.accounts.clmm_program.to_account_info(), swap_accounts)
                    .with_signer(signer_seeds);
                cpi::swap_v2(
                    swap_ctx,
                    got1,
                    other_amount_threshold,
                    sqrt_price_limit_x64,
                    false,
                )?;
                let new_token0 = get_token_balance(&mut ctx.accounts.input_token_account)?;
                total_usdc_after_swap = new_token0;
            }
        } else {
            total_usdc_after_swap = pre_usdc + got1;
            if got0 > 0 {
                let swap_accounts = cpi::accounts::SwapSingleV2 {
                    payer:               ctx.accounts.operation_data.to_account_info(),
                    amm_config:          ctx.accounts.amm_config.to_account_info(),
                    pool_state:          ctx.accounts.pool_state.to_account_info(),
                    input_token_account: ctx.accounts.input_token_account.to_account_info(),  // in: token0
                    output_token_account:ctx.accounts.output_token_account.to_account_info(), // out: token1(USDC)
                    input_vault:         ctx.accounts.input_vault.to_account_info(),
                    output_vault:        ctx.accounts.output_vault.to_account_info(),
                    observation_state:   ctx.accounts.observation_state.to_account_info(),
                    token_program:       ctx.accounts.token_program.to_account_info(),
                    token_program_2022:  ctx.accounts.token_program_2022.to_account_info(),
                    memo_program:        ctx.accounts.memo_program.to_account_info(),
                    input_vault_mint:    ctx.accounts.input_vault_mint.to_account_info(),
                    output_vault_mint:   ctx.accounts.output_vault_mint.to_account_info(),
                };
                let swap_ctx = CpiContext::new(ctx.accounts.clmm_program.to_account_info(), swap_accounts)
                    .with_signer(signer_seeds);
                cpi::swap_v2(
                    swap_ctx,
                    got0,
                    other_amount_threshold, // no limit
                    sqrt_price_limit_x64, // no limit
                    true,
                )?;
                let new_token1 = get_token_balance(&mut ctx.accounts.output_token_account)?;
                total_usdc_after_swap = new_token1;
            }
        }

        // 3) 最小到手保护 + 从 PDA 转给 user 的 USDC ATA
        require!(total_usdc_after_swap >= p.min_usdc_out, OperationError::InvalidParams);

        let (from_acc, usdc_mint_acc) = if usdc_mint == ctx.accounts.input_vault_mint.key() {
            (ctx.accounts.input_token_account.to_account_info(), ctx.accounts.input_vault_mint.to_account_info())
        } else {
            (ctx.accounts.output_token_account.to_account_info(), ctx.accounts.output_vault_mint.to_account_info())
        };

        let cpi_accounts = Transfer {
            from:      from_acc,
            to:        ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.operation_data.to_account_info(),
        };
        let token_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts)
            .with_signer(signer_seeds);
        token::transfer(token_ctx, total_usdc_after_swap)?;

        emit!(ClaimEvent {
            pool: ctx.accounts.pool_state.key(),
            beneficiary: ctx.accounts.user.key(),
            mint: usdc_mint_acc.key(),
            amount: total_usdc_after_swap,
        });

        Ok(())
    }


    pub fn withdraw(ctx: Context<ZapOutExecute>,
                    transfer_id: String,
                    bounds: PositionBounds,
                    p: ZapOutParams
    ) -> Result<()> {
        let od = &ctx.accounts.operation_data;

        require!(od.initialized, OperationError::NotInitialized);
        require!(!od.executed, OperationError::AlreadyExecuted);
        require!(od.amount > 0, OperationError::InvalidAmount);

        // —— 基于 transfer_id 的 signer seeds ——
        let bump = ctx.bumps.operation_data;
        let h = transfer_id_hash_bytes(&transfer_id);
        let signer_seeds: &[&[&[u8]]] = &[&[b"operation_data", &h, &[bump]]];

        // 期望收款人：沿用你现有逻辑
        let expected_recipient = if od.recipient != Pubkey::default() { od.recipient } else { od.authority };

        // 基础授权/一致性校验（原逻辑不变）
        require!(ctx.accounts.input_token_account.owner  == od.key(), OperationError::Unauthorized);
        require!(ctx.accounts.output_token_account.owner == od.key(), OperationError::Unauthorized);
        require!(ctx.accounts.recipient_token_account.owner == expected_recipient, OperationError::Unauthorized);

        // 目标侧 mint 与收款 ATA 一致
        let want_mint = if p.want_base { ctx.accounts.input_vault_mint.key() } else { ctx.accounts.output_vault_mint.key() };
        require!(ctx.accounts.recipient_token_account.mint == want_mint, OperationError::InvalidMint);

        // —— 运行时派生 protocol_position 并校验（用个人仓位的 tick 与池的 spacing）——
        let pool = ctx.accounts.pool_state.load()?;
        let tick_spacing: i32 = pool.tick_spacing.into();
        let tick_lower = ctx.accounts.personal_position.tick_lower_index;
        let tick_upper = ctx.accounts.personal_position.tick_upper_index;

        let lower_start = tick_array_start_index(tick_lower, tick_spacing);
        let upper_start = tick_array_start_index(tick_upper, tick_spacing);

        {
            let ta_lower = ctx.accounts.tick_array_lower.load()?;
            let ta_upper = ctx.accounts.tick_array_upper.load()?;
            require!(ta_lower.start_tick_index == lower_start, OperationError::InvalidParams);
            require!(ta_upper.start_tick_index == upper_start, OperationError::InvalidParams);
        }

        let pool_key: Pubkey = ctx.accounts.pool_state.key();
        let lower_bytes = lower_start.to_be_bytes();
        let upper_bytes = upper_start.to_be_bytes();
        let proto_seeds: [&[u8]; 4] = [
            POSITION_SEED.as_bytes(),
            pool_key.as_ref(),
            &lower_bytes,
            &upper_bytes,
        ];
        let clmm_pid: Pubkey = ctx.accounts.clmm_program.key();
        let (derived_pp, _) = Pubkey::find_program_address(&proto_seeds, &clmm_pid);
        require!(ctx.accounts.protocol_position.key() == derived_pp, OperationError::InvalidParams);

        // 赎回前余额
        let pre0 = get_token_balance(&mut ctx.accounts.input_token_account)?;
        let pre1 = get_token_balance(&mut ctx.accounts.output_token_account)?;

        // 读取仓位与价格
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

        // 估算期望拿回量，生成最小保护
        let (est0, est1) = amounts_from_liquidity_burn_q64(sa, sb, sp, burn_liquidity_u128);
        let min0 = apply_slippage_min(est0, p.slippage_bps);
        let min1 = apply_slippage_min(est1, p.slippage_bps);

        // ---------- Step A: 赎回（DecreaseLiquidityV2），收款直达 PDA 名下 input/output_token_account ----------
        {
            let clmm            = ctx.accounts.clmm_program.to_account_info();
            let nft_owner       = ctx.accounts.user.to_account_info();
            let nft_account     = ctx.accounts.position_nft_account.to_account_info();
            let pool_state      = ctx.accounts.pool_state.to_account_info();
            let protocol_pos    = ctx.accounts.protocol_position.to_account_info();
            let personal_pos    = ctx.accounts.personal_position.to_account_info();
            let ta_lower        = ctx.accounts.tick_array_lower.to_account_info();
            let ta_upper        = ctx.accounts.tick_array_upper.to_account_info();
            let rec0            = ctx.accounts.input_token_account.to_account_info();
            let rec1            = ctx.accounts.output_token_account.to_account_info();
            let token_vault_0   = ctx.accounts.input_vault.to_account_info();
            let token_vault_1   = ctx.accounts.output_vault.to_account_info();
            let token_prog      = ctx.accounts.token_program.to_account_info();
            let token2022       = ctx.accounts.token_program_2022.to_account_info();
            let v0_mint         = ctx.accounts.input_vault_mint.to_account_info();
            let v1_mint         = ctx.accounts.output_vault_mint.to_account_info();
            let memo            = ctx.accounts.memo_program.to_account_info();

            let dec_accounts = cpi::accounts::DecreaseLiquidityV2 {
                nft_owner: nft_owner,
                nft_account: nft_account,
                pool_state: pool_state,
                protocol_position: protocol_pos,
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

            let dec_ctx = CpiContext::new(clmm, dec_accounts).with_signer(signer_seeds);

            cpi::decrease_liquidity_v2(
                dec_ctx,
                burn_liquidity_u128,
                min0,
                min1,
            )?;
        }

        // 赎回后余额增量
        let post0 = get_token_balance(&mut ctx.accounts.input_token_account)?;
        let post1 = get_token_balance(&mut ctx.accounts.output_token_account)?;
        let got0  = post0.checked_sub(pre0).ok_or(error!(OperationError::InvalidParams))?;
        let got1  = post1.checked_sub(pre1).ok_or(error!(OperationError::InvalidParams))?;

        // ---------- Step B: 单边换（可选） ----------
        let (mut total_out, swap_amount, is_base_input) = if p.want_base {
            (got0, got1, false) // 需要 base，手里的 quote 全部换成 base
        } else {
            (got1, got0, true)  // 需要 quote，手里的 base 全部换成 quote
        };

        if swap_amount > 0 {
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
            let other_amount_threshold: u64 = 0;
            let sqrt_price_limit_x64: u128 = 0;

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

            let swap_ctx = CpiContext::new(clmm, swap_accounts).with_signer(signer_seeds);

            cpi::swap_v2(
                swap_ctx,
                swap_amount,
                other_amount_threshold,
                sqrt_price_limit_x64,
                is_base_input,
            )?;

            // 刷新单边后的总量
            if p.want_base {
                let new_base = get_token_balance(&mut ctx.accounts.input_token_account)?;
                total_out = new_base.checked_sub(pre0).ok_or(error!(OperationError::InvalidParams))?;
            } else {
                let new_quote = get_token_balance(&mut ctx.accounts.output_token_account)?;
                total_out = new_quote.checked_sub(pre1).ok_or(error!(OperationError::InvalidParams))?;
            }
        }

        // ---------- Step C: 最低到手 + 与用户期望 amount 的保护 ----------
        require!(total_out >= p.min_payout, OperationError::InvalidParams);
        require!(total_out >= od.amount,       OperationError::InvalidParams);

        // ---------- Step D: 转给收款人 ----------
        let from_acc = if p.want_base {
            ctx.accounts.input_token_account.to_account_info()
        } else {
            ctx.accounts.output_token_account.to_account_info()
        };
        let cpi_accounts = Transfer {
            from: from_acc,
            to:   ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.operation_data.to_account_info(),
        };
        token::transfer(
            CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_accounts, signer_seeds),
            total_out,
        )?;

        // ---------- Step E: 若仓位已空，关闭仓位 ----------
        if ctx.accounts.personal_position.liquidity == 0 {
            let clmm         = ctx.accounts.clmm_program.to_account_info();
            let protocol_pos = ctx.accounts.protocol_position.to_account_info();
            let personal_pos = ctx.accounts.personal_position.to_account_info();
            let nft_mint     = ctx.accounts.position_nft_mint.to_account_info();
            let nft_account  = ctx.accounts.position_nft_account.to_account_info();
            let token_prog   = ctx.accounts.token_program.to_account_info();
            let sys_prog     = ctx.accounts.system_program.to_account_info();

            let close_accounts = cpi::accounts::ClosePosition {
                nft_owner: protocol_pos,
                personal_position: personal_pos,
                position_nft_mint: nft_mint,
                position_nft_account: nft_account,
                token_program: token_prog,
                system_program: sys_prog,
            };
            let close_ctx = CpiContext::new(clmm, close_accounts).with_signer(signer_seeds);
            cpi::close_position(close_ctx)?;
            msg!("Position closed and NFT burned.");
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
fn transfer_id_hash_bytes(transfer_id: &str) -> [u8; 32] {
    solana_hash(transfer_id.as_bytes()).to_bytes()
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


fn get_token_balance(acc: &mut InterfaceAccount<InterfaceTokenAccount>) -> Result<u64> {
    acc.reload()?; // Fetch the latest on-chain data
    Ok(acc.amount)
}

/// Helper function to deserialize params
fn deserialize_params<T: AnchorDeserialize>(data: &[u8]) -> Result<T> {
    T::try_from_slice(data).map_err(|_| error!(OperationError::InvalidParams))
}

#[inline]
fn apply_slippage_min(amount: u64, slippage_bps: u32) -> u64 {
    // min_out = amount * (1 - bps/1e4)
    let one = 10_000u128;
    let bps = (slippage_bps as u128).min(one);
    let num = (amount as u128).saturating_mul(one.saturating_sub(bps));
    (num / one) as u64
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
#[instruction(transfer_id: String)]
pub struct Claim<'info> {
    // ZapIn 的 PDA（vault authority），用 transfer_id 维度
    #[account(
        mut,
        seeds = [b"operation_data", &transfer_id_hash_bytes(&transfer_id)],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,

    // 只有 user（签名者）才能 claim
    pub user: Signer<'info>,

    // ---------- 池 & 配置 ----------
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    // PDA 名下两侧代币账户（分别匹配 vault mint）
    #[account(
        mut,
        constraint = input_token_account.mint == input_vault_mint.key(),
        constraint = input_token_account.owner == operation_data.key()
    )]
    pub input_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(
        mut,
        constraint = output_token_account.mint == output_vault_mint.key(),
        constraint = output_token_account.owner == operation_data.key()
    )]
    pub output_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    // 池金库 & 观测
    #[account(mut, address = pool_state.load()?.token_vault_0)]
    pub input_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.token_vault_1)]
    pub output_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    // 两侧 mint（只读一致性）
    #[account(address = pool_state.load()?.token_mint_0)]
    pub input_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = pool_state.load()?.token_mint_1)]
    pub output_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,

    // ---------- Position（领取手续费所需） ----------
    /// CHECK: Raydium 内部校验
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,
    pub personal_position: Box<Account<'info, PersonalPositionState>>,
    #[account(
        mut,
        seeds = [
        POSITION_SEED.as_bytes(),
        pool_state.key().as_ref(),
        &tick_array_lower.load()?.start_tick_index.to_be_bytes(),
        &tick_array_upper.load()?.start_tick_index.to_be_bytes(),
        ],
        seeds::program = clmm_program,
        bump,
        constraint = protocol_position.pool_id == pool_state.key(),
    )]
    pub protocol_position: Box<Account<'info, ProtocolPositionState>>,
    #[account(mut)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,
    #[account(mut)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    // ---------- 领取目标（必须为 user 的 ATA，且 mint=池子 token0/1 中的 USDC 那侧） ----------
    #[account(
        mut,
        constraint = recipient_token_account.owner == user.key() @ OperationError::Unauthorized
    )]
    pub recipient_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// CHECK: spl-memo
    #[account(address = spl_memo::id())]
    pub memo_program: UncheckedAccount<'info>,

    // 程序
    #[account(constraint = clmm_program.key() == RAYDIUM_CLMM_PROGRAM_ID)]
    pub clmm_program: Program<'info, AmmV3>,
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
}

#[event]
pub struct ClaimEvent {
    pub pool: Pubkey,
    pub beneficiary: Pubkey, // = user_da
    pub mint: Pubkey,        // 实际 USDC mint
    pub amount: u64,         // 实转金额
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ClaimParams {
    /// 领取后，最终到手的 USDC 不得低于该值
    pub min_usdc_out: u64, // required
}


#[derive(Accounts)]
#[instruction(transfer_id: String)]
pub struct Execute<'info> {
    // 基于 transfer_id 的 OperationData（PDA 即 vault authority / signer）
    #[account(
        mut,
        seeds = [b"operation_data", &transfer_id_hash_bytes(&transfer_id)],
        bump
    )]
    pub operation_data: Box<Account<'info, OperationData>>,

    /// 程序名下托管账户（资金先前由 deposit_v2 存入），owner 必须是 operation_data
    #[account(mut)]
    pub program_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// 仅用于“实际收到 < 期望”时退款
    #[account(mut)]
    pub recipient_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    // --- 用户签名者（作为仓位 NFT 的 owner） ---
    #[account(mut)]
    pub user: Signer<'info>,

    // --- 程序名下两侧 token 账户（与池子 mint 一一对应，owner=operation_data） ---
    #[account(
        mut,
        constraint = input_token_account.mint == input_vault_mint.key(),
        constraint = input_token_account.owner == operation_data.key()
    )]
    pub input_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(
        mut,
        constraint = output_token_account.mint == output_vault_mint.key(),
        constraint = output_token_account.owner == operation_data.key()
    )]
    pub output_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// CHECK: position_nft_mint（用 user+pool_state 作种子）
    #[account(
        mut,
        seeds = [b"pos_nft_mint", user.key().as_ref(), pool_state.key().as_ref()],
        bump
    )]
    pub position_nft_mint: UncheckedAccount<'info>,
    /// CHECK:
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    // --- Raydium 池 & 配置 ---
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// 池子金库（两侧）
    #[account(mut, address = pool_state.load()?.token_vault_0)]
    pub input_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.token_vault_1)]
    pub output_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// 价格观测账户
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    /// 两侧 mint
    #[account(address = pool_state.load()?.token_mint_0)]
    pub input_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = pool_state.load()?.token_mint_1)]
    pub output_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,

    /// CHECK: spl-memo
    #[account(address = spl_memo::id())]
    pub memo_program: UncheckedAccount<'info>,

    /// Raydium CLMM 程序
    #[account(constraint = clmm_program.key() == RAYDIUM_CLMM_PROGRAM_ID)]
    pub clmm_program: Program<'info, AmmV3>,

    // --- Position（运行时校验，不再依赖 bounds） ---
    /// CHECK: 运行时派生并 require! 校验
    #[account(mut)]
    pub protocol_position: UncheckedAccount<'info>,
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    pub rent: Sysvar<'info, Rent>,

    // --- TickArray（运行时校验） ---
    #[account(mut)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,
    #[account(mut)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    // --- OpenPositionV2 需要的 token 占位账户 ---
    #[account(mut, constraint = token_account_0.mint == input_vault.mint)]
    pub token_account_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, constraint = token_account_1.mint == output_vault.mint)]
    pub token_account_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// CHECK:
    #[account(mut)]
    pub metadata_account: UncheckedAccount<'info>,
    pub metadata_program: Program<'info, Metadata>,

    pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(transfer_id: String, bounds: PositionBounds)]
pub struct ZapOutExecute<'info> {
    // 程序 PDA（vault authority），transfer_id 维度
    #[account(
        mut,
        seeds = [b"operation_data", &transfer_id_hash_bytes(&transfer_id)],
        bump
    )]
    pub operation_data: Box<Account<'info, OperationData>>,

    // ====== 接收账户（实际打款目标），mint 运行时校验 ======
    #[account(mut)]
    pub recipient_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    // ====== Position / Pool / Raydium 帐户 ======
    /// CHECK: 仅作转发给 Raydium 的 nft_owner（不要求签名）
    pub user: UncheckedAccount<'info>,
    /// CHECK:
    #[account(mut)]
    pub position_nft_mint: UncheckedAccount<'info>,
    /// CHECK:
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    // PDA 名下两侧 token 账户（作为赎回与单边换的资金承接账户）
    #[account(
        mut,
        constraint = input_token_account.mint == input_vault_mint.key(),
        constraint = input_token_account.owner == operation_data.key()
    )]
    pub input_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(
        mut,
        constraint = output_token_account.mint == output_vault_mint.key(),
        constraint = output_token_account.owner == operation_data.key()
    )]
    pub output_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    // 池金库 & 观测
    #[account(mut, address = pool_state.load()?.token_vault_0)]
    pub input_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.token_vault_1)]
    pub output_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    // 两侧 mint（只读一致性）
    #[account(address = pool_state.load()?.token_mint_0)]
    pub input_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = pool_state.load()?.token_mint_1)]
    pub output_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,

    // Raydium Position（协议/个人）——去掉基于 bounds 的 seeds，改运行时校验
    /// CHECK: 运行时派生并 require! 校验
    #[account(mut)]
    pub protocol_position: UncheckedAccount<'info>,
    pub personal_position: Box<Account<'info, PersonalPositionState>>,
    #[account(mut)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,
    #[account(mut)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    /// CHECK: spl-memo
    #[account(address = spl_memo::id())]
    pub memo_program: UncheckedAccount<'info>,

    // 程序
    #[account(constraint = clmm_program.key() == RAYDIUM_CLMM_PROGRAM_ID)]
    pub clmm_program: Program<'info, AmmV3>,
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ZapOutParams {
    /// 期望拿回哪一侧：true=base(token0)，false=quote(token1)
    pub want_base: bool,
    /// 允许的滑点（bps）
    pub slippage_bps: u32,
    /// 要赎回的流动性（为 0 时表示全仓位）
    pub liquidity_to_burn_u64: u64,
    /// 整体流程最终至少要拿回的目标侧资产数量
    pub min_payout: u64,
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
    pub amount_in: u64, // required
    pub pool: Pubkey, // required
    pub tick_lower: i32, // required
    pub tick_upper: i32, // required
    pub slippage_bps: u32, // required
}

impl OperationData {
    pub const LEN: usize =
        32 + // authority
            1 +  // initialized
            4 + 64 + // transfer_id (prefix + max size)
            32 + // recipient pubkey
            1 +  // operation_type (enum discriminator)
            4 + 256 + // action vec<u8> (prefix + max size)
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