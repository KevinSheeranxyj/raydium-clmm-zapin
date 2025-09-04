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
    pub fn execute(ctx: Context<Execute>, bounds: PositionBounds) -> Result<()> {
        require!(ctx.accounts.operation_data.initialized, OperationError::NotInitialized);
        require!(!ctx.accounts.operation_data.executed, OperationError::AlreadyExecuted);
        require!(ctx.accounts.operation_data.amount > 0, OperationError::InvalidAmount);

        let amount = ctx.accounts.operation_data.amount;
        let op_type = ctx.accounts.operation_data.operation_type.clone();
        let action  = ctx.accounts.operation_data.action.clone();
        let ca = ctx.accounts.operation_data.ca;

        // 只允许 ZapIn
        require!(matches!(op_type, OperationType::ZapIn), OperationError::InvalidParams);

        let bump = ctx.bumps.operation_data;
        let seeds: &[&[u8]] = &[b"operation_data", &[bump]];
        let signer_seeds: &[&[&[u8]]] = &[seeds];

        let token_program = ctx.accounts.token_program.to_account_info();

        // ----------------------------- ZapIn start -----------------------------
        let p: ZapInParams = deserialize_params(&action)?;
        require!(p.tick_lower < p.tick_upper, OperationError::InvalidTickRange);

        require!(ca == ctx.accounts.input_vault_mint.key(), OperationError::InvalidMint);

        // --- 如果用户实际存入 amount < 期望 p.amount_in，则全额原路退回到 recipient_token_account ---
        if amount < p.amount_in {
            // 退回账户必须与程序 token 账户 mint 一致
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
        // 交易方向
        let is_base_input = ctx.accounts.program_token_account.mint == ctx.accounts.input_vault_mint.key();

        let amount_in_u = U256::from(p.amount_in);

        // ---- 读取手续费（若有）并做折扣 ----
        let cfg = ctx.accounts.amm_config.as_ref(); // Box<Account<AmmConfig>>
        let trade_fee_bps: u32 = cfg.trade_fee_rate.into();           // 例：30 (0.3%)
        let protocol_fee_bps: u32 = cfg.protocol_fee_rate.into();     // 例：5  (0.05%)，具体以 Raydium 定义为准
        let total_fee_bps: u32 = trade_fee_bps + protocol_fee_bps;

        // user 滑点（正整数 bps）
        let slip_bps = if p.slippage_bps < 0 { 0 } else { p.slippage_bps as u32 };

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

        let is_base_input = ctx.accounts.program_token_account.mint == ctx.accounts.input_vault_mint.key();
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
    // --- 程序状态 & 退款所需 ---
    #[account(
        mut,
        seeds = [b"operation_data"],
        bump
    )]
    pub operation_data: Box<Account<'info, OperationData>>,

    /// 用户先前 deposit 的代币目前在程序名下，该账户是程序名下的托管账户
    #[account(mut)]
    pub program_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// 仅用于当实际收到 < 期望 amount_in 时走全额退款
    #[account(mut)]
    pub recipient_token_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    // --- 用户签名者（作为仓位 NFT 的 owner） ---
    #[account(mut)]
    pub user: Signer<'info>,

    // --- 程序名下两侧 token 账户（PDA 作为 owner），与池子的 mint 一一对应 ---
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

    /// CHECK: position_nft_mint
    #[account(
        mut,
        seeds = [b"pos_nft_mint", user.key().as_ref(), pool_state.key().as_ref()],
        bump
    )]
    pub position_nft_mint: UncheckedAccount<'info>,
    /// CHECK: position_nft_account
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    // --- Raydium 池 & 配置 ---
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// 池子金库（两侧），地址与 pool_state 中的一致
    #[account(mut, address = pool_state.load()?.token_vault_0)]
    pub input_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.token_vault_1)]
    pub output_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// 价格观测账户（swap 需要）
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    /// 两侧 mint（仅作地址/一致性校验）
    #[account(address = pool_state.load()?.token_mint_0)]
    pub input_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = pool_state.load()?.token_mint_1)]
    pub output_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,

    /// CHECK: spl-memo 程序
    #[account(address = spl_memo::id())]
    pub memo_program: UncheckedAccount<'info>,

    /// Raydium CLMM 程序
    #[account(constraint = clmm_program.key() == RAYDIUM_CLMM_PROGRAM_ID)]
    pub clmm_program: Program<'info, AmmV3>,

    // --- 协议/个人仓位（开仓 & 增加流动性需要） ---
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

    // --- TickArray，用于校验与作为 CPI 账户传入 ---
    #[account(mut)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,
    #[account(mut)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    // --- OpenPositionV2 需要的占位 token 账户（两侧，与金库 mint 匹配）---
    #[account(
        mut,
        constraint = token_account_0.mint == input_vault.mint
    )]
    pub token_account_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(
        mut,
        constraint = token_account_1.mint == output_vault.mint
    )]
    pub token_account_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// CHECK: metadata account not necessary
    #[account(mut)]
    pub metadata_account: UncheckedAccount<'info>,
    pub metadata_program: Program<'info, Metadata>,

    // --- 程序/系统 ---
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