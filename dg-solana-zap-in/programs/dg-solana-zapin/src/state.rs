use anchor_lang::prelude::*;


/// Operation type
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum OperationType {
    Transfer,
    ZapIn,
}
impl Default for OperationType {
    fn default() -> Self { OperationType::Transfer }
}

/// ZapIn parameters
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ZapInParams {
    pub amount_in: u64,   // required
    pub pool: Pubkey,     // required
    pub tick_lower: i32,  // required
    pub tick_upper: i32,  // required
    pub slippage_bps: u32,// required
}

/// Transfer parameters
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct TransferParams {
    pub amount: u64,
    pub recipient: Pubkey,
}

/// Action data: different operations carry different parameters
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub enum ActionData {
    Transfer(TransferParams),
    ZapIn(ZapInParams),
}

impl Default for ActionData {
    fn default() -> Self {
        ActionData::Transfer(TransferParams { amount: 0, recipient: Pubkey::default() })
    }
}

/// Registry account used for de-duplication
#[account]
pub struct Registry {
    pub used_ids: Vec<[u8; 32]>,
}
impl Registry {
    pub const START_CAP: usize = 32; // initial capacity
    pub const LEN: usize = 4 + Self::START_CAP * 32;
}

/// Global configuration account for shared settings, e.g. fee receiver
#[account]
pub struct GlobalConfig {
    /// Who has permission to update this config
    pub authority: Pubkey,
    /// Fee receiver address (any Pubkey or owner of an ATA)
    pub fee_receiver: Pubkey,
}

impl GlobalConfig {
    pub const LEN: usize = 32 /* authority */ + 32 /* fee_receiver */;
}