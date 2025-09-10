use anchor_lang::{Accounts, require};
use anchor_lang::prelude::{Account, Context, Signer};

pub fn handler(ctx: Context<Cancel>, transfer_id: [u8; 32]) -> Result<()> {
    let od = &mut ctx.accounts.operation_data;

    require!(od.initialized, OperationError::NotInitialized);
    require!(!od.executed, OperationError::AlreadyExecuted);
    require!(od.transfer_id == transfer_id, OperationError::InvalidTransferId);
    // 允许 authority 或 executor 触发
    require!(
            ctx.accounts.user.key() == od.executor || ctx.accounts.user.key() == od.authority,
            OperationError::Unauthorized
        );
    require!(od.stage != ExecStage::Finalized, OperationError::InvalidParams);

    let bump = ctx.bumps.operation_data;
    let seeds = &[b"operation_data".as_ref(), transfer_id.as_ref(), &[bump]];
    let signer_seeds = &[&seeds[..]];

    // 退回 token0
    let bal0 = load_token_amount(&ctx.accounts.pda_token0.to_account_info())?;
    if bal0 > 0 {
        let cpi_accounts = anchor_spl::token::Transfer {
            from: ctx.accounts.pda_token0.to_account_info(),
            to: ctx.accounts.user_ata_token0.to_account_info(),
            authority: ctx.accounts.operation_data.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts)
            .with_signer(signer_seeds);
        token::transfer(cpi_ctx, bal0)?;
    }
    // 退回 token1
    let bal1 = load_token_amount(&ctx.accounts.pda_token1.to_account_info())?;
    if bal1 > 0 {
        let cpi_accounts = anchor_spl::token::Transfer {
            from: ctx.accounts.pda_token1.to_account_info(),
            to: ctx.accounts.user_ata_token1.to_account_info(),
            authority: ctx.accounts.operation_data.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts)
            .with_signer(signer_seeds);
        token::transfer(cpi_ctx, bal1)?;
    }

    od.executed = true;
    od.stage = ExecStage::Finalized;
    Ok(())
}
#[derive(Accounts)]
#[instruction(transfer_id: [u8; 32])]
pub struct Cancel<'info> {
    #[account(
        mut,
        seeds = [b"operation_data", &transfer_id],
        bump
    )]
    pub operation_data: Account<'info, OperationData>,
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub pda_token0: Account<'info, TokenAccount>,
    #[account(mut)]
    pub pda_token1: Account<'info, TokenAccount>,
    #[account(mut)]
    pub user_ata_token0: Account<'info, TokenAccount>,
    #[account(mut)]
    pub user_ata_token1: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}