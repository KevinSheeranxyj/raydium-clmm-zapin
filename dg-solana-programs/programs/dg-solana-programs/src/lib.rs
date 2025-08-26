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

/// NOTE: For zapIn, we're leveraging the Raydium-Amm-v3 Protocol SDK to robost our requirement

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
        msg!(
            "Initialized PDA with authority: {}",
            operation_data.authority
        );
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
            let p: ZapInParams = deserialize_params(&operation_data.action)?;
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
        let od_ro = &mut ctx.accounts.operation_data;
        require!(od_ro.initialized, OperationError::NotInitialized);
        require!(!od_ro.executed, OperationError::AlreadyExecuted);
        require!(od_ro.amount > 0, OperationError::InvalidAmount);

        let amount = od_ro.amount;
        let op_type = od_ro.operation_type.clone();
        let action  = od_ro.action.clone();

        let bump = ctx.bumps.operation_data;
        let seeds: &[&[u8]] = &[
            b"operation_data",
            &[bump],          // 1-byte slice
        ];

        let signer_seeds: &[&[&[u8]]] = &[seeds];
        let cpi_program = ctx.accounts.token_program.to_account_info();

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
                let cpi_program = ctx.accounts.token_program.to_account_info();
                token::transfer(
                    CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds),
                    amount,
                )?;
            }
            OperationType::ZapIn => {
                let p: ZapInParams = deserialize_params(&action)?;
                require!(p.tick_lower < p.tick_upper, OperationError::InvalidTickRange);

                let (sp, mint0, mint1, vault0, vault1, obs_key) = {
                    let pool = ctx.accounts.pool_state.load()?; // Ref<PoolState>
                    (
                        pool.sqrt_price_x64,
                        pool.token_mint_0,
                        pool.token_mint_1,
                        pool.token_vault_0,
                        pool.token_vault_1,
                        pool.observation_key,
                    )
                };

                let remaining_accounts = ctx.remaining_accounts.to_vec();

                // 1) get sqrt prices for ticks (u128); map any tick_math error to your OperationError
                let sa = tick_math::get_sqrt_price_at_tick(p.tick_lower)
                    .map_err(|_| error!(OperationError::InvalidParams))?;
                let sb = tick_math::get_sqrt_price_at_tick(p.tick_upper)
                    .map_err(|_| error!(OperationError::InvalidParams))?;
                // basic sanity check
                require!(sa < sb, OperationError::InvalidTickRange);
                require!(sp >= sa && sp <= sb, OperationError::InvalidParams);

                let sa_u = U256::from(sa);
                let sb_u = U256::from(sb);
                let sp_u = U256::from(sp);

                // (sp - sa) and (sb - sp) with underflow guards
                let sp_minus_sa = if sp_u >= sa_u { sp_u - sa_u } else { return err!(OperationError::InvalidParams); };
                let sb_minus_sp = if sb_u >= sp_u { sb_u - sp_u } else { return err!(OperationError::InvalidParams); };

                // r_num = sb * (sp - sa)
                // r_den = sp * (sb - sp)
                let r_num = sb_u * sp_minus_sa;
                let r_den = sp_u * sb_minus_sp;

                let frac_den = r_den + r_num;
                require!(frac_den > U256::from(0u8), OperationError::InvalidParams);

                let is_base_input = ctx.accounts.program_token_account.mint == ctx.accounts.input_vault_mint.key();

                let amount_in_u256 = U256::from(amount);

                // swap_amount = amount_in * r_num / frac_den      (base input)
                // swap_amount = amount_in * r_den / frac_den      (quote input)
                let swap_amount_u256 = if is_base_input {
                    amount_in_u256.mul_div_floor(r_num, frac_den).ok_or(error!(OperationError::InvalidParams))?
                } else {
                    amount_in_u256.mul_div_floor(r_den, frac_den).ok_or(error!(OperationError::InvalidParams))?
                };
                // convert safely to u64
                let swap_amount = swap_amount_u256.to_underflow_u64();
                require!(swap_amount as u128 <= u64::MAX as u128, OperationError::InvalidParams);

                // 记录前后余额，得到实际成交量
                let pre_out = get_token_balance(&ctx.accounts.output_token_account)?;
                let pre_in  = get_token_balance(&ctx.accounts.input_token_account)?;

                // 组 CPI 账户（把 input/output_token_account 换成托管的两个）
                // let swap_cpi_accounts = ;
                let swap_ctx = CpiContext::new(
                    ctx.accounts.clmm_program.to_account_info(),
                    cpi::accounts::SwapSingleV2 {
                        payer: ctx.accounts.operation_data.to_account_info(),// set operation_data as a payer
                        amm_config: ctx.accounts.amm_config.to_account_info(),
                        pool_state: ctx.accounts.pool_state.to_account_info(),
                        input_token_account: ctx.accounts.input_token_account.to_account_info(),
                        output_token_account: ctx.accounts.output_token_account.to_account_info(),
                        input_vault: ctx.accounts.input_vault.to_account_info(),
                        output_vault: ctx.accounts.output_vault.to_account_info(),
                        observation_state: ctx.accounts.observation_state.to_account_info(),
                        token_program: ctx.accounts.token_program.to_account_info(),
                        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
                        memo_program: ctx.accounts.memo_program.to_account_info(),
                        input_vault_mint: ctx.accounts.input_vault_mint.to_account_info(),
                        output_vault_mint: ctx.accounts.output_vault_mint.to_account_info(),
                    },
                ).with_signer(signer_seeds)
                    .with_remaining_accounts(remaining_accounts.clone());

                let other_amount_threshold = if is_base_input {
                    p.min_amount_out // min out
                } else {
                    p.other_amount_threshold // max in the other side
                };
                cpi::swap_v2(
                    swap_ctx,
                    swap_amount,
                    other_amount_threshold,
                    p.sqrt_price_limit_x64,
                    is_base_input,
                )?;

                // 计算实际花费与获得
                let post_out = get_token_balance(&ctx.accounts.output_token_account)?;
                let post_in  = get_token_balance(&ctx.accounts.input_token_account)?;
                let received = post_out.checked_sub(pre_out).ok_or(error!(OperationError::InvalidParams))?;
                let spent    = pre_in.checked_sub(post_in).ok_or(error!(OperationError::InvalidParams))?;
                let remaining = amount.checked_sub(spent).ok_or(error!(OperationError::InvalidParams))?;

            // 直接默认没有个人头寸：无条件开仓
             let open_ctx = CpiContext::new(
                     ctx.accounts.clmm_program.to_account_info(),
                     cpi::accounts::OpenPositionV2 {
                             payer: ctx.accounts.operation_data.to_account_info(),
                             pool_state: ctx.accounts.pool_state.to_account_info(),
                             position_nft_owner: ctx.accounts.user.to_account_info(),
                             position_nft_mint: ctx.accounts.position_nft_mint.to_account_info(),
                             position_nft_account: ctx.accounts.position_nft_account.to_account_info(),
                             personal_position: ctx.accounts.personal_position.to_account_info(),
                             protocol_position: ctx.accounts.protocol_position.to_account_info(),
                             tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
                             tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
                             token_program: ctx.accounts.token_program.to_account_info(),
                             system_program: ctx.accounts.system_program.to_account_info(),
                             rent: ctx.accounts.rent.to_account_info(),
                             associated_token_program: ctx.accounts.associated_token_program.to_account_info(),
                             token_account_0: ctx.accounts.token_account_0.to_account_info(),
                             token_account_1: ctx.accounts.token_account_1.to_account_info(),
                             token_vault_0: ctx.accounts.input_vault.to_account_info(),
                             token_vault_1: ctx.accounts.output_vault.to_account_info(),
                             vault_0_mint: ctx.accounts.input_vault_mint.to_account_info(),
                             vault_1_mint: ctx.accounts.output_vault_mint.to_account_info(),
                             metadata_program: ctx.accounts.metadata_program.to_account_info(),
                             metadata_account: ctx.accounts.metadata_account.to_account_info(),
                             token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
                        }
                ).with_signer(signer_seeds)
                .with_remaining_accounts(remaining_accounts.clone());


                let  tick_lower_index:i32 = 1;
                let tick_upper_index: i32 = 1;
                let tick_array_lower_start_index = 1;
                let tick_array_upper_start_index = 1;
                let liquidity = 1;
                let amount_0_max = 2;
                let amount_1_max = 12;
                let with_matedata = false;
                let base_flag = true;
                cpi::open_position_v2(
                    open_ctx,
                    p.tick_lower,
                    p.tick_upper,
                    tick_array_lower_start_index,
                    tick_array_upper_start_index,
                    liquidity,
                    amount_0_max,
                    amount_1_max,
                    with_matedata,
                    Some(base_flag),
                )?;
                // 增加流动性（amount_0_max / amount_1_max 根据方向传值）
                let (amount_0_max, amount_1_max) = if is_base_input {
                    (remaining, received)
                } else {
                    (received, remaining)
                };

                let inc_ctx = CpiContext::new(
                    ctx.accounts.clmm_program.to_account_info(),
                    cpi::accounts::IncreaseLiquidityV2 {
                        nft_owner: ctx.accounts.user.to_account_info(),
                        nft_account: ctx.accounts.position_nft_account.to_account_info(),
                        pool_state: ctx.accounts.pool_state.to_account_info(),
                        protocol_position: ctx.accounts.protocol_position.to_account_info(),
                        personal_position: ctx.accounts.personal_position.to_account_info(),
                        tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
                        tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
                        token_account_0: ctx.accounts.input_token_account.to_account_info(),
                        token_account_1: ctx.accounts.output_token_account.to_account_info(),
                        token_vault_0: ctx.accounts.input_vault.to_account_info(),
                        token_vault_1: ctx.accounts.output_vault.to_account_info(),
                        token_program: ctx.accounts.token_program.to_account_info(),
                        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
                        vault_0_mint: ctx.accounts.input_vault_mint.to_account_info(),
                        vault_1_mint: ctx.accounts.output_vault_mint.to_account_info(),
                    }
                ).with_signer(signer_seeds)
                    .with_remaining_accounts(remaining_accounts.clone());

                cpi::increase_liquidity_v2(
                    inc_ctx,
                    0,                // 让 Raydium 计算 liquidity
                    amount_0_max,
                    amount_1_max,
                    Some(is_base_input),
                )?;
            }
            OperationType::ZapOut => {
                let p: ZapOutParams = deserialize_params(&action)?;

                // Basic auth and routing checks
                require!(ctx.accounts.recipient_token_account.owner == p.recipient, OperationError::Unauthorized);

                // Ensure the working token accounts are controlled by our PDA, since the PDA will sign.
                require!(ctx.accounts.input_token_account.owner == ctx.accounts.operation_data.key(), OperationError::Unauthorized);
                require!(ctx.accounts.output_token_account.owner == ctx.accounts.operation_data.key(), OperationError::Unauthorized);

                // What mint do we want to end up with?
                let want_mint = if p.want_base {
                    ctx.accounts.input_vault_mint.key()
                } else {
                    ctx.accounts.output_vault_mint.key()
                };
                require!(ctx.accounts.recipient_token_account.mint == want_mint, OperationError::InvalidMint);

                let remaining_accounts = ctx.remaining_accounts.to_vec();
                let pre0 = get_token_balance(&ctx.accounts.input_token_account)?;
                let pre1 = get_token_balance(&ctx.accounts.output_token_account)?;

                // Figure out how much liquidity to burn.
                // Burn all if not specified (or zero).
                let full_liquidity: u128 = {
                    // PersonalPositionState has a `liquidity` field (u128 in Raydium v3).
                    ctx.accounts.personal_position.liquidity
                };
                require!(full_liquidity > 0, OperationError::InvalidParams);

                let burn_liquidity_u128: u128 = if p.liquidity_to_burn_u64 > 0 {
                    p.liquidity_to_burn_u64 as u128
                } else {
                    full_liquidity
                };
                require!(burn_liquidity_u128 <= full_liquidity, OperationError::InvalidParams);

                // 1) Decrease liquidity → proceeds go to PDA-owned token accounts (input/output_token_account)
                let dec_ctx = CpiContext::new(
                    ctx.accounts.clmm_program.to_account_info(),
                    cpi::accounts::DecreaseLiquidityV2 {
                        nft_owner: ctx.accounts.user.to_account_info(),
                        nft_account: ctx.accounts.position_nft_account.to_account_info(),
                        pool_state: ctx.accounts.pool_state.to_account_info(),
                        protocol_position: ctx.accounts.protocol_position.to_account_info(),
                        personal_position: ctx.accounts.personal_position.to_account_info(),
                        tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
                        tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
                        recipient_token_account_0: ctx.accounts.recipient_token_account_0.to_account_info(),
                        recipient_token_account_1: ctx.accounts.recipient_token_account_1.to_account_info(),
                        token_vault_0: ctx.accounts.input_vault.to_account_info(),
                        token_vault_1: ctx.accounts.output_vault.to_account_info(),
                        token_program: ctx.accounts.token_program.to_account_info(),
                        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
                        vault_0_mint: ctx.accounts.input_vault_mint.to_account_info(),
                        vault_1_mint: ctx.accounts.output_vault_mint.to_account_info(),
                        memo_program: ctx.accounts.memo_program.to_account_info(),
                    }
                ).with_signer(signer_seeds)
                    .with_remaining_accounts(remaining_accounts.clone());

                // For strict slippage on removal, replace the 0/0 mins below with params.
                // Raydium signature matches increase_liquidity_v2 style: (liquidity, amount_0_min, amount_1_min, base_flag)
                cpi::decrease_liquidity_v2(
                    dec_ctx,
                    burn_liquidity_u128,
                    0, // amount_0_min
                    0, // amount_1_min
                )?;

                // Balances after burn
                let post0 = get_token_balance(&ctx.accounts.input_token_account)?;
                let post1 = get_token_balance(&ctx.accounts.output_token_account)?;
                let got0 = post0.checked_sub(pre0).ok_or(error!(OperationError::InvalidParams))?;
                let got1 = post1.checked_sub(pre1).ok_or(error!(OperationError::InvalidParams))?;

                // 2) If we ended with some of the "other" side, swap it into the desired side
                //    so we can pay out in a single mint.
                let (mut total_out, mut swap_amount, is_base_input, swap_ctx) = if p.want_base {
                    // desire mint_0 → swap any mint_1 into mint_0
                    let swap_ctx = CpiContext::new(
                        ctx.accounts.clmm_program.to_account_info(),
                        cpi::accounts::SwapSingleV2 {
                            payer: ctx.accounts.operation_data.to_account_info(),
                            amm_config: ctx.accounts.amm_config.to_account_info(),
                            pool_state: ctx.accounts.pool_state.to_account_info(),
                            input_token_account: ctx.accounts.output_token_account.to_account_info(),  // giving quote
                            output_token_account: ctx.accounts.input_token_account.to_account_info(),  // receiving base
                            input_vault: ctx.accounts.output_vault.to_account_info(),
                            output_vault: ctx.accounts.input_vault.to_account_info(),
                            observation_state: ctx.accounts.observation_state.to_account_info(),
                            token_program: ctx.accounts.token_program.to_account_info(),
                            token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
                            memo_program: ctx.accounts.memo_program.to_account_info(),
                            input_vault_mint: ctx.accounts.output_vault_mint.to_account_info(),
                            output_vault_mint: ctx.accounts.input_vault_mint.to_account_info(),
                        },
                    ).with_signer(signer_seeds)
                        .with_remaining_accounts(remaining_accounts.clone());
                    (got0, got1, false, swap_ctx)
                } else {
                    // desire mint_1 → swap any mint_0 into mint_1
                    let swap_ctx = CpiContext::new(
                        ctx.accounts.clmm_program.to_account_info(),
                        cpi::accounts::SwapSingleV2 {
                            payer: ctx.accounts.operation_data.to_account_info(),
                            amm_config: ctx.accounts.amm_config.to_account_info(),
                            pool_state: ctx.accounts.pool_state.to_account_info(),
                            input_token_account: ctx.accounts.input_token_account.to_account_info(),   // giving base
                            output_token_account: ctx.accounts.output_token_account.to_account_info(), // receiving quote
                            input_vault: ctx.accounts.input_vault.to_account_info(),
                            output_vault: ctx.accounts.output_vault.to_account_info(),
                            observation_state: ctx.accounts.observation_state.to_account_info(),
                            token_program: ctx.accounts.token_program.to_account_info(),
                            token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
                            memo_program: ctx.accounts.memo_program.to_account_info(),
                            input_vault_mint: ctx.accounts.input_vault_mint.to_account_info(),
                            output_vault_mint: ctx.accounts.output_vault_mint.to_account_info(),
                        },
                    ).with_signer(signer_seeds)
                        .with_remaining_accounts(remaining_accounts.clone());
                    (got1, got0, true, swap_ctx)
                };

                if swap_amount > 0 {
                    cpi::swap_v2(
                        swap_ctx,
                        swap_amount,
                        p.other_amount_threshold,
                        p.sqrt_price_limit_x64,
                        is_base_input,
                    )?;

                    // refresh total_out after swap
                    if p.want_base {
                        let new_base = get_token_balance(&ctx.accounts.input_token_account)?;
                        total_out = new_base.checked_sub(pre0).ok_or(error!(OperationError::InvalidParams))?;
                    } else {
                        let new_quote = get_token_balance(&ctx.accounts.output_token_account)?;
                        total_out = new_quote.checked_sub(pre1).ok_or(error!(OperationError::InvalidParams))?;
                    }
                }

                // 3) Enforce payout floors (both the op.amount you stored and the param-level min)
                require!(total_out >= p.min_payout, OperationError::InvalidParams);
                require!(total_out >= amount, OperationError::InvalidParams); // `amount` from OperationData (acts as a floor)

                // 4) Send to recipient (single-sided)
                let from_ai = if p.want_base {
                    ctx.accounts.input_token_account.to_account_info()
                } else {
                    ctx.accounts.output_token_account.to_account_info()
                };

                let cpi_accounts = Transfer {
                    from: from_ai,
                    to: ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.operation_data.to_account_info(),
                };
                token::transfer(
                    CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds),
                    total_out,
                )?;

                // Optionally: if all liquidity burned, you could close the position NFT here via Raydium's close instruction.
                // (Left out to keep this minimal and because it requires extra accounts; add if needed.)
            }
        }

        {
            let od_rw = &mut ctx.accounts.operation_data;
            od_rw.executed = true;
        }
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


fn get_token_balance(acc: &InterfaceAccount<InterfaceTokenAccount>) -> Result<u64> {
    Ok(acc.amount)
}
// Helper function to deserialize params
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
        constraint = authority_ata.owner == authority.key() @ OperationError::Unauthorized // Changed to check authority ownership
    )]
    pub authority_ata: Account<'info, TokenAccount>, // Changed from user_ata to authority_ata
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
    pub program_token_account: Box<Account<'info, InterfaceTokenAccount>>,

    #[account(mut)]
    pub recipient_token_account: Box<Account<'info, InterfaceTokenAccount>>, // concrete, fine

    #[account(mut)]
    pub user: Signer<'info>,

    // switch to interface types for InterfaceAccount
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

    // pool vaults should be interface accounts too
    #[account(mut, address = pool_state.load()?.token_vault_0)]
    pub input_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.token_vault_1)]
    pub output_vault: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    // these constraints now reference interface mints
    #[account(address = pool_state.load()?.token_mint_0)]
    pub input_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = pool_state.load()?.token_mint_1)]
    pub output_vault_mint: Box<InterfaceAccount<'info, InterfaceMint>>,

    #[account(address = spl_memo::id())]
    pub memo_program: UncheckedAccount<'info>,

    #[account(
        constraint = clmm_program.key() == RAYDIUM_CLMM_PROGRAM_ID
    )]
    pub clmm_program: Program<'info, AmmV3>,

    /// The destination token account for receive amount_0
    #[account(
        mut,
        token::mint = token_vault_0.mint
    )]
    pub recipient_token_account_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The destination token account for receive amount_1
    #[account(
        mut,
        token::mint = token_vault_1.mint
    )]
    pub recipient_token_account_1: Box<InterfaceAccount<'info, TokenAccount>>,

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
        token::mint = token_vault_0.mint
    )]
    pub token_account_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(
        mut,
        token::mint = token_vault_1.mint
    )]
    pub token_account_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(mut)]
    pub metadata_account: UncheckedAccount<'info>,

    pub metadata_program: Program<'info, Metadata>,

    #[account(
        mut,
        constraint = token_vault_0.key() == pool_state.load()?.token_vault_0
    )]
    pub token_vault_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// The address that holds pool tokens for token_1
    #[account(
        mut,
        constraint = token_vault_1.key() == pool_state.load()?.token_vault_1
    )]
    pub token_vault_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    /// Associated Token Program
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
fn get_swapped_amount(_output_token_account: &InterfaceAccount<TokenAccount>) -> Result<u64> {
    // Fetch the actual swapped amount from output_token_account balance
    // This is a placeholder; you may need to track the balance change
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

// Parameters for ZapIn operation (Raydium CLMM)
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ZapInParams {
    pub amount_in: u64,       // Amount of input token to zap in
    pub min_amount_out: u64,  // Minimum output token received from swap
    pub pool: Pubkey,         // Raydium CLMM pool address
    pub token_a_mint: Pubkey, // Mint of token A (e.g., USDC)
    pub token_b_mint: Pubkey, // Mint of token B (e.g., TSLAx)
    pub tick_lower: i32,      // Lower tick for liquidity range
    pub tick_upper: i32,      // Upper tick for liquidity range
    pub sqrt_price_limit_x64: u128,
    pub other_amount_threshold: u64,
}
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ZapOutParams {
    /// If true, end with base token(mint_0), If false, end with quote token(mint_1)
    pub want_base: bool,
    /// Mint total tokens you ultimately send to recipient(same mint as the desired side).
    pub min_payout: u64,
    /// Swap constraints (only used if we need to convert the “other” side).
    pub sqrt_price_limit_x64: u128,
    pub other_amount_threshold: u64,
    /// Who should receive the final token (checked against the provided ATA’s owner).
    pub recipient: Pubkey,
    /// If provided and > 0, burn exactly this liquidity. Otherwise burn all current liquidity.
    /// (Raydium liquidity is u128; we accept u64 here and cast—good enough for many cases.
    /// If you need full-precision control, switch to u128.)
    pub liquidity_to_burn_u64: u64,

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
    InvalidProgramAccount
}
