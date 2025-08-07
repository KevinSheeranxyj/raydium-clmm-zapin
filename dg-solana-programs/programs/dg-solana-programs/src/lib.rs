use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("DxzbvkdnQhXGmEz732meAeF9TS8qLit7SAcpa6JXnB6P");

#[program]
pub mod dg_solana_programs {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let transfer_data = &mut ctx.accounts.transfer_data;
        transfer_data.authority = ctx.accounts.authority.key();
        transfer_data.initialized = true;
        msg!("Initialized PDA with authority: {}", transfer_data.authority);
        Ok(())
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
        transfer_data.transfer_id = transfer_id;
        transfer_data.amount = amount;
        transfer_data.recipient = recipient;
        transfer_data.executed = false;

        msg!(
             "Deposited transfer details: ID={}, Amount={}, Recipient={}",
            transfer_data.transfer_id,
            transfer_data.amount,
            transfer_data.recipient
        );
        Ok(())
    }


    // Execute the token transfer
    pub fn execute(ctx: Context<Execute>) -> Result<()> {
        let transfer_data = &mut ctx.accounts.transfer_data;

        // Verify transfer data
        require!(transfer_data.initialized, TransferError::NotInitialized);
        require!(transfer_data.executed, TransferError::AlreadyExecuted);
        require!(transfer_data.amount > 0, TransferError::InvalidAmount);

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
    pub transfer_data: Account<'info, TransferData>,
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub recipient_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,

}
#[derive(Accounts)]
pub struct ModifyPdaAuthority<'info> {
    #[account(
        mut,
        seeds = [b"transefr_data"],
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
    pub const LEN: usize = 32 + 1 + 64 + 8 + 32 + 1; // Give a proper size for serialization
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
}
