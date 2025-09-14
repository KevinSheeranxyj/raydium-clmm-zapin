use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, Transfer, TokenAccount};
use anchor_spl::token_2022::Token2022;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::metadata::Metadata;
use anchor_spl::memo::Memo;
use anchor_spl::token_interface::{Mint as InterfaceMint, TokenAccount as InterfaceTokenAccount};
use raydium_amm_v3::{
    program::AmmV3,
    states::{PoolState, AmmConfig, ObservationState,FEE_RATE_DENOMINATOR_VALUE},
    libraries::{tick_math, MulDiv, U256, },
};
use crate::{state::*, helpers::*, errors::ErrorCode, OperationData};

const Q64_U128: u128 = 1u128 << 64;

/// Execute ZapIn operation after deposit
/// 
/// This instruction executes the complete ZapIn workflow after funds have been deposited.
/// It includes all the steps: prepare, swap, open position, increase liquidity, and finalize.
/// 
/// # Parameters
/// - `transfer_id`: Unique 32-byte identifier for this operation
/// 
/// # Accounts
/// - `operation_data`: Operation-specific data PDA
/// - `caller`: The caller executing this instruction
/// - `program_token_account`: Program's token account for holding funds
/// - `pda_token0`, `pda_token1`: PDA-owned token accounts
/// - Raydium pool and position accounts
/// 
/// # Errors
/// - `ErrorCode::NotInitialized`: If operation data not initialized
/// - `ErrorCode::AlreadyExecuted`: If operation already executed
/// - `ErrorCode::Unauthorized`: If caller is not authorized
/// - `ErrorCode::InvalidParams`: If parameters are invalid
/// 
/// # Events
/// Emits execution events for tracking
#[inline(never)]
/// Params:
/// - transfer_id: 32-byte identifier of the previously deposited operation
pub fn handler(mut ctx: Context<crate::Execute>, transfer_id: [u8; 32]) -> Result<()> {
    msg!("DEBUG: Execute handler started");
    msg!("DEBUG: transfer_id: {:?}", transfer_id);
    
    // Execute the complete flow
    execute_complete_flow(&mut ctx, transfer_id)?;
    
    msg!("DEBUG: Execute instruction completed successfully");
    Ok(())
}

