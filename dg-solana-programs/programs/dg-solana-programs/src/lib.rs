use anchor_lang::prelude::*;
use anchor_lang::solana_program::pubkey;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use anchor_spl::token_interface::{Token2022};
use raydium_amm_v3::{
    cpi,
    program::AmmV3,
    states::{AmmConfig, ObservationState, PersonalPositionState, PoolState, TickArrayState},
};

declare_id!("j3q33yPf3b74tKkXepzcK5oZ45ULgCUpAjtxXWfUF1z");

/// NOTE: For zapIn, we're leveraging the Raydium-Amm-v3 Protocol SDK to robost our requirement
///
#[program]
pub mod dg_solana_programs {
    use super::*;

    pub const USDC_MINT: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
        pubkey!("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"); // TODO: Given a specific program ID

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
        parameters: Vec<u8>,
        amount: u64,
    ) -> Result<()> {
        let operation_data = &mut ctx.accounts.operation_data;

        // Verify transfer params
        require!(operation_data.initialized, TransferError::NotInitialized);
        require!(amount > 0, TransferError::InvalidAmount);
        require!(!transfer_id.is_empty(), TransferError::InvalidTransferId);

        // Perform SPL token transfer to program token account
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
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

        msg!(
            "Deposited transfer details: ID={}, Amount={}, Recipient={}",
            operation_data.transfer_id,
            operation_data.amount,
            operation_data.recipient,
        );
        emit!(DepositEvent {
            transfer_id: transfer_id.clone(),
            amount,
            recipient,
        });
        Ok(())
    }

    // Execute the token transfer
    pub fn execute(ctx: Context<Execute>) -> Result<()> {
        let operation_data = &mut ctx.accounts.operation_data;

        // Enforce recipient matches stored PDA recipient
        require!(
            ctx.accounts.recipient_token_account.owner == operation_data.recipient,
            TransferError::Unauthorized
        );
        // Verify operation state
        require!(operation_data.initialized, OperationError::NotInitialized);
        require!(!operation_data.executed, OperationError::AlreadyExecuted);
        require!(operation_data.amount > 0, OperationError::InvalidAmount);

        // Prepare signer seeds for program token account transfer
        let seeds = &[b"operation_data"[..], &[ctx.bumps.operation_data]];
        let signer_seeds = &[&seeds[..]];

        match operation_data.operation_type {
            OperationType::Transfer => {
                // Deserialize transfer params
                let params: TransferParams = deserialize_params(&operation_data.action)?;
                require!(
                    params.amount == operation_data.amount,
                    OperationError::InvalidParams
                );

                // Perform SPL token transfer from program to recipient
                let cpi_accounts = Transfer {
                    from: ctx.accounts.program_token_account.to_account_info(),
                    to: ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.operation_data.to_account_info(),
                };
                let cpi_program = ctx.accounts.token_program.to_account_info();
                let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
                token::transfer(cpi_ctx, operation_data.amount)?;

                msg!(
                    "Executed transfer: ID={}, Amount={}, Recipient={}",
                    operation_data.operation_id,
                    operation_data.amount,
                    params.recipient
                );
            }
            OperationType::ZapIn => {
                // Deserialize params
                let params: ZapInParams = deserialize_params(&operation_data.action)?;
                // Validate tick range
                if params.tick_lower >= params.tick_upper {
                    return err!(ErrorCode::InvalidTickRange);
                }
                // Step 1: Fetch pool state to determine swap amount
                let pool_state = ctx.accounts.pool_state.load()?;
                let current_sqrt_price_x64 = pool_state.sqrt_price_x64;
                let token_a_mint = pool_state.token_0;
                let token_b_mint = pool_state.token_1;
                let is_base_input = ctx.accounts.input_token_account.mint == token_a_mint;

                // Calculate swap amount (simplified: swap half of input)
                let swap_amount = operation_data.amount;
                let other_amount_threshold = 0; // Minimum output amount (adjust for slippage)
                let sqrt_price_limit_x64 = 0; // No price limit (adjust for safety)

                // Step 2: Perform swap via Raydium CLMM v3 CPI
                let swap_cpi_accounts = cpi::accounts::SwapSingleV2 {
                    payer: ctx.accounts.user.to_account_info(),
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
                };
                let swap_cpi_context = CpiContext::new(
                    ctx.accounts.clmm_program.to_account_info(),
                    swap_cpi_accounts,
                )
                    .with_remaining_accounts(ctx.remaining_accounts.to_vec());
                cpi::swap_v2(
                    swap_cpi_context,
                    swap_amount,
                    other_amount_threshold,
                    sqrt_price_limit_x64,
                    is_base_input,
                )?;

                // Step 3: Calculate amounts for liquidity addition
                let remaining_amount = amount_in - swap_amount;
                let swapped_amount = get_swapped_amount(&ctx.accounts.output_token_account)?; // Fetch actual swapped amount

                // Step 4: Check if position exists; if not, open a new position
                let position_exists = ctx.accounts.personal_position.load()?.liquidity > 0;
                if !position_exists {
                    // Call open_position CPI to create a new position
                    let open_position_cpi_accounts = cpi::accounts::OpenPositionV2 {
                        payer: ctx.accounts.user.to_account_info(),
                        pool_state: ctx.accounts.pool_state.to_account_info(),
                        nft_owner: ctx.accounts.user.to_account_info(),
                        position_nft_mint: ctx.accounts.position_nft_mint.to_account_info(),
                        position_nft_account: ctx.accounts.position_nft_account.to_account_info(),
                        personal_position: ctx.accounts.personal_position.to_account_info(),
                        protocol_position: ctx.accounts.protocol_position.to_account_info(),
                        tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
                        tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
                        token_program: ctx.accounts.token_program.to_account_info(),
                        system_program: ctx.accounts.system_program.to_account_info(),
                        rent: ctx.accounts.rent.to_account_info(),
                        // Additional accounts (e.g., tick_array_bitmap) may be needed
                    };
                    let open_position_cpi_context = CpiContext::new(
                        ctx.accounts.clmm_program.to_account_info(),
                        open_position_cpi_accounts,
                    )
                        .with_remaining_accounts(ctx.remaining_accounts.to_vec());
                    cpi::open_position_v2(
                        open_position_cpi_context,
                        tick_lower,
                        tick_upper,
                    )?;
                }

                // Step 4: Add liquidity using Raydium CLMM increase_liquidity_v2 CPI
                let add_liquidity_cpi_accounts = cpi::accounts::IncreaseLiquidityV2 {
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
                };
                let add_liquidity_cpi_context = CpiContext::new(
                    ctx.accounts.clmm_program.to_account_info(),
                    add_liquidity_cpi_accounts,
                ).with_remaining_accounts(ctx.remaining_accounts.to_vec());
                cpi::increase_liquidity_v2(
                    add_liquidity_cpi_context,
                    0,                   // Liquidity (0 to let Raydium calculate)
                    remaining_amount,    // amount_0_max
                    swapped_amount,      // amount_1_max
                    Some(is_base_input), // base_flag
                )?;

        // Mark transfer as executed
        operation_data.executed = true;
        msg!(
            "Executed transfer: ID={}, Amount={}, Recipient={}",
            operation_data.transfer_id,
            operation_data.amount,
            operation_data.recipient
        );
        Ok(())
    }

    // Modify PDA Authority
    pub fn modify_pda_authority(
        ctx: Context<ModifyPdaAuthority>,
        new_authority: Pubkey,
    ) -> Result<()> {
        let operation_data = &mut ctx.accounts.operation_data;

        // Verify current authority
        require!(operation_data.initialized, TransferError::NotInitialized);
        require!(
            operation_data.authority == ctx.accounts.current_authority.key(),
            TransferError::Unauthorized
        );

        // Update authority
        operation_data.authority = new_authority;
        msg!("Update PDA Authority to: {}", new_authority);
        Ok(())
    }
}

// Helper function to deserialize params
fn deserialize_params<T: AnchorDeserialize>(data: &[u8]) -> Result<T> {
    T::try_from_slice(data).map_err(|_| error!(OperationError::InvalidParams))
}

fn calculate_swap_amount(amount_in: u64, sqrt_price_x64: u128, is_base_input: bool) -> Result<u64> {
    // Implement using Raydium's price math (e.g., from SDK)
    Ok(amount_in / 2) // Placeholder
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + TransferData::LEN,
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
        constraint = authority_ata.owner == authority.key() @ TransferError::Unauthorized // Changed to check authority ownership
    )]
    pub authority_ata: Account<'info, TokenAccount>, // Changed from user_ata to authority_ata
    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(
        mut,
        constraint = program_token_account.owner == operation_data.key() @ OperationError::InvalidProgramAccount
    )]
    pub program_token_account: Account<'info, TokenAccount>,
    #[account(
        constraint = token_program.key() == token::ID @ TransferError::InvalidTokenProgram
    )]
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Execute<'info> {
    #[account(
        mut,
        seeds = [b"operation_data"],
        bump
    )]
    pub operation_data: Box<Account<'info, OperationData>>,
    #[account(mut)]
    pub program_token_account: Account<'info, TokenAccount>, // Token A (input token)
    #[account(mut)]
    pub program_token_b_account: Account<'info, TokenAccount>, // Token B (other token in pair)
    #[account(mut)]
    pub recipient_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    #[account(address = RAYDIUM_CLMM_PROGRAM_ID)]
    pub raydium_program: AccountInfo<'info>,
    #[account(mut)]
    pub clmm_pool: AccountInfo<'info>, // Raydium CLMM pool
    #[account(mut)]
    pub position_nft_mint: AccountInfo<'info>, // Position NFT mint
    #[account(mut)]
    pub position_nft_account: AccountInfo<'info>, // Position NFT token account
    #[account(mut)]
    pub token_vault_a: Account<'info, TokenAccount>, // Pool vault for token A
    #[account(mut)]
    pub token_vault_b: Account<'info, TokenAccount>, // Pool vault for token B
    #[account(mut)]
    pub tick_array_lower: AccountInfo<'info>, // Lower tick array
    #[account(mut)]
    pub tick_array_upper: AccountInfo<'info>, // Upper tick array
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
        constraint = current_authority.key() == operation_data.authority @ TransferError::Unauthorized
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
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct TransferParams {
    pub amount: u64,
    pub recipient: pubkey,
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
}

#[derive(Accounts)]
pub struct ZapIn<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub input_token_account: Box<InterfaceAccount<'info, TokenAccount>>, // User's input token ATA (e.g., SOL)
    #[account(mut)]
    pub output_token_account: Box<InterfaceAccount<'info, TokenAccount>>, // User's output token ATA (e.g., USDC)
    #[account(mut)]
    pub position_nft_mint: Box<InterfaceAccount<'info, Mint>>, // Mint for position NFT
    #[account(mut)]
    pub position_nft_account: Box<InterfaceAccount<'info, TokenAccount>>, // User's ATA for position NFT
    #[account(mut)]
    pub position_state: AccountLoader<'info, PersonalPositionState>, // Position state for CLMM
    #[account(
        address = pool_state.load()?.amm_config,
    )]
    pub amm_config: Box<Account<'info, AmmConfig>>,
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,
    #[account(mut)]
    pub input_vault: Box<InterfaceAccount<'info, TokenAccount>>, // Pool's token vault A
    #[account(mut)]
    pub output_vault: Box<InterfaceAccount<'info, TokenAccount>>, // Pool's token vault B
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,
    #[account(
        address = input_vault.mint
    )]
    pub input_vault_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        address = output_vault.mint
    )]
    pub output_vault_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        address = spl_memo::id()
    )]
    pub memo_program: UncheckedAccount<'info>,
    #[account(
        constraint = clmm_program.key() == Pubkey::from_str("CAMMCzo5YL8w4VFF8dTwbK6Tu7mbLbJ7nnvXgrQ7j1s").unwrap()
    )]
    pub clmm_program: Program<'info, AmmV3>,
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
    // Remaining accounts: tick arrays (provided dynamically)
}
impl OperationData {
    pub const LEN: usize = 32 + 1 + 64 + 1 + 128 + 8 + 1;
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
    InvalidParameters,
    #[msg("Invalid tick range")]
    InvalidTickRange,
}
