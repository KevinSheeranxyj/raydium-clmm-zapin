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


use crate::helpers::{to_hex32, deserialize_params, tick_array_start_index};
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

    // Deposit transfer details into PDA
    pub fn deposit(
        ctx: Context<Deposit>,
        transfer_id: [u8; 32],
        operation_type: OperationType,
        action: Vec<u8>,
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
        // Parse raw input bytes into specific enum variant
        od.action = match operation_type {
            OperationType::ZapIn => {
                let p: ZapInParams = deserialize_params(&action)?;
                ActionData::ZapIn(p)
            }
            OperationType::Transfer => {
                let p: TransferParams = deserialize_params(&action)?;
                ActionData::Transfer(p)
            }
        };
        od.executor = authorized_executor; // authorized executor

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
    pub fn execute(ctx: Context<Execute>, transfer_id: [u8; 32]) -> Result<()> {
        instructions::execute::handler(ctx, transfer_id)
    }

    pub fn withdraw(ctx: Context<Withdraw>, p: WithdrawParams) -> Result<()> {
        instructions::withdraw::handler(ctx, p)
    }

    pub fn claim(ctx: Context<Claim>, p: ClaimParams) -> Result<()> {
        instructions::claim::handler(ctx, p)
    }

    // Modify PDA Authority
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