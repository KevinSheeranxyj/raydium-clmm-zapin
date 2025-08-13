use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use anchor_lang::solana_program::pubkey;

declare_id!("j3q33yPf3b74tKkXepzcK5oZ45ULgCUpAjtxXWfUF1z");

#[program]
pub mod dg_solana_programs {
    use super::*;

    pub const USDC_MINT: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let transfer_data = &mut ctx.accounts.transfer_data;
        transfer_data.authority = ctx.accounts.authority.key();
        transfer_data.initialized = true;
        msg!("Initialized PDA with authority: {}", transfer_data.authority);
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
      amount: u64,
      recipient: Pubkey,
    ) -> Result<()> {
        let transfer_data = &mut ctx.accounts.transfer_data;

        // Verify transfer params
        require!(transfer_data.initialized, TransferError::NotInitialized);
        require!(amount > 0, TransferError::InvalidAmount);
        require!(!transfer_id.is_empty(), TransferError::InvalidTransferId);

        // Store transfer details
        transfer_data.transfer_id = transfer_id.clone();
        transfer_data.amount = amount;
        transfer_data.recipient = recipient;
        transfer_data.executed = false;

        msg!(
             "Deposited transfer details: ID={}, Amount={}, Recipient={}",
            transfer_data.transfer_id,
            transfer_data.amount,
            transfer_data.recipient
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
        let transfer_data = &mut ctx.accounts.transfer_data;

        // Verify transfer data
        require!(transfer_data.initialized, TransferError::NotInitialized);
        require!(!transfer_data.executed, TransferError::AlreadyExecuted);
        require!(transfer_data.amount > 0, TransferError::InvalidAmount);

        // Enforce recipient matches stored PDA recipient
        require!(
            ctx.accounts.recipient_token_account.owner == transfer_data.recipient,
            TransferError::Unauthorized
        );

        // Prepare SPL token transfer
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.recipient_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);

        // Execute transfer
        token::transfer(cpi_ctx, transfer_data.amount)?;

        // Mark transfer as executed
        transfer_data.executed = true;
        msg!(
            "Executed transfer: ID={}, Amount={}, Recipient={}",
            transfer_data.transfer_id,
            transfer_data.amount,
            transfer_data.recipient
        );
        Ok(())
    }

    // Modify PDA Authority
    pub fn modify_pda_authority(ctx: Context<ModifyPdaAuthority>, new_authority: Pubkey) -> Result<()> {
        let transfer_data = &mut ctx.accounts.transfer_data;

        // Verify current authority
        require!(transfer_data.initialized, TransferError::NotInitialized);
        require!(
            transfer_data.authority == ctx.accounts.current_authority.key(),
            TransferError::Unauthorized
        );

        // Update authority
        transfer_data.authority = new_authority;
        msg!("Update PDA Authority to: {}", new_authority);
        Ok(())
    }
}



#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + TransferData::LEN,
        seeds = [b"transfer_data"],
        bump
    )]
    pub transfer_data: Account<'info, TransferData>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        seeds = [b"transfer_data"],
        bump
    )]
    pub transfer_data: Box<Account<'info, TransferData>>,
    #[account(
        constraint = authority.key() == transfer_data.authority @ TransferError::Unauthorized
    )]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct Execute<'info> {
    #[account(
        mut,
        seeds = [b"transfer_data"],
        bump
    )]
    pub transfer_data: Box<Account<'info, TransferData>>,
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(
        mut,
        constraint = user_token_account.mint == usdc_mint.key() @ TransferError::InvalidMint,
        constraint = user_token_account.owner == user.key() @ TransferError::Unauthorized
    )]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(
        mut,
        constraint = recipient_token_account.mint == usdc_mint.key() @ TransferError::InvalidMint
    )]
    pub recipient_token_account: Account<'info, TokenAccount>,
    #[account(
        constraint = usdc_mint.mint_authority.is_some() @ TransferError::InvalidMint
    )]
    pub usdc_mint: Account<'info, Mint>,
    #[account(
        constraint = token_program.key() == token::ID @ TransferError::InvalidTokenProgram
    )]
    pub token_program: Program<'info, Token>,

}
#[derive(Accounts)]
pub struct ModifyPdaAuthority<'info> {
    #[account(
        mut,
        seeds = [b"transfer_data"],
        bump
    )]
    pub transfer_data: Account<'info, TransferData>,
    #[account(
        constraint = current_authority.key() == transfer_data.authority @ TransferError::Unauthorized
    )]
    pub current_authority: Signer<'info>,
}

#[account]
#[derive(Default)]
pub struct TransferData {
    pub authority: Pubkey,
    pub initialized: bool,
    pub transfer_id: String,
    pub amount: u64,
    pub recipient: Pubkey,
    pub executed: bool,
}

impl TransferData {
    pub const MAX_TRANSFER_ID_LEN: usize = 32;
    pub const LEN: usize = 8 + // discriminator
        32 + // authority (Pubkey)
        1 +  // initialized (bool)
        4 + Self::MAX_TRANSFER_ID_LEN + // transfer_id (String)
        8 +  // amount (u64)
        32 + // recipient (Pubkey)
        1; // executed(bool)
}


#[error_code]
pub enum TransferError {
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
}
