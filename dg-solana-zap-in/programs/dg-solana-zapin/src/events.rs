use anchor_lang::prelude::*;

#[event]
pub struct LiquidityAdded {
    pub transfer_id: String,
    pub token0_used: u64,
    pub token1_used: u64,
}

#[event]
pub struct DepositEvent {
    pub transfer_id_hex: String,
    pub amount: u64,
    pub recipient: Pubkey,
}

#[event]
pub struct ExecutorAssigned {
    pub transfer_id_hex: String,
    pub executor: Pubkey,
}