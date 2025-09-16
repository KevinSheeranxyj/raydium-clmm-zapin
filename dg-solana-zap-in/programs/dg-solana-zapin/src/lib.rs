use anchor_lang::prelude::*;
use anchor_lang::system_program::System;

declare_id!("2h2KDqUHkHf7DVUd3SJCeEPPLMLiYuviojp3YFJgMnZN");

pub mod instructions;
pub mod errors;
pub mod state;
pub mod helpers;
pub mod events;


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
    system_instruction,
};
use anchor_spl::token::spl_token;
use instructions::*;
use crate::errors::ErrorCode;


use crate::helpers::{to_hex32, tick_array_start_index};
use crate::state::{OperationType, TransferParams, ZapInParams, Registry, GlobalConfig, ActionData};
use crate::events::{DepositEvent, ExecutorAssigned};

// Devnet: DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH
pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
    pubkey!("DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH"); // devnet program ID

/// NOTE: For ZapIn & ZapOut, we're leveraging the Raydium-Amm-v3 Protocol SDK to robustly meet our requirements
#[program]
pub mod dg_solana_zapin {
    use super::*;

    // Initialize the PDA and set the authority
    /// Initialize global program state:
    /// - Creates/updates `operation_data` PDA and sets its authority
    /// - Initializes global `config` with the `fee_receiver`
    ///
    /// Accounts:
    /// - operation_data (PDA)
    /// - authority (signer)
    /// - config (PDA)
    /// - fee_receiver (unchecked)
    /// - system_program
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let od = &mut ctx.accounts.operation_data;
        if !od.initialized {
            od.authority = ctx.accounts.authority.key();
            od.initialized = true;
            msg!("Initialized PDA with authority: {}", od.authority);
        } else {
            msg!("PDA already initialized; authority: {}", od.authority);
        }

        // Initialize or update global config
        let cfg = &mut ctx.accounts.config;
        cfg.authority = ctx.accounts.authority.key();
        cfg.fee_receiver = ctx.accounts.fee_receiver.key();
        Ok(())
    }

    /// Deposit transfer details into PDA:
    /// - Transfers funds from `authority_ata` to program-owned token account
    /// - Stores `transfer_id`, `operation_type`, and serialized action params
    /// - Records an authorized executor for later `execute`
    ///
    /// Params:
    /// - transfer_id: unique 32-byte operation id
    /// - operation_type: ZapIn or Transfer
    /// - action: ActionData enum carrying typed parameters
    /// - amount: amount to deposit
    /// - ca: contract address (pool or target address)
    /// - authorized_executor: who is allowed to call `execute`
    pub fn deposit(
        ctx: Context<Deposit>,
        transfer_id: [u8; 32],
        operation_type: OperationType,
        action: ActionData,
        amount: u64,
        ca: Pubkey,
        authorized_executor: Pubkey,
    ) -> Result<()> {
        let od = &mut ctx.accounts.operation_data;
        // Initialize (first time for this transfer_id)
        if !od.initialized {
            od.authority = ctx.accounts.authority.key();
            od.initialized = true;
            msg!("Initialized operation_data for transfer_id {} with authority {}", to_hex32(&transfer_id), od.authority);
        }
        let id_hash = transfer_id;
        let reg = &mut ctx.accounts.registry;
        require!(!reg.used_ids.contains(&id_hash), ErrorCode::DuplicateTransferId);
        reg.used_ids.push(id_hash);

        require!(authorized_executor != Pubkey::default(), ErrorCode::InvalidParams);
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(!transfer_id.is_empty(), ErrorCode::InvalidTransferId);

        // Move funds in (keep original behavior)
        let cpi_accounts = Transfer {
            from: ctx.accounts.authority_ata.to_account_info(),
            to: ctx.accounts.program_token_account.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        // Store base parameters
        od.transfer_id = transfer_id.clone();
        od.amount = amount;
        od.executed = false;
        od.ca = ca;
        od.operation_type = operation_type.clone();
        od.action = action.clone();
        od.executor = authorized_executor; // authorized executor
        msg!("DEBUG: action = {:?}", od.action);

        // If ZapIn, parse params and derive tick array / protocol_position etc., store them
        if let ActionData::ZapIn(p) = od.action.clone() {
            od.tick_lower = p.tick_lower;
            od.tick_upper = p.tick_upper;
        }

        // If Transfer, store recipient
        if let ActionData::Transfer(p) = &od.action {
            od.recipient = p.recipient;
        }

        emit!(DepositEvent { transfer_id_hex: to_hex32(&transfer_id), amount, recipient: od.recipient });
        emit!(ExecutorAssigned { transfer_id_hex: to_hex32(&transfer_id), executor: od.executor });
        Ok(())
    }

    /// Prepare + Swap combined step for ZapIn: validates state/accounts, computes/stores flags, then performs swap
    pub fn swap_zap_in(ctx: Context<Execute>, transfer_id: [u8; 32]) -> Result<()> {
        // Validate operation and caller
        let caller_key = ctx.accounts.caller.key();
        helpers::validate_operation_state(&ctx.accounts.operation_data, &caller_key)?;

        // Clone stored action for params
        let action = ctx.accounts.operation_data.action.clone();
        let params = match action {
            ActionData::ZapIn(p) => p,
            _ => return Err(error!(ErrorCode::InvalidParams)),
        };
        msg!("DEBUG: params = {:?}", params);
        // Minimal: derive input side from pool state without extra address checks
        let is_base_input = helpers::get_is_base_input(&ctx)?;
        ctx.accounts.operation_data.base_input_flag = is_base_input;
        msg!("DEBUG: prepared base_input_flag = {}", is_base_input);
        // Perform the swap using existing helper; amount from operation_data
        let amount = ctx.accounts.operation_data.amount;
        helpers::execute_swap_operation_wrapper(&ctx, transfer_id, &params, is_base_input, amount)
    }

     /// Increase-liquidity step for ZapIn: supplies tokens to the position
     pub fn increase_liquidity_zap_in(ctx: Context<Execute>, transfer_id: [u8; 32]) -> Result<()> {
        let caller_key = ctx.accounts.caller.key();
        helpers::validate_operation_state(&ctx.accounts.operation_data, &caller_key)?;
        let action = ctx.accounts.operation_data.action.clone();
        let params = match action {
            ActionData::ZapIn(p) => p,
            _ => return Err(error!(ErrorCode::InvalidParams)),
        };
        let is_base_input = ctx.accounts.operation_data.base_input_flag;
        msg!("DEBUG: is_base_input = {}", is_base_input);
        helpers::execute_increase_liquidity(&ctx, transfer_id, &params, is_base_input)
    }

    /// Open-position step for ZapIn: creates the position NFT and state
    pub fn open_position_zap_in(ctx: Context<Execute>, transfer_id: [u8; 32]) -> Result<()> {
        let caller_key = ctx.accounts.caller.key();
        helpers::validate_operation_state(&ctx.accounts.operation_data, &caller_key)?;
        let action = ctx.accounts.operation_data.action.clone();
        let params = match action {
            ActionData::ZapIn(p) => p,
            _ => return Err(error!(ErrorCode::InvalidParams)),
        };
        msg!("DEBUG: params = {:?}", params);
        helpers::execute_open_position_with_loading(&ctx, transfer_id, &params)
    }

   
    /// Finalize step for ZapIn: marks the operation executed
    pub fn finalize_zap_in(mut ctx: Context<Execute>, transfer_id: [u8; 32]) -> Result<()> {
        let caller_key = ctx.accounts.caller.key();
        helpers::validate_operation_state(&ctx.accounts.operation_data, &caller_key)?;
        helpers::finalize_execute(&mut ctx, transfer_id)
    }



    /// Withdraw (ZapOut-like) from an existing CLMM position:
    /// - Burns liquidity and optionally single-side swaps
    /// - Enforces minimum payout and takes protocol fee to `fee_receiver`
    pub fn withdraw(ctx: Context<Withdraw>, p: WithdrawParams) -> Result<()> {
        instructions::withdraw::handler(ctx, p)
    }

    /// Claim accrued fees from a CLMM position and deliver to user:
    /// - Settles fees, optionally swaps to the desired token
    /// - Enforces minimum payout and takes protocol fee to `fee_receiver`
    pub fn claim(ctx: Context<Claim>, p: ClaimParams) -> Result<()> {
        instructions::claim::handler(ctx, p)
    }

    // Modify PDA Authority
    /// Update the `operation_data` PDA authority to a new pubkey.
    /// Requires the current authority to sign.
    pub fn modify_pda_authority(
        ctx: Context<ModifyPdaAuthority>,
        new_authority: Pubkey,
    ) -> Result<()> {
        let operation_data = &mut ctx.accounts.operation_data;

        // Verify current authority
        require!(operation_data.initialized, ErrorCode::NotInitialized);
        require!(
            operation_data.authority == ctx.accounts.current_authority.key(),
            ErrorCode::Unauthorized
        );

        // Update authority
        operation_data.authority = new_authority;
        msg!("Update PDA Authority to: {}", new_authority);
        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(transfer_id: [u8; 32])]
pub struct Execute<'info> {
    #[account(mut,
        seeds = [b"operation_data".as_ref(), operation_data.transfer_id.as_ref()],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    /// Caller authorized to execute
    pub caller: Signer<'info>,

    // Program-owned token accounts
    #[account(mut)]
    pub program_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub pda_token0: Account<'info, TokenAccount>,
    #[account(mut)]
    pub pda_token1: Account<'info, TokenAccount>,

    // Caller-owned token accounts for caller-custody swap
    #[account(mut)]
    pub caller_ata0: Account<'info, TokenAccount>,
    #[account(mut)]
    pub caller_ata1: Account<'info, TokenAccount>,

    // Raydium CPI program
    pub clmm_program: Program<'info, AmmV3>,

    // Raydium pool/position accounts (Unchecked to allow external program layout)
    /// CHECK: forwarded to Raydium
    pub pool_state: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub amm_config: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub observation_state: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub protocol_position: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub personal_position: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub tick_array_lower: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub tick_array_upper: UncheckedAccount<'info>,

    // Position NFT
    /// CHECK: forwarded to Raydium
    pub position_nft_mint: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub position_nft_account: UncheckedAccount<'info>,

    // Token vaults and mints
    /// CHECK: forwarded to Raydium
    pub token_vault_0: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub token_vault_1: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub token_mint_0: UncheckedAccount<'info>,
    /// CHECK: forwarded to Raydium
    pub token_mint_1: UncheckedAccount<'info>,

    // Programs & sysvars
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    /// CHECK: memo program
    pub memo_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    /// CHECK: metadata program
    pub metadata_program: UncheckedAccount<'info>,
    /// CHECK: metadata account
    pub metadata_account: UncheckedAccount<'info>,
}
#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + OperationData::LEN,
        seeds = [b"operation_data"],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    #[account(mut)] // solver 
    pub authority: Signer<'info>,
    /// CHECK: solver admin
    pub set_solver: UncheckedAccount<'info>, 
    /// Global configuration PDA
    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + GlobalConfig::LEN,
        seeds = [b"global_config"],
        bump
    )]
    pub config: Account<'info, GlobalConfig>,
    /// CHECK: fee receiver account
    pub fee_receiver: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}
#[account]
#[derive(Default)]
pub struct OperationData {
    pub authority: Pubkey,

    pub initialized: bool,
    pub transfer_id: [u8; 32],
    pub recipient: Pubkey,
    pub operation_type: OperationType,
    pub action: ActionData,     // using Enum to store different operation-specific parameters
    pub amount: u64,
    pub executed: bool,
    pub ca: Pubkey,           // contract address

    pub executor: Pubkey,     // authority executor
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub tick_array_lower: Pubkey,
    pub tick_array_upper: Pubkey,
    pub base_input_flag: bool, // << New: whether token0 is the input
}

impl OperationData {
    // Choose a conservative upper bound for LEN and include newly added fields
    pub const LEN: usize =
        32  // authority
            + 1 // initialized
            + 32 // transfer_id
            + 32 // recipient
            + 1  // operation_type (enum tag)
            + (4 + 1024) // action: 4-byte length + reserve 1024 bytes
            + 8  // amount
            + 1  // executed
            + 32 // ca
            + 32 // executor
            + 4 + 4 // tick_lower/tick_upper
            + 32*2 // tick_array_lower + tick_array_upper
            + 1  // stage (enum tag)
            + 1; // base_input_flag
}

#[derive(Accounts)]
#[instruction(transfer_id: [u8; 32])]
pub struct Deposit<'info> {
    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + Registry::LEN,
        seeds = [b"registry"],
        bump
    )]
    pub registry: Account<'info, Registry>,

    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + OperationData::LEN,
        seeds = [b"operation_data".as_ref(), transfer_id.as_ref()],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,

    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        mut,
        constraint = authority_ata.owner == authority.key() @ ErrorCode::Unauthorized
    )]
    pub authority_ata: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = program_token_account.owner == operation_data.key() @ ErrorCode::InvalidProgramAccount
    )]
    pub program_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(transfer_id: String)]
pub struct ModifyPdaAuthority<'info> {
    #[account(
        mut,
        seeds = [b"operation_data".as_ref(), transfer_id.as_bytes()],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    #[account(
        constraint = current_authority.key() == operation_data.authority @ ErrorCode::Unauthorized
    )]
    pub current_authority: Signer<'info>,
}