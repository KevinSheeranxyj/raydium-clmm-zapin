use anchor_lang::prelude::*;
use anchor_lang::solana_program::pubkey;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use anchor_spl::token_interface::{Token2022, Mint as InterfaceMint, TokenAccount as InterfaceTokenAccount};
use anchor_spl::metadata::Metadata;
use anchor_lang::prelude::Rent;
use anchor_spl::memo::spl_memo;
use anchor_lang::system_program;
use anchor_lang::prelude::Sysvar;
use anchor_lang::error::Error;
use raydium_amm_v3::libraries::{big_num::*, full_math::MulDiv, tick_math};
use anchor_spl::associated_token::AssociatedToken;
use std::str::FromStr;
use anchor_lang::solana_program::sysvar;
use raydium_amm_v3::{
    cpi,
    program::AmmV3,
    states::{PoolState, AmmConfig, POSITION_SEED, TICK_ARRAY_SEED, ObservationState, TickArrayState, ProtocolPositionState, PersonalPositionState},
};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_lang::solana_program::hash::hash as solana_hash;
use anchor_lang::solana_program::{
    program::invoke_signed,
    program_pack::Pack,
    system_instruction,
};
use anchor_spl::token::spl_token;

declare_id!("DgrQqeR5MTFkNNG94siEd5cDxdzgexSNwbv4FHdNW8f3");

pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
    pubkey!("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"); // mainnet program ID

/// NOTE: For ZapIn & ZapOut, we're leveraging the Raydium-Amm-v3 Protocol SDK to robost our requirement
#[program]
pub mod dg_solana_zapin {
    use super::*;


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

    #[event]
    pub struct LiquidityAdded {
        pub transfer_id: String,
        pub token0_used: u64,
        pub token1_used: u64,
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
        let od = &mut ctx.accounts.operation_data;

        // 初始化（首次该 transfer_id）
        if !od.initialized {
            od.authority = ctx.accounts.authority.key();
            od.initialized = true;
            msg!("Initialized operation_data for transfer_id {} with authority {}", transfer_id, od.authority);
        }
        let id_hash = transfer_id_hash_bytes(&transfer_id);
        let reg = &mut ctx.accounts.registry;
        require!(!reg.used_ids.contains(&id_hash), OperationError::DuplicateTransferId);
        reg.used_ids.push(id_hash);

        require!(amount > 0, OperationError::InvalidAmount);
        require!(!transfer_id.is_empty(), OperationError::InvalidTransferId);

        // 资金转入（保持原逻辑）
        let cpi_accounts = Transfer {
            from: ctx.accounts.authority_ata.to_account_info(),
            to: ctx.accounts.program_token_account.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        // 存基础参数
        od.transfer_id = transfer_id.clone();
        od.amount = amount;
        od.executed = false;
        od.ca = ca;
        od.operation_type = operation_type.clone();
        od.action = action.clone(); // 保留原始参数

        // ====== 存 Raydium 固定账户（直接从 ctx 读 pubkey）======
        od.clmm_program_id   = ctx.accounts.clmm_program.key();
        od.pool_state        = ctx.accounts.pool_state.key();
        od.amm_config        = ctx.accounts.amm_config.key();
        od.observation_state = ctx.accounts.observation_state.key();
        od.token_vault_0     = ctx.accounts.token_vault_0.key();
        od.token_vault_1     = ctx.accounts.token_vault_1.key();
        od.token_mint_0      = ctx.accounts.token_mint_0.key();
        od.token_mint_1      = ctx.accounts.token_mint_1.key();

        // 如果是 ZapIn，解析参数并派生 tick array / protocol_position 等，存起来
        if let OperationType::ZapIn = operation_type {
            let p: ZapInParams = deserialize_params(&od.action)?;
            od.tick_lower = p.tick_lower;
            od.tick_upper = p.tick_upper;

            // 根据 pool 的 tick_spacing 计算 tick array 起始
            let pool = ctx.accounts.pool_state.load()?;
            let tick_spacing: i32 = pool.tick_spacing.into();
            let lower_start = tick_array_start_index(p.tick_lower, tick_spacing);
            let upper_start = tick_array_start_index(p.tick_upper, tick_spacing);

            // Raydium tick array PDA（由外部提供，但我们把“应有地址”存起来用作后续校验）
            let (ta_lower, _) = Pubkey::find_program_address(
                &[
                    TICK_ARRAY_SEED.as_bytes(),
                    ctx.accounts.pool_state.key().as_ref(),
                    &lower_start.to_be_bytes(),
                ],
                &ctx.accounts.clmm_program.key(),
            );
            let (ta_upper, _) = Pubkey::find_program_address(
                &[
                    TICK_ARRAY_SEED.as_bytes(),
                    ctx.accounts.pool_state.key().as_ref(),
                    &upper_start.to_be_bytes(),
                ],
                &ctx.accounts.clmm_program.key(),
            );
            od.tick_array_lower = ta_lower;
            od.tick_array_upper = ta_upper;

            // 协议仓位 PDA（Raydium POSITION_SEED, pool, lower_start, upper_start）
            let (pp, _) = Pubkey::find_program_address(
                &[
                    POSITION_SEED.as_bytes(),
                    ctx.accounts.pool_state.key().as_ref(),
                    &lower_start.to_be_bytes(),
                    &upper_start.to_be_bytes(),
                ],
                &ctx.accounts.clmm_program.key(),
            );
            od.protocol_position = pp;

            // Position NFT mint（deposit 阶段未持有 user；先置空，execute 再写）
            od.position_nft_mint = Pubkey::default();
        }

        // 如果是 Transfer，存 recipient
        if let OperationType::Transfer = operation_type {
            let p: TransferParams = deserialize_params(&od.action)?;
            od.recipient = p.recipient;
        }

        emit!(DepositEvent { transfer_id, amount, recipient: od.recipient });
        Ok(())
    }

    // Execute the token transfer (ZapIn only)
    pub fn execute(ctx: Context<Execute>, transfer_id: String) -> Result<()> {
        let id_hash = transfer_id_hash_bytes(&transfer_id);
        require!(ctx.accounts.registry.used_ids.contains(&id_hash), OperationError::InvalidTransferId);
        // recompute expected PDA from the stored transfer_id
        let (expected_pda, _bump) = Pubkey::find_program_address(
            &[b"operation_data", ctx.accounts.operation_data.transfer_id.as_bytes()],
            ctx.program_id,
        );
        require!(expected_pda == ctx.accounts.operation_data.key(), OperationError::InvalidParams);
        // 基础校验（只读借用，立刻结束）
        {
            let od_ref = &ctx.accounts.operation_data;
            require!(od_ref.initialized, OperationError::NotInitialized);
            require!(!od_ref.executed, OperationError::AlreadyExecuted);
            require!(od_ref.amount > 0, OperationError::InvalidAmount);
            require!(od_ref.transfer_id == transfer_id, OperationError::InvalidTransferId);
            require!(matches!(od_ref.operation_type, OperationType::ZapIn), OperationError::InvalidParams);
        }

        // 拷出关键字段（结束对 od 的借用）
        let (
            pool_state_key, amm_config_key, observation_key,
            vault0_key, vault1_key, mint0_key, mint1_key,
            tick_array_lower_key, tick_array_upper_key,
            protocol_pos_key, ca_mint,
            amount_total,
            mut pos_mint, personal_pos_stored,
            action_bytes, pool_key_for_pos_mint,
        ) = {
            let od = &ctx.accounts.operation_data;
            (
                od.pool_state, od.amm_config, od.observation_state,
                od.token_vault_0, od.token_vault_1, od.token_mint_0, od.token_mint_1,
                od.tick_array_lower, od.tick_array_upper,
                od.protocol_position, od.ca,
                od.amount,
                od.position_nft_mint, od.personal_position,
                od.action.clone(), od.pool_state,
            )
        };
        let clmm_pid = { let od = &ctx.accounts.operation_data; od.clmm_program_id };
        let user_key = ctx.accounts.user.key();
        let od_key   = ctx.accounts.operation_data.key();

        // signer seeds
        let bump = ctx.bumps.operation_data;
        let signer_seeds_slice: [&[u8]; 3] = [b"operation_data".as_ref(), transfer_id.as_bytes(), &[bump]];
        let signer_seeds: &[&[&[u8]]] = &[&signer_seeds_slice];
        // signer seeds also use the stored transfer_id
        let mut personal_pos_key_maybe: Option<Pubkey> = None;
        {
            let ras = ctx.remaining_accounts;

            // ---- local index finders (index-based to avoid lifetime fights) ----
            let find_idx = |key: &Pubkey, label: &str| -> Result<usize> {
                ras.iter()
                    .position(|ai| *ai.key == *key)
                    .ok_or_else(|| {
                        msg!("missing account in remaining_accounts: {} = {}", label, key);
                        error!(OperationError::InvalidParams)
                    })
            };

            let find_pda_token_idx = |owner: &Pubkey, mint: &Pubkey, label: &str| -> Result<usize> {
                ras.iter()
                    .position(|ai| {
                        let Ok(data_ref) = ai.try_borrow_data() else { return false; };
                        if data_ref.len() < spl_token::state::Account::LEN { return false; }
                        if let Ok(acc) = spl_token::state::Account::unpack_from_slice(&data_ref) {
                            acc.owner == *owner && acc.mint == *mint
                        } else {
                            false
                        }
                    })
                    .ok_or_else(|| {
                        msg!("missing account in remaining_accounts: {} (owner={}, mint={})", label, owner, mint);
                        error!(OperationError::InvalidParams)
                    })
            };

            let find_user_token_idx = |user: &Pubkey, mint: &Pubkey, label: &str| -> Result<usize> {
                ras.iter()
                    .position(|ai| {
                        let Ok(data_ref) = ai.try_borrow_data() else { return false; };
                        if data_ref.len() < spl_token::state::Account::LEN { return false; }
                        if let Ok(acc) = spl_token::state::Account::unpack_from_slice(&data_ref) {
                            acc.owner == *user && acc.mint == *mint
                        } else {
                            false
                        }
                    })
                    .ok_or_else(|| {
                        msg!("missing account in remaining_accounts: {} (owner=user {}, mint={})", label, user, mint);
                        error!(OperationError::InvalidParams)
                    })
            };

            // ---- programs / sysvars / identities ----
            let clmm_prog_ai    = ras[find_idx(&clmm_pid, "clmm_program")?].clone();
            let token_prog_ai   = ras[find_idx(&token::ID, "token_program")?].clone();
            let token22_prog_ai = ras[find_idx(&Token2022::id(), "token_program_2022")?].clone();
            let memo_prog_ai    = ras[find_idx(&spl_memo::id(), "memo_program")?].clone();
            let system_prog_ai  = ras[find_idx(&system_program::ID, "system_program")?].clone();
            let rent_sysvar_ai  = ras[find_idx(&sysvar::rent::id(), "rent_sysvar")?].clone();
            let user_ai         = ras[find_idx(&user_key, "user")?].clone();
            let operation_ai    = ras[find_idx(&od_key, "operation_data_pda")?].clone();

            // ---- pool/config/observation + vaults + mints ----
            let pool_state      = ras[find_idx(&pool_state_key, "pool_state")?].clone();
            let amm_config      = ras[find_idx(&amm_config_key, "amm_config")?].clone();
            let observation     = ras[find_idx(&observation_key, "observation_state")?].clone();
            let vault0          = ras[find_idx(&vault0_key, "token_vault_0")?].clone();
            let vault1          = ras[find_idx(&vault1_key, "token_vault_1")?].clone();
            let mint0           = ras[find_idx(&mint0_key, "token_mint_0")?].clone();
            let mint1           = ras[find_idx(&mint1_key, "token_mint_1")?].clone();

            // ---- tick arrays & protocol position ----
            let ta_lower        = ras[find_idx(&tick_array_lower_key, "tick_array_lower")?].clone();
            let ta_upper        = ras[find_idx(&tick_array_upper_key, "tick_array_upper")?].clone();
            let protocol_pos    = ras[find_idx(&protocol_pos_key, "protocol_position")?].clone();

            // ---- PDA-owned input/output token accounts (mint0/mint1) ----
            let pda_input_ata   = ras[find_pda_token_idx(&od_key, &mint0_key, "pda_input_token_account")?].clone();
            let pda_output_ata  = ras[find_pda_token_idx(&od_key, &mint1_key, "pda_output_token_account")?].clone();

            // ---- program_token_account (deposit destination; owner = operation_data PDA, mint = mint0 or mint1) ----
            let program_token_account = ras.iter().find(|ai| {
                if ai.key == pda_input_ata.key || ai.key == pda_output_ata.key { return false; }
                unpack_token_account(ai).map_or(false, |acc|
                acc.owner == od_key && (acc.mint == mint0_key || acc.mint == mint1_key)
                )
            }).ok_or_else(|| error!(OperationError::InvalidParams))?.clone();

            // ---- refund recipient token account (user ATA of the same mint as program_token_account.mint) ----
            let program_token_mint = unpack_token_account(&program_token_account)
                .ok_or_else(|| error!(OperationError::InvalidParams))?
                .mint;
            let recipient_refund_ata =
                ras[find_user_token_idx(&user_key, &program_token_mint, "recipient_refund_ata")?].clone();

            // ---- position NFT mint (PDA of *this* program) & user NFT ATA ----
            if pos_mint == Pubkey::default() {
                let (m, _) = Pubkey::find_program_address(
                    &[b"pos_nft_mint", user_key.as_ref(), pool_key_for_pos_mint.as_ref()],
                    ctx.program_id,
                );
                pos_mint = m;
            }
            let position_nft_mint_ai = ras[find_idx(&pos_mint, "position_nft_mint")?].clone();

            // user ATA for position NFT
            let pos_nft_ata_key = anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &user_key,
                &pos_mint,
                &anchor_spl::token::ID,
            );
            let position_nft_account = ras[find_idx(&pos_nft_ata_key, "position_nft_account(user ATA)")?].clone();

            // ---- personal_position（优先已存，否则猜测一个可反序列化的）----
            let (personal_position, guessed) =
                if personal_pos_stored != Pubkey::default() {
                    (ras[find_idx(&personal_pos_stored, "personal_position")?].clone(), None)
                } else {
                    let guess_ref = ras
                        .iter()
                        .find(|ai| is_anchor_account::<raydium_amm_v3::states::PersonalPositionState>(ai))
                        .ok_or_else(|| {
                            msg!("missing personal_position in remaining_accounts");
                            error!(OperationError::InvalidParams)
                        })?;
                    (guess_ref.clone(), Some(guess_ref.key()))
                };
            personal_pos_key_maybe = guessed;

            // ---------- parse ZapIn params ----------
            let p: ZapInParams = deserialize_params(&action_bytes)?;
            require!(p.tick_lower < p.tick_upper, OperationError::InvalidTickRange);

            // ca must equal one of the pool mints
            require!(ca_mint == mint0_key || ca_mint == mint1_key, OperationError::InvalidMint);

            // determine which side the deposit came in
            let is_base_input = program_token_mint == mint0_key;

            // ---------- price/fees ----------
            let pool_state_data = raydium_amm_v3::states::PoolState::try_deserialize(&mut &pool_state.try_borrow_data()?[..])
                .map_err(|_| error!(OperationError::InvalidParams))?;
            let sp = pool_state_data.sqrt_price_x64;

            let sp_u = U256::from(sp);
            let q64_u = U256::from(Q64_U128);
            let price_q64 = sp_u.mul_div_floor(sp_u, q64_u).ok_or(error!(OperationError::InvalidParams))?;

            // amm_config fees
            let cfg = raydium_amm_v3::states::AmmConfig::try_deserialize(&mut &amm_config.try_borrow_data()?[..])
                .map_err(|_| error!(OperationError::InvalidParams))?;
            let trade_fee_bps: u32 = cfg.trade_fee_rate.into();
            let protocol_fee_bps: u32 = cfg.protocol_fee_rate.into();
            let total_fee_bps = trade_fee_bps + protocol_fee_bps;

            let slip_bps = p.slippage_bps.min(10_000);
            let one = U256::from(10_000u32);
            let fee_factor = one - U256::from(total_fee_bps);
            let slip_factor = one - U256::from(slip_bps);
            let discount = fee_factor.mul_div_floor(slip_factor, one).ok_or(error!(OperationError::InvalidParams))?;

            let amount_in_u = U256::from(p.amount_in);
            let min_amount_out_u = if is_base_input {
                amount_in_u.mul_div_floor(price_q64, q64_u).ok_or(error!(OperationError::InvalidParams))?
                    .mul_div_floor(discount, one).ok_or(error!(OperationError::InvalidParams))?
            } else {
                amount_in_u.mul_div_floor(q64_u, price_q64.max(U256::from(1u8))).ok_or(error!(OperationError::InvalidParams))?
                    .mul_div_floor(discount, one).ok_or(error!(OperationError::InvalidParams))?
            };
            let min_amount_out = min_amount_out_u.to_underflow_u64();

            // tick-derived checks / tick array consistency
            let sa = tick_math::get_sqrt_price_at_tick(p.tick_lower).map_err(|_| error!(OperationError::InvalidParams))?;
            let sb = tick_math::get_sqrt_price_at_tick(p.tick_upper).map_err(|_| error!(OperationError::InvalidParams))?;
            require!(sa < sb, OperationError::InvalidTickRange);
            require!(sp >= sa && sp <= sb, OperationError::InvalidParams);

            // ---------- refund path when amount < requested ----------
            if amount_total < p.amount_in {
                let refund_cpi = Transfer {
                    from: program_token_account.clone(),
                    to: recipient_refund_ata.clone(),
                    authority: operation_ai.clone(),
                };
                token::transfer(
                    CpiContext::new_with_signer(token_prog_ai.clone(), refund_cpi, signer_seeds),
                    amount_total,
                )?;
                ctx.accounts.operation_data.executed = true;
                msg!("ZapIn refund: expected {}, received {}, refunded all.", p.amount_in, amount_total);
                return Ok(());
            }

            // ---------- compute single-swap split ----------
            let sa_u = U256::from(sa);
            let sb_u = U256::from(sb);
            let sp_u2 = U256::from(sp);
            let sp_minus_sa = if sp_u2 >= sa_u { sp_u2 - sa_u } else { return err!(OperationError::InvalidParams); };
            let sb_minus_sp = if sb_u >= sp_u2 { sb_u - sp_u2 } else { return err!(OperationError::InvalidParams); };
            let r_num = sb_u * sp_minus_sa;
            let r_den = sp_u2 * sb_minus_sp;
            let frac_den = r_den + r_num;
            require!(frac_den > U256::from(0u8), OperationError::InvalidParams);

            let swap_amount = if is_base_input {
                U256::from(p.amount_in).mul_div_floor(r_num, frac_den).ok_or(error!(OperationError::InvalidParams))?
            } else {
                U256::from(p.amount_in).mul_div_floor(r_den, frac_den).ok_or(error!(OperationError::InvalidParams))?
            }.to_underflow_u64();

            // ---------- move deposit to proper PDA input/output account ----------
            let to_acc = if is_base_input { pda_input_ata.clone() } else { pda_output_ata.clone() };
            let move_cpi = Transfer {
                from: program_token_account.clone(),
                to: to_acc,
                authority: operation_ai.clone(),
            };
            token::transfer(
                CpiContext::new_with_signer(token_prog_ai.clone(), move_cpi, signer_seeds),
                amount_total,
            )?;

            // record balances
            let pre_out = if is_base_input { load_token_amount(&pda_output_ata)? } else { load_token_amount(&pda_input_ata)? };
            let pre_in = if is_base_input { load_token_amount(&pda_input_ata)? } else { load_token_amount(&pda_output_ata)? };

            // ---------- swap in pool ----------
            {
                let (in_acc, out_acc, in_vault, out_vault, in_mint, out_mint) = if is_base_input {
                    (pda_input_ata.clone(), pda_output_ata.clone(), vault0.clone(), vault1.clone(), mint0.clone(), mint1.clone())
                } else {
                    (pda_output_ata.clone(), pda_input_ata.clone(), vault1.clone(), vault0.clone(), mint1.clone(), mint0.clone())
                };

                let swap_accounts = cpi::accounts::SwapSingleV2 {
                    payer: operation_ai.clone(),
                    amm_config: amm_config.clone(),
                    pool_state: pool_state.clone(),
                    input_token_account: in_acc,
                    output_token_account: out_acc,
                    input_vault: in_vault,
                    output_vault: out_vault,
                    observation_state: observation.clone(),
                    token_program: token_prog_ai.clone(),
                    token_program_2022: token22_prog_ai.clone(),
                    memo_program: memo_prog_ai.clone(),
                    input_vault_mint: in_mint,
                    output_vault_mint: out_mint,
                };
                let swap_ctx = CpiContext::new(clmm_prog_ai.clone(), swap_accounts)
                    .with_signer(signer_seeds);
                cpi::swap_v2(
                    swap_ctx,
                    swap_amount,
                    min_amount_out,
                    0,
                    is_base_input,
                )?;
            }

            // delta after swap
            let post_out = if is_base_input { load_token_amount(&pda_output_ata)? } else { load_token_amount(&pda_input_ata)? };
            let post_in = if is_base_input { load_token_amount(&pda_input_ata)? } else { load_token_amount(&pda_output_ata)? };
            let received = post_out.checked_sub(pre_out).ok_or(error!(OperationError::InvalidParams))?;
            let spent = pre_in.checked_sub(pre_in.min(post_in)).ok_or(error!(OperationError::InvalidParams))?;
            let remaining = amount_total.checked_sub(spent).ok_or(error!(OperationError::InvalidParams))?;

            // ---------- ensure position_nft_mint account exists (create if empty) ----------
            if position_nft_mint_ai.data_is_empty() {
                let mint_space = spl_token::state::Mint::LEN;
                let rent_lamports = Rent::get()?.minimum_balance(mint_space);

                let create_ix = system_instruction::create_account(
                    &user_key,
                    &pos_mint,
                    rent_lamports,
                    mint_space as u64,
                    &anchor_spl::token::ID,
                );

                // 为 pos_nft_mint PDA 计算 bump
                let (_pk, bump) = Pubkey::find_program_address(
                    &[b"pos_nft_mint", user_key.as_ref(), pool_key_for_pos_mint.as_ref()],
                    ctx.program_id,
                );
                let bump_bytes = [bump];

                let seeds: &[&[u8]] = &[
                    b"pos_nft_mint",
                    user_key.as_ref(),
                    pool_key_for_pos_mint.as_ref(),
                    &bump_bytes,
                ];
                invoke_signed(
                    &create_ix,
                    &[
                        user_ai.clone(),
                        position_nft_mint_ai.clone(),
                        system_prog_ai.clone(),
                    ],
                    &[seeds],
                )?;
            }

            // ---------- open position (mint NFT) ----------
            {
                let ata_idx = find_idx(&anchor_spl::associated_token::ID, "associated_token_program")?;
                let open_accounts = cpi::accounts::OpenPositionV2 {
                    payer: operation_ai.clone(),
                    pool_state: pool_state.clone(),
                    position_nft_owner: user_ai.clone(),
                    position_nft_mint: position_nft_mint_ai.clone(),
                    position_nft_account: position_nft_account.clone(),
                    personal_position: personal_position.clone(),
                    protocol_position: protocol_pos.clone(),
                    tick_array_lower: ta_lower.clone(),
                    tick_array_upper: ta_upper.clone(),
                    token_program: token_prog_ai.clone(),
                    system_program: system_prog_ai.clone(),
                    rent: rent_sysvar_ai.clone(),
                    associated_token_program: ras[ata_idx].clone(),
                    token_account_0: pda_input_ata.clone(),
                    token_account_1: pda_output_ata.clone(),
                    token_vault_0: vault0.clone(),
                    token_vault_1: vault1.clone(),
                    vault_0_mint: mint0.clone(),
                    vault_1_mint: mint1.clone(),
                    metadata_program: memo_prog_ai.clone(),
                    metadata_account: position_nft_account.clone(),
                    token_program_2022: token22_prog_ai.clone(),
                };

                let pool = pool_state_data;
                let tick_spacing: i32 = pool.tick_spacing.into();
                let lower_start = tick_array_start_index(p.tick_lower, tick_spacing);
                let upper_start = tick_array_start_index(p.tick_upper, tick_spacing);

                let open_ctx = CpiContext::new(clmm_prog_ai.clone(), open_accounts)
                    .with_signer(signer_seeds);

                cpi::open_position_v2(
                    open_ctx,
                    p.tick_lower,
                    p.tick_upper,
                    lower_start,
                    upper_start,
                    0u128,
                    0u64,
                    0u64,
                    false,            // with_metadata
                    Some(true),       // base_flag
                )?;
            }

            // ---------- increase liquidity (use remaining + received) ----------
            {
                let (amount_0_max, amount_1_max) = if is_base_input { (remaining, received) } else { (received, remaining) };

                let inc_accounts = cpi::accounts::IncreaseLiquidityV2 {
                    nft_owner: user_ai.clone(),
                    nft_account: position_nft_account.clone(),
                    pool_state: pool_state.clone(),
                    protocol_position: protocol_pos.clone(),
                    personal_position: personal_position.clone(),
                    tick_array_lower: ta_lower.clone(),
                    tick_array_upper: ta_upper.clone(),
                    token_account_0: pda_input_ata.clone(),
                    token_account_1: pda_output_ata.clone(),
                    token_vault_0: vault0.clone(),
                    token_vault_1: vault1.clone(),
                    token_program: token_prog_ai.clone(),
                    token_program_2022: token22_prog_ai.clone(),
                    vault_0_mint: mint0.clone(),
                    vault_1_mint: mint1.clone(),
                };
                let inc_ctx = CpiContext::new(clmm_prog_ai, inc_accounts)
                    .with_signer(signer_seeds);
                cpi::increase_liquidity_v2(
                    inc_ctx,
                    0,
                    amount_0_max,
                    amount_1_max,
                    Some(is_base_input),
                )?;

                emit!(LiquidityAdded {
                    transfer_id: transfer_id.clone(),
                    token0_used: (if is_base_input { post_in }  else { post_out }).saturating_sub(load_token_amount(&pda_input_ata)?),
                    token1_used: (if is_base_input { post_out } else { post_in  }).saturating_sub(load_token_amount(&pda_output_ata)?),
                });
            }
        }

        // 写回（仅在需要时）
        {
            let od_mut = &mut ctx.accounts.operation_data;
            if od_mut.personal_position == Pubkey::default() {
                if let Some(k) = personal_pos_key_maybe.take() {
                    od_mut.personal_position = k;
                }
            }
            if od_mut.position_nft_mint == Pubkey::default() {
                od_mut.position_nft_mint = pos_mint;
            }
            od_mut.executed = true;
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

fn find_acc_idx(ras: &[AccountInfo], key: &Pubkey, label: &str) -> Result<usize> {
    ras.iter()
        .position(|ai| *ai.key == *key)
        .ok_or_else(|| {
            msg!("missing account in remaining_accounts: {} = {}", label, key);
            error!(OperationError::InvalidParams)
        })
}

fn unpack_token_account(ai: &AccountInfo) -> Option<spl_token::state::Account> {
    let data = ai.try_borrow_data().ok()?;
    spl_token::state::Account::unpack_from_slice(&data).ok()
}


fn try_deser_anchor_account<T: AccountDeserialize>(ai: &AccountInfo) -> Option<T> {
    let data_ref = ai.try_borrow_data().ok()?;   // Ref<[u8]>
    let mut bytes: &[u8] = &data_ref;            // &Ref<[u8]> -> &[u8]
    T::try_deserialize(&mut bytes).ok()
}

/// 只检查某 AccountInfo 是否是某个 Anchor 类型（通过 try_deserialize 是否成功）
fn is_anchor_account<T: AccountDeserialize>(ai: &AccountInfo) -> bool {
    try_deser_anchor_account::<T>(ai).is_some()
}

fn load_token_amount(ai: &AccountInfo) -> Result<u64> {
    let data = ai.try_borrow_data()?;
    let acc = spl_token::state::Account::unpack_from_slice(&data)
        .map_err(|_| error!(OperationError::InvalidParams))?;
    Ok(acc.amount)
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
#[instruction(transfer_id: String)]
pub struct Deposit<'info> {
    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + Registry::LEN,
        seeds = [b"registry"], bump
    )]
    pub registry: Account<'info, Registry>,

    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + OperationData::LEN,
        seeds = [
        b"operation_data".as_ref(),
        transfer_id.as_bytes()
        ],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,

    #[account(mut)]
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


    // ===== 新增：Raydium CPI 相关账户（只读一致性，deposit 时校验并落库） =====
    #[account(constraint = clmm_program.key() == RAYDIUM_CLMM_PROGRAM_ID)]
    pub clmm_program: Program<'info, AmmV3>,

    // 池 & 配置
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    // Vault & Mint
    #[account(mut, address = pool_state.load()?.token_vault_0)]
    pub token_vault_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(mut, address = pool_state.load()?.token_vault_1)]
    pub token_vault_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,
    #[account(address = pool_state.load()?.token_mint_0)]
    pub token_mint_0: Box<InterfaceAccount<'info, InterfaceMint>>,
    #[account(address = pool_state.load()?.token_mint_1)]
    pub token_mint_1: Box<InterfaceAccount<'info, InterfaceMint>>,

    // 系统/程序
    #[account(constraint = token_program.key() == token::ID @ OperationError::InvalidTokenProgram)]
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct PositionBounds {
    pub tick_lower: i32,
    pub tick_upper: i32,
}




#[derive(Accounts)]
#[instruction(transfer_id: String)]
pub struct Execute<'info> {
    #[account(
        mut,
        seeds = [
        b"operation_data".as_ref(),
        transfer_id.as_bytes(),
        ],
        bump
    )]
    pub operation_data: Box<Account<'info, OperationData>>,

    #[account(mut, seeds=[b"registry"], bump)]
    pub registry: Account<'info, Registry>,

    // 用户作为 position NFT 的所有者和 payer
    #[account(mut)]
    pub user: Signer<'info>,

    // 程序/系统
    /// CHECK: memo program
    #[account(address = spl_memo::id())]
    pub memo_program: UncheckedAccount<'info>,
    #[account(constraint = clmm_program.key() == operation_data.clmm_program_id)]
    pub clmm_program: Program<'info, AmmV3>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[account]
pub struct Registry {
    pub used_ids: Vec<[u8; 32]>,
}

pub const REGISTRY_MAX_IDS: usize = 1024;
impl Registry {
    pub const LEN: usize = 4 /* vec len */ + REGISTRY_MAX_IDS * 32;

}


#[derive(Accounts)]
#[instruction(transfer_id: String)]
pub struct ModifyPdaAuthority<'info> {
    #[account(
        mut,
        seeds = [b"operation_data", transfer_id.as_bytes()],
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

    // ===== Raydium CLMM & 池静态信息（deposit 时落库） =====
    pub clmm_program_id: Pubkey,   // 冗余存储，便于 seeds::program 校验
    pub pool_state: Pubkey,
    pub amm_config: Pubkey,
    pub observation_state: Pubkey,
    pub token_vault_0: Pubkey,
    pub token_vault_1: Pubkey,
    pub token_mint_0: Pubkey,
    pub token_mint_1: Pubkey,

    // ===== ZapIn/Position 相关 =====
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub tick_array_lower: Pubkey,
    pub tick_array_upper: Pubkey,
    pub protocol_position: Pubkey,
    pub personal_position: Pubkey,
    pub position_nft_mint: Pubkey,
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
        32 + 1 + (4 + 64) + 32 + 1 + (4 + 256) + 8 + 1 + 32
            // 新字段（Raydium 固定 8 * Pubkey + ticks + 3 * Pubkey）
            + 32 // clmm_program_id
            + 32 // pool_state
            + 32 // amm_config
            + 32 // observation_state
            + 32 // token_vault_0
            + 32 // token_vault_1
            + 32 // token_mint_0
            + 32 // token_mint_1
            + 4  // tick_lower (i32)
            + 4  // tick_upper (i32)
            + 32 // tick_array_lower
            + 32 // tick_array_upper
            + 32 // protocol_position
            + 32 // personal_position
            + 32 ; // position_nft_mint

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
    #[msg("Duplicated transfer ID")]
    DuplicateTransferId,
}